//! Claude Code JSONL 深度事件适配器（M2）。
//!
//! 解析 `~/.claude/projects/**/*.jsonl`（含 subagents 子文件）为结构化事件
//! 批次（事实抽取，不触碰用量合并口径）。行结构与 `session::claude_reader`
//! 保持一致：
//!
//! - 外层 `type`：`user` / `assistant` 等；`timestamp` 为 Unix 秒（数字或
//!   RFC3339 字符串）；
//! - `message.content` 为块数组：user 消息含 `text` 与 `tool_result`
//!   （`tool_use_id` / `content` / `is_error`），assistant 消息含 `text` 与
//!   `tool_use`（`id` / `name` / `input`）；
//! - assistant 消息带 `message.id` 与 `message.usage` → 精确请求关联
//!   `claude_code:{message_id}`（与 scanner_sync 的 `{tool}:{message_id}`
//!   同格式），强度 [`RequestLinkStrength::Exact`]。
//!
//! 事件键 `claude:{root_session_id}:{message_id}:{block_idx}` 基于稳定
//! message id 与块索引，重复索引产出完全一致的批次；无 message id 的行
//! 回退到行号 `claude:{root_session_id}:line:{line_index}`。
//!
//! 子代理 transcript（路径含 `subagents` 段，识别逻辑同
//! `claude_reader::derive_root_session_id`）归并为 `subagent:{文件名}` 代理
//! 节点，父子树不可证明，能力只声明 [`AgentRelationLevel::RootGrouped`]，
//! 不绘制伪树。

use crate::activity::adapter::{
    ActivityError, ActivityIndexBatch, NewAgentNode, NewEventRequestLink, NewSessionEvent,
    NewToolInvocation, SessionActivityAdapter, SessionSourceRef,
};
use crate::activity::model::{
    ActivityCapabilityLevel, AgentRelationLevel, ContentState, EventStatus, RedactedPayloadPage,
    RequestLinkStrength, SafeSourceRef, SessionActivityCapability, SessionEventKind,
};
use crate::activity::redact::redact_text;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// 摘要长度上限（字符；契约要求 summary ≤ 500）。
const SUMMARY_MAX_CHARS: usize = 500;
/// payload 默认读取上限（字节）。
const DEFAULT_MAX_BYTES: usize = 262_144;
/// payload 单次读取上限（字节）。
const MAX_BYTES_CAP: usize = 1024 * 1024;

/// Claude Code 适配器。
///
/// 子代理关系只能证明到“文件级 subagents 归组”这一层，没有可证明的父子树
/// 字段，因此能力声明为 [`AgentRelationLevel::RootGrouped`]。
pub struct ClaudeAdapter;

