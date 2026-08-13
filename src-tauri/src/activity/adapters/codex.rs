//! Codex CLI rollout JSONL 深度事件适配器（M2）。
//!
//! 解析 `~/.codex/sessions/**/rollout-*.jsonl` 为结构化事件批次（事实抽取，
//! 不触碰用量合并口径）。行结构与 `session::codex_reader` 保持一致：
//!
//! - 外层 `type`：`session_meta` / `event_msg` / `response_item` / `turn_context` 等；
//! - `event_msg` 内层 `payload.type`：`user_message`（用户输入，文本在
//!   `payload.message`）、`token_count`（用量，`payload.info` 含
//!   `total_token_usage` / `last_token_usage`）及其它事件；
//! - `response_item` 内层 `payload.type`：`message`（`role=assistant` 为模型回复）、
//!   `function_call`（`name`/`arguments`/`call_id`）、`function_call_output`
//!   （`call_id`/`output`），以及同构的 `custom_tool_call` / `custom_tool_call_output`。
//!
//! 事件键 `codex:{session_id}:{line_index}` 基于行号，稳定可重放（重复索引
//! 产出完全一致的批次）；token_count 差分语义与 codex_reader 同源（含 fork
//! replay 基线推进）；请求关联使用合成 id，强度只声明 [`RequestLinkStrength::TimeWindow`]，
//! 不伪装精确匹配。

