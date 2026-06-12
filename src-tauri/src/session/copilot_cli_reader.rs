//! GitHub Copilot CLI 本地会话读取模块
//!
//! 扫描 `~/.copilot/session-state/*/events.jsonl`，解析 Copilot CLI 的事件流。
//! 这里优先使用 `session.shutdown` 作为会话总量真值；
//! 若会话仍在进行中，则退化为使用 `assistant.message` 事件构造近似请求事实。

use super::constants::TOOL_COPILOT;
use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::shared::{
    extract_project_name, extract_timestamp, parse_u64_from_value, truncate_string,
};
use super::source::{ParsedSessionData, SessionSource, SourceSnapshot, SourceUpdateMode};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};

const COPILOT_CLI_SOURCE_KIND: &str = "copilot_cli_session";

pub(super) struct CopilotCliSource;

impl SessionSource for CopilotCliSource {
    fn tool_id(&self) -> &'static str {
        TOOL_COPILOT
    }

    fn scan(&self) -> SourceSnapshot {
        let sessions = collect_copilot_cli_session_files();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for session in &sessions {
            session.session_id.hash(&mut hasher);
            session.fingerprint.hash(&mut hasher);
        }

        SourceSnapshot {
            source_id: TOOL_COPILOT,
            update_mode: SourceUpdateMode::PerSession,
            sessions,
            scan_fingerprint: hasher.finish(),
        }
    }

    fn parse(&self, session: &SessionFile) -> Result<ParsedSessionData, String> {
        let (meta, requests) = parse_copilot_cli_session(session);
        Ok(ParsedSessionData { meta, requests })
    }
}

fn collect_copilot_cli_session_files() -> Vec<SessionFile> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let root = home.join(".copilot").join("session-state");
    if !root.exists() {
        return Vec::new();
    }

    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };

    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let session_dir = entry.path();
        if !session_dir.is_dir() {
            continue;
        }

        let raw_session_id = session_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_string();
        if raw_session_id.is_empty() {
            continue;
        }

        let events_path = session_dir.join("events.jsonl");
        if !events_path.exists() {
            continue;
        }

        let metadata = fs::metadata(&events_path).ok();
        let file_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        if file_size == 0 {
            continue;
        }

        let last_modified = metadata
            .and_then(|m| m.modified().ok())
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64
            })
            .unwrap_or(0);

        let file_path = events_path.to_string_lossy().to_string();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        file_path.hash(&mut hasher);
        file_size.hash(&mut hasher);
        last_modified.hash(&mut hasher);
        let fingerprint = hasher.finish();

        sessions.push(SessionFile {
            session_id: format!("copilot_cli::{raw_session_id}"),
            tool: TOOL_COPILOT.to_string(),
            project_path: raw_session_id,
            file_path: file_path.clone(),
            transcript_paths: vec![file_path],
            file_size,
            last_modified,
            fingerprint,
        });
    }

    sessions.sort_by_key(|session| std::cmp::Reverse(session.last_modified));
    sessions
}

#[derive(Default)]
struct ShutdownSummary {
    request_count: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_create_tokens: u64,
    reasoning_tokens: u64,
    start_time: Option<i64>,
    end_time: Option<i64>,
    current_model: Option<String>,
    model_metrics: HashMap<String, ModelSummary>,
}

#[derive(Default, Clone)]
struct ModelSummary {
    request_count: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_create_tokens: u64,
    reasoning_tokens: u64,
}

#[derive(Default)]
struct AssistantEvent {
    message_id: String,
    interaction_id: Option<String>,
    timestamp: i64,
    model: String,
    output_tokens: u64,
}