impl SessionActivityAdapter for ClaudeAdapter {
    fn tool_name(&self) -> &'static str {
        crate::session::constants::TOOL_CLAUDE_CODE
    }

    fn capability(&self) -> SessionActivityCapability {
        SessionActivityCapability {
            level: ActivityCapabilityLevel::Structured,
            messages: true,
            tool_invocations: true,
            tool_results: true,
            request_links: true,
            agent_relations: AgentRelationLevel::RootGrouped,
            content_search: false,
            source_content_available: true,
            parser_id: "claude_code".to_string(),
            parser_version: 1,
        }
    }

    fn index_session(
        &self,
        source: &SessionSourceRef,
    ) -> Result<ActivityIndexBatch, ActivityError> {
        let file = fs::File::open(&source.primary_file_path).map_err(|_| {
            ActivityError::Io("unable to open claude transcript source file".to_string())
        })?;
        let reader = BufReader::new(file);

        let mut events: Vec<NewSessionEvent> = Vec::new();
        let mut agents: Vec<NewAgentNode> = Vec::new();
        let mut tool_invocations: Vec<NewToolInvocation> = Vec::new();
        let mut request_links: Vec<NewEventRequestLink> = Vec::new();

        let root_session_id = derive_root_session_id_from_path(&source.primary_file_path);
        let actor_agent_key = if is_subagent_path(&source.primary_file_path) {
            Some(format!(
                "subagent:{}",
                subagent_file_stem(&source.primary_file_path)
            ))
        } else {
            None
        };

        let mut sequence: i64 = 0;
        let mut line_index: i64 = 0;
        let mut bad_lines: u64 = 0;
        // tool_use.id -> 对应 ToolInvocation 事件 key（tool_result 关联用）。
        let mut tool_use_keys: HashMap<String, String> = HashMap::new();

        // 子代理节点元数据（首条 user 消息摘要、首/末行时间）。
        let mut first_user_summary: Option<String> = None;
        let mut first_ts_ms: Option<i64> = None;
        let mut last_ts_ms: Option<i64> = None;

        for line in reader.lines() {
            line_index += 1;
            let line = match line {
                Ok(text) => text,
                Err(_) => {
                    bad_lines += 1;
                    continue;
                }
            };
            let json: Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(_) => {
                    bad_lines += 1;
                    continue;
                }
            };

            let ts_ms = extract_timestamp(&json).map(|secs| secs.saturating_mul(1000));
            if ts_ms.is_some() {
                if first_ts_ms.is_none() {
                    first_ts_ms = ts_ms;
                }
                last_ts_ms = ts_ms;
            }

            let msg_type = json.get("type").and_then(Value::as_str).unwrap_or("");
            let Some(message) = json.get("message") else {
                continue;
            };
            let message_id = message
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string);
            let Some(content_arr) = message.get("content").and_then(Value::as_array) else {
                continue;
            };

            match msg_type {
                "user" | "human" => {
                    // text 块合并为一个 UserMessage；tool_result 每块一个事件。
                    let mut text_parts: Vec<String> = Vec::new();
                    let mut text_block_idx: Option<usize> = None;
                    for (block_idx, block) in content_arr.iter().enumerate() {
                        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
                        match block_type {
                            "text" => {
                                if text_block_idx.is_none() {
                                    text_block_idx = Some(block_idx);
                                }
                                if let Some(text) = block.get("text").and_then(Value::as_str) {
                                    text_parts.push(text.to_string());
                                }
                            }
                            "tool_result" => {
                                let tool_use_id = block
                                    .get("tool_use_id")
                                    .and_then(Value::as_str)
                                    .unwrap_or("");
                                let is_error = block
                                    .get("is_error")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false);
                                sequence += 1;
                                let event_key = event_key_for(
                                    &root_session_id,
                                    message_id.as_deref(),
                                    Some(block_idx),
                                    tool_use_id,
                                    line_index,
                                );
                                events.push(NewSessionEvent {
                                    event_key: event_key.clone(),
                                    sequence,
                                    timestamp_ms: ts_ms,
                                    kind: SessionEventKind::ToolResult,
                                    status: Some(if is_error {
                                        EventStatus::Error
                                    } else {
                                        EventStatus::Success
                                    }),
                                    actor_agent_key: actor_agent_key.clone(),
                                    parent_event_key: tool_use_keys.get(tool_use_id).cloned(),
                                    summary_redacted: None,
                                    content_state: ContentState::Available,
                                    source_file_path: source.primary_file_path.clone(),
                                    source_offset: Some(line_index),
                                    payload_hash: None,
                                    raw_event_kind: "user/tool_result".to_string(),
                                });
                            }
                            _ => {}
                        }
                    }
                    if !text_parts.is_empty() {
                        let merged = text_parts.join("\n");
                        let summary = truncate_chars(&redact_text(&merged), SUMMARY_MAX_CHARS);
                        if actor_agent_key.is_some() && first_user_summary.is_none() {
                            first_user_summary = Some(summary.clone());
                        }
                        sequence += 1;
                        events.push(NewSessionEvent {
                            event_key: event_key_for(
                                &root_session_id,
                                message_id.as_deref(),
                                text_block_idx,
                                "",
                                line_index,
                            ),
                            sequence,
                            timestamp_ms: ts_ms,
                            kind: SessionEventKind::UserMessage,
                            status: None,
                            actor_agent_key: actor_agent_key.clone(),
                            parent_event_key: None,
                            summary_redacted: Some(summary),
                            content_state: ContentState::Available,
                            source_file_path: source.primary_file_path.clone(),
                            source_offset: Some(line_index),
                            payload_hash: None,
                            raw_event_kind: "user/text".to_string(),
                        });
                    }
                }
                "assistant" => {
                    let has_usage = message.get("usage").is_some() || json.get("usage").is_some();
                    // 请求关联挂到本消息首个事件（按块序）。
                    let mut first_event_key: Option<String> = None;
                    for (block_idx, block) in content_arr.iter().enumerate() {
                        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
                        match block_type {
                            "text" => {
                                let Some(text) = block.get("text").and_then(Value::as_str) else {
                                    continue;
                                };
                                if text.trim().is_empty() {
                                    continue;
                                }
                                sequence += 1;
                                let event_key = event_key_for(
                                    &root_session_id,
                                    message_id.as_deref(),
                                    Some(block_idx),
                                    "",
                                    line_index,
                                );
                                if first_event_key.is_none() {
                                    first_event_key = Some(event_key.clone());
                                }
                                events.push(NewSessionEvent {
                                    event_key,
                                    sequence,
                                    timestamp_ms: ts_ms,
                                    kind: SessionEventKind::AssistantMessage,
                                    status: None,
                                    actor_agent_key: actor_agent_key.clone(),
                                    parent_event_key: None,
                                    summary_redacted: Some(truncate_chars(
                                        &redact_text(text),
                                        SUMMARY_MAX_CHARS,
                                    )),
                                    content_state: ContentState::Available,
                                    source_file_path: source.primary_file_path.clone(),
                                    source_offset: Some(line_index),
                                    payload_hash: None,
                                    raw_event_kind: "assistant/text".to_string(),
                                });
                            }
                            "tool_use" => {
                                let tool_use_id =
                                    block.get("id").and_then(Value::as_str).unwrap_or("");
                                let name = block
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .map(str::trim)
                                    .unwrap_or("");
                                if name.is_empty() {
                                    continue;
                                }
                                let input = block.get("input");
                                let input_keys = input
                                    .and_then(Value::as_object)
                                    .map(|map| map.keys().cloned().collect())
                                    .unwrap_or_default();
                                let input_bytes = input.map(|value| {
                                    serde_json::to_string(value)
                                        .map(|text| text.len() as i64)
                                        .unwrap_or(0)
                                });
                                sequence += 1;
                                let event_key = event_key_for(
                                    &root_session_id,
                                    message_id.as_deref(),
                                    Some(block_idx),
                                    tool_use_id,
                                    line_index,
                                );
                                if first_event_key.is_none() {
                                    first_event_key = Some(event_key.clone());
                                }
                                if !tool_use_id.is_empty() {
                                    tool_use_keys
                                        .insert(tool_use_id.to_string(), event_key.clone());
                                }
                                events.push(NewSessionEvent {
                                    event_key: event_key.clone(),
                                    sequence,
                                    timestamp_ms: ts_ms,
                                    kind: SessionEventKind::ToolInvocation,
                                    status: Some(EventStatus::Success),
                                    actor_agent_key: actor_agent_key.clone(),
                                    parent_event_key: None,
                                    summary_redacted: Some(truncate_chars(
                                        &redact_text(name),
                                        SUMMARY_MAX_CHARS,
                                    )),
                                    content_state: ContentState::Available,
                                    source_file_path: source.primary_file_path.clone(),
                                    source_offset: Some(line_index),
                                    payload_hash: None,
                                    raw_event_kind: "assistant/tool_use".to_string(),
                                });
                                tool_invocations.push(NewToolInvocation {
                                    invocation_key: if tool_use_id.is_empty() {
                                        format!("claude:{root_session_id}:line:{line_index}")
                                    } else {
                                        tool_use_id.to_string()
                                    },
                                    event_key,
                                    tool_name: normalize_tool_name(name),
                                    family: tool_family(name).to_string(),
                                    duration_ms: None,
                                    status: Some(EventStatus::Success),
                                    input_bytes,
                                    output_bytes: None,
                                    input_keys,
                                    result_kind: None,
                                });
                            }
                            _ => {}
                        }
                    }
                    if has_usage {
                        if let Some(message_id_str) = message_id.as_deref() {
                            if let Some(event_key) = first_event_key {
                                request_links.push(NewEventRequestLink {
                                    event_key,
                                    request_key: format!(
                                        "{}:{message_id_str}",
                                        crate::session::constants::TOOL_CLAUDE_CODE
                                    ),
                                    strength: RequestLinkStrength::Exact,
                                });
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        let _ = bad_lines;

        if let Some(agent_key) = actor_agent_key {
            agents.push(NewAgentNode {
                agent_key: agent_key.clone(),
                parent_agent_key: None,
                display_kind: Some("subagent".to_string()),
                task_summary_redacted: first_user_summary,
                started_at_ms: first_ts_ms,
                ended_at_ms: last_ts_ms,
                status: Some(EventStatus::Success),
                relation_level: AgentRelationLevel::RootGrouped,
            });
        }

        Ok(ActivityIndexBatch {
            session_key: source.session_id.clone(),
            events,
            agents,
            tool_invocations,
            request_links,
        })
    }

    fn read_payload(
        &self,
        source_ref: &SafeSourceRef,
        section: &str,
        max_bytes: usize,
    ) -> Result<RedactedPayloadPage, ActivityError> {
        // 0 表示调用方未指定，走默认上限。
        let max = if max_bytes == 0 {
            DEFAULT_MAX_BYTES
        } else {
            max_bytes.clamp(1, MAX_BYTES_CAP)
        };
        let offset = source_ref
            .source_offset
            .filter(|value| *value >= 1)
            .unwrap_or(1) as usize;

        let file = match fs::File::open(&source_ref.source_file_path) {
            Ok(file) => file,
            Err(_) => {
                return Ok(RedactedPayloadPage {
                    content: String::new(),
                    truncated: false,
                    next_cursor: None,
                    content_state: ContentState::Unavailable,
                });
            }
        };
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        for _ in 0..offset {
            line.clear();
            let read = reader
                .read_line(&mut line)
                .map_err(|_| ActivityError::Io("failed to read payload source line".to_string()))?;
            if read == 0 {
                return Ok(RedactedPayloadPage {
                    content: String::new(),
                    truncated: false,
                    next_cursor: None,
                    content_state: ContentState::Unavailable,
                });
            }
        }

        let json: Value = serde_json::from_str(&line)
            .map_err(|_| ActivityError::Parse("invalid json line for payload".to_string()))?;
        let text = if section == "raw" {
            line.trim_end().to_string()
        } else {
            extract_section_text(&json, section)?
        };
        let redacted = redact_text(&text);
        let (content, truncated) = truncate_bytes(&redacted, max);
        Ok(RedactedPayloadPage {
            content,
            truncated,
            next_cursor: None,
            content_state: ContentState::Available,
        })
    }
}

// ── 身份识别 ──────────────────────────────────────────────────────────────────

/// 从 transcript 路径推导根会话 id（与 `claude_reader::derive_root_session_id`
/// 同策略：`subagents` 段前一 component；否则文件 stem）。
fn derive_root_session_id_from_path(path: &str) -> String {
    let components: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|comp| !comp.is_empty())
        .collect();
    if let Some(subagent_index) = components.iter().position(|comp| *comp == "subagents") {
        if subagent_index >= 1 {
            return components[subagent_index - 1].to_string();
        }
    }
    Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// 是否为子代理 transcript：路径含 `subagents` 段（同
/// `claude_reader::derive_root_session_id` 的归并识别）。
fn is_subagent_path(path: &str) -> bool {
    let components: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|comp| !comp.is_empty())
        .collect();
    components
        .iter()
        .position(|comp| *comp == "subagents")
        .is_some_and(|index| index >= 1)
}

/// 子代理文件名（去扩展名），用作 `subagent:{stem}` 代理 key。
fn subagent_file_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// 事件键：`claude:{root}:{message_id}:{block_idx}`；tool_use 事件追加
/// tool_use.id 保唯一；无 message id 行回退 `claude:{root}:line:{line_index}`。
fn event_key_for(
    root_session_id: &str,
    message_id: Option<&str>,
    block_idx: Option<usize>,
    tool_use_id: &str,
    line_index: i64,
) -> String {
    let base = match message_id {
        Some(message_id) => {
            format!(
                "claude:{root_session_id}:{message_id}:{}",
                block_idx.unwrap_or(0)
            )
        }
        None => format!("claude:{root_session_id}:line:{line_index}"),
    };
    if tool_use_id.is_empty() {
        base
    } else {
        format!("{base}:{tool_use_id}")
    }
}

// ── 文本提取 ──────────────────────────────────────────────────────────────────

/// 按 payload 行结构与 section 提取文本（供 on-demand payload 读取）；
/// 未知 section 报 Unsupported。
fn extract_section_text(json: &Value, section: &str) -> Result<String, ActivityError> {
    let Some(content) = json
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    else {
        return Ok(String::new());
    };
    let mut out = String::new();
    let mut push = |piece: String| {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&piece);
    };
    match section {
        "summary" => {
            for block in content {
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        push(text.to_string());
                    }
                }
            }
        }
        "input" => {
            for block in content {
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    if let Some(input) = block.get("input") {
                        push(serde_json::to_string(input).unwrap_or_default());
                    }
                }
            }
        }
        "output" => {
            for block in content {
                if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                    if let Some(result_content) = block.get("content") {
                        push(tool_result_content_text(result_content));
                    }
                }
            }
        }
        _ => {
            return Err(ActivityError::Unsupported(format!(
                "unsupported payload section: {section}"
            )));
        }
    }
    Ok(out)
}

