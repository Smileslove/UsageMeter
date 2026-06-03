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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProxyMergeCacheSignature {
    pub usage_record_count: u64,
    pub max_timestamp: i64,
    pub max_updated_at: i64,
    pub session_stats_max_updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProxyDayDependencySnapshot {
    pub record_count: u64,
    pub max_timestamp_ms: i64,
    pub max_updated_at: i64,
}

mod migration;
mod pricing;
mod session;

/// 数据库管理器，用于代理使用数据
/// 使用线程安全的 SQLite 连接包装器
pub struct ProxyDatabase {
    pub(super) conn: Arc<std::sync::Mutex<Connection>>,
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
                message_id TEXT NOT NULL,
                storage_dedupe_key TEXT NOT NULL UNIQUE,
                canonical_request_key TEXT,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                model TEXT NOT NULL DEFAULT '',
                session_id TEXT,
                session_resolution_state TEXT NOT NULL DEFAULT 'unknown',
                message_id_conflicted INTEGER NOT NULL DEFAULT 0,
                request_start_time INTEGER,
                request_end_time INTEGER,
                duration_ms INTEGER NOT NULL DEFAULT 0,
                output_tokens_per_second REAL,
                ttft_ms INTEGER,
                status_code INTEGER NOT NULL DEFAULT 200,
                migration_attempted_at INTEGER,
                estimated_cost REAL NOT NULL DEFAULT 0,
                pricing_snapshot_id TEXT,
                cost_locked INTEGER NOT NULL DEFAULT 0,
                api_key_prefix TEXT,
                request_base_url TEXT,
                client_tool TEXT NOT NULL DEFAULT 'claude_code',
                proxy_profile_id TEXT,
                client_detection_method TEXT NOT NULL DEFAULT 'legacy_path',
                created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
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

    fn usage_records_has_storage_dedupe_unique(conn: &Connection) -> bool {
        let mut stmt = match conn.prepare("PRAGMA index_list('usage_records')") {
            Ok(stmt) => stmt,
            Err(_) => return false,
        };
        let rows = match stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2).unwrap_or(0) != 0,
            ))
        }) {
            Ok(rows) => rows,
            Err(_) => return false,
        };

        for row in rows.flatten() {
            if !row.1 {
                continue;
            }
            let mut info_stmt = match conn.prepare(&format!("PRAGMA index_info('{}')", row.0)) {
                Ok(stmt) => stmt,
                Err(_) => continue,
            };
            let cols = match info_stmt.query_map([], |info_row| info_row.get::<_, String>(2)) {
                Ok(cols) => cols,
                Err(_) => continue,
            };
            let col_names: Vec<String> = cols.flatten().collect();
            if col_names.len() == 1 && col_names[0] == "storage_dedupe_key" {
                return true;
            }
        }
        false
    }

    /// 创建模型价格表（静态方法）
    pub(super) fn create_model_pricing_table_static(conn: &Connection) -> Result<(), String> {
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
            "ALTER TABLE usage_records ADD COLUMN storage_dedupe_key TEXT",
            "ALTER TABLE usage_records ADD COLUMN canonical_request_key TEXT",
            "ALTER TABLE usage_records ADD COLUMN request_start_time INTEGER",
            "ALTER TABLE usage_records ADD COLUMN request_end_time INTEGER",
            "ALTER TABLE usage_records ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE usage_records ADD COLUMN output_tokens_per_second REAL",
            "ALTER TABLE usage_records ADD COLUMN status_code INTEGER NOT NULL DEFAULT 200",
            "ALTER TABLE usage_records ADD COLUMN ttft_ms INTEGER",
            "ALTER TABLE usage_records ADD COLUMN migration_attempted_at INTEGER",
            "ALTER TABLE usage_records ADD COLUMN session_resolution_state TEXT NOT NULL DEFAULT 'unknown'",
            "ALTER TABLE usage_records ADD COLUMN message_id_conflicted INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE usage_records ADD COLUMN estimated_cost REAL NOT NULL DEFAULT 0",
            "ALTER TABLE usage_records ADD COLUMN pricing_snapshot_id TEXT",
            "ALTER TABLE usage_records ADD COLUMN cost_locked INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE usage_records ADD COLUMN updated_at INTEGER NOT NULL DEFAULT 0",
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
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_usage_canonical_key ON usage_records(canonical_request_key)",
            [],
        );

        if !Self::usage_records_has_storage_dedupe_unique(conn) {
            Self::rebuild_usage_records_table(conn)?;
        }

        Ok(())
    }

    fn rebuild_usage_records_table(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(
            r#"
            BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS usage_records_v2 (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                message_id TEXT NOT NULL,
                storage_dedupe_key TEXT NOT NULL UNIQUE,
                canonical_request_key TEXT,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                model TEXT NOT NULL DEFAULT '',
                session_id TEXT,
                session_resolution_state TEXT NOT NULL DEFAULT 'unknown',
                message_id_conflicted INTEGER NOT NULL DEFAULT 0,
                request_start_time INTEGER,
                request_end_time INTEGER,
                duration_ms INTEGER NOT NULL DEFAULT 0,
                output_tokens_per_second REAL,
                ttft_ms INTEGER,
                status_code INTEGER NOT NULL DEFAULT 200,
                migration_attempted_at INTEGER,
                estimated_cost REAL NOT NULL DEFAULT 0,
                pricing_snapshot_id TEXT,
                cost_locked INTEGER NOT NULL DEFAULT 0,
                api_key_prefix TEXT,
                request_base_url TEXT,
                client_tool TEXT NOT NULL DEFAULT 'claude_code',
                proxy_profile_id TEXT,
                client_detection_method TEXT NOT NULL DEFAULT 'legacy_path',
                created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
            );
            INSERT INTO usage_records_v2 (
                id, timestamp, message_id, storage_dedupe_key, canonical_request_key,
                input_tokens, output_tokens, cache_create_tokens, cache_read_tokens, model,
                session_id, session_resolution_state, message_id_conflicted,
                request_start_time, request_end_time, duration_ms, output_tokens_per_second,
                ttft_ms, status_code, migration_attempted_at, estimated_cost, pricing_snapshot_id,
                cost_locked, api_key_prefix, request_base_url, client_tool, proxy_profile_id,
                client_detection_method, created_at, updated_at
            )
            SELECT
                id,
                timestamp,
                message_id,
                CASE
                    WHEN COALESCE(storage_dedupe_key, '') != '' THEN storage_dedupe_key
                    WHEN COALESCE(client_tool, 'claude_code') = 'opencode' AND COALESCE(message_id, '') != ''
                        THEN COALESCE(client_tool, 'claude_code') || ':' || message_id || ':' || COALESCE(NULLIF(request_start_time, 0), timestamp)
                    WHEN COALESCE(message_id, '') != ''
                        THEN COALESCE(client_tool, 'claude_code') || ':' || message_id
                    ELSE COALESCE(client_tool, 'claude_code') || ':' || COALESCE(session_id, '') || ':' || timestamp || ':' || model || ':' ||
                         input_tokens || ':' || output_tokens || ':' || cache_create_tokens || ':' || cache_read_tokens
                END,
                CASE
                    WHEN COALESCE(canonical_request_key, '') != '' THEN canonical_request_key
                    WHEN COALESCE(message_id, '') != ''
                        THEN COALESCE(client_tool, 'claude_code') || ':' || message_id
                    ELSE COALESCE(client_tool, 'claude_code') || ':' || COALESCE(session_id, '') || ':' || timestamp || ':' || model || ':' ||
                         input_tokens || ':' || output_tokens || ':' || cache_create_tokens || ':' || cache_read_tokens
                END,
                input_tokens,
                output_tokens,
                cache_create_tokens,
                cache_read_tokens,
                model,
                session_id,
                CASE
                    WHEN COALESCE(session_resolution_state, '') != '' THEN session_resolution_state
                    WHEN COALESCE(client_tool, 'claude_code') = 'opencode' AND (session_id IS NULL OR session_id = '') THEN 'unknown'
                    WHEN session_id IS NULL OR session_id = '' THEN 'unknown'
                    ELSE 'known'
                END,
                COALESCE(message_id_conflicted, 0),
                request_start_time,
                request_end_time,
                duration_ms,
                output_tokens_per_second,
                ttft_ms,
                status_code,
                migration_attempted_at,
                estimated_cost,
                pricing_snapshot_id,
                cost_locked,
                api_key_prefix,
                request_base_url,
                COALESCE(client_tool, 'claude_code'),
                proxy_profile_id,
                COALESCE(client_detection_method, 'legacy_path'),
                created_at,
                updated_at
            FROM usage_records;
            DROP TABLE usage_records;
            ALTER TABLE usage_records_v2 RENAME TO usage_records;
            CREATE INDEX IF NOT EXISTS idx_timestamp ON usage_records(timestamp);
            CREATE INDEX IF NOT EXISTS idx_message_id ON usage_records(message_id);
            CREATE INDEX IF NOT EXISTS idx_usage_storage_key ON usage_records(storage_dedupe_key);
            CREATE INDEX IF NOT EXISTS idx_usage_canonical_key ON usage_records(canonical_request_key);
            CREATE INDEX IF NOT EXISTS idx_session_id ON usage_records(session_id);
            CREATE INDEX IF NOT EXISTS idx_model_timestamp ON usage_records(model, timestamp);
            CREATE INDEX IF NOT EXISTS idx_source_lookup ON usage_records(api_key_prefix, request_base_url);
            CREATE INDEX IF NOT EXISTS idx_usage_tool_source ON usage_records(client_tool, api_key_prefix, request_base_url);
            CREATE INDEX IF NOT EXISTS idx_usage_tool_time ON usage_records(client_tool, timestamp);
            COMMIT;
            "#,
        )
        .map_err(|e| format!("Failed to rebuild usage_records table: {}", e))
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

    fn computed_storage_dedupe_key(record: &UsageRecord) -> String {
        if let Some(key) = record.storage_dedupe_key.as_ref() {
            let trimmed = key.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        if record.client_tool == "opencode" && !record.message_id.trim().is_empty() {
            let stable_time = if record.request_start_time > 0 {
                record.request_start_time
            } else {
                record.timestamp
            };
            format!(
                "{}:{}:{}",
                record.client_tool, record.message_id, stable_time
            )
        } else {
            session::computed_canonical_request_key(record)
        }
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
        let storage_dedupe_key = Self::computed_storage_dedupe_key(record);
        let canonical_request_key = session::computed_canonical_request_key(record);
        let session_resolution_state = session::computed_session_resolution_state(record);
        let now = chrono::Utc::now().timestamp();
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        conn.execute(
            r#"
            INSERT OR REPLACE INTO usage_records
            (timestamp, message_id, storage_dedupe_key, canonical_request_key, input_tokens, output_tokens, cache_create_tokens,
             cache_read_tokens, model, session_id, session_resolution_state, message_id_conflicted, request_start_time,
             request_end_time, duration_ms, output_tokens_per_second, ttft_ms, status_code,
             migration_attempted_at, estimated_cost, pricing_snapshot_id, cost_locked, api_key_prefix, request_base_url,
             client_tool, proxy_profile_id, client_detection_method, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, NULL, ?19, ?20, 1, ?21, ?22, ?23, ?24, ?25, ?26)
            "#,
            rusqlite::params![
                record.timestamp,
                &record.message_id,
                &storage_dedupe_key,
                &canonical_request_key,
                record.input_tokens as i64,
                record.output_tokens as i64,
                record.cache_create_tokens as i64,
                record.cache_read_tokens as i64,
                &record.model,
                &record.session_id,
                &session_resolution_state,
                if record.message_id_conflicted { 1 } else { 0 },
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
                now,
            ],
        )
        .map_err(|e| format!("Failed to insert record: {}", e))?;

        let id = conn.last_insert_rowid();
        let date = Self::record_local_date(record.timestamp);
        if date < Self::today_local_date() {
            Self::refresh_daily_summary_for_date_conn(&conn, &date)?;
            if let Ok(local_db) = crate::local_usage::LocalUsageDatabase::get_global() {
                let _ = local_db.invalidate_unified_materialization_dates(&[date]);
            }
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
        let now = chrono::Utc::now().timestamp();
        let mut stmt = conn
            .prepare(
                r#"
                UPDATE usage_records
                SET estimated_cost = ?1, pricing_snapshot_id = ?2, cost_locked = 1, updated_at = ?3
                WHERE id = ?4
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
            stmt.execute(rusqlite::params![cost, snapshot_id, now, id])
                .map_err(|e| format!("Failed to update cost backfill record: {}", e))?;
            let date = Self::record_local_date(*timestamp);
            if date < Self::today_local_date() {
                touched_dates.insert(date);
            }
        }
        drop(stmt);

        for date in &touched_dates {
            Self::refresh_daily_summary_for_date_conn(&conn, date)?;
        }

        eprintln!(
            "[database] Backfilled frozen cost for {} usage records",
            records.len()
        );
        if !touched_dates.is_empty() {
            if let Ok(local_db) = crate::local_usage::LocalUsageDatabase::get_global() {
                let _ = local_db.invalidate_unified_materialization_dates(
                    &touched_dates.into_iter().collect::<Vec<_>>(),
                );
            }
        }
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
        let now = chrono::Utc::now().timestamp();

        const BATCH_SIZE: usize = 1000;
        let mut total_updated: i64 = 0;

        let mut update_stmt = tx
            .prepare(
                r#"
                UPDATE usage_records
                SET estimated_cost = ?1, pricing_snapshot_id = ?2, cost_locked = 1, updated_at = ?3
                WHERE id = ?4
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
                    .execute(rusqlite::params![cost, &snapshot_id, now, id])
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
        if !touched_dates.is_empty() {
            if let Ok(local_db) = crate::local_usage::LocalUsageDatabase::get_global() {
                let _ = local_db.invalidate_unified_materialization_dates(
                    &touched_dates.into_iter().collect::<Vec<_>>(),
                );
            }
        }
        Ok(total_updated)
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
                       client_detection_method, storage_dedupe_key, canonical_request_key,
                       session_resolution_state, message_id_conflicted
                FROM usage_records
                WHERE timestamp >= ?1
                ORDER BY timestamp DESC
                "#,
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let records = stmt
            .query_map([cutoff_ms], session::usage_record_from_row)
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
                   client_detection_method, storage_dedupe_key, canonical_request_key,
                   session_resolution_state, message_id_conflicted
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
            .query_map(params.as_slice(), session::usage_record_from_row)
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

    pub fn get_merge_cache_signature(&self) -> Result<ProxyMergeCacheSignature, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;
        conn.query_row(
            r#"
            SELECT
                (SELECT COUNT(*) FROM usage_records),
                (SELECT COALESCE(MAX(timestamp), 0) FROM usage_records),
                (SELECT COALESCE(MAX(updated_at), 0) FROM usage_records),
                (SELECT COALESCE(MAX(last_updated), 0) FROM session_stats)
            "#,
            [],
            |row| {
                Ok(ProxyMergeCacheSignature {
                    usage_record_count: row.get::<_, i64>(0)?.max(0) as u64,
                    max_timestamp: row.get::<_, i64>(1)?,
                    max_updated_at: row.get::<_, i64>(2)?,
                    session_stats_max_updated_at: row.get::<_, i64>(3)?,
                })
            },
        )
        .map_err(|e| format!("Failed to compute proxy merge cache signature: {}", e))
    }

    pub fn get_day_dependency_snapshot(
        &self,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<ProxyDayDependencySnapshot, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;
        conn.query_row(
            r#"
            SELECT
                COUNT(*),
                COALESCE(MAX(timestamp), 0),
                COALESCE(MAX(updated_at), 0)
            FROM usage_records
            WHERE timestamp >= ?1 AND timestamp < ?2
            "#,
            rusqlite::params![start_ms, end_ms],
            |row| {
                Ok(ProxyDayDependencySnapshot {
                    record_count: row.get::<_, i64>(0)?.max(0) as u64,
                    max_timestamp_ms: row.get::<_, i64>(1)?,
                    max_updated_at: row.get::<_, i64>(2)?,
                })
            },
        )
        .map_err(|e| format!("Failed to compute proxy day dependency snapshot: {}", e))
    }

    pub fn get_request_time_bounds(&self) -> Result<Option<(i64, i64)>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;
        conn.query_row(
            "SELECT MIN(timestamp), MAX(timestamp) FROM usage_records",
            [],
            |row| {
                let min_ts: Option<i64> = row.get(0)?;
                let max_ts: Option<i64> = row.get(1)?;
                Ok(match (min_ts, max_ts) {
                    (Some(start_ms), Some(end_ms)) => {
                        Some((start_ms / 1000, (end_ms / 1000).saturating_add(1)))
                    }
                    _ => None,
                })
            },
        )
        .map_err(|e| format!("Failed to query proxy request time bounds: {}", e))
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

        if affected > 0 {
            if let Ok(local_db) = crate::local_usage::LocalUsageDatabase::get_global() {
                let _ = local_db.clear_unified_materialization();
            }
        }

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
                tool: crate::models::DEFAULT_CLIENT_TOOL.to_string(),
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
                        tool: crate::models::DEFAULT_CLIENT_TOOL.to_string(),
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

/// 窗口速率统计
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WindowRateStats {
    pub request_count: i64,
    pub total_output_tokens: i64,
    pub total_duration_ms: i64,
    pub avg_output_tokens_per_second: f64,
}

/// 状态码分布
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct StatusCodeDistribution {
    pub status_code: i64,
    pub count: i64,
    pub category: String, // "success", "client_error", "server_error" 成功、客户端错误、服务端错误
}