use crate::activity::adapter::{
    parse_payload_cursor, read_payload_line, slice_payload_page, validate_payload_path,
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

/// 摘要长度上限（字符；设计 12.3 要求 summary 为短文本）。
const SUMMARY_MAX_CHARS: usize = 500;
/// payload 默认读取上限（字节）。
const DEFAULT_MAX_BYTES: usize = 262_144;
/// payload 单次读取上限（字节）。
const MAX_BYTES_CAP: usize = 1024 * 1024;
/// `session_meta` 所在行范围（与 codex_reader 一致：前 20 行）。
const META_SCAN_LINES: usize = 20;

/// Codex 适配器。
///
/// 子代理关系只能证明到“文件级 is_subagent 标记”这一层（`session_meta.source`
/// 含 `subagent` 键），没有可证明的父子树字段，因此能力声明为
/// [`AgentRelationLevel::FlagOnly`]，不绘制伪树。
pub struct CodexAdapter;

/// rollout 文件身份（取自首个 `session_meta` 行，语义同 codex_reader）。
#[derive(Default)]
struct RolloutIdentity {
    /// `session_meta.payload.source` 为对象且含 `subagent` 键。
    is_subagent: bool,
    /// 子代理线程标记：优先 `source.subagent.thread_spawn.parent_thread_id`，
    /// 缺失时退回 session id 自身（与 codex_reader 的 root_session_id 同策略）。
    thread_id: String,
    /// fork 起点时间戳（秒）；`forked_from_id` 存在时取外层 timestamp。
    fork_start_ts: Option<i64>,
    /// 会话标题（agent 节点任务摘要用，可选）。
    title: Option<String>,
}

/// token 累计值（与 codex_reader 的 CodexCumulativeTokens 同构）。
#[derive(Clone, Debug, Default)]
struct CumulativeTokens {
    input: u64,
    output: u64,
    cache_create: u64,
    cache_read: u64,
}

impl SessionActivityAdapter for CodexAdapter {
    fn tool_name(&self) -> &'static str {
        crate::session::constants::TOOL_CODEX
    }

    fn capability(&self) -> SessionActivityCapability {
        SessionActivityCapability {
            level: ActivityCapabilityLevel::Structured,
            messages: true,
            tool_invocations: true,
            tool_results: true,
            request_links: true,
            agent_relations: AgentRelationLevel::FlagOnly,
            content_search: false,
            source_content_available: true,
            parser_id: "codex".to_string(),
            parser_version: 1,
        }
    }

    fn index_session(
        &self,
        source: &SessionSourceRef,
    ) -> Result<ActivityIndexBatch, ActivityError> {
        let file = fs::File::open(&source.primary_file_path).map_err(|_| {
            ActivityError::Io("unable to open codex rollout source file".to_string())
        })?;
        let identity = inspect_rollout_identity(&source.primary_file_path);
        let reader = BufReader::new(file);

        let mut events: Vec<NewSessionEvent> = Vec::new();
        let mut agents: Vec<NewAgentNode> = Vec::new();
        let mut tool_invocations: Vec<NewToolInvocation> = Vec::new();
        let mut request_links: Vec<NewEventRequestLink> = Vec::new();

        let mut sequence: i64 = 0;
        let mut request_index: u64 = 0;
        let mut prev_total: Option<CumulativeTokens> = None;
        // call_id -> (调用事件 event_key, 在 tool_invocations 中的下标)
        let mut calls: HashMap<String, (String, usize)> = HashMap::new();
        let mut bad_lines: u64 = 0;
        let mut line_index: u64 = 0;

        let actor_agent_key = if identity.is_subagent {
            let agent_key = format!("{}:subagent:{}", source.session_id, identity.thread_id);
            agents.push(NewAgentNode {
                agent_key: agent_key.clone(),
                parent_agent_key: None,
                display_kind: Some("subagent".to_string()),
                task_summary_redacted: identity
                    .title
                    .as_deref()
                    .map(redact_text)
                    .map(|title| truncate_chars(&title, SUMMARY_MAX_CHARS)),
                started_at_ms: None,
                ended_at_ms: None,
                status: None,
                relation_level: AgentRelationLevel::FlagOnly,
            });
            Some(agent_key)
        } else {
            None
        };

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
            let ts = extract_timestamp(&json);
            let ts_ms = ts.map(|secs| secs.saturating_mul(1000));
            let event_type = json.get("type").and_then(Value::as_str).unwrap_or("");

            match event_type {
                "event_msg" => {
                    let Some(payload) = json.get("payload") else {
                        continue;
                    };
                    let inner = payload.get("type").and_then(Value::as_str).unwrap_or("");
                    match inner {
                        "user_message" => {
                            if is_fork_replay(&identity, ts) {
                                continue;
                            }
                            let text = payload.get("message").and_then(Value::as_str).unwrap_or("");
                            if text.trim().is_empty() || is_codex_system_message(text) {
                                continue;
                            }
                            sequence += 1;
                            events.push(NewSessionEvent {
                                event_key: codex_event_key(&source.session_id, line_index),
                                sequence,
                                timestamp_ms: ts_ms,
                                kind: SessionEventKind::UserMessage,
                                status: None,
                                actor_agent_key: actor_agent_key.clone(),
                                parent_event_key: None,
                                summary_redacted: Some(truncate_chars(
                                    &redact_text(text),
                                    SUMMARY_MAX_CHARS,
                                )),
                                content_state: ContentState::Available,
                                source_file_path: source.primary_file_path.clone(),
                                source_offset: Some(line_index as i64),
                                payload_hash: None,
                                raw_event_kind: "event_msg/user_message".to_string(),
                            });
                        }
                        "token_count" => {
                            let Some(info) = payload.get("info") else {
                                continue;
                            };
                            if info.is_null() {
                                continue;
                            }
                            let total = info.get("total_token_usage").and_then(parse_cumulative);
                            let last = info.get("last_token_usage").and_then(parse_cumulative);
                            // Fork replay：只推进累计基线，不产出事件（与
                            // codex_reader 406-413 语义一致，避免重复事件）。
                            if is_fork_replay(&identity, ts) {
                                if let Some(current) = total {
                                    prev_total = Some(current);
                                }
                                continue;
                            }
                            let delta = if let Some(current_total) = total.clone() {
                                let computed = if let Some(previous) = prev_total.as_ref() {
                                    if tokens_rolled_back(previous, &current_total) {
                                        last.clone().unwrap_or(current_total.clone())
                                    } else {
                                        tokens_delta(Some(previous), &current_total)
                                    }
                                } else {
                                    last.clone().unwrap_or(current_total.clone())
                                };
                                prev_total = Some(current_total);
                                Some(computed)
                            } else {
                                last.clone()
                            };
                            let Some(delta) = delta else {
                                continue;
                            };
                            let normalized = normalize_delta(delta);
                            let total_tokens = normalized.input
                                + normalized.output
                                + normalized.cache_read
                                + normalized.cache_create;
                            if total_tokens == 0 {
                                continue;
                            }

                            request_index += 1;
                            sequence += 1;
                            let event_key = codex_event_key(&source.session_id, line_index);
                            events.push(NewSessionEvent {
                                event_key: event_key.clone(),
                                sequence,
                                timestamp_ms: ts_ms,
                                kind: SessionEventKind::SystemEvent,
                                status: None,
                                actor_agent_key: actor_agent_key.clone(),
                                parent_event_key: None,
                                summary_redacted: None,
                                content_state: ContentState::Available,
                                source_file_path: source.primary_file_path.clone(),
                                source_offset: Some(line_index as i64),
                                payload_hash: None,
                                raw_event_kind: "event_msg/token_count".to_string(),
                            });
                            // 合成 request id（与 codex_reader 450 行同款格式）；
                            // 非精确 message id，只声明 TimeWindow。
                            request_links.push(NewEventRequestLink {
                                event_key,
                                request_key: format!(
                                    "codex:{}:{}",
                                    source.session_id, request_index
                                ),
                                strength: RequestLinkStrength::TimeWindow,
                            });
                        }
                        _ => {}
                    }
                }
                "response_item" => {
                    if is_fork_replay(&identity, ts) {
                        continue;
                    }
                    let Some(payload) = json.get("payload") else {
                        continue;
                    };
                    let inner = payload.get("type").and_then(Value::as_str).unwrap_or("");
                    match inner {
                        "message" => {
                            let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
                            // 用户消息以 event_msg/user_message 为权威来源，
                            // 避免与 response_item 重复表示；system 消息跳过。
                            if role != "assistant" {
                                continue;
                            }
                            let Some(text) = extract_response_item_text(payload) else {
                                continue;
                            };
                            if text.trim().is_empty() || is_codex_system_message(&text) {
                                continue;
                            }
                            sequence += 1;
                            events.push(NewSessionEvent {
                                event_key: codex_event_key(&source.session_id, line_index),
                                sequence,
                                timestamp_ms: ts_ms,
                                kind: SessionEventKind::AssistantMessage,
                                status: None,
                                actor_agent_key: actor_agent_key.clone(),
                                parent_event_key: None,
                                summary_redacted: Some(truncate_chars(
                                    &redact_text(&text),
                                    SUMMARY_MAX_CHARS,
                                )),
                                content_state: ContentState::Available,
                                source_file_path: source.primary_file_path.clone(),
                                source_offset: Some(line_index as i64),
                                payload_hash: None,
                                raw_event_kind: "response_item/message".to_string(),
                            });
                        }
                        "function_call" | "custom_tool_call" => {
                            let is_custom = inner == "custom_tool_call";
                            let Some(name) = payload
                                .get("name")
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .filter(|name| !name.is_empty())
                            else {
                                continue;
                            };
                            sequence += 1;
                            let event_key = codex_event_key(&source.session_id, line_index);
                            let call_id = payload
                                .get("call_id")
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            let invocation_key = call_id.clone().unwrap_or_else(|| {
                                format!("codex:{}:line:{}", source.session_id, line_index)
                            });
                            // custom_tool_call 的入参在 `input` 字段（可为 diff 文本，
                            // 不一定是 JSON）；function_call 的入参是 JSON 字符串。
                            let (input_text, input_keys) = if is_custom {
                                (
                                    payload
                                        .get("input")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string(),
                                    Vec::new(),
                                )
                            } else {
                                let arguments = payload
                                    .get("arguments")
                                    .and_then(Value::as_str)
                                    .unwrap_or("");
                                (arguments.to_string(), top_level_keys(arguments))
                            };
                            let status = if is_custom {
                                payload
                                    .get("status")
                                    .and_then(Value::as_str)
                                    .map(custom_call_status)
                                    .unwrap_or(EventStatus::Success)
                            } else {
                                EventStatus::Success
                            };
                            let event = NewSessionEvent {
                                event_key: event_key.clone(),
                                sequence,
                                timestamp_ms: ts_ms,
                                kind: SessionEventKind::ToolInvocation,
                                status: Some(status),
                                actor_agent_key: actor_agent_key.clone(),
                                parent_event_key: None,
                                summary_redacted: Some(truncate_chars(
                                    &redact_text(name),
                                    SUMMARY_MAX_CHARS,
                                )),
                                content_state: ContentState::Available,
                                source_file_path: source.primary_file_path.clone(),
                                source_offset: Some(line_index as i64),
                                payload_hash: None,
                                raw_event_kind: format!("response_item/{inner}"),
                            };
                            let invocation_index = tool_invocations.len();
                            if let Some(cid) = &call_id {
                                calls.insert(cid.clone(), (event_key.clone(), invocation_index));
                            }
                            events.push(event);
                            tool_invocations.push(NewToolInvocation {
                                invocation_key,
                                event_key,
                                tool_name: normalize_tool_name(name),
                                family: tool_family(name).to_string(),
                                duration_ms: None,
                                status: Some(status),
                                input_bytes: Some(input_text.len() as i64),
                                output_bytes: None,
                                input_keys,
                                result_kind: None,
                            });
                        }
                        "function_call_output" | "custom_tool_call_output" => {
                            let output =
                                payload.get("output").and_then(Value::as_str).unwrap_or("");
                            let status = if inner == "custom_tool_call_output" {
                                output_exit_status(output)
                            } else {
                                EventStatus::Success
                            };
                            sequence += 1;
                            let event_key = codex_event_key(&source.session_id, line_index);
                            let parent_event_key = payload
                                .get("call_id")
                                .and_then(Value::as_str)
                                .and_then(|cid| calls.get(cid))
                                .map(|(call_event_key, invocation_index)| {
                                    // 回填调用行的输出字节大小（完整 payload 不落库）。
                                    tool_invocations[*invocation_index].output_bytes =
                                        Some(output.len() as i64);
                                    call_event_key.clone()
                                });
                            events.push(NewSessionEvent {
                                event_key,
                                sequence,
                                timestamp_ms: ts_ms,
                                kind: SessionEventKind::ToolResult,
                                status: Some(status),
                                actor_agent_key: actor_agent_key.clone(),
                                parent_event_key,
                                summary_redacted: None,
                                content_state: ContentState::Available,
                                source_file_path: source.primary_file_path.clone(),
                                source_offset: Some(line_index as i64),
                                payload_hash: None,
                                raw_event_kind: format!("response_item/{inner}"),
                            });
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        let _ = bad_lines;
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
        cursor: Option<String>,
    ) -> Result<RedactedPayloadPage, ActivityError> {
        // 0 表示调用方未指定，走默认上限。
        let max = if max_bytes == 0 {
            DEFAULT_MAX_BYTES
        } else {
            max_bytes.clamp(1, MAX_BYTES_CAP)
        };
        // cursor：None 表示首次读取（从头）；非 "B{n}" 格式视为非法 →
        // Unavailable，不读取不 panic。
        let from = match cursor.as_deref() {
            None => 0,
            Some(raw) => match parse_payload_cursor(Some(raw)) {
                Some(offset) => offset,
                None => {
                    return Ok(RedactedPayloadPage {
                        content: String::new(),
                        truncated: false,
                        next_cursor: None,
                        content_state: ContentState::Unavailable,
                    });
                }
            },
        };
        let offset = source_ref
            .source_offset
            .filter(|value| *value >= 1)
            .unwrap_or(1) as usize;
        // 21.5 路径安全：只读 .jsonl、大小 ≤ 512MB，否则 Unavailable。
        if validate_payload_path(&source_ref.source_file_path).is_err() {
            return Ok(RedactedPayloadPage {
                content: String::new(),
                truncated: false,
                next_cursor: None,
                content_state: ContentState::Unavailable,
            });
        }
        // 文件不可读或行号超文件长度 → Unavailable（诚实响应，不伪造内容）。
        let Some(line) = read_payload_line(&source_ref.source_file_path, offset)? else {
            return Ok(RedactedPayloadPage {
                content: String::new(),
                truncated: false,
                next_cursor: None,
                content_state: ContentState::Unavailable,
            });
        };
        let json: Value = serde_json::from_str(&line)
            .map_err(|_| ActivityError::Parse("invalid json line for payload".to_string()))?;
        let text = if section == "raw" {
            line.trim_end().to_string()
        } else {
            extract_section_text(&json, section)?
        };
        let redacted = redact_text(&text);
        // cursor 偏移超界（> 文本长度）→ Unavailable，不 panic。
        if from > redacted.len() || !redacted.is_char_boundary(from) {
            return Ok(RedactedPayloadPage {
                content: String::new(),
                truncated: false,
                next_cursor: None,
                content_state: ContentState::Unavailable,
            });
        }
        let (content, truncated, next_cursor) = slice_payload_page(&redacted, from, max);
        Ok(RedactedPayloadPage {
            content,
            truncated,
            next_cursor,
            content_state: ContentState::Available,
        })
    }
}

// ── 身份识别 ──────────────────────────────────────────────────────────────────

/// 读取首个 `session_meta` 行（前 20 行内）得到 rollout 身份；无 meta 时
/// 返回全默认值（fork 语义缺失，按普通文件处理）。
fn inspect_rollout_identity(path: &str) -> RolloutIdentity {
    let mut identity = RolloutIdentity {
        thread_id: String::new(),
        ..RolloutIdentity::default()
    };
    let Ok(file) = fs::File::open(path) else {
        return identity;
    };
    for line in BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .take(META_SCAN_LINES)
    {
        let Ok(json) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if json.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        let Some(payload) = json.get("payload") else {
            continue;
        };
        if payload.get("forked_from_id").is_some() {
            identity.fork_start_ts = extract_timestamp(&json);
        }
        let source_is_subagent = payload
            .get("source")
            .and_then(Value::as_object)
            .map(|source| source.contains_key("subagent"))
            .unwrap_or(false);
        if source_is_subagent {
            identity.is_subagent = true;
        }
        if identity.thread_id.is_empty() {
            // 与 codex_reader 的 root_session_id 同策略：优先父线程 id，
            // 缺失时退回会话 id 自身。
            let thread_id = payload
                .get("source")
                .and_then(|source| source.get("subagent"))
                .and_then(|subagent| subagent.get("thread_spawn"))
                .and_then(|spawn| spawn.get("parent_thread_id"))
                .and_then(Value::as_str)
                .or_else(|| payload.get("id").and_then(Value::as_str))
                .or_else(|| payload.get("session_id").and_then(Value::as_str))
                .or_else(|| payload.get("sessionId").and_then(Value::as_str));
            if let Some(id) = thread_id {
                identity.thread_id = id.to_string();
            }
        }
        if identity.title.is_none() {
            for key in ["title", "name", "summary", "slug"] {
                let value = payload
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                if let Some(value) = value {
                    identity.title = Some(value.to_string());
                    break;
                }
            }
        }
        break;
    }
    identity
}

/// fork replay 判定：时间戳存在且不晚于 fork 起点（codex_reader 406-407 语义）。
fn is_fork_replay(identity: &RolloutIdentity, ts: Option<i64>) -> bool {
    match identity.fork_start_ts {
        Some(fork_ts) => ts.is_some_and(|value| value <= fork_ts),
        None => false,
    }
}

fn codex_event_key(session_id: &str, line_index: u64) -> String {
    format!("codex:{session_id}:{line_index}")
}

// ── 文本提取 ──────────────────────────────────────────────────────────────────

/// 提取 response_item 消息文本（与 codex_reader 的
/// `extract_codex_response_item_text` 同语义：content 字符串或
/// output_text/text 数组项拼接）。
fn extract_response_item_text(payload: &Value) -> Option<String> {
    if let Some(content) = payload.get("content").and_then(Value::as_str) {
        return Some(content.to_string());
    }
    let content = payload.get("content")?.as_array()?;
    let mut text_parts = Vec::new();
    for item in content {
        let item_type = item.get("type").and_then(Value::as_str);
        match item_type {
            Some("output_text") | Some("text") => {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    text_parts.push(text.to_string());
                }
            }
            _ => {}
        }
    }
    if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join("\n"))
    }
}

/// Codex 系统注入消息（AGENTS.md / 环境上下文）不视为用户活动。
fn is_codex_system_message(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with("# AGENTS.md") || trimmed.starts_with("<environment_context>")
}

/// function_call 的 arguments（JSON 字符串）顶层 key 列表；非对象返回空。
fn top_level_keys(arguments: &str) -> Vec<String> {
    match serde_json::from_str::<Value>(arguments) {
        Ok(Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// 按 payload 结构与 section 提取文本（供 on-demand payload 读取）；
/// 未知 section 报 Unsupported（与 claude 适配器语义一致）。
fn extract_section_text(json: &Value, section: &str) -> Result<String, ActivityError> {
    let Some(payload) = json.get("payload") else {
        return Ok(String::new());
    };
    let inner = payload.get("type").and_then(Value::as_str).unwrap_or("");
    match section {
        "input" | "summary" => Ok(match inner {
            "message" => extract_response_item_text(payload).unwrap_or_default(),
            "function_call" => payload
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            "custom_tool_call" => payload
                .get("input")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            "user_message" => payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            _ => String::new(),
        }),
        "output" => Ok(match inner {
            "function_call_output" | "custom_tool_call_output" => payload
                .get("output")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            _ => String::new(),
        }),
        _ => Err(ActivityError::Unsupported(format!(
            "unsupported payload section: {section}"
        ))),
    }
}

// ── 工具名规范化与族映射 ──────────────────────────────────────────────────────

/// 规范化工具名：小写、非字母数字转下划线、去首尾下划线。
/// `exec_command` → `exec_command`；`Read` → `read`；`@server/tool` → `server_tool`。
fn normalize_tool_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_underscore = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_underscore = false;
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
        }
    }
    out.trim_matches('_').to_string()
}

/// 工具族映射（对齐设计 9.5：shell/file/search/web/agent/mcp/unknown）。
/// 顺序敏感：具体关键词优先于泛化匹配。
fn tool_family(name: &str) -> &'static str {
    let normalized = name.to_ascii_lowercase();
    let contains_any = |parts: &[&str]| parts.iter().any(|part| normalized.contains(part));
    if normalized.starts_with("exec")
        || contains_any(&[
            "command",
            "shell",
            "terminal",
            "bash",
            "zsh",
            "run_script",
            "console",
            "ssh",
        ])
    {
        "shell"
    } else if contains_any(&[
        "apply_patch",
        "write_file",
        "writefile",
        "edit_file",
        "editfile",
        "file_write",
        "update_file",
        "create_file",
        "append_file",
        "patch",
        "read_file",
        "readfile",
        "view_file",
        "viewfile",
        "list_dir",
        "listdir",
        "open_file",
        "glob",
    ]) || normalized.starts_with("read")
    {
        "file"
    } else if contains_any(&["web", "fetch", "http", "url", "browse", "curl"]) {
        "web"
    } else if contains_any(&["search", "grep", "rg", "find", "query"]) {
        "search"
    } else if contains_any(&["agent", "task", "delegate", "spawn", "subagent"]) {
        "agent"
    } else if contains_any(&["mcp"]) || normalized.contains('.') || normalized.contains("__") {
        "mcp"
    } else {
        "unknown"
    }
}