/// tool_result.content 文本化：字符串原样；text 块数组按序拼接。
fn tool_result_content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<&str>>()
            .join("\n"),
        _ => String::new(),
    }
}

// ── 工具名规范化与族映射 ──────────────────────────────────────────────────────

/// 规范化工具名：小写、非字母数字转下划线、去首尾下划线。
/// `Bash` → `bash`；`WebFetch` → `web_fetch`；`TodoWrite` → `todo_write`。
fn normalize_tool_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_lower_or_digit = false;
    let mut prev_underscore = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            // camelCase 拆分：大写字母前一个小写/数字时插入下划线（WebFetch → web_fetch）。
            if ch.is_ascii_uppercase() && prev_lower_or_digit && !prev_underscore {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            prev_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
            prev_underscore = false;
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
            prev_lower_or_digit = false;
        }
    }
    out.trim_matches('_').to_string()
}

/// 工具族映射（对齐设计 9.5）：Bash/terminal→shell、Read/Write/Edit→file、
/// Glob/Grep→search、WebFetch/WebSearch→web、Task→agent、其余 unknown。
/// 各关键词族互斥，顺序无关紧要。
fn tool_family(name: &str) -> &'static str {
    let normalized = name.to_ascii_lowercase();
    if normalized.contains("bash")
        || normalized.contains("terminal")
        || normalized.contains("shell")
        || normalized.starts_with("exec")
    {
        "shell"
    } else if normalized.starts_with("read")
        || normalized.starts_with("write")
        || normalized.starts_with("edit")
    {
        "file"
    } else if normalized.contains("glob") || normalized.contains("grep") {
        "search"
    } else if normalized.contains("web") {
        "web"
    } else if normalized.contains("task") {
        "agent"
    } else {
        "unknown"
    }
}

