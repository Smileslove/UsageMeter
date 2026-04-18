//! SQLite 数据库，用于持久化代理使用数据

use super::types::{SessionStats, UsageRecord};
use crate::models::ModelPricingConfig;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// 数据库管理器，用于代理使用数据
/// 使用线程安全的 SQLite 连接包装器
pub struct ProxyDatabase {
    conn: Arc<std::sync::Mutex<Connection>>,
}

impl ProxyDatabase {
    /// 创建新的数据库连接
    pub fn new() -> Result<Self, String> {
        let db_path = Self::get_db_path()?;

        // 确保父目录存在
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create database directory: {}", e))?;
        }

        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open database: {}", e))?;

        // 启用 WAL 模式以获得更好的并发性
        conn.pragma_update(None, "journal_mode", &"WAL")
            .map_err(|e| format!("Failed to enable WAL mode: {}", e))?;

        // 创建表
        Self::create_tables(&conn)?;

        // 迁移旧表结构（添加新字段）
        Self::migrate_schema(&conn)?;

        // 创建模型价格表
        Self::create_model_pricing_table_static(&conn)?;

        Ok(Self {
            conn: Arc::new(std::sync::Mutex::new(conn)),
        })
    }

    /// 获取数据库路径
    fn get_db_path() -> Result<PathBuf, String> {
        let home = dirs::home_dir().ok_or_else(|| "Home directory not found".to_string())?;
        Ok(home.join(".usagemeter").join("proxy_data.db"))
    }

    /// 创建数据库表
    pub fn create_tables(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(
            r#"
            -- 使用记录表
            -- 存储单次 API 请求的使用数据
            -- 注意：不存储 total_tokens，因为四种 token 价格不同，简单相加无意义
            -- 实际处理量 = input_tokens + output_tokens
            -- 计费相关需要单独处理四种 token
            CREATE TABLE IF NOT EXISTS usage_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                message_id TEXT NOT NULL UNIQUE,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                model TEXT NOT NULL DEFAULT '',
                session_id TEXT,
                request_start_time INTEGER,
                request_end_time INTEGER,
                duration_ms INTEGER NOT NULL DEFAULT 0,
                output_tokens_per_second REAL,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
            );

            -- 索引用于加速查询
            CREATE INDEX IF NOT EXISTS idx_timestamp ON usage_records(timestamp);
            CREATE INDEX IF NOT EXISTS idx_message_id ON usage_records(message_id);
            CREATE INDEX IF NOT EXISTS idx_session_id ON usage_records(session_id);
            -- 组合索引用于加速按模型分组的速率统计查询
            CREATE INDEX IF NOT EXISTS idx_model_timestamp ON usage_records(model, timestamp);

            -- 会话统计表
            CREATE TABLE IF NOT EXISTS session_stats (
                session_id TEXT PRIMARY KEY,
                total_requests INTEGER NOT NULL DEFAULT 0,
                total_input_tokens INTEGER NOT NULL DEFAULT 0,
                total_output_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                total_duration_ms INTEGER NOT NULL DEFAULT 0,
                avg_output_tokens_per_second REAL DEFAULT 0,
                first_request_time INTEGER,
                last_request_time INTEGER,
                models TEXT
            );

            -- 每日汇总表（用于更快的聚合）
            -- total_tokens = input_tokens + output_tokens（实际处理量）
            CREATE TABLE IF NOT EXISTS daily_summary (
                date TEXT PRIMARY KEY,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                request_count INTEGER NOT NULL DEFAULT 0
            );

            -- 模型使用量表
            -- total_tokens = input_tokens + output_tokens（实际处理量）
            CREATE TABLE IF NOT EXISTS model_usage (
                date TEXT NOT NULL,
                model TEXT NOT NULL,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                request_count INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (date, model)
            );
            "#,
        )
        .map_err(|e| format!("Failed to create tables: {}", e))?;

        Ok(())
    }

    /// 创建模型价格表（静态方法）
    fn create_model_pricing_table_static(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(
            r#"
            -- 模型价格表
            CREATE TABLE IF NOT EXISTS model_pricing (
                model_id TEXT PRIMARY KEY,
                display_name TEXT,
                input_price REAL NOT NULL,
                output_price REAL NOT NULL,
                cache_read_price REAL,
                cache_write_price REAL,
                source TEXT NOT NULL DEFAULT 'api',
                last_updated INTEGER NOT NULL
            );

            -- 创建索引加速搜索
            CREATE INDEX IF NOT EXISTS idx_model_pricing_search ON model_pricing(model_id, display_name);
            "#,
        )
        .map_err(|e| format!("Failed to create model_pricing table: {}", e))?;
        Ok(())
    }

    /// 迁移数据库模式（为旧表添加新字段）
    fn migrate_schema(conn: &Connection) -> Result<(), String> {
        // 尝试添加新字段（如果不存在则添加）
        let migrations = [
            "ALTER TABLE usage_records ADD COLUMN request_start_time INTEGER",
            "ALTER TABLE usage_records ADD COLUMN request_end_time INTEGER",
            "ALTER TABLE usage_records ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE usage_records ADD COLUMN output_tokens_per_second REAL",
            "ALTER TABLE usage_records ADD COLUMN status_code INTEGER NOT NULL DEFAULT 200",
            "ALTER TABLE usage_records ADD COLUMN ttft_ms INTEGER",
        ];

        for migration in migrations {
            // SQLite 不支持 IF NOT EXISTS for ALTER TABLE ADD COLUMN
            // 所以我们忽略错误（字段已存在时会报错）
            let _ = conn.execute(migration, []);
        }

        Ok(())
    }

    /// 插入使用记录
    pub async fn insert_record(&self, record: &UsageRecord) -> Result<i64, String> {
        let conn = self.conn.lock().map_err(|e| format!("Failed to lock connection: {}", e))?;

        conn.execute(
            r#"
            INSERT OR REPLACE INTO usage_records
            (timestamp, message_id, input_tokens, output_tokens, cache_create_tokens,
             cache_read_tokens, model, session_id, request_start_time,
             request_end_time, duration_ms, output_tokens_per_second, ttft_ms, status_code)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            "#,
            (
                record.timestamp,
                &record.message_id,
                record.input_tokens as i64,
                record.output_tokens as i64,
                record.cache_create_tokens as i64,
                record.cache_read_tokens as i64,
                &record.model,
                &record.session_id,
                record.request_start_time,
                record.request_end_time,
                record.duration_ms as i64,
                record.output_tokens_per_second,
                record.ttft_ms.map(|v| v as i64),
                record.status_code as i64,
            ),
        )
        .map_err(|e| format!("Failed to insert record: {}", e))?;

        let id = conn.last_insert_rowid();
        Ok(id)
    }

    /// 获取时间窗口内的记录
    #[allow(dead_code)]
    pub async fn get_records_since(&self, cutoff_ms: i64) -> Result<Vec<UsageRecord>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Failed to lock connection: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT timestamp, message_id, input_tokens, output_tokens,
                       cache_create_tokens, cache_read_tokens, model, session_id,
                       request_start_time, request_end_time, duration_ms, output_tokens_per_second,
                       ttft_ms, status_code
                FROM usage_records
                WHERE timestamp >= ?1
                ORDER BY timestamp DESC
                "#,
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let records = stmt
            .query_map([cutoff_ms], |row| {
                let input = row.get::<_, i64>(2)? as u64;
                let output = row.get::<_, i64>(3)? as u64;
                Ok(UsageRecord {
                    timestamp: row.get::<_, i64>(0)?,
                    message_id: row.get(1)?,
                    input_tokens: input,
                    output_tokens: output,
                    cache_create_tokens: row.get::<_, i64>(4)? as u64,
                    cache_read_tokens: row.get::<_, i64>(5)? as u64,
                    total_tokens: input + output, // 计算实际处理量
                    model: row.get(6)?,
                    session_id: row.get(7)?,
                    request_start_time: row.get::<_, Option<i64>>(8)?.unwrap_or(0),
                    request_end_time: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
                    duration_ms: row.get::<_, i64>(10)? as u64,
                    output_tokens_per_second: row.get(11)?,
                    ttft_ms: row.get::<_, Option<i64>>(12)?.map(|v| v as u64),
                    status_code: row.get::<_, i64>(13)? as u16,
                })
            })
            .map_err(|e| format!("Failed to query records: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect records: {}", e))?;

        Ok(records)
    }

    /// 获取总记录数
    pub async fn get_record_count(&self) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| format!("Failed to lock connection: {}", e))?;

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM usage_records", [], |row| row.get(0))
            .map_err(|e| format!("Failed to count records: {}", e))?;

        Ok(count as usize)
    }

    /// 删除指定天数之前的记录
    #[allow(dead_code)]
    pub async fn cleanup_old_records(&self, days: i64) -> Result<usize, String> {
        let cutoff = chrono::Utc::now().timestamp_millis() - (days * 24 * 60 * 60 * 1000);
        let conn = self.conn.lock().map_err(|e| format!("Failed to lock connection: {}", e))?;

        let affected = conn
            .execute("DELETE FROM usage_records WHERE timestamp < ?1", [cutoff])
            .map_err(|e| format!("Failed to cleanup records: {}", e))?;

        Ok(affected)
    }

    /// 获取时间窗口的聚合统计（包含所有请求）
    #[allow(dead_code)]
    pub async fn get_window_stats(&self, cutoff_ms: i64) -> Result<WindowAggregate, String> {
        self.get_window_stats_filtered(cutoff_ms, true).await
    }

    /// 获取时间窗口的聚合统计（支持过滤错误请求）
    ///
    /// # 参数
    /// - `cutoff_ms`: 窗口起始时间戳（毫秒）
    /// - `include_errors`: 是否包含错误请求（4xx/5xx）
    pub async fn get_window_stats_filtered(
        &self,
        cutoff_ms: i64,
        include_errors: bool,
    ) -> Result<WindowAggregate, String> {
        let conn = self.conn.lock().map_err(|e| format!("Failed to lock connection: {}", e))?;

        let stats = if include_errors {
            // 包含所有请求
            conn.query_row(
                r#"
                SELECT
                    COUNT(*) as request_count,
                    COALESCE(SUM(input_tokens + output_tokens), 0) as total_tokens,
                    COALESCE(SUM(input_tokens), 0) as input_tokens,
                    COALESCE(SUM(output_tokens), 0) as output_tokens,
                    COALESCE(SUM(cache_create_tokens), 0) as cache_create_tokens,
                    COALESCE(SUM(cache_read_tokens), 0) as cache_read_tokens,
                    COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN 1 ELSE 0 END), 0) as status_2xx,
                    COALESCE(SUM(CASE WHEN status_code >= 400 AND status_code < 500 THEN 1 ELSE 0 END), 0) as status_4xx,
                    COALESCE(SUM(CASE WHEN status_code >= 500 AND status_code < 600 THEN 1 ELSE 0 END), 0) as status_5xx
                FROM usage_records
                WHERE timestamp >= ?1
                "#,
                [cutoff_ms],
                |row| {
                    Ok(WindowAggregate {
                        request_count: row.get(0)?,
                        total_tokens: row.get(1)?,
                        input_tokens: row.get(2)?,
                        output_tokens: row.get(3)?,
                        cache_create_tokens: row.get(4)?,
                        cache_read_tokens: row.get(5)?,
                        status_2xx: row.get(6)?,
                        status_4xx: row.get(7)?,
                        status_5xx: row.get(8)?,
                    })
                },
            )
        } else {
            // 只包含成功请求（2xx）
            conn.query_row(
                r#"
                SELECT
                    COUNT(*) as request_count,
                    COALESCE(SUM(input_tokens + output_tokens), 0) as total_tokens,
                    COALESCE(SUM(input_tokens), 0) as input_tokens,
                    COALESCE(SUM(output_tokens), 0) as output_tokens,
                    COALESCE(SUM(cache_create_tokens), 0) as cache_create_tokens,
                    COALESCE(SUM(cache_read_tokens), 0) as cache_read_tokens,
                    COUNT(*) as status_2xx,
                    0 as status_4xx,
                    0 as status_5xx
                FROM usage_records
                WHERE timestamp >= ?1
                  AND status_code >= 200 AND status_code < 300
                "#,
                [cutoff_ms],
                |row| {
                    Ok(WindowAggregate {
                        request_count: row.get(0)?,
                        total_tokens: row.get(1)?,
                        input_tokens: row.get(2)?,
                        output_tokens: row.get(3)?,
                        cache_create_tokens: row.get(4)?,
                        cache_read_tokens: row.get(5)?,
                        status_2xx: row.get(6)?,
                        status_4xx: row.get(7)?,
                        status_5xx: row.get(8)?,
                    })
                },
            )
        }
        .map_err(|e| format!("Failed to get window stats: {}", e))?;

        Ok(stats)
    }

    /// 获取时间窗口内的模型分布
    pub async fn get_model_distribution(
        &self,
        cutoff_ms: i64,
    ) -> Result<Vec<ModelDistribution>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Failed to lock connection: {}", e))?;

        // 先查询基础数据
        let mut stmt = conn
            .prepare(
                r#"
                SELECT
                    model,
                    COUNT(*) as request_count,
                    SUM(input_tokens + output_tokens) as total_tokens,
                    SUM(input_tokens) as input_tokens,
                    SUM(output_tokens) as output_tokens,
                    SUM(cache_create_tokens) as cache_create_tokens,
                    SUM(cache_read_tokens) as cache_read_tokens
                FROM usage_records
                WHERE timestamp >= ?1
                GROUP BY model
                ORDER BY total_tokens DESC
                "#,
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let models: Vec<(String, i64, i64, i64, i64, i64, i64)> = stmt
            .query_map([cutoff_ms], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
            .map_err(|e| format!("Failed to query model distribution: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect model distribution: {}", e))?;

        // 为每个模型查询状态码分布
        let mut result = Vec::new();
        for (model, request_count, total_tokens, input_tokens, output_tokens, cache_create_tokens, cache_read_tokens) in models {
            // 查询该模型的状态码分布
            let status_codes: Vec<(i64, i64)> = conn
                .prepare(
                    "SELECT status_code, COUNT(*) as count FROM usage_records WHERE timestamp >= ?1 AND model = ?2 GROUP BY status_code ORDER BY count DESC"
                )
                .and_then(|mut stmt| {
                    let rows = stmt.query_map(rusqlite::params![cutoff_ms, &model], |row| Ok((row.get(0)?, row.get(1)?)))?;
                    rows.collect::<Result<Vec<_>, _>>()
                })
                .unwrap_or_default();

            // 转换为 JSON
            let status_codes_json = serde_json::to_string(
                &status_codes.iter()
                    .map(|(code, count)| serde_json::json!({"statusCode": code, "count": count}))
                    .collect::<Vec<_>>()
            ).unwrap_or_else(|_| "[]".to_string());

            result.push(ModelDistribution {
                model,
                request_count,
                total_tokens,
                input_tokens,
                output_tokens,
                cache_create_tokens,
                cache_read_tokens,
                status_codes_json,
            });
        }

        Ok(result)
    }

    /// 获取会话统计信息
    #[allow(dead_code)]
    pub async fn get_session_stats(&self, session_id: &str, pricings: &[ModelPricingConfig], match_mode: &str) -> Result<Option<SessionStats>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Failed to lock connection: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT
                    session_id,
                    COUNT(*) as total_requests,
                    COALESCE(SUM(input_tokens), 0) as total_input_tokens,
                    COALESCE(SUM(output_tokens), 0) as total_output_tokens,
                    COALESCE(SUM(cache_create_tokens), 0) as total_cache_create_tokens,
                    COALESCE(SUM(cache_read_tokens), 0) as total_cache_read_tokens,
                    COALESCE(SUM(duration_ms), 0) as total_duration_ms,
                    MIN(request_start_time) as first_request_time,
                    MAX(request_end_time) as last_request_time,
                    GROUP_CONCAT(DISTINCT model) as models,
                    AVG(ttft_ms) as avg_ttft_ms,
                    SUM(CASE WHEN status_code < 400 THEN 1 ELSE 0 END) as success_requests,
                    SUM(CASE WHEN status_code >= 400 THEN 1 ELSE 0 END) as error_requests
                FROM usage_records
                WHERE session_id = ?1
                GROUP BY session_id
                "#,
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let result = stmt
            .query_row([session_id], |row| {
                let total_output_tokens: i64 = row.get(3)?;
                let total_duration_ms: i64 = row.get(6)?;
                let models_str: String = row.get::<_, String>(9)?;
                let total_input_tokens: i64 = row.get(2)?;
                let total_cache_create_tokens: i64 = row.get(4)?;
                let total_cache_read_tokens: i64 = row.get(5)?;

                // 计算平均生成速率
                let avg_rate = if total_duration_ms > 0 {
                    (total_output_tokens as f64) / (total_duration_ms as f64 / 1000.0)
                } else {
                    0.0
                };

                // 获取第一个模型用于定价
                let first_model = models_str.split(',').next().unwrap_or("");

                // 计算估算费用
                let estimated_cost = crate::models::estimate_session_cost(
                    total_input_tokens as u64,
                    total_output_tokens as u64,
                    total_cache_create_tokens as u64,
                    total_cache_read_tokens as u64,
                    first_model,
                    pricings,
                    match_mode,
                );

                Ok(SessionStats {
                    session_id: row.get(0)?,
                    total_requests: row.get::<_, i64>(1)? as u64,
                    total_input_tokens: total_input_tokens as u64,
                    total_output_tokens: total_output_tokens as u64,
                    total_cache_create_tokens: total_cache_create_tokens as u64,
                    total_cache_read_tokens: total_cache_read_tokens as u64,
                    total_duration_ms: total_duration_ms as u64,
                    avg_output_tokens_per_second: avg_rate,
                    first_request_time: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
                    last_request_time: row.get::<_, Option<i64>>(8)?.unwrap_or(0),
                    models: if models_str.is_empty() {
                        Vec::new()
                    } else {
                        models_str.split(',').map(|s| s.to_string()).collect()
                    },
                    avg_ttft_ms: row.get::<_, Option<f64>>(10)?.unwrap_or(0.0),
                    success_requests: row.get::<_, i64>(11)? as u64,
                    error_requests: row.get::<_, i64>(12)? as u64,
                    estimated_cost,
                    is_cost_estimated: true,
                    cwd: None,
                    project_name: None,
                    topic: None,
                    last_prompt: None,
                    session_name: None,
                })
            });

        match result {
            Ok(stats) => Ok(Some(stats)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to get session stats: {}", e)),
        }
    }

    /// 获取所有会话列表（按最后请求时间倒序）
    #[allow(dead_code)]
    pub async fn get_all_sessions(&self, limit: i64, pricings: &[ModelPricingConfig], match_mode: &str) -> Result<Vec<SessionStats>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Failed to lock connection: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT
                    session_id,
                    COUNT(*) as total_requests,
                    COALESCE(SUM(input_tokens), 0) as total_input_tokens,
                    COALESCE(SUM(output_tokens), 0) as total_output_tokens,
                    COALESCE(SUM(cache_create_tokens), 0) as total_cache_create_tokens,
                    COALESCE(SUM(cache_read_tokens), 0) as total_cache_read_tokens,
                    COALESCE(SUM(duration_ms), 0) as total_duration_ms,
                    MIN(request_start_time) as first_request_time,
                    MAX(request_end_time) as last_request_time,
                    GROUP_CONCAT(DISTINCT model) as models,
                    AVG(ttft_ms) as avg_ttft_ms,
                    SUM(CASE WHEN status_code < 400 THEN 1 ELSE 0 END) as success_requests,
                    SUM(CASE WHEN status_code >= 400 THEN 1 ELSE 0 END) as error_requests
                FROM usage_records
                WHERE session_id IS NOT NULL AND session_id != ''
                GROUP BY session_id
                ORDER BY MAX(request_end_time) DESC
                LIMIT ?1
                "#,
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let sessions = stmt
            .query_map([limit], |row| {
                let total_output_tokens: i64 = row.get(3)?;
                let total_duration_ms: i64 = row.get(6)?;
                let models_str: String = row.get::<_, String>(9)?;
                let total_input_tokens: i64 = row.get(2)?;
                let total_cache_create_tokens: i64 = row.get(4)?;
                let total_cache_read_tokens: i64 = row.get(5)?;

                let avg_rate = if total_duration_ms > 0 {
                    (total_output_tokens as f64) / (total_duration_ms as f64 / 1000.0)
                } else {
                    0.0
                };

                // 获取第一个模型用于定价
                let first_model = models_str.split(',').next().unwrap_or("");

                // 计算估算费用
                let estimated_cost = crate::models::estimate_session_cost(
                    total_input_tokens as u64,
                    total_output_tokens as u64,
                    total_cache_create_tokens as u64,
                    total_cache_read_tokens as u64,
                    first_model,
                    pricings,
                    match_mode,
                );

                Ok(SessionStats {
                    session_id: row.get(0)?,
                    total_requests: row.get::<_, i64>(1)? as u64,
                    total_input_tokens: total_input_tokens as u64,
                    total_output_tokens: total_output_tokens as u64,
                    total_cache_create_tokens: total_cache_create_tokens as u64,
                    total_cache_read_tokens: total_cache_read_tokens as u64,
                    total_duration_ms: total_duration_ms as u64,
                    avg_output_tokens_per_second: avg_rate,
                    first_request_time: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
                    last_request_time: row.get::<_, Option<i64>>(8)?.unwrap_or(0),
                    models: if models_str.is_empty() {
                        Vec::new()
                    } else {
                        models_str.split(',').map(|s| s.to_string()).collect()
                    },
                    avg_ttft_ms: row.get::<_, Option<f64>>(10)?.unwrap_or(0.0),
                    success_requests: row.get::<_, i64>(11)? as u64,
                    error_requests: row.get::<_, i64>(12)? as u64,
                    estimated_cost,
                    is_cost_estimated: true,
                    cwd: None,
                    project_name: None,
                    topic: None,
                    last_prompt: None,
                    session_name: None,
                })
            })
            .map_err(|e| format!("Failed to query sessions: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect sessions: {}", e))?;

        Ok(sessions)
    }

    /// 获取窗口内的平均生成速率统计
    ///
    /// 只统计 duration_ms > 0 且 output_tokens_per_second IS NOT NULL 的记录
    /// 使用加权平均计算整体速率：total_output_tokens * 1000.0 / total_duration_ms
    /// 这比简单 AVG 更能反映真实的总体吞吐效率
    pub async fn get_window_rate_stats(&self, cutoff_ms: i64) -> Result<WindowRateStats, String> {
        let conn = self.conn.lock().map_err(|e| format!("Failed to lock connection: {}", e))?;

        let stats = conn
            .query_row(
                r#"
                SELECT
                    COUNT(*) as request_count,
                    COALESCE(SUM(output_tokens), 0) as total_output_tokens,
                    COALESCE(SUM(duration_ms), 0) as total_duration_ms,
                    -- 加权平均速率：总 tokens / 总时间（毫秒转秒需要乘 1000）
                    CASE
                        WHEN SUM(duration_ms) > 0
                        THEN SUM(output_tokens) * 1000.0 / SUM(duration_ms)
                        ELSE 0
                    END as avg_rate
                FROM usage_records
                WHERE timestamp >= ?1
                  AND duration_ms > 0
                  AND output_tokens_per_second IS NOT NULL
                "#,
                [cutoff_ms],
                |row| {
                    Ok(WindowRateStats {
                        request_count: row.get(0)?,
                        total_output_tokens: row.get(1)?,
                        total_duration_ms: row.get(2)?,
                        avg_output_tokens_per_second: row.get(3)?,
                    })
                },
            )
            .map_err(|e| format!("Failed to get window rate stats: {}", e))?;

        Ok(stats)
    }

    /// 获取时间窗口内按模型分组的速率统计
    ///
    /// 只统计 duration_ms > 0 且 output_tokens_per_second IS NOT NULL 的记录
    /// 结果按平均速率降序排列
    pub async fn get_model_rate_stats(&self, cutoff_ms: i64) -> Result<Vec<ModelRateStats>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Failed to lock connection: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT
                    model,
                    COUNT(*) as request_count,
                    COALESCE(SUM(output_tokens), 0) as total_output_tokens,
                    COALESCE(SUM(duration_ms), 0) as total_duration_ms,
                    COALESCE(AVG(output_tokens_per_second), 0) as avg_rate,
                    COALESCE(MIN(output_tokens_per_second), 0) as min_rate,
                    COALESCE(MAX(output_tokens_per_second), 0) as max_rate
                FROM usage_records
                WHERE timestamp >= ?1
                  AND duration_ms > 0
                  AND output_tokens_per_second IS NOT NULL
                  AND model != ''
                GROUP BY model
                ORDER BY avg_rate DESC
                "#,
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let models = stmt
            .query_map([cutoff_ms], |row| {
                Ok(ModelRateStats {
                    model: row.get(0)?,
                    request_count: row.get(1)?,
                    total_output_tokens: row.get(2)?,
                    total_duration_ms: row.get(3)?,
                    avg_tokens_per_second: row.get(4)?,
                    min_tokens_per_second: row.get(5)?,
                    max_tokens_per_second: row.get(6)?,
                })
            })
            .map_err(|e| format!("Failed to query model rate stats: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect model rate stats: {}", e))?;

        Ok(models)
    }
}

