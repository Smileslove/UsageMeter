//! Session module - JSONL session metadata extraction
//!
//! This module provides functionality to scan and extract metadata
//! from Claude Code session JSONL files.

mod meta;
mod scanner;

#[allow(unused_imports)]
pub use meta::SessionMeta;
pub use scanner::{
    find_session_id_by_message_id, get_all_session_meta_cached, get_session_meta_by_id,
};
