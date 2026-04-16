//! Session metadata module
//!
//! Extracts session metadata from JSONL files including:
//! - Working directory (cwd)
//! - Session topic (first user message)
//! - Last prompt (last user message)
//! - Custom session name (slug/customTitle)
//! - Basic token statistics (when available in JSONL)

use serde::{Deserialize, Serialize};

/// Session metadata extracted from JSONL files
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    /// Session ID (derived from file path)
    pub session_id: String,
    /// Working directory (from JSONL cwd field)
    pub cwd: Option<String>,
    /// Project name (extracted from cwd, last path component)
    pub project_name: Option<String>,
    /// Session topic (first user message, truncated to 50 chars)
    pub topic: Option<String>,
    /// Last user prompt (last user message, truncated to 100 chars)
    pub last_prompt: Option<String>,
    /// Custom session name (from slug or customTitle)
    pub session_name: Option<String>,
    /// File path
    pub file_path: String,
    /// File size in bytes
    pub file_size: u64,
    /// Last modified time (Unix timestamp)
    pub last_modified: i64,
    // === 从 JSONL 提取的统计信息 ===
    /// 总输入 Token（从 JSONL usage 字段提取）
    #[serde(default)]
    pub total_input_tokens: u64,
    /// 总输出 Token
    #[serde(default)]
    pub total_output_tokens: u64,
    /// 总缓存创建 Token
    #[serde(default)]
    pub total_cache_create_tokens: u64,
    /// 总缓存读取 Token
    #[serde(default)]
    pub total_cache_read_tokens: u64,
    /// 模型列表
    #[serde(default)]
    pub models: Vec<String>,
    /// 消息数量
    #[serde(default)]
    pub message_count: u64,
    /// 开始时间（Unix timestamp）
    #[serde(default)]
    pub start_time: i64,
    /// 结束时间（Unix timestamp）
    #[serde(default)]
    pub end_time: i64,
    /// 数据来源
    #[serde(default)]
    pub source: String,
}

/// Session file information (used for scanning)
#[derive(Debug, Clone)]
pub struct SessionFile {
    /// Session ID (unique identifier)
    pub session_id: String,
    /// Project path name (e.g., "-Users-xxx-ProjectA")
    pub project_path: String,
    /// Full JSONL file path
    pub file_path: String,
    /// File size in bytes
    pub file_size: u64,
    /// Last modified time (Unix timestamp)
    pub last_modified: i64,
}
