//! 会话活动适配器接口（与 `doc/桌面主应用窗口详细设计.md` 12.7 节对齐）。
//!
//! 每个来源工具一个独立适配器，注册到 [`crate::activity::registry`]。
//! 适配器负责“事实抽取”，不负责 UI 文案；能力差异用
//! [`SessionActivityCapability`] 显式表达，不允许用空值伪装同等支持。

use crate::activity::model::{
    AgentRelationLevel, ContentState, EventStatus, RedactedPayloadPage, RequestLinkStrength,
    SafeSourceRef, SessionActivityCapability, SessionEventKind,
};
use std::io::{BufRead, BufReader};
use std::path::Path;

/// payload 源文件大小上限（字节；21.5 安全验收：超过不读取，直接 Unavailable）。
pub(crate) const PAYLOAD_MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// Individual JSONL lines are bounded separately so a malformed one cannot
/// allocate the entire file while locating a payload.
pub(crate) const PAYLOAD_MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// 解析分页 cursor：`"B{n}"` → 行内脱敏文本字节偏移；`None` 表示从头读。
/// 非法格式（非 `B` 前缀、空、负数、非数字）返回 `None`，调用方按
/// `Unavailable` 处理（不 panic、不读取）。
pub(crate) fn parse_payload_cursor(cursor: Option<&str>) -> Option<usize> {
    let text = cursor?;
    let digits = text.strip_prefix('B')?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<usize>().ok()
}

/// payload 路径安全校验（21.5：payload reader 不能读取任意文件）：
/// 扩展名必须为 `.jsonl`（大小写不敏感），且文件大小 ≤ 512MB。任一不满足
/// 返回 `Err`（调用方按 `content_state=Unavailable` 响应，不读取内容）。
pub(crate) fn validate_payload_path(path: &str) -> Result<(), ()> {
    let ext_ok = Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"));
    if !ext_ok {
        return Err(());
    }
    let size_ok = std::fs::metadata(path)
        .map(|meta| meta.len() <= PAYLOAD_MAX_FILE_BYTES)
        .unwrap_or(false);
    if !size_ok {
        return Err(());
    }
    Ok(())
}

/// Lightweight source fingerprint shared by indexing and on-demand reads.
/// It intentionally uses metadata only; the activity index stores this value
/// per event so a replaced source file cannot be read under an old event key.
pub(crate) fn payload_source_fingerprint(path: &str) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| (duration.as_secs(), duration.subsec_nanos()))
        .unwrap_or((0, 0));
    Some(format!("{}.{:09}:{}", mtime.0, mtime.1, metadata.len()))
}

pub(crate) fn validate_payload_fingerprint(path: &str, expected: Option<&str>) -> Result<(), ()> {
    validate_payload_path(path)?;
    if let Some(expected) = expected {
        if payload_source_fingerprint(path).as_deref() != Some(expected) {
            return Err(());
        }
    }
    Ok(())
}

/// Enforce that an IPC payload path belongs to a supported local session root.
/// Canonicalization also rejects symlinks escaping those roots.
pub(crate) fn validate_payload_source_path(path: &str) -> Result<(), ()> {
    validate_payload_path(path)?;
    let canonical = std::fs::canonicalize(path).map_err(|_| ())?;
    let Some(home) = dirs::home_dir() else {
        return Err(());
    };
    let roots = [
        home.join(".claude").join("projects"),
        home.join(".config").join("claude").join("projects"),
        home.join(".codex").join("sessions"),
    ];
    let native_match = roots.iter().any(|root| {
        std::fs::canonicalize(root)
            .map(|root| canonical.starts_with(root))
            .unwrap_or(false)
    });
    #[cfg(windows)]
    let wsl_match = crate::session::wsl::scan_config_if_enabled()
        .map(|cfg| {
            crate::session::wsl::claude_projects_roots(&cfg)
                .into_iter()
                .chain(crate::session::wsl::codex_session_roots(&cfg))
                .any(|root| {
                    std::fs::canonicalize(root)
                        .map(|root| canonical.starts_with(root))
                        .unwrap_or(false)
                })
        })
        .unwrap_or(false);
    #[cfg(not(windows))]
    let wsl_match = false;
    (native_match || wsl_match).then_some(()).ok_or(())
}