// ── 时间与截断 ────────────────────────────────────────────────────────────────

/// 行时间戳 → Unix 秒（与 `session::shared::extract_timestamp` 同语义；
/// 额外支持浮点秒，如 Claude JSONL 的 `1747000000.5`）。
fn extract_timestamp(json: &Value) -> Option<i64> {
    let ts = json
        .get("timestamp")
        .or_else(|| json.get("createdAt"))
        .or_else(|| json.get("created_at"))
        .or_else(|| json.get("time"))
        .or_else(|| json.get("date"))?;
    if let Some(num) = ts.as_u64() {
        return Some(if num > 10_000_000_000 {
            (num / 1000) as i64
        } else {
            num as i64
        });
    }
    if let Some(num) = ts.as_f64() {
        return Some(num as i64);
    }
    if let Some(text) = ts.as_str() {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(text) {
            return Some(dt.timestamp());
        }
    }
    None
}

/// 按字符截断（保证 ≤ max 字符；超长时不追加省略号，避免越界）。
fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// 按字节截断（安全处理 UTF-8 边界），返回 (内容, 是否截断)。
fn truncate_bytes(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

// ── 测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::constants::TOOL_CLAUDE_CODE;
    use serde_json::json;
    use std::io::Write;

    fn make_source(session_id: &str, path: &str) -> SessionSourceRef {
        SessionSourceRef {
            session_id: session_id.to_string(),
            tool: TOOL_CLAUDE_CODE.to_string(),
            primary_file_path: path.to_string(),
            source_file_id: None,
        }
    }

    fn write_fixture(lines: &[Value]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("claude-test.jsonl");
        let mut file = fs::File::create(&path).expect("create fixture");
        for line in lines {
            writeln!(file, "{line}").expect("write fixture line");
        }
        (dir, path.to_string_lossy().to_string())
    }

    /// 原始文本行 fixture（用于构造坏 JSON 行）。
    fn write_raw_fixture(lines: &[&str]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("claude-test.jsonl");
        let mut file = fs::File::create(&path).expect("create fixture");
        for line in lines {
            writeln!(file, "{line}").expect("write fixture line");
        }
        (dir, path.to_string_lossy().to_string())
    }

    /// 最小完整 fixture：user text + assistant text（带 usage）+ tool_use +
    /// tool_result（带 error 变体）。
    fn minimal_fixture() -> Vec<Value> {
        vec![
            json!({"timestamp": 1747000000.0, "type": "user", "message": {
                "role": "user", "id": "msg-user-1",
                "content": [{"type": "text", "text": "fix the login bug"}]
            }}),
            json!({"timestamp": 1747000001.5, "type": "assistant", "message": {
                "role": "assistant", "id": "msg-assistant-1",
                "content": [{"type": "text", "text": "let me look"}],
                "usage": {"input_tokens": 100, "output_tokens": 50}
            }}),
            json!({"timestamp": 1747000002.0, "type": "assistant", "message": {
                "role": "assistant", "id": "msg-assistant-2",
                "content": [{"type": "tool_use", "id": "toolu_01", "name": "Bash",
                    "input": {"command": "pwd", "cwd": "/tmp"}}]
            }}),
            json!({"timestamp": 1747000003.0, "type": "user", "message": {
                "role": "user", "id": "msg-user-2",
                "content": [{"type": "tool_result", "tool_use_id": "toolu_01",
                    "content": "/tmp\n", "is_error": false}]
            }}),
            json!({"timestamp": 1747000004.0, "type": "assistant", "message": {
                "role": "assistant", "id": "msg-assistant-3",
                "content": [{"type": "tool_use", "id": "toolu_02", "name": "Read",
                    "input": {"file_path": "/tmp/x.rs", "offset": 1}}]
            }}),
            json!({"timestamp": 1747000005.0, "type": "user", "message": {
                "role": "user", "id": "msg-user-3",
                "content": [{"type": "tool_result", "tool_use_id": "toolu_02",
                    "content": [{"type": "text", "text": "file body"}],
                    "is_error": true}]
            }}),
        ]
    }

    #[test]
    fn indexes_minimal_fixture_with_kinds_parent_and_request_links() {
        let (_dir, path) = write_fixture(&minimal_fixture());
        let adapter = ClaudeAdapter;
        let source = make_source("proj::sess-1", &path);
        let batch = adapter.index_session(&source).expect("index session");

        let kinds: Vec<SessionEventKind> = batch.events.iter().map(|event| event.kind).collect();
        assert_eq!(
            kinds,
            vec![
                SessionEventKind::UserMessage,
                SessionEventKind::AssistantMessage,
                SessionEventKind::ToolInvocation,
                SessionEventKind::ToolResult,
                SessionEventKind::ToolInvocation,
                SessionEventKind::ToolResult,
            ]
        );

        // 工具结果 parent 关联到对应 tool_use 事件。
        let tool_result = batch
            .events
            .iter()
            .find(|event| event.kind == SessionEventKind::ToolResult)
            .expect("tool result");
        let invocation = batch
            .events
            .iter()
            .find(|event| event.kind == SessionEventKind::ToolInvocation)
            .expect("invocation");
        assert_eq!(
            tool_result.parent_event_key.as_deref(),
            Some(invocation.event_key.as_str())
        );
        // 无匹配 tool_use 的结果 parent=None（这里都有匹配；构造独立用例见损坏行测试）。
        assert!(tool_result.parent_event_key.is_some());

        // tool_result is_error → Error 状态；is_error=false → Success。
        let results: Vec<&NewSessionEvent> = batch
            .events
            .iter()
            .filter(|event| event.kind == SessionEventKind::ToolResult)
            .collect();
        assert_eq!(results[0].status, Some(EventStatus::Success));
        assert_eq!(results[1].status, Some(EventStatus::Error));

        // 请求关联：assistant 消息带 usage → Exact，key 与 scanner 一致。
        assert_eq!(batch.request_links.len(), 1);
        assert_eq!(
            batch.request_links[0].request_key,
            format!("{TOOL_CLAUDE_CODE}:msg-assistant-1")
        );
        assert_eq!(batch.request_links[0].strength, RequestLinkStrength::Exact);
        // 挂在 assistant 消息事件上。
        let assistant_event = batch
            .events
            .iter()
            .find(|event| event.kind == SessionEventKind::AssistantMessage)
            .expect("assistant message");
        assert_eq!(batch.request_links[0].event_key, assistant_event.event_key);

        // 工具调用元数据：invocation_key=tool_use.id，input_keys 顶层 key。
        let inv = batch
            .tool_invocations
            .iter()
            .find(|inv| inv.invocation_key == "toolu_01")
            .expect("toolu_01 invocation");
        assert_eq!(inv.tool_name, "bash");
        assert_eq!(inv.family, "shell");
        assert_eq!(
            inv.input_keys,
            vec!["command".to_string(), "cwd".to_string()]
        );
        assert!(inv.input_bytes.is_some_and(|bytes| bytes > 0));
        assert_eq!(inv.status, Some(EventStatus::Success));

        // 事件键格式：claude:{root}:{message_id}:{block_idx}；root 由文件 stem
        // 推导（fixture 文件名为 claude-test.jsonl）。
        assert!(assistant_event
            .event_key
            .starts_with("claude:claude-test:msg-assistant-1:0"));
        assert!(invocation.event_key.contains(":toolu_01"));
        // 无父代理。
        assert!(batch.agents.is_empty());
    }

    #[test]
    fn event_keys_stable_across_two_index_runs() {
        let (_dir, path) = write_fixture(&minimal_fixture());
        let adapter = ClaudeAdapter;
        let source = make_source("proj::sess-1", &path);
        let first = adapter.index_session(&source).expect("first index");
        let second = adapter.index_session(&source).expect("second index");
        assert_eq!(first.events, second.events);
        assert_eq!(first.tool_invocations, second.tool_invocations);
        assert_eq!(first.request_links, second.request_links);
        assert_eq!(first.agents, second.agents);
    }

    #[test]
    fn summary_is_redacted_and_bounded() {
        let fixture = vec![
            json!({"timestamp": 1747000000.0, "type": "user", "message": {
                "role": "user", "id": "msg-1",
                "content": [{"type": "text", "text": "rotate the key sk-abcdefgh12345678 please"}]
            }}),
            json!({"timestamp": 1747000001.0, "type": "assistant", "message": {
                "role": "assistant", "id": "msg-2",
                "content": [{"type": "text", "text": "ok, done"}]
            }}),
        ];
        let (_dir, path) = write_fixture(&fixture);
        let adapter = ClaudeAdapter;
        let batch = adapter
            .index_session(&make_source("proj::sess-2", &path))
            .expect("index session");

        let user_summary = batch.events[0]
            .summary_redacted
            .as_deref()
            .expect("user summary");
        assert!(!user_summary.contains("abcdefgh12345678"), "secret leaked");
        assert!(user_summary.contains("sk-[redacted]"));
        assert!(user_summary.len() <= SUMMARY_MAX_CHARS);
        assert!(user_summary.contains("rotate the key"));

        // 超长消息截断 ≤ 500 字符。
        let long_text = "x".repeat(1200);
        let long_fixture = vec![
            json!({"timestamp": 1747000000.0, "type": "user", "message": {
                "role": "user", "id": "msg-3",
                "content": [{"type": "text", "text": long_text}]
            }}),
        ];
        let (_dir2, path2) = write_fixture(&long_fixture);
        let batch2 = adapter
            .index_session(&make_source("proj::sess-3", &path2))
            .expect("index");
        assert_eq!(batch2.events.len(), 1);
        let summary = batch2.events[0]
            .summary_redacted
            .as_deref()
            .expect("summary");
        assert_eq!(summary.chars().count(), SUMMARY_MAX_CHARS);
    }

    #[test]
    fn tolerates_corrupt_lines_without_panicking() {
        let lines = [
            r#"{"timestamp":1747000000.0,"type":"user","message":{"role":"user","id":"msg-1","content":[{"type":"text","text":"hello"}]}}"#,
            "this is not json {{",
            r#"{"timestamp":1747000001.0,"type":"assistant","message":{"role":"assistant","id":"msg-2","content":[{"type":"tool_use","id":"toolu_x","name":"Bash","input":{"command":"ls"}}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":""#,
            r#"{"timestamp":1747000002.0,"type":"user","message":{"role":"user","id":"msg-3","content":[{"type":"tool_result","tool_use_id":"toolu_x","content":"out","is_error":false}]}}"#,
            "trailing garbage",
        ];
        let (_dir, path) = write_raw_fixture(&lines);

        let adapter = ClaudeAdapter;
        let batch = adapter
            .index_session(&make_source("proj::sess-4", &path))
            .expect("index with corrupt lines");
        let kinds: Vec<SessionEventKind> = batch.events.iter().map(|event| event.kind).collect();
        assert_eq!(
            kinds,
            vec![
                SessionEventKind::UserMessage,
                SessionEventKind::ToolInvocation,
                SessionEventKind::ToolResult,
            ]
        );
        // 坏行两侧的 tool_use/tool_result 关联仍然成立。
        assert_eq!(
            batch.events[2].parent_event_key.as_deref(),
            Some(batch.events[1].event_key.as_str())
        );
        // 行号回退 key 唯一（无 message.id 的坏行不产出事件，不受影响）。
        assert_eq!(
            batch.events[1].event_key,
            "claude:claude-test:msg-2:0:toolu_x"
        );
    }

    #[test]
    fn maps_multiple_tool_families() {
        let fixture = vec![
            json!({"timestamp": 1747000000.0, "type": "assistant", "message": {
                "role": "assistant", "id": "msg-1",
                "content": [
                    {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}},
                    {"type": "tool_use", "id": "t2", "name": "Read", "input": {"file_path": "a"}},
                    {"type": "tool_use", "id": "t3", "name": "Write", "input": {"file_path": "b"}},
                    {"type": "tool_use", "id": "t4", "name": "Glob", "input": {"pattern": "*.rs"}},
                    {"type": "tool_use", "id": "t5", "name": "Grep", "input": {"pattern": "x"}},
                    {"type": "tool_use", "id": "t6", "name": "WebFetch", "input": {"url": "https://x.example"}},
                    {"type": "tool_use", "id": "t7", "name": "WebSearch", "input": {"query": "rust"}},
                    {"type": "tool_use", "id": "t8", "name": "Task", "input": {"description": "review"}},
                    {"type": "tool_use", "id": "t9", "name": "NotebookEdit", "input": {"notebook_path": "n.ipynb"}}
                ]
            }}),
        ];
        let (_dir, path) = write_fixture(&fixture);
        let adapter = ClaudeAdapter;
        let batch = adapter
            .index_session(&make_source("proj::sess-5", &path))
            .expect("index");

        let by_name: HashMap<&str, &str> = batch
            .tool_invocations
            .iter()
            .map(|inv| (inv.tool_name.as_str(), inv.family.as_str()))
            .collect();
        assert_eq!(by_name.get("bash"), Some(&"shell"));
        assert_eq!(by_name.get("read"), Some(&"file"));
        assert_eq!(by_name.get("write"), Some(&"file"));
        assert_eq!(by_name.get("glob"), Some(&"search"));
        assert_eq!(by_name.get("grep"), Some(&"search"));
        assert_eq!(by_name.get("web_fetch"), Some(&"web"));
        assert_eq!(by_name.get("web_search"), Some(&"web"));
        assert_eq!(by_name.get("task"), Some(&"agent"));
        assert_eq!(by_name.get("notebook_edit"), Some(&"unknown"));

        // 规范化名称：小写 snake_case（WebFetch → web_fetch）。
        let families: std::collections::HashSet<&str> = batch
            .tool_invocations
            .iter()
            .map(|inv| inv.family.as_str())
            .collect();
        assert!(families.contains("shell") && families.contains("file"));
    }

    #[test]
    fn marks_subagent_file_with_actor_agent_key() {
        let fixture = vec![
            json!({"timestamp": 1747000000.0, "type": "user", "message": {
                "role": "user", "id": "msg-1",
                "content": [{"type": "text", "text": "review this diff"}]
            }}),
            json!({"timestamp": 1747000001.0, "type": "assistant", "message": {
                "role": "assistant", "id": "msg-2",
                "content": [{"type": "tool_use", "id": "toolu_s1", "name": "Bash",
                    "input": {"command": "git diff"}}]
            }}),
        ];
        let (dir, path) = write_fixture(&fixture);
        // 构造子代理路径：subagents 段 + 文件名 stem 为代理 key。
        let subagent_path = {
            let dir_path = dir.path().join("proj").join("sess-root").join("subagents");
            let sub_path = dir_path.join("sub-1.jsonl");
            fs::create_dir_all(&dir_path).expect("create subagents dir");
            fs::copy(&path, &sub_path).expect("copy fixture");
            sub_path.to_string_lossy().to_string()
        };

        let adapter = ClaudeAdapter;
        let batch = adapter
            .index_session(&make_source("proj::sess-root::sub-1", &subagent_path))
            .expect("index");

        assert_eq!(batch.agents.len(), 1);
        let agent = &batch.agents[0];
        assert_eq!(agent.agent_key, "subagent:sub-1".to_string());
        assert_eq!(agent.relation_level, AgentRelationLevel::RootGrouped);
        assert_eq!(agent.display_kind.as_deref(), Some("subagent"));
        assert_eq!(agent.status, Some(EventStatus::Success));
        assert_eq!(
            agent.task_summary_redacted.as_deref(),
            Some("review this diff")
        );
        assert_eq!(agent.started_at_ms, Some(1747000000000));
        assert_eq!(agent.ended_at_ms, Some(1747000001000));

        // 全部事件归到该代理；事件键用根会话 id（subagents 前一 component）。
        assert!(batch.events.iter().all(|event| {
            event.actor_agent_key.as_deref() == Some("subagent:sub-1")
                && event.event_key.starts_with("claude:sess-root:")
        }));
    }

    #[test]
    fn missing_file_index_returns_io_error_without_path() {
        let adapter = ClaudeAdapter;
        let source = make_source("proj::missing", "/nonexistent/claude-x.jsonl");
        let err = adapter.index_session(&source).expect_err("io error");
        let message = err.to_string();
        assert!(message.starts_with("ERR_ACTIVITY_IO"));
        assert!(!message.contains("nonexistent"), "error must not leak path");
    }

    #[test]
    fn reads_redacted_payload_by_section() {
        let (_dir, path) = write_fixture(&minimal_fixture());
        let adapter = ClaudeAdapter;

        // tool_use 行（第 3 行）：input section → input JSON 序列化。
        let call_ref = SafeSourceRef {
            source_file_id: None,
            source_file_path: path.clone(),
            source_offset: Some(3),
            fingerprint: None,
        };
        let page = adapter
            .read_payload(&call_ref, "input", DEFAULT_MAX_BYTES)
            .expect("read payload");
        assert_eq!(page.content_state, ContentState::Available);
        assert!(!page.truncated);
        assert!(page.content.contains("\"command\":\"pwd\""));

        // 小 max_bytes 截断（按字节安全截断）。
        let page = adapter
            .read_payload(&call_ref, "input", 8)
            .expect("read payload truncated");
        assert!(page.truncated);
        assert!(page.content.len() <= 8);

        // tool_result 行（第 4 行）：output section → content 文本。
        let output_ref = SafeSourceRef {
            source_file_id: None,
            source_file_path: path.clone(),
            source_offset: Some(4),
            fingerprint: None,
        };
        let page = adapter
            .read_payload(&output_ref, "output", DEFAULT_MAX_BYTES)
            .expect("read output");
        assert_eq!(page.content, "/tmp\n");

        // summary section：text 块内容。
        let summary_ref = SafeSourceRef {
            source_file_id: None,
            source_file_path: path.clone(),
            source_offset: Some(1),
            fingerprint: None,
        };
        let page = adapter
            .read_payload(&summary_ref, "summary", DEFAULT_MAX_BYTES)
            .expect("read summary");
        assert_eq!(page.content, "fix the login bug");

        // raw section 返回整行。
        let raw_ref = SafeSourceRef {
            source_file_id: None,
            source_file_path: path,
            source_offset: Some(2),
            fingerprint: None,
        };
        let page = adapter
            .read_payload(&raw_ref, "raw", DEFAULT_MAX_BYTES)
            .expect("read raw");
        assert!(page.content.contains("msg-assistant-1"));

        // 未知 section → Unsupported 错误。
        let err = adapter
            .read_payload(&raw_ref, "bogus", DEFAULT_MAX_BYTES)
            .expect_err("unsupported section");
        assert!(err.to_string().starts_with("ERR_ACTIVITY_UNSUPPORTED"));
    }

    #[test]
    fn payload_redacts_secrets_and_unavailable_when_file_missing() {
        let fixture = vec![
            json!({"timestamp": 1747000000.0, "type": "user", "message": {
                "role": "user", "id": "msg-1",
                "content": [{"type": "text", "text": "key is sk-abcdefgh12345678"}]
            }}),
        ];
        let (_dir, path) = write_fixture(&fixture);

        let adapter = ClaudeAdapter;
        let source_ref = SafeSourceRef {
            source_file_id: None,
            source_file_path: path,
            source_offset: Some(1),
            fingerprint: None,
        };
        let page = adapter
            .read_payload(&source_ref, "summary", DEFAULT_MAX_BYTES)
            .expect("read payload");
        assert_eq!(page.content_state, ContentState::Available);
        assert!(!page.content.contains("abcdefgh12345678"));
        assert!(page.content.contains("sk-[redacted]"));

        // 文件不可读 → Unavailable + 空内容（不是错误）。
        let missing_ref = SafeSourceRef {
            source_file_id: None,
            source_file_path: "/nonexistent/claude-y.jsonl".to_string(),
            source_offset: Some(1),
            fingerprint: None,
        };
        let page = adapter
            .read_payload(&missing_ref, "input", DEFAULT_MAX_BYTES)
            .expect("unavailable page is not an error");
        assert_eq!(page.content_state, ContentState::Unavailable);
        assert!(page.content.is_empty());
    }
}
