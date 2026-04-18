//! Session module - JSONL session metadata extraction
//!
//! This module provides functionality to scan and extract metadata
//! from Claude Code session JSONL files.

mod meta;
mod scanner;

#[allow(unused_imports)]
pub use meta::SessionMeta;
#[allow(unused_imports)]
pub use scanner::{
    extract_session_meta, get_all_session_meta_raw, get_session_meta_by_id,
};