/// custom_tool_call 显式 status 字段 → EventStatus（无字段按 Success）。
fn custom_call_status(status: &str) -> EventStatus {
    match status {
        "completed" => EventStatus::Success,
        "failed" => EventStatus::Error,
        "pending" | "in_progress" => EventStatus::Pending,
        "cancelled" | "canceled" => EventStatus::Cancelled,
        _ => EventStatus::Success,
    }
}

/// custom_tool_call_output 的输出 JSON 中显式 exit_code 非 0 → Error；
/// 无该字段（或 function_call_output）→ Success（保守，不臆断）。
fn output_exit_status(output: &str) -> EventStatus {
    let Ok(json) = serde_json::from_str::<Value>(output) else {
        return EventStatus::Success;
    };
    let exit_code = json
        .get("metadata")
        .and_then(|metadata| metadata.get("exit_code"))
        .and_then(Value::as_i64)
        .or_else(|| json.get("exit_code").and_then(Value::as_i64));
    match exit_code {
        Some(code) if code != 0 => EventStatus::Error,
        _ => EventStatus::Success,
    }
}

// ── 时间与截断 ────────────────────────────────────────────────────────────────

/// 行时间戳 → Unix 秒（与 session::shared::extract_timestamp 同语义；
/// 毫秒数字自动换算为秒，RFC3339 字符串解析）。
fn extract_timestamp(json: &Value) -> Option<i64> {
    let ts = json
        .get("timestamp")
        .or_else(|| json.get("createdAt"))
        .or_else(|| json.get("created_at"))
        .or_else(|| json.get("time"))
        .or_else(|| json.get("date"));
    let ts = ts?;
    if let Some(num) = ts.as_u64() {
        return Some(if num > 10_000_000_000 {
            (num / 1000) as i64
        } else {
            num as i64
        });
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

// ── Token 差分（与 codex_reader 同源语义）─────────────────────────────────────

fn parse_u64_from_value(value: &Value) -> Option<u64> {
    if let Some(num) = value.as_u64() {
        return Some(num);
    }
    if let Some(num) = value.as_i64() {
        return Some(num.max(0) as u64);
    }
    if let Some(num) = value.as_f64() {
        return Some(num.max(0.0) as u64);
    }
    None
}

fn parse_cumulative(value: &Value) -> Option<CumulativeTokens> {
    if !value.is_object() {
        return None;
    }
    Some(CumulativeTokens {
        input: value
            .get("input_tokens")
            .and_then(parse_u64_from_value)
            .unwrap_or(0),
        output: value
            .get("output_tokens")
            .and_then(parse_u64_from_value)
            .unwrap_or(0),
        cache_create: value
            .get("cache_creation_input_tokens")
            .or_else(|| value.get("cache_create_tokens"))
            .and_then(parse_u64_from_value)
            .unwrap_or(0),
        cache_read: value
            .get("cached_input_tokens")
            .or_else(|| value.get("cache_read_input_tokens"))
            .or_else(|| value.get("cache_read_tokens"))
            .and_then(parse_u64_from_value)
            .unwrap_or(0),
    })
}

fn tokens_rolled_back(prev: &CumulativeTokens, current: &CumulativeTokens) -> bool {
    current.input < prev.input
        || current.output < prev.output
        || current.cache_create < prev.cache_create
        || current.cache_read < prev.cache_read
}

fn tokens_delta(prev: Option<&CumulativeTokens>, current: &CumulativeTokens) -> CumulativeTokens {
    match prev {
        Some(previous) => CumulativeTokens {
            input: current.input.saturating_sub(previous.input),
            output: current.output.saturating_sub(previous.output),
            cache_create: current.cache_create.saturating_sub(previous.cache_create),
            cache_read: current.cache_read.saturating_sub(previous.cache_read),
        },
        None => current.clone(),
    }
}

fn normalize_delta(delta: CumulativeTokens) -> CumulativeTokens {
    let cache_read = delta.cache_read.min(delta.input);
    CumulativeTokens {
        input: delta.input.saturating_sub(cache_read),
        output: delta.output,
        cache_create: delta.cache_create,
        cache_read,
    }
}

// ── 测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::constants::TOOL_CODEX;
    use serde_json::json;
    use std::io::Write;

    fn make_source(session_id: &str, path: &str) -> SessionSourceRef {
        SessionSourceRef {
            session_id: session_id.to_string(),
            tool: TOOL_CODEX.to_string(),
            primary_file_path: path.to_string(),
            source_file_id: None,
        }
    }

    fn write_fixture(lines: &[Value]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout-test.jsonl");
        let mut file = fs::File::create(&path).expect("create fixture");
        for line in lines {
            writeln!(file, "{line}").expect("write fixture line");
        }
        (dir, path.to_string_lossy().to_string())
    }

    /// 原始文本行 fixture（用于构造坏 JSON 行）。
    fn write_raw_fixture(lines: &[&str]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout-test.jsonl");
        let mut file = fs::File::create(&path).expect("create fixture");
        for line in lines {
            writeln!(file, "{line}").expect("write fixture line");
        }
        (dir, path.to_string_lossy().to_string())
    }

    /// 最小完整 fixture：session_meta + 用户消息 + 助手回复 + 工具调用 +
    /// 工具结果 + token_count。
    fn minimal_fixture() -> Vec<Value> {
        vec![
            json!({"timestamp": "2026-05-09T10:00:00Z", "type": "session_meta", "payload": {
                "id": "sess-1", "cwd": "/Users/test/work/proj", "source": "cli", "title": "fix bug"
            }}),
            json!({"timestamp": "2026-05-09T10:00:01Z", "type": "event_msg", "payload": {
                "type": "user_message", "message": "fix the login bug\nAuthorization: Bearer abcdefgh12345678"
            }}),
            json!({"timestamp": "2026-05-09T10:00:02Z", "type": "response_item", "payload": {
                "type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "let me look"}]
            }}),
            json!({"timestamp": "2026-05-09T10:00:03Z", "type": "response_item", "payload": {
                "type": "function_call", "name": "exec_command",
                "arguments": "{\"cmd\":\"pwd\",\"workdir\":\"/tmp\"}", "call_id": "call_1"
            }}),
            json!({"timestamp": "2026-05-09T10:00:04Z", "type": "response_item", "payload": {
                "type": "function_call_output", "call_id": "call_1", "output": "/tmp\n"
            }}),
            json!({"timestamp": "2026-05-09T10:00:05Z", "type": "event_msg", "payload": {
                "type": "token_count", "info": {
                    "total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 30}
                }
            }}),
        ]
    }

    #[test]
    fn indexes_minimal_fixture_with_kinds_and_links() {
        let (_dir, path) = write_fixture(&minimal_fixture());
        let adapter = CodexAdapter;
        let source = make_source("codex::sess-1", &path);
        let batch = adapter.index_session(&source).expect("index session");

        let kinds: Vec<SessionEventKind> = batch.events.iter().map(|event| event.kind).collect();
        assert_eq!(
            kinds,
            vec![
                SessionEventKind::UserMessage,
                SessionEventKind::AssistantMessage,
                SessionEventKind::ToolInvocation,
                SessionEventKind::ToolResult,
                SessionEventKind::SystemEvent,
            ]
        );

        // sequence 从 1 递增
        let sequences: Vec<i64> = batch.events.iter().map(|event| event.sequence).collect();
        assert_eq!(sequences, vec![1, 2, 3, 4, 5]);

        // event_key 稳定：codex:{session_id}:{line_index}
        assert_eq!(
            batch.events[2].event_key,
            "codex:codex::sess-1:4".to_string()
        );

        // 工具调用与结果精确关联（call_id → 调用事件 key）
        let invocation_event = &batch.events[2];
        let result_event = &batch.events[3];
        assert_eq!(result_event.kind, SessionEventKind::ToolResult);
        assert_eq!(
            result_event.parent_event_key.as_deref(),
            Some(invocation_event.event_key.as_str())
        );

        // 工具调用元数据：名称规范化 + family + input_keys
        assert_eq!(batch.tool_invocations.len(), 1);
        let invocation = &batch.tool_invocations[0];
        assert_eq!(invocation.tool_name, "exec_command");
        assert_eq!(invocation.family, "shell");
        assert_eq!(
            invocation.input_keys,
            vec!["cmd".to_string(), "workdir".to_string()]
        );
        assert_eq!(invocation.input_bytes, Some(30));
        assert_eq!(invocation.output_bytes, Some(5));
        assert_eq!(invocation.status, Some(EventStatus::Success));

        // 请求关联：TimeWindow + 合成 id
        assert_eq!(batch.request_links.len(), 1);
        let link = &batch.request_links[0];
        assert_eq!(link.request_key, "codex:codex::sess-1:1".to_string());
        assert_eq!(link.strength, RequestLinkStrength::TimeWindow);
        assert_eq!(link.event_key, batch.events[4].event_key);

        // 无子代理时无 agent 节点、无 actor 标记
        assert!(batch.agents.is_empty());
        assert!(batch
            .events
            .iter()
            .all(|event| event.actor_agent_key.is_none()));
    }

    #[test]
    fn indexing_twice_produces_identical_batch() {
        let (_dir, path) = write_fixture(&minimal_fixture());
        let adapter = CodexAdapter;
        let source = make_source("codex::sess-1", &path);
        let first = adapter.index_session(&source).expect("first index");
        let second = adapter.index_session(&source).expect("second index");
        assert_eq!(first.events, second.events);
        assert_eq!(first.tool_invocations, second.tool_invocations);
        assert_eq!(first.request_links, second.request_links);
        assert_eq!(first.agents, second.agents);
    }

    #[test]
    fn summary_is_redacted_and_bounded() {
        let (_dir, path) = write_fixture(&minimal_fixture());
        let adapter = CodexAdapter;
        let source = make_source("codex::sess-1", &path);
        let batch = adapter.index_session(&source).expect("index session");

        let user_summary = batch.events[0]
            .summary_redacted
            .as_deref()
            .expect("summary");
        assert!(!user_summary.contains("abcdefgh12345678"), "secret leaked");
        assert!(!user_summary.contains("Bearer"), "auth header leaked");
        assert!(user_summary.len() <= SUMMARY_MAX_CHARS);
        assert!(user_summary.contains("fix the login bug"));

        // 超长消息截断 ≤ 500 字符
        let long_text = "x".repeat(1200);
        let long_fixture = vec![json!({
            "timestamp": "2026-05-09T10:00:01Z", "type": "event_msg",
            "payload": {"type": "user_message", "message": long_text}
        })];
        let (_dir2, path2) = write_fixture(&long_fixture);
        let batch2 = adapter
            .index_session(&make_source("codex::sess-2", &path2))
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
        // 坏 JSON 行、截断 JSON 行、尾随垃圾行混在合法行之间。
        let lines = [
            r#"{"timestamp":"2026-05-09T10:00:00Z","type":"session_meta","payload":{"id":"sess-1"}}"#,
            r#"{"timestamp":"2026-05-09T10:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"hello"}}"#,
            "this is not json {{",
            r#"{"timestamp":"2026-05-09T10:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}}"#,
            r#"{"type": "response_item", "payload": {"type": ""#,
            r#"{"timestamp":"2026-05-09T10:00:03Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{}","call_id":"c1"}}"#,
            r#"{"timestamp":"2026-05-09T10:00:04Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"out"}}"#,
            r#"{"timestamp":"2026-05-09T10:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":50,"output_tokens":10}}}}"#,
            "trailing garbage",
        ];
        let (_dir, path) = write_raw_fixture(&lines);

        let adapter = CodexAdapter;
        let source = make_source("codex::sess-1", &path);
        let batch = adapter
            .index_session(&source)
            .expect("index with corrupt lines");
        // 坏行跳过不中断：仍产出其余 5 个事件（session_meta 不产出事件）
        let kinds: Vec<SessionEventKind> = batch.events.iter().map(|event| event.kind).collect();
        assert_eq!(
            kinds,
            vec![
                SessionEventKind::UserMessage,
                SessionEventKind::AssistantMessage,
                SessionEventKind::ToolInvocation,
                SessionEventKind::ToolResult,
                SessionEventKind::SystemEvent,
            ]
        );
        // 行号偏移不影响 key 唯一性与顺序（事件按文件行序）
        assert_eq!(
            batch.events[2].event_key,
            "codex:codex::sess-1:6".to_string()
        );
        // 坏行两侧的工具调用/结果关联仍然成立
        assert_eq!(
            batch.events[3].parent_event_key.as_deref(),
            Some(batch.events[2].event_key.as_str())
        );
    }

    #[test]
    fn maps_multiple_tool_families() {
        let fixture = vec![
            json!({"timestamp": "2026-05-09T10:00:01Z", "type": "response_item", "payload": {
                "type": "function_call", "name": "exec_command", "arguments": "{}", "call_id": "c1"
            }}),
            json!({"timestamp": "2026-05-09T10:00:02Z", "type": "response_item", "payload": {
                "type": "custom_tool_call", "name": "apply_patch", "input": "*** Begin Patch", "call_id": "c2", "status": "completed"
            }}),
            json!({"timestamp": "2026-05-09T10:00:03Z", "type": "response_item", "payload": {
                "type": "function_call", "name": "web_search", "arguments": "{}", "call_id": "c3"
            }}),
            json!({"timestamp": "2026-05-09T10:00:04Z", "type": "response_item", "payload": {
                "type": "function_call", "name": "grep_search", "arguments": "{}", "call_id": "c4"
            }}),
            json!({"timestamp": "2026-05-09T10:00:05Z", "type": "response_item", "payload": {
                "type": "function_call", "name": "Read", "arguments": "{}", "call_id": "c5"
            }}),
            json!({"timestamp": "2026-05-09T10:00:06Z", "type": "response_item", "payload": {
                "type": "function_call", "name": "not_a_real_tool", "arguments": "{}", "call_id": "c6"
            }}),
            json!({"timestamp": "2026-05-09T10:00:07Z", "type": "response_item", "payload": {
                "type": "custom_tool_call_output", "call_id": "c2",
                "output": "{\"output\":\"ok\",\"metadata\":{\"exit_code\":1}}"
            }}),
        ];
        let (_dir, path) = write_fixture(&fixture);
        let adapter = CodexAdapter;
        let batch = adapter
            .index_session(&make_source("codex::sess-3", &path))
            .expect("index");

        let by_name: HashMap<&str, &str> = batch
            .tool_invocations
            .iter()
            .map(|inv| (inv.tool_name.as_str(), inv.family.as_str()))
            .collect();
        assert_eq!(by_name.get("exec_command"), Some(&"shell"));
        assert_eq!(by_name.get("apply_patch"), Some(&"file"));
        assert_eq!(by_name.get("web_search"), Some(&"web"));
        assert_eq!(by_name.get("grep_search"), Some(&"search"));
        assert_eq!(by_name.get("read"), Some(&"file"));
        assert_eq!(by_name.get("not_a_real_tool"), Some(&"unknown"));

        // ≥2 类 family
        let families: std::collections::HashSet<&str> = batch
            .tool_invocations
            .iter()
            .map(|inv| inv.family.as_str())
            .collect();
        assert!(
            families.len() >= 2,
            "expected >=2 families, got {families:?}"
        );

        // custom_tool_call_output exit_code=1 → Error 状态
        let result_event = batch
            .events
            .iter()
            .find(|event| event.kind == SessionEventKind::ToolResult)
            .expect("tool result");
        assert_eq!(result_event.status, Some(EventStatus::Error));
        // apply_patch 调用状态 completed → Success
        let patch_inv = batch
            .tool_invocations
            .iter()
            .find(|inv| inv.tool_name == "apply_patch")
            .expect("patch invocation");
        assert_eq!(patch_inv.status, Some(EventStatus::Success));
        // 输出字节回填到对应调用
        assert!(patch_inv.output_bytes.is_some());
    }

    #[test]
    fn marks_subagent_file_with_actor_agent_key() {
        let fixture = vec![
            json!({"timestamp": "2026-05-09T10:00:00Z", "type": "session_meta", "payload": {
                "id": "sub-uuid-1", "source": {"subagent": "review"}, "thread_source": "subagent"
            }}),
            json!({"timestamp": "2026-05-09T10:00:01Z", "type": "event_msg", "payload": {
                "type": "user_message", "message": "review this diff"
            }}),
            json!({"timestamp": "2026-05-09T10:00:02Z", "type": "response_item", "payload": {
                "type": "function_call", "name": "read_file", "arguments": "{}", "call_id": "c1"
            }}),
        ];
        let (_dir, path) = write_fixture(&fixture);
        let adapter = CodexAdapter;
        let batch = adapter
            .index_session(&make_source("codex::sub-uuid-1", &path))
            .expect("index");

        assert_eq!(batch.agents.len(), 1);
        let agent = &batch.agents[0];
        assert_eq!(
            agent.agent_key,
            "codex::sub-uuid-1:subagent:sub-uuid-1".to_string()
        );
        assert_eq!(agent.relation_level, AgentRelationLevel::FlagOnly);
        assert_eq!(agent.display_kind.as_deref(), Some("subagent"));

        assert!(batch.events.iter().all(|event| {
            event.actor_agent_key.as_deref() == Some("codex::sub-uuid-1:subagent:sub-uuid-1")
        }));
    }

    #[test]
    fn skips_fork_replay_events_but_keeps_token_baseline() {
        let fork_ts = "2026-06-16T10:00:00Z";
        let new_ts = "2026-06-16T10:05:00Z";
        let fixture = vec![
            json!({"timestamp": fork_ts, "type": "session_meta", "payload": {
                "id": "fork-session-id", "forked_from_id": "original-session-id", "cwd": "/work/proj"
            }}),
            // replay 区间：用户消息 + token_count（基线 100）
            json!({"timestamp": fork_ts, "type": "event_msg", "payload": {
                "type": "user_message", "message": "replayed history message"
            }}),
            json!({"timestamp": fork_ts, "type": "event_msg", "payload": {
                "type": "token_count", "info": {
                    "total_token_usage": {"input_tokens": 100, "output_tokens": 30}
                }
            }}),
            // replay 区间：token_count（基线 200）
            json!({"timestamp": fork_ts, "type": "event_msg", "payload": {
                "type": "token_count", "info": {
                    "total_token_usage": {"input_tokens": 200, "output_tokens": 60}
                }
            }}),
            // 真正的新事件：delta = 50 input / 20 output
            json!({"timestamp": new_ts, "type": "event_msg", "payload": {
                "type": "token_count", "info": {
                    "total_token_usage": {"input_tokens": 250, "output_tokens": 80}
                }
            }}),
        ];
        let (_dir, path) = write_fixture(&fixture);
        let adapter = CodexAdapter;
        let batch = adapter
            .index_session(&make_source("codex::fork-session-id", &path))
            .expect("index");

        // replay 事件不产出；只有新 token_count 一个事件
        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.events[0].kind, SessionEventKind::SystemEvent);
        assert_eq!(batch.request_links.len(), 1);
        assert_eq!(
            batch.request_links[0].request_key,
            "codex:codex::fork-session-id:1"
        );
    }

    #[test]
    fn reads_redacted_payload_by_section() {
        let (_dir, path) = write_fixture(&minimal_fixture());
        let adapter = CodexAdapter;

        // function_call 行（第 4 行）：input section → arguments
        let call_ref = SafeSourceRef {
            source_file_id: None,
            source_file_path: path.clone(),
            source_offset: Some(4),
            fingerprint: None,
        };
        let page = adapter
            .read_payload(&call_ref, "input", DEFAULT_MAX_BYTES, None)
            .expect("read payload");
        assert_eq!(page.content_state, ContentState::Available);
        assert!(!page.truncated);
        assert!(page.content.contains("\"cmd\":\"pwd\""));

        // 非字符边界 cursor（多字节内容 B1）→ Unavailable，不 panic。
        let zh_fixture = vec![json!({"type": "event_msg", "payload": {
            "type": "user_message", "message": "中文摘要内容"
        }})];
        let (_zh_dir, zh_path) = write_fixture(&zh_fixture);
        let zh_ref = SafeSourceRef {
            source_file_id: None,
            source_file_path: zh_path,
            source_offset: Some(1),
            fingerprint: None,
        };
        let page = adapter
            .read_payload(
                &zh_ref,
                "summary",
                DEFAULT_MAX_BYTES,
                Some("B1".to_string()),
            )
            .expect("non-boundary cursor page is not an error");
        assert_eq!(page.content_state, ContentState::Unavailable);
        assert!(page.content.is_empty());

        // 小 max_bytes 截断（按字节安全截断）
        let page = adapter
            .read_payload(&call_ref, "input", 8, None)
            .expect("read payload truncated");
        assert!(page.truncated);
        assert!(page.content.len() <= 8);

        // function_call_output 行（第 5 行）：output section
        let output_ref = SafeSourceRef {
            source_file_id: None,
            source_file_path: path.clone(),
            source_offset: Some(5),
            fingerprint: None,
        };
        let page = adapter
            .read_payload(&output_ref, "output", DEFAULT_MAX_BYTES, None)
            .expect("read output");
        assert_eq!(page.content, "/tmp\n");

        // raw section 返回整行且经脱敏
        let raw_ref = SafeSourceRef {
            source_file_id: None,
            source_file_path: path,
            source_offset: Some(2),
            fingerprint: None,
        };
        let page = adapter
            .read_payload(&raw_ref, "raw", DEFAULT_MAX_BYTES, None)
            .expect("read raw");
        assert!(!page.content.contains("abcdefgh12345678"));
        assert!(page.content.contains("user_message"));
    }

    #[test]
    fn payload_unavailable_when_file_missing() {
        let adapter = CodexAdapter;
        let source_ref = SafeSourceRef {
            source_file_id: None,
            source_file_path: "/nonexistent/rollout-x.jsonl".to_string(),
            source_offset: Some(1),
            fingerprint: None,
        };
        let page = adapter
            .read_payload(&source_ref, "input", DEFAULT_MAX_BYTES, None)
            .expect("unavailable page is not an error");
        assert_eq!(page.content_state, ContentState::Unavailable);
        assert!(page.content.is_empty());
    }

    #[test]
    fn missing_file_index_returns_io_error_without_path() {
        let adapter = CodexAdapter;
        let source = make_source("codex::missing", "/nonexistent/rollout-x.jsonl");
        let err = adapter.index_session(&source).expect_err("io error");
        let message = err.to_string();
        assert!(message.starts_with("ERR_ACTIVITY_IO"));
        assert!(!message.contains("nonexistent"), "error must not leak path");
    }
}