fn parse_copilot_cli_session(session: &SessionFile) -> (SessionMeta, Vec<LocalRequestRecord>) {
    let mut meta = SessionMeta {
        session_id: session.session_id.clone(),
        tool: session.tool.clone(),
        cwd: None,
        project_name: None,
        topic: None,
        last_prompt: None,
        session_name: None,
        file_path: session.file_path.clone(),
        file_size: session.file_size,
        last_modified: session.last_modified,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_cache_create_tokens: 0,
        total_cache_read_tokens: 0,
        models: Vec::new(),
        message_count: 0,
        start_time: 0,
        end_time: 0,
        source: COPILOT_CLI_SOURCE_KIND.to_string(),
        message_ids: Vec::new(),
    };

    let file = match fs::File::open(&session.file_path) {
        Ok(file) => file,
        Err(_) => return (meta, Vec::new()),
    };

    let mut cwd_found: Option<String> = None;
    let mut first_user_message: Option<String> = None;
    let mut last_user_message: Option<String> = None;
    let mut models_set: BTreeSet<String> = BTreeSet::new();
    let mut earliest_timestamp: Option<i64> = None;
    let mut latest_timestamp: Option<i64> = None;
    let mut assistant_events: Vec<AssistantEvent> = Vec::new();
    let mut shutdown = ShutdownSummary::default();

    let reader = BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
        let Ok(json) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        let event_type = json
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let data = json.get("data").unwrap_or(&json);
        let event_timestamp = extract_timestamp(&json).or_else(|| extract_timestamp(data));

        if let Some(ts) = event_timestamp {
            earliest_timestamp = Some(
                earliest_timestamp
                    .map(|current| current.min(ts))
                    .unwrap_or(ts),
            );
            latest_timestamp = Some(
                latest_timestamp
                    .map(|current| current.max(ts))
                    .unwrap_or(ts),
            );
        }

        match event_type {
            "session.start" => {
                if cwd_found.is_none() {
                    cwd_found = data
                        .get("context")
                        .and_then(|value| value.get("cwd"))
                        .and_then(|value| value.as_str())
                        .map(str::to_string);
                }
                if let Some(model) = data.get("selectedModel").and_then(|value| value.as_str()) {
                    if !model.trim().is_empty() {
                        models_set.insert(model.to_string());
                    }
                }
                if shutdown.start_time.is_none() {
                    shutdown.start_time = data
                        .get("startTime")
                        .and_then(|value| value.as_str())
                        .and_then(parse_rfc3339_ts)
                        .or(event_timestamp);
                }
            }
            "user.message" => {
                if let Some(text) = data.get("content").and_then(|value| value.as_str()) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        if first_user_message.is_none() {
                            first_user_message = Some(trimmed.to_string());
                        }
                        last_user_message = Some(trimmed.to_string());
                    }
                }
            }
            "assistant.message" => {
                let output_tokens = data
                    .get("outputTokens")
                    .and_then(parse_u64_from_value)
                    .unwrap_or(0);
                let model = data
                    .get("model")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("unknown")
                    .to_string();
                if model != "unknown" {
                    models_set.insert(model.clone());
                }
                let timestamp = event_timestamp.unwrap_or(session.last_modified);
                let message_id = data
                    .get("requestId")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.trim().is_empty())
                    .or_else(|| data.get("messageId").and_then(|value| value.as_str()))
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        format!("copilot_cli_msg_{}_{}", timestamp, assistant_events.len())
                    });
                assistant_events.push(AssistantEvent {
                    message_id,
                    interaction_id: data
                        .get("interactionId")
                        .and_then(|value| value.as_str())
                        .map(str::to_string),
                    timestamp,
                    model,
                    output_tokens,
                });
            }
            "session.shutdown" => {
                shutdown.end_time = event_timestamp;
                shutdown.start_time = data
                    .get("sessionStartTime")
                    .and_then(parse_u64_from_value)
                    .map(|value| (value / 1000) as i64)
                    .or(shutdown.start_time);
                shutdown.current_model = data
                    .get("currentModel")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string);

                let model_metrics = data.get("modelMetrics").and_then(|value| value.as_object());
                if let Some(model_metrics) = model_metrics {
                    for (model_name, metric_value) in model_metrics {
                        let usage = metric_value.get("usage").unwrap_or(metric_value);
                        let requests = metric_value
                            .get("requests")
                            .and_then(|value| value.get("count"))
                            .and_then(parse_u64_from_value)
                            .unwrap_or(0);
                        let summary = ModelSummary {
                            request_count: requests,
                            input_tokens: usage
                                .get("inputTokens")
                                .and_then(parse_u64_from_value)
                                .unwrap_or(0),
                            output_tokens: usage
                                .get("outputTokens")
                                .and_then(parse_u64_from_value)
                                .unwrap_or(0),
                            cache_read_tokens: usage
                                .get("cacheReadTokens")
                                .and_then(parse_u64_from_value)
                                .unwrap_or(0),
                            cache_create_tokens: usage
                                .get("cacheWriteTokens")
                                .and_then(parse_u64_from_value)
                                .unwrap_or(0),
                            reasoning_tokens: usage
                                .get("reasoningTokens")
                                .and_then(parse_u64_from_value)
                                .unwrap_or(0),
                        };
                        if summary.request_count > 0
                            || summary.input_tokens > 0
                            || summary.output_tokens > 0
                            || summary.cache_read_tokens > 0
                            || summary.cache_create_tokens > 0
                        {
                            models_set.insert(model_name.clone());
                            shutdown.request_count += summary.request_count;
                            shutdown.input_tokens += summary.input_tokens;
                            shutdown.output_tokens += summary.output_tokens;
                            shutdown.cache_read_tokens += summary.cache_read_tokens;
                            shutdown.cache_create_tokens += summary.cache_create_tokens;
                            shutdown.reasoning_tokens += summary.reasoning_tokens;
                            shutdown.model_metrics.insert(model_name.clone(), summary);
                        }
                    }
                }

                if shutdown.input_tokens == 0
                    && shutdown.output_tokens == 0
                    && shutdown.cache_read_tokens == 0
                    && shutdown.cache_create_tokens == 0
                {
                    let token_details = data.get("tokenDetails").unwrap_or(data);
                    shutdown.input_tokens = token_details
                        .get("input")
                        .and_then(|value| value.get("tokenCount"))
                        .and_then(parse_u64_from_value)
                        .unwrap_or(0);
                    shutdown.output_tokens = token_details
                        .get("output")
                        .and_then(|value| value.get("tokenCount"))
                        .and_then(parse_u64_from_value)
                        .unwrap_or(0);
                    shutdown.cache_read_tokens = token_details
                        .get("cache_read")
                        .and_then(|value| value.get("tokenCount"))
                        .and_then(parse_u64_from_value)
                        .unwrap_or(0);
                }
            }
            _ => {}
        }
    }

    meta.cwd = cwd_found.clone();
    meta.project_name = cwd_found.as_deref().and_then(extract_project_name);
    meta.topic = first_user_message
        .as_deref()
        .map(|value| truncate_string(value, 50));
    meta.last_prompt = last_user_message
        .as_deref()
        .map(|value| truncate_string(value, 100));
    meta.start_time = shutdown
        .start_time
        .or(earliest_timestamp)
        .unwrap_or(session.last_modified);
    meta.end_time = shutdown
        .end_time
        .or(latest_timestamp)
        .unwrap_or(session.last_modified);

    let mut requests = if shutdown.request_count > 0 {
        build_requests_from_shutdown(session, &assistant_events, &shutdown)
    } else {
        build_requests_from_assistant_events(session, &assistant_events)
    };

    if requests.is_empty() && shutdown.request_count == 0 && !assistant_events.is_empty() {
        requests = build_requests_from_assistant_events(session, &assistant_events);
    }

    if shutdown.input_tokens > 0
        || shutdown.output_tokens > 0
        || shutdown.cache_read_tokens > 0
        || shutdown.cache_create_tokens > 0
    {
        meta.total_input_tokens = shutdown.input_tokens;
        meta.total_output_tokens = shutdown.output_tokens;
        meta.total_cache_read_tokens = shutdown.cache_read_tokens;
        meta.total_cache_create_tokens = shutdown.cache_create_tokens;
    } else {
        for request in &requests {
            meta.total_input_tokens += request.input_tokens;
            meta.total_output_tokens += request.output_tokens;
            meta.total_cache_create_tokens += request.cache_create_tokens;
            meta.total_cache_read_tokens += request.cache_read_tokens;
        }
    }

    if shutdown.request_count > 0 {
        meta.message_count = shutdown.request_count.max(requests.len() as u64);
    } else {
        meta.message_count = requests.len() as u64;
    }

    if let Some(model) = shutdown
        .current_model
        .filter(|value| !value.trim().is_empty())
    {
        models_set.insert(model);
    }
    meta.models = models_set.into_iter().collect();
    meta.message_ids = requests
        .iter()
        .map(|request| request.message_id.clone())
        .collect();

    (meta, requests)
}