impl Default for ProxyDatabase {
    fn default() -> Self {
        Self::new().expect("Failed to create database")
    }
}

/// 时间窗口的聚合统计
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct WindowAggregate {
    pub request_count: i64,
    #[allow(dead_code)]
    pub total_tokens: i64,        // 实际处理量 = input + output
    pub input_tokens: i64,        // 实际输入（不含缓存）
    pub output_tokens: i64,
    pub cache_create_tokens: i64,
    pub cache_read_tokens: i64,
    pub status_2xx: i64,          // 成功请求数
    pub status_4xx: i64,          // 客户端错误数
    pub status_5xx: i64,          // 服务端错误数
}

/// 模型分布统计
#[derive(Debug, Clone)]
pub struct ModelDistribution {
    pub model: String,
    pub request_count: i64,
    pub total_tokens: i64,        // 实际处理量 = input + output
    pub input_tokens: i64,        // 实际输入（不含缓存）
    pub output_tokens: i64,
    pub cache_create_tokens: i64,
    pub cache_read_tokens: i64,
    /// 状态码分布 JSON 字符串，格式: [{"statusCode":200,"count":10},{"statusCode":429,"count":2}]
    pub status_codes_json: String,
}

/// 窗口速率统计
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WindowRateStats {
    pub request_count: i64,
    pub total_output_tokens: i64,
    pub total_duration_ms: i64,
    pub avg_output_tokens_per_second: f64,
}

