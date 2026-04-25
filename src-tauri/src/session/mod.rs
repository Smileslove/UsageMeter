//! 会话模块 - JSONL 会话元数据提取
//!
//! 本模块提供扫描和提取 Claude Code 会话 JSONL 文件元数据的功能。

mod meta;
mod scanner;

#[allow(unused_imports)]
pub use meta::SessionMeta;
pub use scanner::{
    find_session_id_by_message_id, get_all_session_meta_cached, get_session_meta_by_id,
};
