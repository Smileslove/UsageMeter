//! SQLite 数据库，用于持久化代理使用数据

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

/// 全局数据库实例（用于查询操作，避免重复打开连接）
static GLOBAL_DB: OnceLock<Arc<ProxyDatabase>> = OnceLock::new();

/// 进程级初始化互斥：SQLite 是单写者模型，多个线程并发打开连接并执行
/// schema 迁移（写事务）时，在“都已读后升级写锁”等场景下 busy handler
/// 不会等待，会直接报 `database is locked`。用这把锁将 pragma / 建表 /
/// 迁移等初始化写段串行化，从根上消除启动期并发初始化的锁竞争。
/// 同时兼任全局单例创建的互斥：`get_or_create_global` 的“检查-创建-
/// 缓存”整体在锁内完成，保证全局实例只初始化一次、迁移只执行一次。
static INIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) const LEGACY_UNMATCHED_SESSION_ID: &str = "__legacy_unmatched__";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProxyMergeCacheSignature {
    pub merge_cache_generation: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProxyDayDependencySnapshot {
    pub record_count: u64,
    pub max_timestamp_ms: i64,
    pub max_updated_at: i64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CcSwitchImportState {
    pub cursor_created_at: i64,
    pub cursor_request_id: String,
    pub last_import_at_ms: Option<i64>,
    pub total_imported: u64,
    pub last_error_code: Option<String>,
}

mod filters;
mod ingest;
mod migration;
mod pricing;
mod queries;
mod schema;
mod session;
mod stats;

pub use pricing::{PreviewPricingApplyResult, PricingMatchFilter};
pub(crate) use session::computed_session_resolution_state;

/// 数据库管理器，用于代理使用数据
/// 使用线程安全的 SQLite 连接包装器
pub struct ProxyDatabase {
    pub(super) conn: Arc<std::sync::Mutex<Connection>>,
}

impl ProxyDatabase {
    fn stored_day_boundary_mode_conn(conn: &Connection) -> Result<Option<String>, String> {
        conn.query_row(
            "SELECT state_value FROM daily_rollup_state WHERE state_key = 'day_boundary_mode'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to load proxy daily rollup state: {}", e))
    }

    fn set_day_boundary_mode_conn(conn: &Connection, mode: &str) -> Result<(), String> {
        conn.execute(
            "INSERT INTO daily_rollup_state (state_key, state_value, updated_at)
             VALUES ('day_boundary_mode', ?1, ?2)
             ON CONFLICT(state_key) DO UPDATE
             SET state_value = excluded.state_value,
                 updated_at = excluded.updated_at",
            rusqlite::params![mode, chrono::Utc::now().timestamp()],
        )
        .map_err(|e| format!("Failed to store proxy day boundary mode: {}", e))?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn ensure_daily_rollup_mode_current(&self) -> Result<(), String> {
        // 当前口径来自用户设置文件；核心重置逻辑放在可注入口径的内部方法中，
        // 便于测试不依赖本机真实设置。
        self.ensure_daily_rollup_mode_current_with_mode(&Self::current_day_boundary_mode())
    }

    /// 以显式给定的“当前日界口径”校验并重置 rollup（口径不一致时清空重建）。
    pub(crate) fn ensure_daily_rollup_mode_current_with_mode(
        &self,
        current_mode: &str,
    ) -> Result<(), String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;
        let stored_mode = Self::stored_day_boundary_mode_conn(&conn)?;
        if stored_mode.as_deref() != Some(current_mode) {
            conn.execute("DELETE FROM daily_summary", [])
                .map_err(|e| format!("Failed to clear proxy daily summary: {}", e))?;
            conn.execute("DELETE FROM model_usage", [])
                .map_err(|e| format!("Failed to clear proxy model usage: {}", e))?;
            Self::set_day_boundary_mode_conn(&conn, current_mode)?;
        }
        Ok(())
    }

    /// 获取全局数据库实例（用于查询操作）
    /// 如果数据库文件存在，返回共享的数据库实例
    /// 如果不存在，返回 None
    pub fn get_global() -> Option<Arc<ProxyDatabase>> {
        GLOBAL_DB.get().cloned().or_else(|| {
            let db_path = Self::get_db_path().ok()?;
            if !db_path.exists() {
                return None;
            }
            Self::get_or_create_global().ok()
        })
    }

    /// 获取全局数据库实例；尚未初始化时创建并缓存（文件不存在也会创建）。
    /// 并发安全：整个“检查-创建-缓存”在 `INIT_LOCK` 内原子完成，严格
    /// 保证全局单例只初始化一次、schema 迁移只执行一次。
    pub fn get_or_create_global() -> Result<Arc<ProxyDatabase>, String> {
        let _init_guard = INIT_LOCK
            .lock()
            .map_err(|e| format!("Failed to lock database initialization: {e}"))?;
        if let Some(db) = GLOBAL_DB.get() {
            return Ok(db.clone());
        }
        let db_path = Self::get_db_path()?;
        let db = Arc::new(Self::new_with_path_unlocked(&db_path)?);
        let _ = GLOBAL_DB.set(db.clone());
        Ok(db)
    }

    /// 创建新的数据库连接
    pub fn new() -> Result<Self, String> {
        let db_path = Self::get_db_path()?;
        Self::new_with_path(&db_path)
    }

    /// Opens only the shared model-pricing table without running usage-data
    /// migrations. Configuration import uses this path to avoid recursively
    /// loading application settings from a proxy schema migration.
    pub fn new_pricing_store() -> Result<Self, String> {
        let db_path = Self::get_db_path()?;
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create database directory: {}", e))?;
        }
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open pricing database: {}", e))?;
        let _init_guard = INIT_LOCK
            .lock()
            .map_err(|e| format!("Failed to lock pricing database initialization: {e}"))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("Failed to enable pricing WAL mode: {}", e))?;
        conn.pragma_update(None, "wal_autocheckpoint", 1000_i64)
            .map_err(|e| format!("Failed to configure pricing WAL autocheckpoint: {}", e))?;
        conn.pragma_update(None, "journal_size_limit", 8_i64 * 1024 * 1024)
            .map_err(|e| format!("Failed to configure pricing WAL size limit: {}", e))?;
        conn.busy_timeout(Duration::from_secs(30))
            .map_err(|e| format!("Failed to set pricing database busy timeout: {}", e))?;
        Self::create_model_pricing_table_static(&conn)?;
        Ok(Self {
            conn: Arc::new(std::sync::Mutex::new(conn)),
        })
    }

    /// 使用指定路径创建数据库连接（用于独立查询）。初始化写段由
    /// `INIT_LOCK` 串行化，避免并发打开连接时互相撞写锁。
    pub fn new_with_path(db_path: &PathBuf) -> Result<Self, String> {
        let _init_guard = INIT_LOCK
            .lock()
            .map_err(|e| format!("Failed to lock database initialization: {e}"))?;
        Self::new_with_path_unlocked(db_path)
    }

    /// 初始化主体：调用方必须已持有 `INIT_LOCK`（见 `new_with_path`）。
    fn new_with_path_unlocked(db_path: &PathBuf) -> Result<Self, String> {
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
        conn.pragma_update(None, "wal_autocheckpoint", 1000_i64)
            .map_err(|e| format!("Failed to configure WAL autocheckpoint: {}", e))?;
        conn.pragma_update(None, "journal_size_limit", 8_i64 * 1024 * 1024)
            .map_err(|e| format!("Failed to configure WAL size limit: {}", e))?;
        conn.busy_timeout(Duration::from_secs(30))
            .map_err(|e| format!("Failed to set SQLite busy timeout: {}", e))?;

        // 创建表
        Self::create_tables(&conn)?;

        // 迁移旧表结构（添加新字段）。若迁移规范化了历史 Proxy 记录，
        // 在 Proxy 事务完成后使对应 Local 物化日期失效。
        let normalized_history_dates = Self::migrate_schema(&conn)?;
        Self::create_merge_cache_generation_tracking(&conn)?;
        let rate_metrics_changed = Self::migrate_rate_metrics(&conn)?;
        conn.execute("DROP INDEX IF EXISTS idx_usage_storage_key", [])
            .map_err(|e| format!("Failed to drop redundant storage key index: {e}"))?;
        if rate_metrics_changed {
            let local_db = crate::local_usage::LocalUsageDatabase::get_global().map_err(|e| {
                format!("Failed to open Local database for Proxy rate migration: {e}")
            })?;
            local_db.clear_unified_materialization().map_err(|e| {
                format!("Failed to clear Local materialization after Proxy rate migration: {e}")
            })?;
        } else {
            Self::invalidate_local_materialization_after_proxy_commit(&normalized_history_dates)?;
        }
        // 创建模型价格表
        Self::create_model_pricing_table_static(&conn)?;

        let db = Self {
            conn: Arc::new(std::sync::Mutex::new(conn)),
        };
        db.checkpoint_wal_passive();
        Ok(db)
    }

    /// 获取数据库路径
    fn get_db_path() -> Result<PathBuf, String> {
        Ok(crate::utils::usagemeter_dir()?.join("proxy_data.db"))
    }

    pub(super) fn checkpoint_wal_passive(&self) {
        let Ok(conn) = self.conn.lock() else { return };
        if let Err(error) = conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE)") {
            eprintln!("[database] Passive proxy WAL checkpoint skipped: {error}");
        }
    }

    /// 安全地将 i64 转换为 u64，负值返回 0
    fn safe_i64_to_u64(v: i64) -> u64 {
        if v < 0 {
            0
        } else {
            v as u64
        }
    }

    pub fn clear_daily_rollups(&self) -> Result<(), String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;
        conn.execute("DELETE FROM daily_summary", [])
            .map_err(|e| format!("Failed to clear proxy daily summary: {}", e))?;
        conn.execute("DELETE FROM model_usage", [])
            .map_err(|e| format!("Failed to clear proxy model usage: {}", e))?;
        Self::set_day_boundary_mode_conn(&conn, &Self::current_day_boundary_mode())?;
        Ok(())
    }

    /// Proxy 与 Local 使用独立 SQLite 数据库，无法将 Proxy 更新和物化状态失效
    /// 放入同一事务。Proxy 事务提交后先精确失效受影响历史日；若精确删除失败，
    /// 降级清空全部物化数据，避免后续读取错误信任旧状态行。
    pub(super) fn invalidate_local_materialization_after_proxy_commit(
        local_dates: &[String],
    ) -> Result<(), String> {
        if local_dates.is_empty() {
            return Ok(());
        }

        let local_db = crate::local_usage::LocalUsageDatabase::get_global()
            .map_err(|e| format!("Failed to open Local database for Proxy invalidation: {e}"))?;
        if let Err(exact_error) = local_db.invalidate_unified_materialization_dates(local_dates) {
            local_db.clear_unified_materialization().map_err(|clear_error| {
                format!(
                    "Failed to invalidate Local materialization after Proxy commit: exact={exact_error}; fallback_clear={clear_error}"
                )
            })?;
            eprintln!(
                "[database] Exact Proxy materialization invalidation failed; cleared all materialized history: {exact_error}"
            );
        }
        Ok(())
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
    /// Derived sum of per-request generation durations, not end-to-end latency.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归测试：多个线程并发初始化同一个数据库文件时，不得因
    /// `database is locked` 失败。此前 UsageCollector 在启动期被多个
    /// tokio worker 并发构造，各自打开独立连接并并发跑 schema 迁移
    /// （写事务），SQLite 单写者锁竞争导致 panic。
    #[test]
    fn concurrent_new_with_path_initialization_succeeds() {
        // tempdir 随 drop 自动清理，测试不残留临时数据。
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("proxy_data.db");

        let threads: Vec<_> = (0..6)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || ProxyDatabase::new_with_path(&path))
            })
            .collect();

        for thread in threads {
            let result = thread.join().expect("init thread panicked");
            assert!(
                result.is_ok(),
                "concurrent database initialization failed: {:?}",
                result.err()
            );
        }
    }
}
