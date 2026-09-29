//! 结构化会话活动（深度事件索引）领域模型。
//!
//! 类型与 `doc/桌面主应用窗口详细设计.md` 12.3 节的 TypeScript DTO 草案对齐，
//! serde 统一使用 `camelCase`（如 `toolInvocations`、`startedAtMs`），供前端
//! IPC 直用；SQLite 中的枚举字符串也复用同一 `as_str()` 序列化值，保证
//! 存储、IPC、前端三处取值一致。

use std::collections::HashMap;

/// 活动能力级别：适配器实际支持的结构化程度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActivityCapabilityLevel {
    /// 无深度事件索引能力（仅聚合会话）
    None,
    /// 仅事件元数据（类型/时间/状态/大小），无正文
    Metadata,
    /// 结构化事件、工具调用摘要、代理关系
    Structured,
    /// 结构化事件 + 可读取完整正文内容
    FullContent,
}

impl ActivityCapabilityLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Metadata => "metadata",
            Self::Structured => "structured",
            Self::FullContent => "fullContent",
        }
    }
}

/// 代理关系可证明层级（关系 provenance，不允许把推断伪装成精确事实）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentRelationLevel {
    /// 无法证明任何代理关系
    None,
    /// 只能标记“该事件可能来自子代理”，无法分组
    FlagOnly,
    /// 只能把事件归到根会话组，无法建立父子树
    RootGrouped,
    /// 完整父子树
    FullTree,
}

impl AgentRelationLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::FlagOnly => "flagOnly",
            Self::RootGrouped => "rootGrouped",
            Self::FullTree => "fullTree",
        }
    }

    pub fn parse_db(value: &str) -> Self {
        match value {
            "flagOnly" => Self::FlagOnly,
            "rootGrouped" => Self::RootGrouped,
            "fullTree" => Self::FullTree,
            _ => Self::None,
        }
    }
}

/// 单工具深度活动能力快照（运行时返回值，UI 不得按工具名硬编码）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActivityCapability {
    pub level: ActivityCapabilityLevel,
    pub messages: bool,
    pub tool_invocations: bool,
    pub tool_results: bool,
    pub request_links: bool,
    pub agent_relations: AgentRelationLevel,
    pub content_search: bool,
    pub source_content_available: bool,
    pub parser_id: String,
    pub parser_version: i64,
}

/// 会话事件种类。
///
/// serde 序列化使用 `camelCase`（`userMessage`、`toolInvocation` …），与
/// 12.3 节 TS DTO 对齐；SQLite `kind` 列存同一 `as_str()` 值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionEventKind {
    UserMessage,
    AssistantMessage,
    ToolInvocation,
    ToolResult,
    AgentStarted,
    AgentFinished,
    SystemEvent,
    Compaction,
    Error,
    Unknown,
}

impl SessionEventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UserMessage => "userMessage",
            Self::AssistantMessage => "assistantMessage",
            Self::ToolInvocation => "toolInvocation",
            Self::ToolResult => "toolResult",
            Self::AgentStarted => "agentStarted",
            Self::AgentFinished => "agentFinished",
            Self::SystemEvent => "systemEvent",
            Self::Compaction => "compaction",
            Self::Error => "error",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse_db(value: &str) -> Self {
        match value {
            "userMessage" => Self::UserMessage,
            "assistantMessage" => Self::AssistantMessage,
            "toolInvocation" => Self::ToolInvocation,
            "toolResult" => Self::ToolResult,
            "agentStarted" => Self::AgentStarted,
            "agentFinished" => Self::AgentFinished,
            "systemEvent" => Self::SystemEvent,
            "compaction" => Self::Compaction,
            "error" => Self::Error,
            _ => Self::Unknown,
        }
    }
}

/// 事件/代理状态。
///
/// 除任务约定的 Pending|Running|Success|Error|Cancelled 外，另含 `Unknown`
/// 变体：设计文档 12.3 的 `AgentNodeDto.status` 联合类型包含 `'unknown'`，
/// 且 `session_agents.status` 列可空，需要显式值表达“未知”而不是伪造状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventStatus {
    Pending,
    Running,
    Success,
    Error,
    Cancelled,
    Unknown,
}