/// 单模型速率统计（用于前端展示）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRateStats {
    /// 模型名称
    pub model: String,
    /// 请求数量
    pub request_count: i64,
    /// 总输出 Token 数
    pub total_output_tokens: i64,
    /// 总耗时（毫秒）
    pub total_duration_ms: i64,
    /// 平均生成速率（tokens/s）
    pub avg_tokens_per_second: f64,
    /// 最小生成速率（tokens/s）
    pub min_tokens_per_second: f64,
    /// 最大生成速率（tokens/s）
    pub max_tokens_per_second: f64,
}

/// 窗口速率汇总（整体 + 按模型分组）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowRateSummary {
    /// 窗口名称
    pub window: String,
    /// 整体速率统计
    pub overall: WindowRateStats,
    /// 按模型分组的速率统计
    pub by_model: Vec<ModelRateStats>,
}

/// 状态码分布
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct StatusCodeDistribution {
    pub status_code: i64,
    pub count: i64,
    pub category: String,  // "success", "client_error", "server_error"
}

/// TTFT 统计（首 Token 生成时间）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TtftStats {
    pub request_count: i64,
    pub avg_ttft_ms: f64,
    pub min_ttft_ms: i64,
    pub max_ttft_ms: i64,
}

/// 单模型 TTFT 统计
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTtftStats {
    /// 模型名称
    pub model: String,
    /// 请求数量
    pub request_count: i64,
    /// 平均 TTFT（毫秒）
    pub avg_ttft_ms: f64,
    /// 最小 TTFT（毫秒）
    pub min_ttft_ms: i64,
    /// 最大 TTFT（毫秒）
    pub max_ttft_ms: i64,
}

