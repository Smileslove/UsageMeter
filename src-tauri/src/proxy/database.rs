//! SQLite 数据库，用于持久化代理使用数据

use super::types::{SessionStats, UsageRecord};
use crate::models::{ModelPricingConfig, SourceFilter, ToolFilter, UsageQueryFilter};
use chrono::{Local, TimeZone};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

/// 全局数据库实例（用于查询操作，避免重复打开连接）
static GLOBAL_DB: OnceLock<Arc<ProxyDatabase>> = OnceLock::new();

const LEGACY_UNMATCHED_SESSION_ID: &str = "__legacy_unmatched__";

#[derive(Debug, Clone)]
pub struct DailyActivitySummary {
    pub date: String,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub request_count: u64,
    pub cost: f64,
    pub success_total_tokens: u64,
    pub success_input_tokens: u64,
    pub success_output_tokens: u64,
    pub success_cache_create_tokens: u64,
    pub success_cache_read_tokens: u64,
    pub success_cost: f64,
    pub model_count: u64,
    pub success_requests: u64,
    pub client_error_requests: u64,
    pub server_error_requests: u64,
}

/// 数据库管理器，用于代理使用数据
/// 使用线程安全的 SQLite 连接包装器
pub struct ProxyDatabase {
    conn: Arc<std::sync::Mutex<Connection>>,
}

impl ProxyDatabase {
    /// 获取全局数据库实例（用于查询操作）
    /// 如果数据库文件存在，返回共享的数据库实例
    /// 如果不存在，返回 None
    pub fn get_global() -> Option<Arc<ProxyDatabase>> {
        GLOBAL_DB.get().cloned().or_else(|| {
            // 尝试初始化全局实例
            let db_path = Self::get_db_path().ok()?;
            if db_path.exists() {
                if let Ok(db) = Self::new_with_path(&db_path) {
                    let db = Arc::new(db);
                    let _ = GLOBAL_DB.set(db.clone());
                    return Some(db);
                }
            }
            None
        })
    }

    /// 创建新的数据库连接
    pub fn new() -> Result<Self, String> {
        let db_path = Self::get_db_path()?;
        Self::new_with_path(&db_path)
    }

    /// 使用指定路径创建数据库连接（用于独立查询）
    pub fn new_with_path(db_path: &PathBuf) -> Result<Self, String> {
        // 确保父目录存在
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create database directory: {}", e))?;
        }

        let conn =
            Connection::open(db_path).map_err(|e| format!("Failed to open database: {}", e))?;