impl EventStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Success => "success",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse_db(value: &str) -> Self {
        match value {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "success" => Self::Success,
            "error" => Self::Error,
            "cancelled" => Self::Cancelled,
            _ => Self::Unknown,
        }
    }
}

/// 正文内容可用性状态（列表查询不返回大 payload，只报可用性）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContentState {
    None,
    Available,
    Redacted,
    Truncated,
    Unavailable,
}

impl ContentState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Available => "available",
            Self::Redacted => "redacted",
            Self::Truncated => "truncated",
            Self::Unavailable => "unavailable",
        }
    }

    pub fn parse_db(value: &str) -> Self {
        match value {
            "available" => Self::Available,
            "redacted" => Self::Redacted,
            "truncated" => Self::Truncated,
            "unavailable" => Self::Unavailable,
            _ => Self::None,
        }
    }
}

/// 请求关联强度（provenance）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RequestLinkStrength {
    /// 完全精确匹配（如 request id 相同）
    Exact,
    /// 来源文件显式声明（如事件内嵌 request id 字段）
    SourceExplicit,
    /// 时间窗口推断（confidence 最低）
    TimeWindow,
}

impl RequestLinkStrength {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::SourceExplicit => "sourceExplicit",
            Self::TimeWindow => "timeWindow",
        }
    }

    pub fn parse_db(value: &str) -> Self {
        match value {
            "exact" => Self::Exact,
            "timeWindow" => Self::TimeWindow,
            _ => Self::SourceExplicit,
        }
    }
}

/// 事件与用量请求的关联（只服务解释，不影响 `local_request_facts` 去重）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestEventLink {
    pub request_key: String,
    pub strength: RequestLinkStrength,
}

/// 工具调用摘要（完整 payload 不落库，on-demand 读取）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInvocationSummary {
    pub invocation_key: String,
    pub raw_name: String,
    pub normalized_name: String,
    pub family: String,
    pub duration_ms: Option<i64>,
    pub input_bytes: Option<i64>,
    pub output_bytes: Option<i64>,
    pub input_keys: Vec<String>,
    pub result_kind: Option<String>,
}

/// 安全来源引用（定位 on-demand payload 读取，不含未脱敏路径以外敏感信息）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SafeSourceRef {
    pub source_file_id: Option<i64>,
    pub source_file_path: String,
    pub source_offset: Option<i64>,
    pub fingerprint: Option<String>,
    /// Internal locator for adapters that need sub-line block precision.
    #[serde(skip)]
    pub event_key: Option<String>,
}

/// 会话事件列表项。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEventListItem {
    pub event_key: String,
    pub session_key: String,
    pub sequence: i64,
    pub timestamp_ms: Option<i64>,
    pub kind: SessionEventKind,
    pub status: Option<EventStatus>,
    pub actor_agent_key: Option<String>,
    pub parent_event_key: Option<String>,
    pub summary: Option<String>,
    pub content_state: ContentState,
    pub tool: Option<ToolInvocationSummary>,
    pub request_links: Vec<RequestEventLink>,
    pub source_ref: SafeSourceRef,
}

/// 代理节点（前端树形展示用 DTO）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentNodeDto {
    pub agent_key: String,
    pub session_key: String,
    pub parent_agent_key: Option<String>,
    pub relation_level: AgentRelationLevel,
    pub display_kind: Option<String>,
    pub task_summary: Option<String>,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    pub status: EventStatus,
    pub request_count: Option<i64>,
    pub total_tokens: Option<i64>,
    pub estimated_cost: Option<f64>,
    pub child_count: i64,
}

/// 单会话深度索引汇总。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActivitySummary {
    pub session_key: String,
    pub capability: SessionActivityCapability,
    /// kind -> 事件数（键为 `SessionEventKind::as_str()` 值）
    pub event_counts: HashMap<String, i64>,
    /// 代理状态 -> 数量（键为 `EventStatus::as_str()` 值）
    pub agent_counts: HashMap<String, i64>,
    /// 工具名 -> 调用数
    pub tool_counts: Vec<(String, i64)>,
    /// 覆盖度描述（M2 取能力级别字符串，M3 细化）
    pub coverage: String,
}

