//! SQLite 数据库，用于持久化代理使用数据

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
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

mod filters;
mod ingest;
mod migration;
mod pricing;
mod queries;
mod schema;
mod session;
mod stats;

pub use pricing::{PreviewPricingApplyResult, PricingMatchFilter};

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
        Ok(crate::utils::usagemeter_dir()?.join("proxy_data.db"))
    }

    /// 安全地将 i64 转换为 u64，负值返回 0
    fn safe_i64_to_u64(v: i64) -> u64 {
        if v < 0 {
            0
        } else {
            v as u64
        }
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
