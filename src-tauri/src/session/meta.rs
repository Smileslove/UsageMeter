//! 会话元数据模块
//!
//! 从 JSONL 文件提取会话元数据和本地请求事实，包括：
//! - 工作目录（cwd）
//! - 会话主题（首条用户消息）
//! - 最后提示（最后一条用户消息）
//! - 自定义会话名称（slug/customTitle）
//! - 去重后的 assistant request 统计

use serde::{Deserialize, Serialize};

/// 从 JSONL 文件提取的会话元数据
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    /// 会话 ID（从文件路径派生）
    pub session_id: String,
    /// 客户端工具标识，如 claude_code / codex / opencode
    #[serde(default)]
    pub tool: String,
    /// 工作目录（从 JSONL cwd 字段提取）
    pub cwd: Option<String>,
    /// 项目名称（从 cwd 提取，路径最后部分）
    pub project_name: Option<String>,
    /// 会话主题（首条用户消息，截断至 50 字符）
    pub topic: Option<String>,
    /// 最后用户提示（最后一条用户消息，截断至 100 字符）
    pub last_prompt: Option<String>,
    /// 自定义会话名称（来自 slug 或 customTitle）
    pub session_name: Option<String>,
    /// 文件路径
    pub file_path: String,
    /// 文件大小（字节）
    pub file_size: u64,
    /// 最后修改时间（Unix 时间戳）
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
    /// 请求数量（去重后的 assistant message.id 数）
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
    /// 该会话中所有消息的 ID 列表（用于关联代理数据库记录）
    #[serde(default, skip_serializing)]
    pub message_ids: Vec<String>,
}

/// 本地 transcript 中抽取出的单条请求事实
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocalRequestRecord {
    /// 所属会话 ID
    pub session_id: String,
    /// 客户端工具标识
    #[serde(default)]
    pub tool: String,
    /// 请求时间（Unix 时间戳，秒）
    pub timestamp: i64,
    /// assistant message.id
    pub message_id: String,
    /// 输入 Token（不含缓存）
    #[serde(default)]
    pub input_tokens: u64,
    /// 输出 Token
    #[serde(default)]
    pub output_tokens: u64,
    /// 缓存创建 Token
    #[serde(default)]
    pub cache_create_tokens: u64,
    /// 缓存读取 Token
    #[serde(default)]
    pub cache_read_tokens: u64,
    /// 总 Token = input + cache_create + cache_read + output
    #[serde(default)]
    pub total_tokens: u64,
    /// 使用模型
    #[serde(default)]
    pub model: String,
    /// 是否来自子代理 transcript
    #[serde(default)]
    pub is_subagent: bool,
    /// 全局请求键（持久化在 local_request_facts.request_key）；
    /// scanner 直接解析文件时不填，由 local_usage 层加载并填充。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_key: Option<String>,
    /// 来源文件当前是否仍存在；scanner 不填，由 local_usage 层加载并填充。
    /// None 视为"未知/仍存在"。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_file_present: Option<bool>,
}

/// 会话文件信息（用于扫描）
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SessionFile {
    /// 会话 ID（唯一标识符）
    pub session_id: String,
    /// 客户端工具标识
    pub tool: String,
    /// 项目路径名（如 "-Users-xxx-ProjectA"）
    pub project_path: String,
    /// 主 transcript 文件路径（优先顶层会话文件）
    pub file_path: String,
    /// 该会话归属的所有 transcript 路径（含子代理）
    pub transcript_paths: Vec<String>,
    /// 所有 transcript 文件总大小（字节）
    pub file_size: u64,
    /// 所有 transcript 的最新修改时间（Unix 时间戳）
    pub last_modified: i64,
    /// 内容指纹，用于增量缓存判定
    pub fingerprint: u64,
}