/// 会话事件查询过滤（M2 的 search 仅作用于已加载摘要，不做全文索引）。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEventFilter {
    pub kinds: Option<Vec<SessionEventKind>>,
    pub tool_names: Option<Vec<String>>,
    pub agents: Option<Vec<String>>,
    pub statuses: Option<Vec<EventStatus>>,
    pub search: Option<String>,
    pub min_sequence: Option<i64>,
    pub max_sequence: Option<i64>,
}

/// 事件分页结果。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsPage {
    pub items: Vec<SessionEventListItem>,
    pub total: i64,
    pub has_more: bool,
}

/// 工具汇总行。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSummaryRow {
    pub tool_name: String,
    pub family: String,
    pub invocation_count: i64,
    pub success_count: i64,
    pub error_count: i64,
    pub total_duration_ms: i64,
    pub avg_duration_ms: i64,
    pub input_bytes_total: i64,
    pub output_bytes_total: i64,
}

/// on-demand payload 分页（M2 骨架阶段通常返回 Unavailable）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactedPayloadPage {
    pub content: String,
    pub truncated: bool,
    pub next_cursor: Option<String>,
    pub content_state: ContentState,
}

/// 活动导出选项（M3；21.5：正文与工具 payload 默认不导出，前端负责
/// 范围预览确认，后端只执行选项）。
///
/// 所有字段 serde 默认缺省即取 [`ExportOptions::default`] 值，前端可只传
/// 需要覆盖的字段。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExportOptions {
    /// 导出格式："json" | "csv"。
    pub format: String,
    /// 是否包含事件脱敏摘要（summary_redacted 已脱敏）。
    pub include_summaries: bool,
    /// 是否包含工具调用摘要（工具名/族/耗时/大小/入参 key）。
    pub include_tool_summaries: bool,
    /// 是否包含事件-请求关联。
    pub include_request_links: bool,
    /// 是否包含脱敏 payload（高风险内容；默认 false，前端预览确认后显式开启）。
    pub include_payloads: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: "json".to_string(),
            include_summaries: true,
            include_tool_summaries: true,
            include_request_links: true,
            include_payloads: false,
        }
    }
}

/// 活动导出结果。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    /// 导出文件绝对路径（`~/.usagemeter/exports/activity-*.{json|csv}`）。
    pub file_path: String,
    /// 导出的事件行数。
    pub row_count: i64,
    /// 是否实际写入了脱敏 payload（include_payloads 且读取到内容）。
    pub payload_included: bool,
    /// 是否因累计 payload 超过上限（50MB）提前停止读取而截断。
    pub truncated: bool,
}