fn build_requests_from_shutdown(
    session: &SessionFile,
    assistant_events: &[AssistantEvent],
    shutdown: &ShutdownSummary,
) -> Vec<LocalRequestRecord> {
    let request_count = shutdown.request_count.max(assistant_events.len() as u64);
    if request_count == 0 {
        return Vec::new();
    }

    let event_count = request_count as usize;
    let timestamps = distribute_timestamps(
        shutdown.start_time.unwrap_or(session.last_modified),
        shutdown.end_time.unwrap_or(session.last_modified),
        event_count,
    );
    let input_parts = distribute_u64(shutdown.input_tokens, event_count);
    let output_parts = distribute_u64(shutdown.output_tokens, event_count);
    let cache_read_parts = distribute_u64(shutdown.cache_read_tokens, event_count);
    let cache_create_parts = distribute_u64(shutdown.cache_create_tokens, event_count);
    let reasoning_parts = distribute_u64(shutdown.reasoning_tokens, event_count);

    let model_sequence = build_model_sequence(&shutdown.model_metrics, event_count);
    let mut requests = Vec::with_capacity(event_count);

    for idx in 0..event_count {
        let maybe_event = assistant_events.get(idx);
        let message_id = maybe_event
            .map(|event| event.message_id.clone())
            .unwrap_or_else(|| format!("copilot_cli_shutdown_{}_{idx}", session.session_id));
        let timestamp = maybe_event
            .map(|event| event.timestamp)
            .unwrap_or(timestamps[idx]);
        let model = maybe_event
            .map(|event| event.model.clone())
            .filter(|value| !value.trim().is_empty() && value != "unknown")
            .unwrap_or_else(|| model_sequence[idx].clone());
        let output_tokens = if maybe_event.is_some() && shutdown.output_tokens > 0 {
            output_parts[idx]
        } else {
            maybe_event
                .map(|event| event.output_tokens)
                .filter(|value| *value > 0)
                .unwrap_or(output_parts[idx])
        };
        let input_tokens = input_parts[idx];
        let cache_create_tokens = cache_create_parts[idx];
        let cache_read_tokens = cache_read_parts[idx];
        let reasoning_tokens = reasoning_parts[idx];
        let total_tokens = input_tokens + output_tokens + cache_create_tokens + cache_read_tokens;

        requests.push(LocalRequestRecord {
            session_id: session.session_id.clone(),
            tool: TOOL_COPILOT.to_string(),
            timestamp,
            message_id,
            input_tokens,
            output_tokens,
            reasoning_tokens,
            cache_create_tokens,
            cache_read_tokens,
            total_tokens,
            model,
            is_subagent: false,
            request_key: None,
            source_file_present: None,
        });
    }

    requests.sort_by_key(|request| request.timestamp);
    requests
}

