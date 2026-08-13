//! 结构化会话活动（深度事件索引）领域模块（M2）。
//!
//! 与 `doc/桌面主应用窗口详细设计.md` 第 12 章对齐：领域模型（model）、
//! 脱敏（redact）、适配器接口（adapter）、适配器注册表（registry）、
//! SQLite 读写（db）。

pub mod adapter;
pub mod adapters;
pub mod db;
pub mod fts;
pub mod indexer;
pub mod maintenance;
pub mod model;
pub mod redact;
pub mod registry;

// IPC 命令骨架：文件位于 commands/activity.rs（与其它 Tauri 命令同目录），
// 经 #[path] 挂为本模块子模块，供 lib.rs 的 invoke_handler 引用，
// 避免侵入 commands/mod.rs 的既有导出面。
#[path = "../commands/activity.rs"]
pub mod commands;
// M3 全文搜索命令（commands/activity_search.rs），同一挂载模式。
#[path = "../commands/activity_search.rs"]
pub mod commands_search;

pub use adapter::*;
pub use db::{ActivityIndexEntry, ACTIVITY_TABLES_DDL};
pub use model::*;
pub use redact::*;
pub use registry::*;