/// 深度索引重建结果。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RebuildResult {
    pub sessions_indexed: i64,
    pub sessions_failed: i64,
    pub events_written: i64,
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_serde_roundtrip_camel_case() {
        for kind in [
            SessionEventKind::UserMessage,
            SessionEventKind::AssistantMessage,
            SessionEventKind::ToolInvocation,
            SessionEventKind::ToolResult,
            SessionEventKind::AgentStarted,
            SessionEventKind::AgentFinished,
            SessionEventKind::SystemEvent,
            SessionEventKind::Compaction,
            SessionEventKind::Error,
            SessionEventKind::Unknown,
        ] {
            let json = serde_json::to_string(&kind).expect("serialize kind");
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            let back: SessionEventKind = serde_json::from_str(&json).expect("deserialize kind");
            assert_eq!(back, kind);
            assert_eq!(SessionEventKind::parse_db(kind.as_str()), kind);
        }
    }

    #[test]
    fn enum_str_values_match_design_dto() {
        assert_eq!(SessionEventKind::UserMessage.as_str(), "userMessage");
        assert_eq!(SessionEventKind::ToolInvocation.as_str(), "toolInvocation");
        assert_eq!(RequestLinkStrength::Exact.as_str(), "exact");
        assert_eq!(
            RequestLinkStrength::SourceExplicit.as_str(),
            "sourceExplicit"
        );
        assert_eq!(RequestLinkStrength::TimeWindow.as_str(), "timeWindow");
        assert_eq!(AgentRelationLevel::RootGrouped.as_str(), "rootGrouped");
        assert_eq!(ActivityCapabilityLevel::FullContent.as_str(), "fullContent");
    }

    #[test]
    fn event_list_item_serde_roundtrip() {
        let item = SessionEventListItem {
            event_key: "evt-1".to_string(),
            session_key: "sess-1".to_string(),
            sequence: 3,
            timestamp_ms: Some(1700000000123),
            kind: SessionEventKind::ToolInvocation,
            status: Some(EventStatus::Running),
            actor_agent_key: Some("agent-2".to_string()),
            parent_event_key: Some("evt-0".to_string()),
            summary: Some("read file".to_string()),
            content_state: ContentState::Redacted,
            tool: Some(ToolInvocationSummary {
                invocation_key: "inv-1".to_string(),
                raw_name: "Read".to_string(),
                normalized_name: "read_file".to_string(),
                family: "fs".to_string(),
                duration_ms: Some(1200),
                input_bytes: Some(64),
                output_bytes: Some(2048),
                input_keys: vec!["file_path".to_string()],
                result_kind: Some("ok".to_string()),
            }),
            request_links: vec![RequestEventLink {
                request_key: "req-1".to_string(),
                strength: RequestLinkStrength::Exact,
            }],
            source_ref: SafeSourceRef {
                source_file_id: Some(7),
                source_file_path: "~/.claude/projects/x.jsonl".to_string(),
                source_offset: Some(1024),
                fingerprint: Some("abc".to_string()),
                event_key: None,
            },
        };
        let json = serde_json::to_string(&item).expect("serialize item");
        assert!(!json.contains("\"toolInvocations\"")); // list item 无该字段
        assert!(!json.contains("\"startedAtMs\""));
        assert!(json.contains("\"eventKey\":\"evt-1\""));
        let back: SessionEventListItem = serde_json::from_str(&json).expect("deserialize item");
        assert_eq!(back, item);
    }

    #[test]
    fn agent_node_dto_serde_roundtrip() {
        let node = AgentNodeDto {
            agent_key: "agent-1".to_string(),
            session_key: "sess-1".to_string(),
            parent_agent_key: None,
            relation_level: AgentRelationLevel::FullTree,
            display_kind: Some("Task".to_string()),
            task_summary: Some("fix bug".to_string()),
            started_at_ms: Some(1),
            ended_at_ms: Some(2),
            status: EventStatus::Success,
            request_count: Some(3),
            total_tokens: Some(1000),
            estimated_cost: Some(0.01),
            child_count: 2,
        };
        let json = serde_json::to_string(&node).expect("serialize");
        assert!(json.contains("\"agentKey\""));
        assert!(json.contains("\"relationLevel\":\"fullTree\""));
        assert!(json.contains("\"childCount\":2"));
        let back: AgentNodeDto = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, node);
    }

    #[test]
    fn event_status_unknown_roundtrip() {
        let json = serde_json::to_string(&EventStatus::Unknown).expect("serialize");
        assert_eq!(json, "\"unknown\"");
        let back: EventStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, EventStatus::Unknown);
    }

    #[test]
    fn filter_and_page_serde_defaults() {
        let filter: SessionEventFilter = serde_json::from_str("{}").expect("empty filter");
        assert_eq!(filter.kinds, None);
        let page = EventsPage {
            items: vec![],
            total: 0,
            has_more: false,
        };
        let json = serde_json::to_string(&page).expect("serialize page");
        assert!(json.contains("\"hasMore\":false"));
    }

    #[test]
    fn summary_tool_counts_serialize_as_entries() {
        let summary = SessionActivitySummary {
            session_key: "s".to_string(),
            capability: SessionActivityCapability {
                level: ActivityCapabilityLevel::Structured,
                messages: true,
                tool_invocations: true,
                tool_results: true,
                request_links: false,
                agent_relations: AgentRelationLevel::RootGrouped,
                content_search: false,
                source_content_available: true,
                parser_id: "claude_code_v2".to_string(),
                parser_version: 2,
            },
            event_counts: HashMap::from([("userMessage".to_string(), 2)]),
            agent_counts: HashMap::new(),
            tool_counts: vec![("Read".to_string(), 3)],
            coverage: "structured".to_string(),
        };
        let json = serde_json::to_string(&summary).expect("serialize summary");
        assert!(json.contains("\"toolInvocations\":true"));
        assert!(json.contains("[\"Read\",3]"));
        let back: SessionActivitySummary =
            serde_json::from_str(&json).expect("deserialize summary");
        assert_eq!(back, summary);
    }
}