fn build_requests_from_assistant_events(
    session: &SessionFile,
    assistant_events: &[AssistantEvent],
) -> Vec<LocalRequestRecord> {
    assistant_events
        .iter()
        .map(|event| LocalRequestRecord {
            session_id: session.session_id.clone(),
            tool: TOOL_COPILOT.to_string(),
            timestamp: event.timestamp,
            message_id: event.message_id.clone(),
            input_tokens: 0,
            output_tokens: event.output_tokens,
            reasoning_tokens: 0,
            cache_create_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: event.output_tokens,
            model: event.model.clone(),
            is_subagent: false,
            request_key: event.interaction_id.clone(),
            source_file_present: None,
        })
        .collect()
}

fn build_model_sequence(
    model_metrics: &HashMap<String, ModelSummary>,
    count: usize,
) -> Vec<String> {
    let mut sequence = Vec::new();
    let mut models: Vec<(&String, &ModelSummary)> = model_metrics.iter().collect();
    models.sort_by(|a, b| a.0.cmp(b.0));
    for (model_name, summary) in models {
        let repeats = summary.request_count.max(1) as usize;
        for _ in 0..repeats {
            sequence.push(model_name.clone());
        }
    }
    if sequence.is_empty() {
        sequence.push("unknown".to_string());
    }
    while sequence.len() < count {
        let fallback = sequence
            .last()
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        sequence.push(fallback);
    }
    sequence.truncate(count);
    sequence
}

fn distribute_u64(total: u64, buckets: usize) -> Vec<u64> {
    if buckets == 0 {
        return Vec::new();
    }
    let base = total / buckets as u64;
    let remainder = (total % buckets as u64) as usize;
    let mut result = vec![base; buckets];
    for item in result.iter_mut().take(remainder) {
        *item += 1;
    }
    result
}

fn distribute_timestamps(start: i64, end: i64, buckets: usize) -> Vec<i64> {
    if buckets == 0 {
        return Vec::new();
    }
    if buckets == 1 || end <= start {
        return vec![end.max(start); buckets];
    }
    let span = end - start;
    (0..buckets)
        .map(|index| start + ((span * index as i64) / (buckets as i64 - 1)))
        .collect()
}

