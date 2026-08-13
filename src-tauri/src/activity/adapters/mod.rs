//! 各来源工具的深度事件适配器（M2）。
//!
//! 每个适配器只负责事实抽取：解析源文件为结构化事件批次，
//! 摘要一律经 [`crate::activity::redact`] 脱敏，完整正文不落库。

pub mod claude;
pub mod codex;
