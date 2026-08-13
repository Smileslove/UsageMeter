//! 会话活动适配器接口（与 `doc/桌面主应用窗口详细设计.md` 12.7 节对齐）。
//!
//! 每个来源工具一个独立适配器，注册到 [`crate::activity::registry`]。
//! 适配器负责“事实抽取”，不负责 UI 文案；能力差异用
//! [`SessionActivityCapability`] 显式表达，不允许用空值伪装同等支持。

use crate::activity::model::{
    AgentRelationLevel, ContentState, EventStatus, RedactedPayloadPage, RequestLinkStrength,
    SafeSourceRef, SessionActivityCapability, SessionEventKind,
};

/// 深度事件索引的来源会话引用。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSourceRef {
    pub session_id: String,
    pub tool: String,
    pub primary_file_path: String,
    pub source_file_id: Option<i64>,
}

/// 适配器一次索引产出（会话级全量快照；写入层按 session_key 替换，幂等）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActivityIndexBatch {
    pub session_key: String,
    pub events: Vec<NewSessionEvent>,
    pub agents: Vec<NewAgentNode>,
    pub tool_invocations: Vec<NewToolInvocation>,
    pub request_links: Vec<NewEventRequestLink>,
}

/// 新事件（写入层负责脱敏摘要截断与落库；`summary_redacted` 由适配器
/// 经 [`crate::activity::redact::redact_text`] 处理后提供，长度 ≤ 500）。
#[derive(Debug, Clone, PartialEq)]
pub struct NewSessionEvent {
    pub event_key: String,
    pub sequence: i64,
    pub timestamp_ms: Option<i64>,
    pub kind: SessionEventKind,
    pub status: Option<EventStatus>,
    pub actor_agent_key: Option<String>,
    pub parent_event_key: Option<String>,
    pub summary_redacted: Option<String>,
    pub content_state: ContentState,
    pub source_file_path: String,
    pub source_offset: Option<i64>,
    pub payload_hash: Option<String>,
    pub raw_event_kind: String,
}

/// 新代理节点。
#[derive(Debug, Clone, PartialEq)]
pub struct NewAgentNode {
    pub agent_key: String,
    pub parent_agent_key: Option<String>,
    pub display_kind: Option<String>,
    pub task_summary_redacted: Option<String>,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    pub status: Option<EventStatus>,
    pub relation_level: AgentRelationLevel,
}

/// 新工具调用（完整 payload 不落库；输入输出只记大小与 key 列表）。
#[derive(Debug, Clone, PartialEq)]
pub struct NewToolInvocation {
    pub invocation_key: String,
    pub event_key: String,
    pub tool_name: String,
    pub family: String,
    pub duration_ms: Option<i64>,
    pub status: Option<EventStatus>,
    pub input_bytes: Option<i64>,
    pub output_bytes: Option<i64>,
    pub input_keys: Vec<String>,
    pub result_kind: Option<String>,
}

/// 新事件-请求关联。
#[derive(Debug, Clone, PartialEq)]
pub struct NewEventRequestLink {
    pub event_key: String,
    pub request_key: String,
    pub strength: RequestLinkStrength,
}

/// 适配器错误：只输出稳定错误码与类别，不携带正文、路径、key 等敏感内容。
#[derive(Debug)]
pub enum ActivityError {
    /// 源文件读取/IO 失败
    Io(String),
    /// 源文件解析失败
    Parse(String),
    /// 该工具/会话不支持深度事件索引
    Unsupported(String),
    /// 脱敏失败（拒绝在无法保证脱敏的情况下输出内容）
    Redact(String),
    /// 内部错误
    Internal(String),
}

impl std::fmt::Display for ActivityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) => write!(f, "ERR_ACTIVITY_IO: {message}"),
            Self::Parse(message) => write!(f, "ERR_ACTIVITY_PARSE: {message}"),
            Self::Unsupported(message) => write!(f, "ERR_ACTIVITY_UNSUPPORTED: {message}"),
            Self::Redact(message) => write!(f, "ERR_ACTIVITY_REDACT: {message}"),
            Self::Internal(message) => write!(f, "ERR_ACTIVITY_INTERNAL: {message}"),
        }
    }
}

impl std::error::Error for ActivityError {}

/// 会话活动适配器接口。
pub trait SessionActivityAdapter: Send + Sync {
    /// 来源工具 id（与 `session::constants::TOOL_*` 一致，如 `claude_code`）。
    fn tool_name(&self) -> &'static str;

    /// 该工具当前的深度活动能力快照。
    fn capability(&self) -> SessionActivityCapability;

    /// 索引单个会话：产出全量事件/代理/工具调用/请求关联批次。
    fn index_session(&self, source: &SessionSourceRef)
        -> Result<ActivityIndexBatch, ActivityError>;

    /// 按需读取（脱敏后的）payload 分页。
    fn read_payload(
        &self,
        source_ref: &SafeSourceRef,
        section: &str,
        max_bytes: usize,
    ) -> Result<RedactedPayloadPage, ActivityError>;
}