fn parse_rfc3339_ts(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|datetime| datetime.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn parse_copilot_cli_session_prefers_shutdown_totals() {
        let tmpdir = tempdir().unwrap();
        let events_path = tmpdir.path().join("events.jsonl");
        let mut file = fs::File::create(&events_path).unwrap();

        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "session.start",
                "timestamp": "2026-06-12T12:00:00Z",
                "data": {
                    "sessionId": "session-1",
                    "selectedModel": "gpt-5-mini",
                    "startTime": "2026-06-12T12:00:00Z",
                    "context": { "cwd": "/Users/test/project-a" }
                }
            })
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "user.message",
                "timestamp": "2026-06-12T12:00:05Z",
                "data": {
                    "content": "Help me fix this bug"
                }
            })
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "assistant.message",
                "timestamp": "2026-06-12T12:00:10Z",
                "data": {
                    "messageId": "assistant-1",
                    "interactionId": "interaction-1",
                    "model": "gpt-5-mini",
                    "outputTokens": 90
                }
            })
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "session.shutdown",
                "timestamp": "2026-06-12T12:05:00Z",
                "data": {
                    "sessionStartTime": 1781265600000u64,
                    "currentModel": "gpt-5-mini",
                    "modelMetrics": {
                        "gpt-5-mini": {
                            "requests": { "count": 2 },
                            "usage": {
                                "inputTokens": 120,
                                "outputTokens": 60,
                                "cacheReadTokens": 30,
                                "cacheWriteTokens": 0,
                                "reasoningTokens": 10
                            }
                        }
                    }
                }
            })
        )
        .unwrap();

        let session = SessionFile {
            session_id: "copilot_cli::session-1".to_string(),
            tool: TOOL_COPILOT.to_string(),
            project_path: "session-1".to_string(),
            file_path: events_path.to_string_lossy().to_string(),
            transcript_paths: vec![events_path.to_string_lossy().to_string()],
            file_size: fs::metadata(&events_path).unwrap().len(),
            last_modified: 1_781_265_900,
            fingerprint: 1,
        };

        let (meta, requests) = parse_copilot_cli_session(&session);
        assert_eq!(meta.project_name.as_deref(), Some("project-a"));
        assert_eq!(meta.topic.as_deref(), Some("Help me fix this bug"));
        assert_eq!(meta.total_input_tokens, 120);
        assert_eq!(meta.total_output_tokens, 60);
        assert_eq!(meta.total_cache_read_tokens, 30);
        assert_eq!(meta.message_count, 2);
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests
                .iter()
                .map(|request| request.total_tokens)
                .sum::<u64>(),
            210
        );
        assert_eq!(requests[0].model, "gpt-5-mini");
    }

    #[test]
    fn parse_copilot_cli_session_supports_active_session_without_shutdown() {
        let tmpdir = tempdir().unwrap();
        let events_path = tmpdir.path().join("events.jsonl");
        let mut file = fs::File::create(&events_path).unwrap();

        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "session.start",
                "timestamp": "2026-06-12T12:00:00Z",
                "data": {
                    "sessionId": "session-2",
                    "selectedModel": "gpt-5-mini",
                    "context": { "cwd": "/Users/test/project-b" }
                }
            })
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "assistant.message",
                "timestamp": "2026-06-12T12:00:10Z",
                "data": {
                    "requestId": "request-1",
                    "model": "gpt-5-mini",
                    "outputTokens": 42
                }
            })
        )
        .unwrap();

        let session = SessionFile {
            session_id: "copilot_cli::session-2".to_string(),
            tool: TOOL_COPILOT.to_string(),
            project_path: "session-2".to_string(),
            file_path: events_path.to_string_lossy().to_string(),
            transcript_paths: vec![events_path.to_string_lossy().to_string()],
            file_size: fs::metadata(&events_path).unwrap().len(),
            last_modified: 1_781_265_900,
            fingerprint: 2,
        };

        let (meta, requests) = parse_copilot_cli_session(&session);
        assert_eq!(meta.project_name.as_deref(), Some("project-b"));
        assert_eq!(meta.total_input_tokens, 0);
        assert_eq!(meta.total_output_tokens, 42);
        assert_eq!(meta.message_count, 1);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].message_id, "request-1");
        assert_eq!(requests[0].total_tokens, 42);
    }
}