        // 启用 WAL 模式以获得更好的并发性
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("Failed to enable WAL mode: {}", e))?;
        conn.busy_timeout(Duration::from_secs(30))
            .map_err(|e| format!("Failed to set SQLite busy timeout: {}", e))?;

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
        // 检查 session_stats 表是否需要重建（旧版本表结构不兼容）
        let needs_rebuild = Self::check_session_stats_needs_rebuild(conn);

        if needs_rebuild {
            eprintln!("[database] Rebuilding session_stats table due to schema change");
            conn.execute("DROP TABLE IF EXISTS session_stats", [])
                .map_err(|e| format!("Failed to drop old session_stats table: {}", e))?;
        }

        // 创建基础表
        conn.execute_batch(
            r#"
            -- 使用记录表
            -- 存储单次 API 请求的使用数据
            -- 注意：不存储 total_tokens，查询时动态计算
            -- 总 Token = input_tokens + cache_create_tokens + cache_read_tokens + output_tokens
            -- 计费相关需要单独处理四种 token（价格不同）
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
                estimated_cost REAL NOT NULL DEFAULT 0,
                pricing_snapshot_id TEXT,
                cost_locked INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
            );

            -- 索引用于加速查询
            CREATE INDEX IF NOT EXISTS idx_timestamp ON usage_records(timestamp);
            CREATE INDEX IF NOT EXISTS idx_message_id ON usage_records(message_id);
            CREATE INDEX IF NOT EXISTS idx_session_id ON usage_records(session_id);
            -- 组合索引用于加速按模型分组的速率统计查询
            CREATE INDEX IF NOT EXISTS idx_model_timestamp ON usage_records(model, timestamp);

            -- 会话性能统计表
            -- 存储代理独有数据，与 JSONL 数据合并使用
            CREATE TABLE IF NOT EXISTS session_stats (
                session_id TEXT PRIMARY KEY,
                -- 性能指标
                total_duration_ms INTEGER NOT NULL DEFAULT 0,
                avg_output_tokens_per_second REAL NOT NULL DEFAULT 0,
                avg_ttft_ms REAL NOT NULL DEFAULT 0,
                -- 请求统计
                proxy_request_count INTEGER NOT NULL DEFAULT 0,
                success_requests INTEGER NOT NULL DEFAULT 0,
                error_requests INTEGER NOT NULL DEFAULT 0,
                -- Token 统计（代理视角）
                total_input_tokens INTEGER NOT NULL DEFAULT 0,
                total_output_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                -- 模型信息
                models TEXT,
                -- 时间范围
                first_request_time INTEGER,
                last_request_time INTEGER,
                -- 费用估算
                estimated_cost REAL NOT NULL DEFAULT 0,
                -- 元数据
                last_updated INTEGER NOT NULL DEFAULT 0
            );

            -- 索引用于加速更新时间查询
            CREATE INDEX IF NOT EXISTS idx_session_stats_updated ON session_stats(last_updated);

            -- 每日汇总表（用于更快的聚合）
            -- total_tokens = input_tokens + cache_create_tokens + cache_read_tokens + output_tokens（总 Token）
            CREATE TABLE IF NOT EXISTS daily_summary (
                date TEXT PRIMARY KEY,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                request_count INTEGER NOT NULL DEFAULT 0,
                cost REAL NOT NULL DEFAULT 0,
                success_total_tokens INTEGER NOT NULL DEFAULT 0,
                success_input_tokens INTEGER NOT NULL DEFAULT 0,
                success_output_tokens INTEGER NOT NULL DEFAULT 0,
                success_cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                success_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                success_cost REAL NOT NULL DEFAULT 0,
                model_count INTEGER NOT NULL DEFAULT 0,
                success_requests INTEGER NOT NULL DEFAULT 0,
                client_error_requests INTEGER NOT NULL DEFAULT 0,
                server_error_requests INTEGER NOT NULL DEFAULT 0,
                finalized_at INTEGER NOT NULL DEFAULT 0
            );

            -- 模型使用量表
            -- total_tokens = input_tokens + cache_create_tokens + cache_read_tokens + output_tokens（总 Token）
            CREATE TABLE IF NOT EXISTS model_usage (
                date TEXT NOT NULL,
                model TEXT NOT NULL,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                request_count INTEGER NOT NULL DEFAULT 0,
                cost REAL NOT NULL DEFAULT 0,
                success_requests INTEGER NOT NULL DEFAULT 0,
                client_error_requests INTEGER NOT NULL DEFAULT 0,
                server_error_requests INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (date, model)
            );
            "#,
        )
        .map_err(|e| format!("Failed to create tables: {}", e))?;

        Ok(())
    }

    /// 检查 session_stats 表是否需要重建
    fn check_session_stats_needs_rebuild(conn: &Connection) -> bool {
        // 获取表的列信息
        let columns: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('session_stats')")
            .and_then(|mut stmt| {
                let mut cols = Vec::new();
                let rows = stmt.query_map([], |row| row.get(0))?;
                for row in rows {
                    cols.push(row?);
                }
                Ok(cols)
            })
            .unwrap_or_default();

        // 检查必要的列是否存在
        let required_columns = [
            "proxy_request_count",
            "success_requests",
            "error_requests",
            "avg_ttft_ms",
            "last_updated",
        ];

        for col in &required_columns {
            if !columns.iter().any(|c| c == col) {
                eprintln!("[database] session_stats missing column: {}", col);
                return true;
            }
        }

        false
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
            "ALTER TABLE usage_records ADD COLUMN migration_attempted_at INTEGER",
            "ALTER TABLE usage_records ADD COLUMN estimated_cost REAL NOT NULL DEFAULT 0",
            "ALTER TABLE usage_records ADD COLUMN pricing_snapshot_id TEXT",
            "ALTER TABLE usage_records ADD COLUMN cost_locked INTEGER NOT NULL DEFAULT 0",
            // 来源识别字段
            "ALTER TABLE usage_records ADD COLUMN api_key_prefix TEXT",
            "ALTER TABLE usage_records ADD COLUMN request_base_url TEXT",
            // 客户端工具识别字段（单端口 + path prefix）
            "ALTER TABLE usage_records ADD COLUMN client_tool TEXT NOT NULL DEFAULT 'claude_code'",
            "ALTER TABLE usage_records ADD COLUMN proxy_profile_id TEXT",
            "ALTER TABLE usage_records ADD COLUMN client_detection_method TEXT NOT NULL DEFAULT 'legacy_path'",
            "ALTER TABLE daily_summary ADD COLUMN cost REAL NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN success_total_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN success_input_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN success_output_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN success_cache_create_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN success_cache_read_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN success_cost REAL NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN model_count INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN success_requests INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN client_error_requests INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN server_error_requests INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN finalized_at INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE model_usage ADD COLUMN cost REAL NOT NULL DEFAULT 0",
            "ALTER TABLE model_usage ADD COLUMN success_requests INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE model_usage ADD COLUMN client_error_requests INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE model_usage ADD COLUMN server_error_requests INTEGER NOT NULL DEFAULT 0",
        ];

        for migration in migrations {
            // SQLite 不支持 ALTER TABLE ADD COLUMN 的 IF NOT EXISTS
            // 所以我们忽略错误（字段已存在时会报错）
            let _ = conn.execute(migration, []);
        }

        // 创建来源查询索引（忽略已存在错误）
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_source_lookup ON usage_records(api_key_prefix, request_base_url)",
            []
        );
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_usage_tool_source ON usage_records(client_tool, api_key_prefix, request_base_url)",
            [],
        );
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_usage_tool_time ON usage_records(client_tool, timestamp)",
            [],
        );

        Ok(())
    }

    fn pricing_snapshot_id(pricings: &[ModelPricingConfig], match_mode: &str) -> String {
        let mut normalized = pricings.to_vec();
        normalized.sort_by(|a, b| {
            a.model_id
                .cmp(&b.model_id)
                .then_with(|| a.source.cmp(&b.source))
                .then_with(|| a.last_updated.cmp(&b.last_updated))
        });
        let payload = serde_json::json!({
            "matchMode": match_mode,
            "pricings": normalized,
        });
        let mut hasher = DefaultHasher::new();
        payload.to_string().hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    fn record_local_date(timestamp_ms: i64) -> String {
        Local
            .timestamp_opt(timestamp_ms / 1000, 0)
            .single()
            .unwrap_or_else(Local::now)
            .format("%Y-%m-%d")
            .to_string()
    }

    fn today_local_date() -> String {
        Local::now().format("%Y-%m-%d").to_string()
    }

    fn current_pricing_context(&self) -> (Vec<ModelPricingConfig>, String, String) {
        let settings = crate::commands::load_settings().unwrap_or_default();
        let mut pricings = settings.model_pricing.pricings;
        if let Ok(db_pricings) = self.get_all_model_pricings() {
            pricings.extend(db_pricings);
        }
        let match_mode = settings.model_pricing.match_mode;
        let snapshot_id = Self::pricing_snapshot_id(&pricings, &match_mode);
        (pricings, match_mode, snapshot_id)
    }

    fn estimate_record_cost(
        record: &UsageRecord,
        pricings: &[ModelPricingConfig],
        match_mode: &str,
    ) -> f64 {
        crate::models::estimate_session_cost(
            record.input_tokens,
            record.output_tokens,
            record.cache_create_tokens,
            record.cache_read_tokens,
            &record.model,
            pricings,
            match_mode,
        )
    }

    fn usage_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageRecord> {
        let input = Self::safe_i64_to_u64(row.get::<_, i64>(2)?);
        let output = Self::safe_i64_to_u64(row.get::<_, i64>(3)?);
        let cache_create = Self::safe_i64_to_u64(row.get::<_, i64>(4)?);
        let cache_read = Self::safe_i64_to_u64(row.get::<_, i64>(5)?);
        Ok(UsageRecord {
            timestamp: row.get::<_, i64>(0)?,
            message_id: row.get(1)?,
            input_tokens: input,
            output_tokens: output,
            cache_create_tokens: cache_create,
            cache_read_tokens: cache_read,
            reasoning_tokens: 0,
            total_tokens: input + cache_create + cache_read + output,
            model: row.get(6)?,
            session_id: row.get(7)?,
            request_start_time: row.get::<_, Option<i64>>(8)?.unwrap_or(0),
            request_end_time: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
            duration_ms: row.get::<_, i64>(10)? as u64,
            output_tokens_per_second: row.get(11)?,
            ttft_ms: row.get::<_, Option<i64>>(12)?.map(|v| v as u64),
            status_code: row.get::<_, i64>(13)? as u16,
            estimated_cost: row.get::<_, Option<f64>>(14)?.unwrap_or(0.0),
            pricing_snapshot_id: row.get(15)?,
            cost_locked: row.get::<_, Option<i64>>(16)?.unwrap_or(0) != 0,
            api_key_prefix: row.get(17)?,
            request_base_url: row.get(18)?,
            client_tool: row
                .get::<_, Option<String>>(19)?
                .unwrap_or_else(|| crate::models::DEFAULT_CLIENT_TOOL.to_string()),
            proxy_profile_id: row.get(20)?,
            client_detection_method: row
                .get::<_, Option<String>>(21)?
                .unwrap_or_else(|| crate::models::DEFAULT_CLIENT_DETECTION_METHOD.to_string()),
        })
    }

    /// 插入使用记录
    pub async fn insert_record(&self, record: &UsageRecord) -> Result<i64, String> {
        let (pricings, match_mode, snapshot_id) = self.current_pricing_context();
        let estimated_cost = if record.cost_locked {
            record.estimated_cost
        } else {
            Self::estimate_record_cost(record, &pricings, &match_mode)
        };
        let pricing_snapshot_id = record
            .pricing_snapshot_id
            .clone()
            .unwrap_or_else(|| snapshot_id.clone());
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        conn.execute(
            r#"
            INSERT OR REPLACE INTO usage_records
            (timestamp, message_id, input_tokens, output_tokens, cache_create_tokens,
             cache_read_tokens, model, session_id, request_start_time,
             request_end_time, duration_ms, output_tokens_per_second, ttft_ms, status_code,
             estimated_cost, pricing_snapshot_id, cost_locked, api_key_prefix, request_base_url,
             client_tool, proxy_profile_id, client_detection_method)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 1, ?17, ?18, ?19, ?20, ?21)
            "#,
            rusqlite::params![
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
                estimated_cost,
                pricing_snapshot_id,
                &record.api_key_prefix,
                &record.request_base_url,
                &record.client_tool,
                &record.proxy_profile_id,
                &record.client_detection_method,
            ],
        )
        .map_err(|e| format!("Failed to insert record: {}", e))?;

        let id = conn.last_insert_rowid();
        let date = Self::record_local_date(record.timestamp);
        if date < Self::today_local_date() {
            Self::refresh_daily_summary_for_date_conn(&conn, &date)?;
        }
        Ok(id)
    }

    fn refresh_daily_summary_for_date_conn(conn: &Connection, date: &str) -> Result<(), String> {
        conn.execute("DELETE FROM daily_summary WHERE date = ?1", [date])
            .map_err(|e| format!("Failed to clear daily summary: {}", e))?;
        conn.execute("DELETE FROM model_usage WHERE date = ?1", [date])
            .map_err(|e| format!("Failed to clear model usage: {}", e))?;

        conn.execute(
            r#"
            INSERT INTO daily_summary (
                date, total_tokens, input_tokens, output_tokens, cache_create_tokens,
                cache_read_tokens, request_count, cost, success_total_tokens,
                success_input_tokens, success_output_tokens, success_cache_create_tokens,
                success_cache_read_tokens, success_cost, model_count, success_requests,
                client_error_requests, server_error_requests, finalized_at
            )
            SELECT
                ?1,
                COALESCE(SUM(input_tokens + cache_create_tokens + cache_read_tokens + output_tokens), 0),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(cache_create_tokens), 0),
                COALESCE(SUM(cache_read_tokens), 0),
                COUNT(*),
                COALESCE(SUM(estimated_cost), 0),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN input_tokens + cache_create_tokens + cache_read_tokens + output_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN input_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN output_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN cache_create_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN cache_read_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN estimated_cost ELSE 0 END), 0),
                COUNT(DISTINCT CASE WHEN model != '' THEN model END),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 400 AND status_code < 500 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 500 THEN 1 ELSE 0 END), 0),
                ?2
            FROM usage_records
            WHERE date(timestamp / 1000, 'unixepoch', 'localtime') = ?1
            HAVING COUNT(*) > 0
            "#,
            rusqlite::params![date, chrono::Utc::now().timestamp_millis()],
        )
        .map_err(|e| format!("Failed to refresh daily summary: {}", e))?;

        conn.execute(
            r#"
            INSERT INTO model_usage (
                date, model, total_tokens, input_tokens, output_tokens, cache_create_tokens,
                cache_read_tokens, request_count, cost, success_requests,
                client_error_requests, server_error_requests
            )
            SELECT
                ?1,
                model,
                COALESCE(SUM(input_tokens + cache_create_tokens + cache_read_tokens + output_tokens), 0),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(cache_create_tokens), 0),
                COALESCE(SUM(cache_read_tokens), 0),
                COUNT(*),
                COALESCE(SUM(estimated_cost), 0),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 400 AND status_code < 500 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 500 THEN 1 ELSE 0 END), 0)
            FROM usage_records
            WHERE date(timestamp / 1000, 'unixepoch', 'localtime') = ?1
            GROUP BY model
            "#,
            [date],
        )
        .map_err(|e| format!("Failed to refresh model usage: {}", e))?;

        Ok(())
    }

    pub async fn backfill_unlocked_costs(&self) -> Result<usize, String> {
        let (pricings, match_mode, snapshot_id) = self.current_pricing_context();
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let records = {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, timestamp, input_tokens, output_tokens, cache_create_tokens,
                           cache_read_tokens, model
                    FROM usage_records
                    WHERE cost_locked = 0 OR cost_locked IS NULL
                    "#,
                )
                .map_err(|e| format!("Failed to prepare cost backfill query: {}", e))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        Self::safe_i64_to_u64(row.get::<_, i64>(2)?),
                        Self::safe_i64_to_u64(row.get::<_, i64>(3)?),
                        Self::safe_i64_to_u64(row.get::<_, i64>(4)?),
                        Self::safe_i64_to_u64(row.get::<_, i64>(5)?),
                        row.get::<_, String>(6)?,
                    ))
                })
                .map_err(|e| format!("Failed to query cost backfill records: {}", e))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect cost backfill records: {}", e))?;
            rows
        };

        if records.is_empty() {
            return Ok(0);
        }

        let mut touched_dates = std::collections::HashSet::new();
        let mut stmt = conn
            .prepare(
                r#"
                UPDATE usage_records
                SET estimated_cost = ?1, pricing_snapshot_id = ?2, cost_locked = 1
                WHERE id = ?3
                "#,
            )
            .map_err(|e| format!("Failed to prepare cost backfill update: {}", e))?;

        for (id, timestamp, input, output, cache_create, cache_read, model) in &records {
            let cost = crate::models::estimate_session_cost(
                *input,
                *output,
                *cache_create,
                *cache_read,
                model,
                &pricings,
                &match_mode,
            );
            stmt.execute(rusqlite::params![cost, snapshot_id, id])
                .map_err(|e| format!("Failed to update cost backfill record: {}", e))?;
            let date = Self::record_local_date(*timestamp);
            if date < Self::today_local_date() {
                touched_dates.insert(date);
            }
        }
        drop(stmt);

        for date in touched_dates {
            Self::refresh_daily_summary_for_date_conn(&conn, &date)?;
        }

        eprintln!(
            "[database] Backfilled frozen cost for {} usage records",
            records.len()
        );
        Ok(records.len())
    }

    /// 构建匹配模型列表和查询参数
    ///
    /// 精确模式：直接构造 `model = ?`，无需查询全表去重模型。
    /// 模糊模式：查询所有去重模型名，在 Rust 侧做模糊匹配后构造 `model IN (?...)`。
    ///
    /// 安全说明：此函数用 `format!` 构建 SQL 骨架（占位符编号、IN 子句结构），
    /// 所有实际值均通过 `rusqlite::params!` 参数化绑定，不存在 SQL 注入风险。
    fn build_pricing_match_params(
        conn: &rusqlite::Connection,
        filter: &PricingMatchFilter<'_>,
    ) -> Result<PricingMatchQuery, String> {
        if filter.match_mode == "exact" {
            return Self::build_exact_match_params(filter);
        }
        Self::build_fuzzy_match_params(conn, filter)
    }

    /// 附加筛选条件：时间范围、client_tool、api_key_prefix
    ///
    /// 将非模型匹配的筛选条件追加到 `params` 和 `extra_conditions` 中。
    /// 占位符编号从当前 params 长度 + 1 开始，保证与上游参数顺序一致。
    fn push_extra_conditions(
        filter: &PricingMatchFilter<'_>,
        params: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
        extra_conditions: &mut String,
    ) {
        if let Some(start) = filter.time_range_start {
            extra_conditions.push_str(&format!(" AND timestamp >= ?{}", params.len() + 1));
            params.push(Box::new(start));
        }
        if let Some(end) = filter.time_range_end {
            extra_conditions.push_str(&format!(" AND timestamp <= ?{}", params.len() + 1));
            params.push(Box::new(end));
        }
        if let Some(tool) = filter.client_tool_filter {
            extra_conditions.push_str(&format!(" AND client_tool = ?{}", params.len() + 1));
            params.push(Box::new(tool.to_string()));
        }
        if let Some(prefixes) = filter.api_source_key_prefixes {
            if !prefixes.is_empty() {
                let prefix_placeholders: Vec<String> = prefixes
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("?{}", params.len() + 1 + i))
                    .collect();
                extra_conditions.push_str(&format!(
                    " AND api_key_prefix IN ({})",
                    prefix_placeholders.join(",")
                ));
                for p in prefixes {
                    params.push(Box::new(p.clone()));
                }
            }
        }
    }

    /// 精确匹配：直接使用 `model = ?`，无需查询全表
    fn build_exact_match_params(
        filter: &PricingMatchFilter<'_>,
    ) -> Result<PricingMatchQuery, String> {
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(filter.model_id.to_string())];
        let mut extra_conditions = String::new();

        Self::push_extra_conditions(filter, &mut params, &mut extra_conditions);

        Ok(PricingMatchQuery {
            matched_models: vec![filter.model_id.to_string()],
            where_clause: format!("= ?1{}", extra_conditions),
            params,
        })
    }

    /// 模糊匹配：查询所有去重模型后在 Rust 侧做模糊匹配
    fn build_fuzzy_match_params(
        conn: &rusqlite::Connection,
        filter: &PricingMatchFilter<'_>,
    ) -> Result<PricingMatchQuery, String> {
        // 注意：format! 仅用于构建占位符编号骨架，所有值通过 params 参数化绑定
        // 获取所有去重的模型名
        let mut stmt = conn
            .prepare("SELECT DISTINCT model FROM usage_records WHERE model != ''")
            .map_err(|e| format!("Failed to prepare model query: {}", e))?;
        let all_models: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query models: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect models: {}", e))?;

        let pricing_config = crate::models::ModelPricingConfig {
            model_id: filter.model_id.to_string(),
            display_name: None,
            input_price: 0.0,
            output_price: 0.0,
            cache_write_price: None,
            cache_read_price: None,
            source: String::new(),
            last_updated: 0,
        };
        let normalized = crate::models::normalize_model_id(filter.model_id);
        let matched_models: Vec<String> = all_models
            .into_iter()
            .filter(|m| crate::models::fuzzy_match_score(m, &normalized, &pricing_config).is_some())
            .collect();

        if matched_models.is_empty() {
            return Ok(PricingMatchQuery {
                matched_models: vec![],
                where_clause: String::new(),
                params: vec![],
            });
        }

        // 构建 IN 子句
        let placeholders: Vec<String> = matched_models
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect();
        let in_clause = placeholders.join(",");

        let mut extra_conditions = String::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = matched_models
            .iter()
            .map(|m| Box::new(m.clone()) as Box<dyn rusqlite::types::ToSql>)
            .collect();

        Self::push_extra_conditions(filter, &mut params, &mut extra_conditions);

        Ok(PricingMatchQuery {
            matched_models,
            where_clause: format!("IN ({}){}", in_clause, extra_conditions),
            params,
        })
    }

    /// 安全地将 i64 转换为 u64，负值返回 0
    fn safe_i64_to_u64(v: i64) -> u64 {
        if v < 0 {
            0
        } else {
            v as u64
        }
    }

    /// 预览按模型名和时间范围匹配的记录
    pub async fn preview_pricing_apply(
        &self,
        filter: &PricingMatchFilter<'_>,
    ) -> Result<PreviewPricingApplyResult, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let query = Self::build_pricing_match_params(&conn, filter)?;

        if query.matched_models.is_empty() {
            return Ok(PreviewPricingApplyResult {
                matched_count: 0,
                total_current_cost: 0.0,
                model_counts: vec![],
            });
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            query.params.iter().map(|p| p.as_ref()).collect();

        // 总计查询（仅统计未被锁定的记录，与 apply 保持一致）
        // where_clause 由 build_*_match_params 构建，占位符均参数化绑定
        let sql = format!(
            r#"
            SELECT COUNT(*), COALESCE(SUM(estimated_cost), 0)
            FROM usage_records
            WHERE model {}
              AND (cost_locked = 0 OR cost_locked IS NULL)
            "#,
            query.where_clause
        );
        let (matched_count, total_current_cost) = conn
            .query_row(&sql, param_refs.as_slice(), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
            })
            .map_err(|e| format!("Failed to preview pricing apply: {}", e))?;

        // 按模型分组查询（仅统计未被锁定的记录）
        let sql_models = format!(
            r#"
            SELECT model, COUNT(*) as cnt
            FROM usage_records
            WHERE model {}
              AND (cost_locked = 0 OR cost_locked IS NULL)
            GROUP BY model
            ORDER BY cnt DESC
            "#,
            query.where_clause
        );
        let mut stmt = conn
            .prepare(&sql_models)
            .map_err(|e| format!("Failed to prepare model count query: {}", e))?;
        let model_counts: Vec<ModelMatchCount> = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok(ModelMatchCount {
                    model: row.get::<_, String>(0)?,
                    count: row.get::<_, i64>(1)?,
                })
            })
            .map_err(|e| format!("Failed to query model counts: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect model counts: {}", e))?;

        Ok(PreviewPricingApplyResult {
            matched_count,
            total_current_cost,
            model_counts,
        })
    }

    /// 将指定价格应用到匹配的历史记录
    pub async fn apply_pricing_to_records(
        &self,
        pricing: &crate::models::ModelPricingConfig,
        filter: &PricingMatchFilter<'_>,
    ) -> Result<i64, String> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let query = Self::build_pricing_match_params(&conn, filter)?;

        if query.matched_models.is_empty() {
            return Ok(0);
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            query.params.iter().map(|p| p.as_ref()).collect();

        // where_clause 由 build_*_match_params 构建，占位符均参数化绑定
        let sql = format!(
            r#"
            SELECT id, timestamp, input_tokens, output_tokens, cache_create_tokens,
                   cache_read_tokens, model
            FROM usage_records
            WHERE model {}
              AND (cost_locked = 0 OR cost_locked IS NULL)
            "#,
            query.where_clause
        );

        let records: Vec<(i64, i64, u64, u64, u64, u64, String)> = {
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| format!("Failed to prepare apply query: {}", e))?;
            let rows: Vec<(i64, i64, u64, u64, u64, u64, String)> = stmt
                .query_map(param_refs.as_slice(), |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        Self::safe_i64_to_u64(row.get::<_, i64>(2)?),
                        Self::safe_i64_to_u64(row.get::<_, i64>(3)?),
                        Self::safe_i64_to_u64(row.get::<_, i64>(4)?),
                        Self::safe_i64_to_u64(row.get::<_, i64>(5)?),
                        row.get::<_, String>(6)?,
                    ))
                })
                .map_err(|e| format!("Failed to query records for apply: {}", e))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect records for apply: {}", e))?;
            rows
        };

        if records.is_empty() {
            return Ok(0);
        }

        // 使用传入的 pricing 计算新费用
        let pricings = vec![pricing.clone()];
        let snapshot_id = Self::pricing_snapshot_id(&pricings, "exact");

        // 收集需要刷新的历史日期（今天之前的）
        let touched_dates: std::collections::HashSet<String> = records
            .iter()
            .filter(|(_, timestamp, _, _, _, _, _)| {
                let date = Self::record_local_date(*timestamp);
                date < Self::today_local_date()
            })
            .map(|(_, timestamp, _, _, _, _, _)| Self::record_local_date(*timestamp))
            .collect();

        // 使用事务确保原子性，分批处理以减少内存峰值
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to begin transaction: {}", e))?;

        const BATCH_SIZE: usize = 1000;
        let mut total_updated: i64 = 0;

        let mut update_stmt = tx
            .prepare(
                r#"
                UPDATE usage_records
                SET estimated_cost = ?1, pricing_snapshot_id = ?2, cost_locked = 1
                WHERE id = ?3
                "#,
            )
            .map_err(|e| format!("Failed to prepare update statement: {}", e))?;

        for batch in records.chunks(BATCH_SIZE) {
            for (id, _timestamp, input, output, cache_create, cache_read, model) in batch {
                let cost = crate::models::estimate_session_cost(
                    *input,
                    *output,
                    *cache_create,
                    *cache_read,
                    model,
                    &pricings,
                    "exact",
                );
                update_stmt
                    .execute(rusqlite::params![cost, &snapshot_id, id])
                    .map_err(|e| format!("Failed to update record: {}", e))?;
                total_updated += 1;
            }
        }
        drop(update_stmt);

        // 刷新受影响日期的 daily_summary（在事务内）
        for date in &touched_dates {
            Self::refresh_daily_summary_for_date_conn(&tx, date)?;
        }

        tx.commit()
            .map_err(|e| format!("Failed to commit transaction: {}", e))?;

        eprintln!(
            "[database] Applied pricing to {} records for model '{}'",
            total_updated, filter.model_id
        );
        Ok(total_updated)
    }

    pub async fn ensure_daily_summaries(
        &self,
        start_date: &str,
        end_date_exclusive: &str,
    ) -> Result<(), String> {
        self.backfill_unlocked_costs().await?;

        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let dates = {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT DISTINCT date(timestamp / 1000, 'unixepoch', 'localtime') as date_key
                    FROM usage_records
                    WHERE date(timestamp / 1000, 'unixepoch', 'localtime') >= ?1
                      AND date(timestamp / 1000, 'unixepoch', 'localtime') < ?2
                    "#,
                )
                .map_err(|e| format!("Failed to prepare summary date query: {}", e))?;
            let rows = stmt
                .query_map(rusqlite::params![start_date, end_date_exclusive], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| format!("Failed to query summary dates: {}", e))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect summary dates: {}", e))?;
            rows
        };

        let existing = {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT date FROM daily_summary
                    WHERE date >= ?1 AND date < ?2
                    "#,
                )
                .map_err(|e| format!("Failed to prepare existing summary query: {}", e))?;
            let rows = stmt
                .query_map(rusqlite::params![start_date, end_date_exclusive], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| format!("Failed to query existing summaries: {}", e))?
                .collect::<Result<std::collections::HashSet<_>, _>>()
                .map_err(|e| format!("Failed to collect existing summaries: {}", e))?;
            rows
        };

        for date in dates {
            if !existing.contains(&date) {
                Self::refresh_daily_summary_for_date_conn(&conn, &date)?;
            }
        }

        Ok(())
    }

    pub async fn get_daily_activity_summaries(
        &self,
        start_date: &str,
        end_date_exclusive: &str,
    ) -> Result<Vec<DailyActivitySummary>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT date, total_tokens, input_tokens, output_tokens, cache_create_tokens,
                       cache_read_tokens, request_count, cost, success_total_tokens,
                       success_input_tokens, success_output_tokens, success_cache_create_tokens,
                       success_cache_read_tokens, success_cost, model_count, success_requests,
                       client_error_requests, server_error_requests
                FROM daily_summary
                WHERE date >= ?1 AND date < ?2
                ORDER BY date ASC
                "#,
            )
            .map_err(|e| format!("Failed to prepare daily summaries query: {}", e))?;

        let rows = stmt
            .query_map(rusqlite::params![start_date, end_date_exclusive], |row| {
                Ok(DailyActivitySummary {
                    date: row.get(0)?,
                    total_tokens: row.get::<_, i64>(1)? as u64,
                    input_tokens: row.get::<_, i64>(2)? as u64,
                    output_tokens: row.get::<_, i64>(3)? as u64,
                    cache_create_tokens: row.get::<_, i64>(4)? as u64,
                    cache_read_tokens: row.get::<_, i64>(5)? as u64,
                    request_count: row.get::<_, i64>(6)? as u64,
                    cost: row.get::<_, f64>(7)?,
                    success_total_tokens: row.get::<_, i64>(8)? as u64,
                    success_input_tokens: row.get::<_, i64>(9)? as u64,
                    success_output_tokens: row.get::<_, i64>(10)? as u64,
                    success_cache_create_tokens: row.get::<_, i64>(11)? as u64,
                    success_cache_read_tokens: row.get::<_, i64>(12)? as u64,
                    success_cost: row.get::<_, f64>(13)?,
                    model_count: row.get::<_, i64>(14)? as u64,
                    success_requests: row.get::<_, i64>(15)? as u64,
                    client_error_requests: row.get::<_, i64>(16)? as u64,
                    server_error_requests: row.get::<_, i64>(17)? as u64,
                })
            })
            .map_err(|e| format!("Failed to query daily summaries: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect daily summaries: {}", e))?;
        Ok(rows)
    }

    /// 获取时间窗口内的记录
    #[allow(dead_code)]
    pub async fn get_records_since(&self, cutoff_ms: i64) -> Result<Vec<UsageRecord>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT timestamp, message_id, input_tokens, output_tokens,
                       cache_create_tokens, cache_read_tokens, model, session_id,
                       request_start_time, request_end_time, duration_ms, output_tokens_per_second,
                       ttft_ms, status_code, estimated_cost, pricing_snapshot_id, cost_locked,
                       api_key_prefix, request_base_url, client_tool, proxy_profile_id,
                       client_detection_method
                FROM usage_records
                WHERE timestamp >= ?1
                ORDER BY timestamp DESC
                "#,
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let records = stmt
            .query_map([cutoff_ms], Self::usage_record_from_row)
            .map_err(|e| format!("Failed to query records: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect records: {}", e))?;

        Ok(records)
    }

    /// 获取指定时间范围内的记录
    ///
    /// 使用半开区间 [start_ms, end_ms)，便于前端按日期和小时拼接连续范围。
    pub async fn get_records_between(
        &self,
        start_ms: i64,
        end_ms: i64,
        include_errors: bool,
    ) -> Result<Vec<UsageRecord>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let status_filter = if include_errors {
            ""
        } else {
            "AND status_code >= 200 AND status_code < 300"
        };

        let sql = format!(
            r#"
            SELECT timestamp, message_id, input_tokens, output_tokens,
                   cache_create_tokens, cache_read_tokens, model, session_id,
                   request_start_time, request_end_time, duration_ms, output_tokens_per_second,
                   ttft_ms, status_code, estimated_cost, pricing_snapshot_id, cost_locked,
                   api_key_prefix, request_base_url, client_tool, proxy_profile_id,
                   client_detection_method
            FROM usage_records
            WHERE timestamp >= ?1 AND timestamp < ?2
              {status_filter}
            ORDER BY timestamp ASC
            "#
        );

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let records = stmt
            .query_map(
                rusqlite::params![start_ms, end_ms],
                Self::usage_record_from_row,
            )
            .map_err(|e| format!("Failed to query records: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect records: {}", e))?;

        Ok(records)
    }

    /// 获取指定时间范围内的记录（带来源过滤）
    ///
    /// 使用半开区间 [start_ms, end_ms)，便于前端按日期和小时拼接连续范围。
    pub async fn get_records_between_with_source(
        &self,
        start_ms: i64,
        end_ms: i64,
        include_errors: bool,
        usage_filter: &UsageQueryFilter,
    ) -> Result<Vec<UsageRecord>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let status_filter = if include_errors {
            ""
        } else {
            "AND status_code >= 200 AND status_code < 300"
        };

        let (filter_where, filter_params) = Self::build_usage_filter_sql(usage_filter);

        let sql = format!(
            r#"
            SELECT timestamp, message_id, input_tokens, output_tokens,
                   cache_create_tokens, cache_read_tokens, model, session_id,
                   request_start_time, request_end_time, duration_ms, output_tokens_per_second,
                   ttft_ms, status_code, estimated_cost, pricing_snapshot_id, cost_locked,
                   api_key_prefix, request_base_url, client_tool, proxy_profile_id,
                   client_detection_method
            FROM usage_records
            WHERE timestamp >= ?1 AND timestamp < ?2
              {status_filter}
              {filter_where}
            ORDER BY timestamp ASC
            "#
        );

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        // 构建参数
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(start_ms), Box::new(end_ms)];
        for p in &filter_params {
            params_vec.push(Box::new(p.clone()));
        }
        let params: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

        let records = stmt
            .query_map(params.as_slice(), Self::usage_record_from_row)
            .map_err(|e| format!("Failed to query records: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect records: {}", e))?;

        Ok(records)
    }

    /// 获取总记录数
    pub async fn get_record_count(&self) -> Result<usize, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM usage_records", [], |row| row.get(0))
            .map_err(|e| format!("Failed to count records: {}", e))?;

        Ok(count as usize)
    }

    /// 删除指定天数之前的记录
    #[allow(dead_code)]
    pub async fn cleanup_old_records(&self, days: i64) -> Result<usize, String> {
        let cutoff = chrono::Utc::now().timestamp_millis() - (days * 24 * 60 * 60 * 1000);
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

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
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let stats = if include_errors {
            // 包含所有请求
            conn.query_row(
                r#"
                SELECT
                    COUNT(*) as request_count,
                    COALESCE(SUM(input_tokens + cache_create_tokens + cache_read_tokens + output_tokens), 0) as total_tokens,
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
                    COALESCE(SUM(input_tokens + cache_create_tokens + cache_read_tokens + output_tokens), 0) as total_tokens,
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

    /// 获取时间窗口的聚合统计（支持来源过滤）
    ///
    /// # 参数
    /// - `cutoff_ms`: 窗口起始时间戳（毫秒）
    /// - `include_errors`: 是否包含错误请求（4xx/5xx）
    /// - `source_filter`: 来源过滤条件
    pub async fn get_window_stats_with_source(
        &self,
        cutoff_ms: i64,
        include_errors: bool,
        usage_filter: &UsageQueryFilter,
    ) -> Result<WindowAggregate, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        // 构建来源过滤 SQL
        let (filter_where, filter_params) = Self::build_usage_filter_sql(usage_filter);

        let status_filter = if include_errors {
            ""
        } else {
            "AND status_code >= 200 AND status_code < 300"
        };

        let sql = format!(
            r#"
            SELECT
                COUNT(*) as request_count,
                COALESCE(SUM(input_tokens + cache_create_tokens + cache_read_tokens + output_tokens), 0) as total_tokens,
                COALESCE(SUM(input_tokens), 0) as input_tokens,
                COALESCE(SUM(output_tokens), 0) as output_tokens,
                COALESCE(SUM(cache_create_tokens), 0) as cache_create_tokens,
                COALESCE(SUM(cache_read_tokens), 0) as cache_read_tokens,
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN 1 ELSE 0 END), 0) as status_2xx,
                COALESCE(SUM(CASE WHEN status_code >= 400 AND status_code < 500 THEN 1 ELSE 0 END), 0) as status_4xx,
                COALESCE(SUM(CASE WHEN status_code >= 500 AND status_code < 600 THEN 1 ELSE 0 END), 0) as status_5xx
            FROM usage_records
            WHERE timestamp >= ?1
              {status_filter}
              {filter_where}
            "#,
        );

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        // 构建参数
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(cutoff_ms)];
        params_vec.extend(
            filter_params
                .into_iter()
                .map(|p| Box::new(p) as Box<dyn rusqlite::ToSql>),
        );

        // 转换为引用切片
        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let stats = stmt
            .query_row(params_refs.as_slice(), |row| {
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
            })
            .map_err(|e| format!("Failed to get window stats with source: {}", e))?;

        Ok(stats)
    }

    /// 获取时间窗口内的模型分布（带来源过滤）
    pub async fn get_model_distribution_with_source(
        &self,
        cutoff_ms: i64,
        usage_filter: &UsageQueryFilter,
        include_errors: bool,
    ) -> Result<Vec<ModelDistribution>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let status_filter = if include_errors {
            ""
        } else {
            "AND status_code >= 200 AND status_code < 300"
        };

        let (filter_where, filter_params) = Self::build_usage_filter_sql(usage_filter);

        // 查询模型分布
        let sql = format!(
            r#"
            SELECT
                model,
                COUNT(*) as request_count,
                SUM(input_tokens + cache_create_tokens + cache_read_tokens + output_tokens) as total_tokens,
                SUM(input_tokens) as input_tokens,
                SUM(output_tokens) as output_tokens,
                SUM(cache_create_tokens) as cache_create_tokens,
                SUM(cache_read_tokens) as cache_read_tokens
            FROM usage_records
            WHERE timestamp >= ?1
              {status_filter}
              {filter_where}
            GROUP BY model
            ORDER BY total_tokens DESC
            "#
        );

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        // 构建参数
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(cutoff_ms)];
        for p in &filter_params {
            params_vec.push(Box::new(p.clone()));
        }
        let params: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

        let models: Vec<(String, i64, i64, i64, i64, i64, i64)> = stmt
            .query_map(params.as_slice(), |row| {
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
        for (
            model,
            request_count,
            total_tokens,
            input_tokens,
            output_tokens,
            cache_create_tokens,
            cache_read_tokens,
        ) in models
        {
            // 查询该模型的状态码分布
            let status_sql = format!(
                "SELECT status_code, COUNT(*) as count FROM usage_records WHERE timestamp >= ?1 AND model = ?2 {status_filter} {filter_where} GROUP BY status_code ORDER BY count DESC"
            );
            let mut status_params_vec: Vec<Box<dyn rusqlite::ToSql>> =
                vec![Box::new(cutoff_ms), Box::new(model.clone())];
            for p in &filter_params {
                status_params_vec.push(Box::new(p.clone()));
            }
            let status_params: Vec<&dyn rusqlite::ToSql> =
                status_params_vec.iter().map(|p| p.as_ref()).collect();
            let status_codes: Vec<(i64, i64)> = conn
                .prepare(&status_sql)
                .and_then(|mut stmt| {
                    let rows = stmt.query_map(status_params.as_slice(), |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })?;
                    rows.collect::<Result<Vec<_>, _>>()
                })
                .unwrap_or_default();

            // 转换为 JSON
            let status_codes_json = serde_json::to_string(
                &status_codes
                    .iter()
                    .map(|(code, count)| serde_json::json!({"statusCode": code, "count": count}))
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_else(|_| "[]".to_string());

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

    /// 构建来源过滤的 SQL WHERE 子句和参数
    fn build_source_filter_sql(source_filter: &SourceFilter) -> (String, Vec<String>) {
        match source_filter {
            SourceFilter::All => (String::new(), vec![]),
            SourceFilter::Source {
                api_key_prefixes,
                base_url,
            } => {
                if api_key_prefixes.is_empty() {
                    return ("AND 1 = 0".to_string(), vec![]);
                }
                let placeholders: Vec<String> =
                    api_key_prefixes.iter().map(|_| "?".to_string()).collect();
                let mut params: Vec<String> = api_key_prefixes.clone();
                params.push(base_url.clone().unwrap_or_default());
                (
                    format!(
                        "AND api_key_prefix IN ({}) AND COALESCE(request_base_url, '') = ?",
                        placeholders.join(",")
                    ),
                    params,
                )
            }
            SourceFilter::Unknown { known_pairs } => {
                if known_pairs.is_empty() {
                    (String::new(), vec![])
                } else {
                    let mut clauses = Vec::new();
                    let mut params = Vec::new();
                    for (prefix, base_url) in known_pairs {
                        clauses.push(
                            "(api_key_prefix = ? AND COALESCE(request_base_url, '') = ?)"
                                .to_string(),
                        );
                        params.push(prefix.clone());
                        params.push(base_url.clone().unwrap_or_default());
                    }
                    (
                        format!(
                            "AND (api_key_prefix IS NULL OR NOT ({}))",
                            clauses.join(" OR ")
                        ),
                        params,
                    )
                }
            }
        }
    }

    fn build_tool_filter_sql(tool_filter: &ToolFilter) -> (String, Vec<String>) {
        match tool_filter {
            ToolFilter::All => (String::new(), vec![]),
            ToolFilter::Tool(tool) if tool.trim().is_empty() => (String::new(), vec![]),
            ToolFilter::Tool(tool) => ("AND client_tool = ?".to_string(), vec![tool.clone()]),
        }
    }

    fn build_usage_filter_sql(usage_filter: &UsageQueryFilter) -> (String, Vec<String>) {
        let (source_where, mut params) = Self::build_source_filter_sql(&usage_filter.source);
        let (tool_where, tool_params) = Self::build_tool_filter_sql(&usage_filter.tool);
        params.extend(tool_params);
        let where_clause = match (source_where.is_empty(), tool_where.is_empty()) {
            (true, true) => String::new(),
            (false, true) => source_where,
            (true, false) => tool_where,
            (false, false) => format!("{source_where} {tool_where}"),
        };
        (where_clause, params)
    }

    /// 获取所有会话统计
    /// 获取会话统计信息
    #[allow(dead_code)]
    pub async fn get_session_stats(
        &self,
        session_id: &str,
        pricings: &[ModelPricingConfig],
        match_mode: &str,
    ) -> Result<Option<SessionStats>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

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

        let result = stmt.query_row([session_id], |row| {
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
    pub async fn get_all_sessions(
        &self,
        limit: i64,
        pricings: &[ModelPricingConfig],
        match_mode: &str,
    ) -> Result<Vec<SessionStats>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

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
                WHERE session_id IS NOT NULL
                  AND session_id != ''
                  AND session_id != ?2
                GROUP BY session_id
                ORDER BY MAX(request_end_time) DESC
                LIMIT ?1
                "#,
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let sessions = stmt
            .query_map(
                rusqlite::params![limit, LEGACY_UNMATCHED_SESSION_ID],
                |row| {
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
                },
            )
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
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

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
    /// 使用加权平均计算各模型速率：total_output_tokens * 1000.0 / total_duration_ms
    /// 结果按加权平均速率降序排列
    pub async fn get_model_rate_stats(
        &self,
        cutoff_ms: i64,
    ) -> Result<Vec<ModelRateStats>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT
                    model,
                    COUNT(*) as request_count,
                    COALESCE(SUM(output_tokens), 0) as total_output_tokens,
                    COALESCE(SUM(duration_ms), 0) as total_duration_ms,
                    -- 加权平均速率：总 tokens / 总时间（毫秒转秒需要乘 1000）
                    CASE
                        WHEN SUM(duration_ms) > 0
                        THEN SUM(output_tokens) * 1000.0 / SUM(duration_ms)
                        ELSE 0
                    END as avg_rate,
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

/// 价格应用筛选参数
#[derive(Debug, Clone, Default)]
pub struct PricingMatchFilter<'a> {
    pub model_id: &'a str,
    pub match_mode: &'a str,
    pub time_range_start: Option<i64>,
    pub time_range_end: Option<i64>,
    pub client_tool_filter: Option<&'a str>,
    pub api_source_key_prefixes: Option<&'a [String]>,
}

/// 模型匹配查询结果
struct PricingMatchQuery {
    matched_models: Vec<String>,
    where_clause: String,
    params: Vec<Box<dyn rusqlite::types::ToSql>>,
}

/// 单模型匹配计数
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelMatchCount {
    pub model: String,
    pub count: i64,
}

/// 价格应用预览结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewPricingApplyResult {
    pub matched_count: i64,
    pub total_current_cost: f64,
    pub model_counts: Vec<ModelMatchCount>,
}

/// 时间窗口的聚合统计
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct WindowAggregate {
    pub request_count: i64,
    #[allow(dead_code)]
    pub total_tokens: i64, // 总 Token = input + cache_create + cache_read + output
    pub input_tokens: i64, // 实际输入（不含缓存）
    pub output_tokens: i64,
    pub cache_create_tokens: i64,
    pub cache_read_tokens: i64,
    pub status_2xx: i64, // 成功请求数
    pub status_4xx: i64, // 客户端错误数
    pub status_5xx: i64, // 服务端错误数
}

/// 模型分布统计
#[derive(Debug, Clone)]
pub struct ModelDistribution {
    pub model: String,
    pub request_count: i64,
    pub total_tokens: i64, // 总 Token = input + cache_create + cache_read + output
    pub input_tokens: i64, // 实际输入（不含缓存）
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
    pub category: String, // "success", "client_error", "server_error" 成功、客户端错误、服务端错误
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
    pub async fn get_status_code_distribution(
        &self,
        cutoff_ms: i64,
    ) -> Result<Vec<StatusCodeDistribution>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

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
                let category = if (200..300).contains(&status_code) {
                    "success".to_string()
                } else if (400..500).contains(&status_code) {
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
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

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
    pub async fn get_model_ttft_stats(
        &self,
        cutoff_ms: i64,
    ) -> Result<Vec<ModelTtftStats>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

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
                WHERE source != 'custom'
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
    /// 用于搜索同步模型（排除自定义模型）
    pub fn search_model_pricings(
        &self,
        query: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ModelPricingConfig>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let pricings = if let Some(q) = query {
            let search_pattern = format!("%{}%", q.to_lowercase());
            let mut stmt = conn.prepare(
                r#"
                SELECT model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated
                FROM model_pricing
                WHERE source != 'custom' AND (model_id LIKE ?1 OR LOWER(display_name) LIKE ?1)
                ORDER BY model_id
                LIMIT ?2 OFFSET ?3
                "#
            ).map_err(|e| format!("Failed to prepare statement: {}", e))?;

            let rows = stmt
                .query_map(rusqlite::params![search_pattern, limit, offset], |row| {
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
                .map_err(|e| format!("Failed to search model pricings: {}", e))?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect results: {}", e))?
        } else {
            let mut stmt = conn.prepare(
                r#"
                SELECT model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated
                FROM model_pricing
                WHERE source != 'custom'
                ORDER BY model_id
                LIMIT ?1 OFFSET ?2
                "#
            ).map_err(|e| format!("Failed to prepare statement: {}", e))?;

            let rows = stmt
                .query_map(rusqlite::params![limit, offset], |row| {
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
                .map_err(|e| format!("Failed to query model pricings: {}", e))?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect results: {}", e))?
        };

        Ok(pricings)
    }

    /// 获取自定义模型价格列表（支持搜索）
    pub fn get_custom_model_pricings(
        &self,
        query: Option<&str>,
    ) -> Result<Vec<ModelPricingConfig>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let pricings = if let Some(q) = query {
            let search_pattern = format!("%{}%", q.to_lowercase());
            let mut stmt = conn.prepare(
                r#"
                SELECT model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated
                FROM model_pricing
                WHERE source = 'custom' AND (model_id LIKE ?1 OR LOWER(display_name) LIKE ?1)
                ORDER BY model_id
                "#
            ).map_err(|e| format!("Failed to prepare statement: {}", e))?;

            let rows = stmt
                .query_map(rusqlite::params![search_pattern], |row| {
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
                .map_err(|e| format!("Failed to query custom model pricings: {}", e))?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect results: {}", e))?
        } else {
            let mut stmt = conn.prepare(
                r#"
                SELECT model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated
                FROM model_pricing
                WHERE source = 'custom'
                ORDER BY model_id
                "#
            ).map_err(|e| format!("Failed to prepare statement: {}", e))?;

            let rows = stmt
                .query_map([], |row| {
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
                .map_err(|e| format!("Failed to query custom model pricings: {}", e))?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect results: {}", e))?
        };

        Ok(pricings)
    }

    /// 获取同步模型总数
    pub fn count_synced_model_pricings(&self, query: Option<&str>) -> Result<i64, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let count = if let Some(q) = query {
            let search_pattern = format!("%{}%", q.to_lowercase());
            conn.query_row(
                "SELECT COUNT(*) FROM model_pricing WHERE source != 'custom' AND (model_id LIKE ?1 OR LOWER(display_name) LIKE ?1)",
                rusqlite::params![search_pattern],
                |row| row.get(0),
            )
        } else {
            conn.query_row("SELECT COUNT(*) FROM model_pricing WHERE source != 'custom'", [], |row| row.get(0))
        }
        .map_err(|e| format!("Failed to count synced model pricings: {}", e))?;

        Ok(count)
    }

    /// 添加自定义模型价格（使用 UPSERT，如果已存在则更新）
    pub fn add_custom_pricing(&self, pricing: &ModelPricingConfig) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        conn.execute(
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

    /// 清空所有同步的模型价格（保留自定义模型）
    pub fn clear_synced_model_pricings(&self) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let count = conn
            .execute("DELETE FROM model_pricing WHERE source != 'custom'", [])
            .map_err(|e| format!("Failed to clear synced model pricings: {}", e))?;
        Ok(count)
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

        let pricings = stmt
            .query_map([], |row| {
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

    /// 通过 message_id 列表查询会话统计信息
    ///
    /// 用于将 JSONL 会话文件中的消息与代理数据库记录关联
    /// 返回聚合后的统计数据：总耗时、总输出 Token、平均生成速率等
    #[allow(dead_code)]
    pub async fn get_session_stats_by_message_ids(
        &self,
        message_ids: &[String],
        pricings: &[ModelPricingConfig],
        match_mode: &str,
    ) -> Option<SessionStats> {
        if message_ids.is_empty() {
            return None;
        }

        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return None,
        };

        // 构建 IN 子句参数
        let placeholders: Vec<String> = message_ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            r#"
            SELECT
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
            WHERE message_id IN ({})
            "#,
            placeholders.join(", ")
        );

        let params: Vec<&dyn rusqlite::types::ToSql> = message_ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();

        let result = conn.query_row(&sql, params.as_slice(), |row| {
            let total_output_tokens: i64 = row.get(2)?;
            let total_duration_ms: i64 = row.get(5)?;
            let models_str: String = row.get::<_, String>(8)?;
            let total_input_tokens: i64 = row.get(1)?;
            let total_cache_create_tokens: i64 = row.get(3)?;
            let total_cache_read_tokens: i64 = row.get(4)?;

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
                session_id: String::new(), // 调用方会填充
                total_requests: row.get::<_, i64>(0)? as u64,
                total_input_tokens: total_input_tokens as u64,
                total_output_tokens: total_output_tokens as u64,
                total_cache_create_tokens: total_cache_create_tokens as u64,
                total_cache_read_tokens: total_cache_read_tokens as u64,
                total_duration_ms: total_duration_ms as u64,
                avg_output_tokens_per_second: avg_rate,
                first_request_time: row.get::<_, Option<i64>>(6)?.unwrap_or(0),
                last_request_time: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
                models: if models_str.is_empty() {
                    Vec::new()
                } else {
                    models_str.split(',').map(|s| s.to_string()).collect()
                },
                avg_ttft_ms: row.get::<_, Option<f64>>(9)?.unwrap_or(0.0),
                success_requests: row.get::<_, i64>(10)? as u64,
                error_requests: row.get::<_, i64>(11)? as u64,
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
            Ok(stats) if stats.total_requests > 0 => Some(stats),
            Ok(_) => None, // 没有找到记录
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => {
                eprintln!("Failed to get session stats by message_ids: {}", e);
                None
            }
        }
    }

    // ========== session_stats 表操作 ==========

    /// 增量更新会话统计（新请求产生时调用）
    ///
    /// 如果会话不存在则创建新记录，否则增量更新
    pub async fn update_session_stats_incremental(
        &self,
        record: &UsageRecord,
    ) -> Result<(), String> {
        // 如果没有 session_id，尝试从 JSONL 获取；无匹配时使用请求时间窗口作为回退
        let session_id = match &record.session_id {
            Some(id) if !id.is_empty() => id.clone(),
            _ => {
                match self.find_session_id_by_message_id(&record.message_id).await {
                    Some(id) => id,
                    None => {
                        // 无法匹配 JSONL 的请求也保留在 session_stats 中，
                        // 后续 JSONL 重新扫描时可通过 message_id 回填正确值
                        LEGACY_UNMATCHED_SESSION_ID.to_string()
                    }
                }
            }
        };

        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let now = chrono::Utc::now().timestamp_millis();

        // 检查是否已存在
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM session_stats WHERE session_id = ?1",
                [&session_id],
                |row| row.get::<_, i64>(0),
            )
            .is_ok();

        if exists {
            // 增量更新
            conn.execute(
                r#"
                UPDATE session_stats SET
                    total_duration_ms = total_duration_ms + ?2,
                    total_input_tokens = total_input_tokens + ?3,
                    total_output_tokens = total_output_tokens + ?4,
                    total_cache_create_tokens = total_cache_create_tokens + ?5,
                    total_cache_read_tokens = total_cache_read_tokens + ?6,
                    proxy_request_count = proxy_request_count + 1,
                    success_requests = success_requests + CASE WHEN ?7 < 400 THEN 1 ELSE 0 END,
                    error_requests = error_requests + CASE WHEN ?7 >= 400 THEN 1 ELSE 0 END,
                    last_request_time = MAX(last_request_time, ?8),
                    first_request_time = COALESCE(first_request_time, ?9),
                    last_updated = ?10
                WHERE session_id = ?1
                "#,
                rusqlite::params![
                    session_id,
                    record.duration_ms as i64,
                    record.input_tokens as i64,
                    record.output_tokens as i64,
                    record.cache_create_tokens as i64,
                    record.cache_read_tokens as i64,
                    record.status_code as i64,
                    record.request_end_time,
                    record.request_start_time,
                    now
                ],
            )
            .map_err(|e| format!("Failed to update session stats: {}", e))?;

            // 重新计算平均速率
            conn.execute(
                r#"
                UPDATE session_stats SET
                    avg_output_tokens_per_second = CASE
                        WHEN total_duration_ms > 0 THEN total_output_tokens * 1000.0 / total_duration_ms
                        ELSE 0
                    END
                WHERE session_id = ?1
                "#,
                [&session_id],
            )
            .map_err(|e| format!("Failed to update avg rate: {}", e))?;
        } else {
            // 插入新记录
            conn.execute(
                r#"
                INSERT INTO session_stats (
                    session_id, total_duration_ms, total_input_tokens, total_output_tokens,
                    total_cache_create_tokens, total_cache_read_tokens, proxy_request_count,
                    success_requests, error_requests, first_request_time, last_request_time,
                    avg_output_tokens_per_second, last_updated, models, estimated_cost
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, 1,
                    CASE WHEN ?7 < 400 THEN 1 ELSE 0 END,
                    CASE WHEN ?7 >= 400 THEN 1 ELSE 0 END,
                    ?8, ?9,
                    CASE WHEN ?2 > 0 THEN ?4 * 1000.0 / ?2 ELSE 0 END,
                    ?10, ?11, 0
                )
                "#,
                rusqlite::params![
                    session_id,
                    record.duration_ms as i64,
                    record.input_tokens as i64,
                    record.output_tokens as i64,
                    record.cache_create_tokens as i64,
                    record.cache_read_tokens as i64,
                    record.status_code as i64,
                    record.request_start_time,
                    record.request_end_time,
                    now,
                    record.model.clone()
                ],
            )
            .map_err(|e| format!("Failed to insert session stats: {}", e))?;
        }

        Ok(())
    }

    /// 通过 message_id 查找对应的 session_id（从 JSONL 文件）
    async fn find_session_id_by_message_id(&self, message_id: &str) -> Option<String> {
        // 使用 session 模块的缓存索引查找（O(1) 时间复杂度）
        crate::session::find_session_id_by_message_id(message_id)
    }

    /// 批量获取会话统计
    ///
    /// 从 session_stats 表直接读取，不进行计算
    pub async fn get_session_stats_batch(
        &self,
        session_ids: &[String],
    ) -> Result<std::collections::HashMap<String, SessionStats>, String> {
        if session_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let placeholders: Vec<String> = session_ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            r#"
            SELECT
                session_id,
                total_duration_ms,
                avg_output_tokens_per_second,
                avg_ttft_ms,
                proxy_request_count,
                success_requests,
                error_requests,
                total_input_tokens,
                total_output_tokens,
                total_cache_create_tokens,
                total_cache_read_tokens,
                models,
                first_request_time,
                last_request_time,
                estimated_cost
            FROM session_stats
            WHERE session_id IN ({})
            "#,
            placeholders.join(", ")
        );

        let params: Vec<&dyn rusqlite::types::ToSql> = session_ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();

        let mut result = std::collections::HashMap::new();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let rows = stmt
            .query_map(params.as_slice(), |row| {
                let session_id: String = row.get(0)?;
                let total_duration_ms: i64 = row.get(1)?;
                let avg_rate: f64 = row.get(2)?;
                let avg_ttft_ms: f64 = row.get(3)?;
                let proxy_request_count: i64 = row.get(4)?;
                let success_requests: i64 = row.get(5)?;
                let error_requests: i64 = row.get(6)?;
                let total_input_tokens: i64 = row.get(7)?;
                let total_output_tokens: i64 = row.get(8)?;
                let total_cache_create_tokens: i64 = row.get(9)?;
                let total_cache_read_tokens: i64 = row.get(10)?;
                let models_str: String = row.get::<_, Option<String>>(11)?.unwrap_or_default();
                let first_request_time: i64 = row.get::<_, Option<i64>>(12)?.unwrap_or(0);
                let last_request_time: i64 = row.get::<_, Option<i64>>(13)?.unwrap_or(0);
                let estimated_cost: f64 = row.get::<_, Option<f64>>(14)?.unwrap_or(0.0);

                Ok((
                    session_id.clone(),
                    SessionStats {
                        session_id,
                        total_requests: proxy_request_count as u64,
                        total_input_tokens: total_input_tokens as u64,
                        total_output_tokens: total_output_tokens as u64,
                        total_cache_create_tokens: total_cache_create_tokens as u64,
                        total_cache_read_tokens: total_cache_read_tokens as u64,
                        total_duration_ms: total_duration_ms as u64,
                        avg_output_tokens_per_second: avg_rate,
                        first_request_time,
                        last_request_time,
                        models: if models_str.is_empty() {
                            Vec::new()
                        } else {
                            models_str.split(',').map(|s| s.to_string()).collect()
                        },
                        avg_ttft_ms,
                        success_requests: success_requests as u64,
                        error_requests: error_requests as u64,
                        estimated_cost,
                        is_cost_estimated: true,
                        cwd: None,
                        project_name: None,
                        topic: None,
                        last_prompt: None,
                        session_name: None,
                    },
                ))
            })
            .map_err(|e| format!("Failed to query session stats: {}", e))?;

        for row_result in rows {
            match row_result {
                Ok((session_id, stats)) => {
                    result.insert(session_id, stats);
                }
                Err(e) => {
                    eprintln!("Error parsing session stats row: {}", e);
                }
            }
        }

        Ok(result)
    }

    /// 获取按来源过滤后的会话列表。
    ///
    /// 来源过滤必须从 usage_records 聚合，不能使用 session_stats 的全量缓存。
    pub async fn get_sessions_with_source(
        &self,
        limit: i64,
        offset: i64,
        usage_filter: &UsageQueryFilter,
    ) -> Result<Vec<SessionStats>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let (filter_where, filter_params) = Self::build_usage_filter_sql(usage_filter);
        let sql = format!(
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
                SUM(CASE WHEN status_code >= 400 THEN 1 ELSE 0 END) as error_requests,
                COALESCE(SUM(estimated_cost), 0) as estimated_cost
            FROM usage_records
            WHERE session_id IS NOT NULL AND session_id <> '' AND session_id <> ?
              {filter_where}
            GROUP BY session_id
            ORDER BY last_request_time DESC
            LIMIT ? OFFSET ?
            "#
        );

        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        params_vec.push(Box::new(LEGACY_UNMATCHED_SESSION_ID.to_string()));
        for p in &filter_params {
            params_vec.push(Box::new(p.clone()));
        }
        params_vec.push(Box::new(limit));
        params_vec.push(Box::new(offset));
        let params: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare source sessions statement: {}", e))?;

        let sessions = stmt
            .query_map(params.as_slice(), Self::session_stats_from_usage_row)
            .map_err(|e| format!("Failed to query source sessions: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect source sessions: {}", e))?;

        Ok(sessions)
    }

    /// 获取单个会话在当前来源过滤下的聚合统计。
    pub async fn get_session_detail_with_source(
        &self,
        session_id: &str,
        usage_filter: &UsageQueryFilter,
    ) -> Result<Option<SessionStats>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let (filter_where, filter_params) = Self::build_usage_filter_sql(usage_filter);
        let sql = format!(
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
                SUM(CASE WHEN status_code >= 400 THEN 1 ELSE 0 END) as error_requests,
                COALESCE(SUM(estimated_cost), 0) as estimated_cost
            FROM usage_records
            WHERE session_id = ?
              {filter_where}
            GROUP BY session_id
            "#
        );

        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(session_id.to_string())];
        for p in &filter_params {
            params_vec.push(Box::new(p.clone()));
        }
        let params: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare source session detail statement: {}", e))?;

        match stmt.query_row(params.as_slice(), Self::session_stats_from_usage_row) {
            Ok(stats) if stats.total_requests > 0 => Ok(Some(stats)),
            Ok(_) | Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to query source session detail: {}", e)),
        }
    }

    fn session_stats_from_usage_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionStats> {
        let session_id: String = row.get(0)?;
        let total_requests: i64 = row.get(1)?;
        let total_input_tokens: i64 = row.get(2)?;
        let total_output_tokens: i64 = row.get(3)?;
        let total_cache_create_tokens: i64 = row.get(4)?;
        let total_cache_read_tokens: i64 = row.get(5)?;
        let total_duration_ms: i64 = row.get(6)?;
        let first_request_time: i64 = row.get::<_, Option<i64>>(7)?.unwrap_or(0);
        let last_request_time: i64 = row.get::<_, Option<i64>>(8)?.unwrap_or(0);
        let models_str: String = row.get::<_, Option<String>>(9)?.unwrap_or_default();
        let avg_ttft_ms: f64 = row.get::<_, Option<f64>>(10)?.unwrap_or(0.0);
        let success_requests: i64 = row.get::<_, Option<i64>>(11)?.unwrap_or(0);
        let error_requests: i64 = row.get::<_, Option<i64>>(12)?.unwrap_or(0);
        let estimated_cost: f64 = row.get::<_, Option<f64>>(13)?.unwrap_or(0.0);
        let avg_rate = if total_duration_ms > 0 {
            total_output_tokens as f64 / (total_duration_ms as f64 / 1000.0)
        } else {
            0.0
        };

        Ok(SessionStats {
            session_id,
            total_requests: total_requests as u64,
            total_input_tokens: total_input_tokens as u64,
            total_output_tokens: total_output_tokens as u64,
            total_cache_create_tokens: total_cache_create_tokens as u64,
            total_cache_read_tokens: total_cache_read_tokens as u64,
            total_duration_ms: total_duration_ms as u64,
            avg_output_tokens_per_second: avg_rate,
            first_request_time,
            last_request_time,
            models: if models_str.is_empty() {
                Vec::new()
            } else {
                models_str.split(',').map(|s| s.to_string()).collect()
            },
            avg_ttft_ms,
            success_requests: success_requests as u64,
            error_requests: error_requests as u64,
            estimated_cost,
            is_cost_estimated: false,
            cwd: None,
            project_name: None,
            topic: None,
            last_prompt: None,
            session_name: None,
        })
    }

    /// 迁移现有数据：只迁移没有 session_id 的 usage_records
    ///
    /// 启动时调用一次，增量迁移新数据
    pub async fn migrate_to_session_stats(&self) -> Result<usize, String> {
        // 类型别名：简化复杂类型定义
        // UsageRecordRow: 从数据库查询的 usage_record 行（用于迁移）
        type UsageRecordRow = (
            i64,         // id: 记录 ID
            String,      // message_id: 消息 ID
            i64,         // duration_ms: 耗时（毫秒）
            i64,         // input_tokens: 输入 Token
            i64,         // output_tokens: 输出 Token
            i64,         // cache_create_tokens: 缓存创建 Token
            i64,         // cache_read_tokens: 缓存读取 Token
            i64,         // request_start_time: 请求开始时间
            i64,         // request_end_time: 请求结束时间
            i64,         // status_code: 状态码
            String,      // model: 模型名称
            Option<f64>, // ttft_ms: TTFT（毫秒）
        );
        // SessionAggregate: 按 session_id 聚合的统计数据
        type SessionAggregate = (
            i64,                               // total_duration_ms: 总耗时
            i64,                               // total_input_tokens: 总输入
            i64,                               // total_output_tokens: 总输出
            i64,                               // total_cache_create_tokens: 总缓存创建
            i64,                               // total_cache_read_tokens: 总缓存读取
            i64,                               // first_request_time: 最早开始时间
            i64,                               // last_request_time: 最晚结束时间
            i64,                               // success_requests: 成功请求数
            i64,                               // error_requests: 错误请求数
            i64,                               // request_count: 请求数
            std::collections::HashSet<String>, // 模型集合
            Vec<f64>,                          // TTFT 值列表
        );

        let now = chrono::Utc::now().timestamp_millis();

        // 检查是否有需要迁移的记录（session_id 为空的历史记录）
        let needs_migration = {
            let conn = self
                .conn
                .lock()
                .map_err(|e| format!("Failed to lock connection: {}", e))?;

            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM usage_records WHERE session_id IS NULL OR session_id = ''",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            drop(conn);
            count
        };

        // 如果没有需要迁移的记录，直接返回
        if needs_migration == 0 {
            return Ok(0);
        }

        eprintln!(
            "[migration] Found {} records without session_id, starting migration...",
            needs_migration
        );

        // 获取没有 session_id 的记录
        let records = {
            let conn = self
                .conn
                .lock()
                .map_err(|e| format!("Failed to lock connection: {}", e))?;

            // 只查询没有 session_id 的记录
            let result: Vec<UsageRecordRow> = conn
                .prepare(
                    r#"
                    SELECT
                        id,
                        message_id,
                        duration_ms,
                        input_tokens,
                        output_tokens,
                        cache_create_tokens,
                        cache_read_tokens,
                        request_start_time,
                        request_end_time,
                        status_code,
                        model,
                        ttft_ms
                    FROM usage_records
                    WHERE session_id IS NULL OR session_id = ''
                    ORDER BY timestamp
                    "#,
                )
                .map_err(|e| format!("Failed to prepare migration query: {}", e))?
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get::<_, String>(10)?,
                        row.get::<_, Option<f64>>(11)?,
                    ))
                })
                .map_err(|e| format!("Failed to execute migration query: {}", e))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect migration results: {}", e))?;

            result
        }; // conn 在此处被释放

        // 获取 JSONL 会话元数据缓存（使用缓存，60秒内不会重复扫描）
        let all_meta = crate::session::get_all_session_meta_cached();

        // 构建 message_id -> session_id 的映射
        let mut msg_to_session: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for meta in &all_meta {
            for msg_id in &meta.message_ids {
                msg_to_session.insert(msg_id.clone(), meta.session_id.clone());
            }
        }
        eprintln!(
            "[migration] Built mapping for {} message_ids",
            msg_to_session.len()
        );

        // 按 session_id 聚合记录
        // 同时记录需要更新的 record_id
        let mut session_aggregates: std::collections::HashMap<String, SessionAggregate> =
            std::collections::HashMap::new();

        let mut matched = 0;
        let mut unmatched = 0;
        let mut record_updates: Vec<(String, i64)> = Vec::new(); // (session_id, record_id) 会话ID, 记录ID
        let mut unmatched_record_ids: Vec<i64> = Vec::new();

        for (
            record_id,
            message_id,
            duration_ms,
            input,
            output,
            cache_create,
            cache_read,
            start_time,
            end_time,
            status_code,
            model,
            ttft_ms,
        ) in records
        {
            if let Some(session_id) = msg_to_session.get(&message_id) {
                matched += 1;
                record_updates.push((session_id.clone(), record_id));

                let entry = session_aggregates.entry(session_id.clone()).or_insert((
                    0,                                // 计数
                    0,                                // 总耗时
                    0,                                // 总输入
                    0,                                // 总输出
                    0,                                // 总缓存创建
                    0,                                // 总缓存读取
                    i64::MAX,                         // 最早时间
                    0,                                // 最晚时间
                    0,                                // 成功数
                    0,                                // 错误数
                    std::collections::HashSet::new(), // 模型集合
                    Vec::new(),                       // TTFT 值列表
                ));

                entry.0 += 1;
                entry.1 += duration_ms;
                entry.2 += input;
                entry.3 += output;
                entry.4 += cache_create;
                entry.5 += cache_read;
                entry.6 = entry.6.min(start_time);
                entry.7 = entry.7.max(end_time);
                if status_code < 400 {
                    entry.8 += 1;
                } else {
                    entry.9 += 1;
                }
                if !model.is_empty() {
                    entry.10.insert(model);
                }
                if let Some(ttft) = ttft_ms {
                    entry.11.push(ttft);
                }
            } else {
                unmatched += 1;
                unmatched_record_ids.push(record_id);
            }
        }

        eprintln!(
            "[migration] Matched {} records, unmatched {} records",
            matched, unmatched
        );

        // 保存到 session_stats 表（使用增量更新，避免覆盖已有数据）
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to start migration transaction: {}", e))?;

        let mut migrated = 0;

        {
            let mut update_record_stmt = tx
                .prepare("UPDATE usage_records SET session_id = ?1 WHERE id = ?2")
                .map_err(|e| format!("Failed to prepare record migration update: {}", e))?;

            for (session_id, record_id) in &record_updates {
                let _ = update_record_stmt.execute(rusqlite::params![session_id, record_id]);
            }
        }

        if !unmatched_record_ids.is_empty() {
            let mut mark_unmatched_stmt = tx
                .prepare(
                    "UPDATE usage_records SET session_id = ?1, migration_attempted_at = ?2 WHERE id = ?3",
                )
                .map_err(|e| format!("Failed to prepare unmatched migration update: {}", e))?;

            for record_id in &unmatched_record_ids {
                let _ = mark_unmatched_stmt.execute(rusqlite::params![
                    LEGACY_UNMATCHED_SESSION_ID,
                    now,
                    record_id
                ]);
            }
        }

        if matched == 0 {
            tx.commit()
                .map_err(|e| format!("Failed to commit unmatched migration transaction: {}", e))?;
            drop(conn);
            eprintln!(
                "[migration] No records matched; archived {} records as legacy unmatched",
                unmatched
            );
            return Ok(0);
        }

        for (
            session_id,
            (
                count,
                duration,
                input,
                output,
                cache_create,
                cache_read,
                first_time,
                last_time,
                success,
                error,
                models,
                ttfts,
            ),
        ) in session_aggregates
        {
            let avg_rate = if duration > 0 {
                (output as f64) * 1000.0 / (duration as f64)
            } else {
                0.0
            };

            let avg_ttft = if !ttfts.is_empty() {
                ttfts.iter().sum::<f64>() / ttfts.len() as f64
            } else {
                0.0
            };

            let models_str: String = models.into_iter().collect::<Vec<_>>().join(",");

            // 检查是否已存在该 session
            let exists: bool = tx
                .query_row(
                    "SELECT 1 FROM session_stats WHERE session_id = ?1",
                    [&session_id],
                    |row| row.get::<_, i64>(0),
                )
                .is_ok();

            if exists {
                // 增量更新已存在的记录
                let result = tx.execute(
                    r#"
                    UPDATE session_stats SET
                        total_duration_ms = total_duration_ms + ?2,
                        total_input_tokens = total_input_tokens + ?3,
                        total_output_tokens = total_output_tokens + ?4,
                        total_cache_create_tokens = total_cache_create_tokens + ?5,
                        total_cache_read_tokens = total_cache_read_tokens + ?6,
                        proxy_request_count = proxy_request_count + ?7,
                        success_requests = success_requests + ?8,
                        error_requests = error_requests + ?9,
                        last_request_time = MAX(last_request_time, ?10),
                        first_request_time = COALESCE(first_request_time, ?11),
                        last_updated = ?12
                    WHERE session_id = ?1
                    "#,
                    rusqlite::params![
                        session_id,
                        duration,
                        input,
                        output,
                        cache_create,
                        cache_read,
                        count,
                        success,
                        error,
                        last_time,
                        if first_time == i64::MAX {
                            None
                        } else {
                            Some(first_time)
                        },
                        now
                    ],
                );

                if result.is_ok() {
                    migrated += 1;
                }
            } else {
                // 插入新记录
                let result = tx.execute(
                    r#"
                    INSERT INTO session_stats (
                        session_id, total_duration_ms, avg_output_tokens_per_second, avg_ttft_ms,
                        proxy_request_count, success_requests, error_requests,
                        total_input_tokens, total_output_tokens, total_cache_create_tokens, total_cache_read_tokens,
                        models, first_request_time, last_request_time, estimated_cost, last_updated
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 0, ?15
                    )
                    "#,
                    rusqlite::params![
                        session_id,
                        duration,
                        avg_rate,
                        avg_ttft,
                        count,
                        success,
                        error,
                        input,
                        output,
                        cache_create,
                        cache_read,
                        models_str,
                        if first_time == i64::MAX { None } else { Some(first_time) },
                        last_time,
                        now
                    ],
                );

                if result.is_ok() {
                    migrated += 1;
                }
            }
        }

        tx.commit()
            .map_err(|e| format!("Failed to commit migration transaction: {}", e))?;
        drop(conn);
        eprintln!(
            "[migration] Migrated {} sessions to session_stats table",
            migrated
        );
        Ok(migrated)
    }

    /// 删除指定来源的请求记录
    pub async fn delete_records_by_source(
        &self,
        api_key_prefixes: &[String],
        base_url: Option<&str>,
    ) -> Result<(), String> {
        if api_key_prefixes.is_empty() {
            return Ok(());
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        // 构建删除 SQL
        let placeholders: Vec<String> = api_key_prefixes.iter().map(|_| "?".to_string()).collect();
        let base_url_val = base_url.unwrap_or("");

        let sql = format!(
            "DELETE FROM usage_records WHERE api_key_prefix IN ({}) AND COALESCE(request_base_url, '') = ?",
            placeholders.join(",")
        );

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare delete statement: {}", e))?;

        // 构建参数
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![];
        for p in api_key_prefixes {
            params.push(p);
        }
        params.push(&base_url_val);

        let deleted = stmt
            .execute(params.as_slice())
            .map_err(|e| format!("Failed to delete records: {}", e))?;

        eprintln!("[database] Deleted {} records for source", deleted);
        Ok(())
    }
}
