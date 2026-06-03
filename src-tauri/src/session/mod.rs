//! 会话模块 - JSONL 会话元数据提取（Claude Code / Codex / OpenCode）
//!
//! 本模块提供扫描和提取本地会话数据的功能。

mod meta;
pub(crate) mod opencode_reader;
mod scanner;

#[allow(unused_imports)]
pub use meta::{LocalRequestRecord, SessionFile, SessionMeta};
pub use scanner::{
    find_session_id_by_message_id, get_all_session_meta_cached, parse_session_file_for_storage,
    scan_session_files,
};
