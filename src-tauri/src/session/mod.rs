//! Session module - JSONL session metadata extraction
//!
//! This module provides functionality to scan and extract metadata
//! from Claude Code session JSONL files.

mod meta;
mod scanner;

pub use meta::{SessionFile, SessionMeta};
pub use scanner::{
    extract_session_meta, get_all_session_meta, get_all_session_meta_raw, get_session_meta_by_id,
    scan_session_files,
};
