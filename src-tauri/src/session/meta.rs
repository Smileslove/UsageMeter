//! 会话元数据模块
//!
//! 从 JSONL 文件提取会话元数据，包括：
//! - 工作目录（cwd）
//! - 会话主题（首条用户消息）
//! - 最后提示（最后一条用户消息）
//! - 自定义会话名称（slug/customTitle）
//! - 基本 Token 统计（JSONL 中可用时）

use serde::{Deserialize, Serialize};

/// 从 JSONL 文件提取的会话元数据
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    /// 会话 ID（从文件路径派生）
    pub session_id: String,
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
    /// 该会话中所有消息的 ID 列表（用于关联代理数据库记录）
    #[serde(default, skip_serializing)]
    pub message_ids: Vec<String>,
}

/// 会话文件信息（用于扫描）
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SessionFile {
    /// 会话 ID（唯一标识符）
    pub session_id: String,
    /// 项目路径名（如 "-Users-xxx-ProjectA"）
    pub project_path: String,
    /// 完整 JSONL 文件路径
    pub file_path: String,
    /// 文件大小（字节）
    pub file_size: u64,
    /// 最后修改时间（Unix 时间戳）
    pub last_modified: i64,
}