impl ProxyDatabase {
    /// 获取状态码分布
    #[allow(dead_code)]
    pub async fn get_status_code_distribution(&self, cutoff_ms: i64) -> Result<Vec<StatusCodeDistribution>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Failed to lock connection: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT status_code, COUNT(*) as count
                FROM usage_records
                WHERE timestamp >= ?1
                GROUP BY status_code
                ORDER BY count DESC
                "#,
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let distribution = stmt
            .query_map([cutoff_ms], |row| {
                let status_code: i64 = row.get(0)?;
                let count: i64 = row.get(1)?;
                let category = if status_code >= 200 && status_code < 300 {
                    "success".to_string()
                } else if status_code >= 400 && status_code < 500 {
                    "client_error".to_string()
                } else if status_code >= 500 {
                    "server_error".to_string()
                } else {
                    "other".to_string()
                };
                Ok(StatusCodeDistribution {
                    status_code,
                    count,
                    category,
                })
            })
            .map_err(|e| format!("Failed to query status code distribution: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect status code distribution: {}", e))?;

        Ok(distribution)
    }

    /// 获取窗口内的 TTFT 统计（首 Token 生成时间）
    ///
    /// 只统计 ttft_ms IS NOT NULL 的记录
    pub async fn get_ttft_stats(&self, cutoff_ms: i64) -> Result<TtftStats, String> {
        let conn = self.conn.lock().map_err(|e| format!("Failed to lock connection: {}", e))?;

        let stats = conn
            .query_row(
                r#"
                SELECT
                    COUNT(*) as request_count,
                    COALESCE(AVG(ttft_ms), 0) as avg_ttft_ms,
                    COALESCE(MIN(ttft_ms), 0) as min_ttft_ms,
                    COALESCE(MAX(ttft_ms), 0) as max_ttft_ms
                FROM usage_records
                WHERE timestamp >= ?1
                  AND ttft_ms IS NOT NULL
                "#,
                [cutoff_ms],
                |row| {
                    Ok(TtftStats {
                        request_count: row.get(0)?,
                        avg_ttft_ms: row.get(1)?,
                        min_ttft_ms: row.get(2)?,
                        max_ttft_ms: row.get(3)?,
                    })
                },
            )
            .map_err(|e| format!("Failed to get TTFT stats: {}", e))?;

        Ok(stats)
    }

    /// 获取时间窗口内按模型分组的 TTFT 统计
    ///
    /// 结果按平均 TTFT 升序排列（响应快的在前）
    pub async fn get_model_ttft_stats(&self, cutoff_ms: i64) -> Result<Vec<ModelTtftStats>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Failed to lock connection: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT
                    model,
                    COUNT(*) as request_count,
                    COALESCE(AVG(ttft_ms), 0) as avg_ttft_ms,
                    COALESCE(MIN(ttft_ms), 0) as min_ttft_ms,
                    COALESCE(MAX(ttft_ms), 0) as max_ttft_ms
                FROM usage_records
                WHERE timestamp >= ?1
                  AND ttft_ms IS NOT NULL
                  AND model != ''
                GROUP BY model
                ORDER BY avg_ttft_ms ASC
                "#,
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let models = stmt
            .query_map([cutoff_ms], |row| {
                Ok(ModelTtftStats {
                    model: row.get(0)?,
                    request_count: row.get(1)?,
                    avg_ttft_ms: row.get(2)?,
                    min_ttft_ms: row.get(3)?,
                    max_ttft_ms: row.get(4)?,
                })
            })
            .map_err(|e| format!("Failed to query model TTFT stats: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect model TTFT stats: {}", e))?;

        Ok(models)
    }

    // ========== 模型价格相关操作 ==========

    /// 创建模型价格表
    pub fn create_model_pricing_table(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        conn.execute_batch(
            r#"
            -- 模型价格表
            CREATE TABLE IF NOT EXISTS model_pricing (
                model_id TEXT PRIMARY KEY,
                display_name TEXT,
                input_price REAL NOT NULL,
                output_price REAL NOT NULL,
                cache_read_price REAL,
                cache_write_price REAL,
                source TEXT NOT NULL DEFAULT 'api',
                last_updated INTEGER NOT NULL
            );

            -- 创建索引加速搜索
            CREATE INDEX IF NOT EXISTS idx_model_pricing_search ON model_pricing(model_id, display_name);
            "#,
        )
        .map_err(|e| format!("Failed to create model_pricing table: {}", e))?;
        Ok(())
    }

    /// 批量插入/更新模型价格（用于同步 API 数据）
    pub fn upsert_model_pricings(&self, pricings: &[ModelPricingConfig]) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let mut count = 0;

        for pricing in pricings {
            let result = conn.execute(
                r#"
                INSERT INTO model_pricing (model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(model_id) DO UPDATE SET
                    display_name = excluded.display_name,
                    input_price = excluded.input_price,
                    output_price = excluded.output_price,
                    cache_read_price = excluded.cache_read_price,
                    cache_write_price = excluded.cache_write_price,
                    source = excluded.source,
                    last_updated = excluded.last_updated
                "#,
                rusqlite::params![
                    pricing.model_id,
                    pricing.display_name,
                    pricing.input_price,
                    pricing.output_price,
                    pricing.cache_read_price,
                    pricing.cache_write_price,
                    pricing.source,
                    pricing.last_updated,
                ],
            );
            if result.is_ok() {
                count += 1;
            }
        }

        Ok(count)
    }

    /// 搜索模型价格（支持分页和关键词搜索）
    pub fn search_model_pricings(&self, query: Option<&str>, limit: i64, offset: i64) -> Result<Vec<ModelPricingConfig>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let pricings = if let Some(q) = query {
            let search_pattern = format!("%{}%", q.to_lowercase());
            let mut stmt = conn.prepare(
                r#"
                SELECT model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated
                FROM model_pricing
                WHERE model_id LIKE ?1 OR LOWER(display_name) LIKE ?1
                ORDER BY model_id
                LIMIT ?2 OFFSET ?3
                "#
            ).map_err(|e| format!("Failed to prepare statement: {}", e))?;

            let rows = stmt.query_map(
                rusqlite::params![search_pattern, limit, offset],
                |row| {
                    Ok(ModelPricingConfig {
                        model_id: row.get(0)?,
                        display_name: row.get(1)?,
                        input_price: row.get(2)?,
                        output_price: row.get(3)?,
                        cache_read_price: row.get(4)?,
                        cache_write_price: row.get(5)?,
                        source: row.get(6)?,
                        last_updated: row.get(7)?,
                    })
                },
            ).map_err(|e| format!("Failed to search model pricings: {}", e))?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect results: {}", e))?
        } else {
            let mut stmt = conn.prepare(
                r#"
                SELECT model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated
                FROM model_pricing
                ORDER BY model_id
                LIMIT ?1 OFFSET ?2
                "#
            ).map_err(|e| format!("Failed to prepare statement: {}", e))?;

            let rows = stmt.query_map(
                rusqlite::params![limit, offset],
                |row| {
                    Ok(ModelPricingConfig {
                        model_id: row.get(0)?,
                        display_name: row.get(1)?,
                        input_price: row.get(2)?,
                        output_price: row.get(3)?,
                        cache_read_price: row.get(4)?,
                        cache_write_price: row.get(5)?,
                        source: row.get(6)?,
                        last_updated: row.get(7)?,
                    })
                },
            ).map_err(|e| format!("Failed to query model pricings: {}", e))?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect results: {}", e))?
        };

        Ok(pricings)
    }

    /// 获取模型价格总数
    pub fn count_model_pricings(&self, query: Option<&str>) -> Result<i64, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let count = if let Some(q) = query {
            let search_pattern = format!("%{}%", q.to_lowercase());
            conn.query_row(
                "SELECT COUNT(*) FROM model_pricing WHERE model_id LIKE ?1 OR LOWER(display_name) LIKE ?1",
                rusqlite::params![search_pattern],
                |row| row.get(0),
            )
        } else {
            conn.query_row("SELECT COUNT(*) FROM model_pricing", [], |row| row.get(0))
        }
        .map_err(|e| format!("Failed to count model pricings: {}", e))?;

        Ok(count)
    }

    /// 添加自定义模型价格
    pub fn add_custom_pricing(&self, pricing: &ModelPricingConfig) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        conn.execute(
            r#"
            INSERT INTO model_pricing (model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            rusqlite::params![
                pricing.model_id,
                pricing.display_name,
                pricing.input_price,
                pricing.output_price,
                pricing.cache_read_price,
                pricing.cache_write_price,
                "custom",
                pricing.last_updated,
            ],
        )
        .map_err(|e| format!("Failed to add custom pricing: {}", e))?;
        Ok(())
    }

    /// 更新自定义模型价格
    pub fn update_custom_pricing(&self, pricing: &ModelPricingConfig) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        conn.execute(
            r#"
            UPDATE model_pricing SET
                display_name = ?2,
                input_price = ?3,
                output_price = ?4,
                cache_read_price = ?5,
                cache_write_price = ?6,
                last_updated = ?7
            WHERE model_id = ?1
            "#,
            rusqlite::params![
                pricing.model_id,
                pricing.display_name,
                pricing.input_price,
                pricing.output_price,
                pricing.cache_read_price,
                pricing.cache_write_price,
                pricing.last_updated,
            ],
        )
        .map_err(|e| format!("Failed to update custom pricing: {}", e))?;
        Ok(())
    }

    /// 删除模型价格
    pub fn delete_model_pricing(&self, model_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        conn.execute(
            "DELETE FROM model_pricing WHERE model_id = ?1",
            rusqlite::params![model_id],
        )
        .map_err(|e| format!("Failed to delete model pricing: {}", e))?;
        Ok(())
    }

    /// 根据 model_id 查找价格配置
    #[allow(dead_code)]
    pub fn get_model_pricing(&self, model_id: &str) -> Result<Option<ModelPricingConfig>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated FROM model_pricing WHERE model_id = ?1"
        ).map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let result = stmt.query_row(rusqlite::params![model_id], |row| {
            Ok(ModelPricingConfig {
                model_id: row.get(0)?,
                display_name: row.get(1)?,
                input_price: row.get(2)?,
                output_price: row.get(3)?,
                cache_read_price: row.get(4)?,
                cache_write_price: row.get(5)?,
                source: row.get(6)?,
                last_updated: row.get(7)?,
            })
        });

        match result {
            Ok(pricing) => Ok(Some(pricing)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to get model pricing: {}", e)),
        }
    }

    /// 获取所有模型价格配置（用于费用计算）
    pub fn get_all_model_pricings(&self) -> Result<Vec<ModelPricingConfig>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated FROM model_pricing ORDER BY model_id"
        ).map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let pricings = stmt.query_map([], |row| {
            Ok(ModelPricingConfig {
                model_id: row.get(0)?,
                display_name: row.get(1)?,
                input_price: row.get(2)?,
                output_price: row.get(3)?,
                cache_read_price: row.get(4)?,
                cache_write_price: row.get(5)?,
                source: row.get(6)?,
                last_updated: row.get(7)?,
            })
        })
        .map_err(|e| format!("Failed to query model pricings: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to collect results: {}", e))?;

        Ok(pricings)
    }
}