/// 打开源文件并定位到第 `line_no` 行（1-based）。文件不存在或行号超过
/// 文件长度返回 `Ok(None)`（Unavailable 语义）；IO 失败返回 `Err`。
pub(crate) fn read_payload_line(
    path: &str,
    line_no: usize,
) -> Result<Option<String>, ActivityError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    for line_index in 0..line_no {
        line.clear();
        loop {
            let (consumed, finished) = {
                let buffer = reader.fill_buf().map_err(|_| {
                    ActivityError::Io("failed to read payload source line".to_string())
                })?;
                if buffer.is_empty() {
                    return if !line.is_empty() && line_index + 1 == line_no {
                        Ok(Some(line))
                    } else {
                        Ok(None)
                    };
                }
                let take = buffer
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map(|index| index + 1)
                    .unwrap_or(buffer.len());
                if line.len().saturating_add(take) > PAYLOAD_MAX_LINE_BYTES {
                    return Err(ActivityError::Io(
                        "payload source line exceeds size limit".to_string(),
                    ));
                }
                line.push_str(std::str::from_utf8(&buffer[..take]).map_err(|_| {
                    ActivityError::Io("payload source line is not utf8".to_string())
                })?);
                (take, buffer[..take].contains(&b'\n'))
            };
            reader.consume(consumed);
            if finished {
                break;
            }
        }
    }
    Ok(Some(line))
}

/// 从脱敏文本的字节偏移 `from` 起截取最多 `max` 字节（安全处理 UTF-8 边界）。
/// 返回 `(内容, 是否截断, 下一页 cursor)`；剩余内容时
/// `next_cursor = Some("B{新偏移}")`，读完为 `None`。
pub(crate) fn slice_payload_page(
    redacted: &str,
    from: usize,
    max: usize,
) -> (String, bool, Option<String>) {
    // cursor 偏移必须是 UTF-8 字符边界：非边界（多字节字符中间）切片会
    // panic，视为非法 cursor 返回空页（调用方转 Unavailable，不 panic）。
    if from >= redacted.len() || !redacted.is_char_boundary(from) {
        return (String::new(), false, None);
    }
    let rest = &redacted[from..];
    if rest.len() <= max {
        return (rest.to_string(), false, None);
    }
    let mut end = max;
    while end > 0 && !redacted.is_char_boundary(from + end) {
        end -= 1;
    }
    (
        redacted[from..from + end].to_string(),
        true,
        Some(format!("B{}", from + end)),
    )
}

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
    ///
    /// `cursor`：上一页返回的 `next_cursor`（`"B{n}"` = 行内脱敏文本字节
    /// 偏移），`None` 表示从头读取；每页最多返回 `max_bytes` 字节，还有
    /// 剩余时 `next_cursor = Some("B{新偏移}")`，读完为 `None`。
    fn read_payload(
        &self,
        source_ref: &SafeSourceRef,
        section: &str,
        max_bytes: usize,
        cursor: Option<String>,
    ) -> Result<RedactedPayloadPage, ActivityError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_respects_utf8_char_boundaries() {
        // 中文摘要：字节偏移 1 落在多字节字符中间，必须安全返回空页（不 panic）。
        let text = "中文摘要内容";
        let (content, truncated, next) = slice_payload_page(text, 1, 64);
        assert_eq!(content, "");
        assert!(!truncated);
        assert!(next.is_none());

        // 合法边界偏移正常分页且 next_cursor 指向下一字符边界。
        let (first, truncated, next) = slice_payload_page(text, 0, 3);
        assert!(truncated);
        let next = next.expect("next cursor");
        let offset: usize = next.strip_prefix('B').unwrap().parse().unwrap();
        assert!(text.is_char_boundary(offset));
        let (second, _, _) = slice_payload_page(text, offset, 64);
        assert_eq!(format!("{first}{second}"), text);
    }

    #[test]
    fn cursor_parsing_accepts_only_b_prefix_digits() {
        assert_eq!(parse_payload_cursor(None), None);
        assert_eq!(parse_payload_cursor(Some("B0")), Some(0));
        assert_eq!(parse_payload_cursor(Some("B1024")), Some(1024));
        assert_eq!(parse_payload_cursor(Some("b1")), None);
        assert_eq!(parse_payload_cursor(Some("B-1")), None);
        assert_eq!(parse_payload_cursor(Some("B")), None);
        assert_eq!(parse_payload_cursor(Some("1")), None);
        assert_eq!(parse_payload_cursor(Some("B1x")), None);
    }

    #[test]
    fn payload_fingerprint_rejects_replaced_source() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout.jsonl");
        std::fs::write(&path, b"{}\n").expect("write fixture");
        assert!(
            validate_payload_fingerprint(&path.to_string_lossy(), Some("stale-fingerprint"))
                .is_err()
        );
        assert!(validate_payload_fingerprint(&path.to_string_lossy(), None).is_ok());
    }

    #[test]
    fn payload_path_requires_jsonl_and_size_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let txt = dir.path().join("payload.txt");
        std::fs::write(&txt, b"{}").expect("write fixture");
        assert!(validate_payload_path(&txt.to_string_lossy()).is_err());
    }
}
