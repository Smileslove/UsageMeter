//! GitHub Copilot CLI 本地会话读取模块
//!
//! 优先读取 Copilot CLI `session-store.db` 的逐请求用量；旧版本回退到
//! `~/.copilot/session-state/*/events.jsonl` 的模型汇总和 assistant 输出事件。

use super::constants::TOOL_COPILOT;
use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::shared::{extract_project_name, extract_timestamp, parse_u64_from_value};
use super::source::{ParsedSessionData, SessionSource, SourceSnapshot, SourceUpdateMode};
use rusqlite::{params, Connection, OpenFlags};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

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
        let (meta, requests) = if session.file_path.contains("#store:") {
            parse_copilot_store_session(session)
        } else {
            parse_copilot_cli_session(session)
        };
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

        let producer = read_copilot_session_producer(&events_path);
        if producer
            .as_deref()
            .is_some_and(|producer| !is_copilot_cli_producer(producer))
        {
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

    let cli_session_ids = sessions
        .iter()
        .filter_map(|session| {
            let path = Path::new(&session.file_path);
            read_copilot_session_producer(path)
                .filter(|producer| is_copilot_cli_producer(producer))
                .map(|_| session.project_path.clone())
        })
        .collect::<HashSet<_>>();
    let store_path = root
        .parent()
        .map(|copilot_home| copilot_home.join("session-store.db"));
    if let Some(store_path) = store_path {
        let store_sessions = collect_copilot_store_session_files(&store_path, &cli_session_ids);
        let store_ids = store_sessions
            .iter()
            .map(|session| session.project_path.as_str())
            .collect::<HashSet<_>>();
        sessions.retain(|session| !store_ids.contains(session.project_path.as_str()));
        sessions.extend(store_sessions);
    }

    sessions.sort_by_key(|session| std::cmp::Reverse(session.last_modified));
    sessions
}

fn is_copilot_cli_producer(producer: &str) -> bool {
    let producer = producer.trim().to_ascii_lowercase();
    producer.starts_with("copilot-agent") || producer.starts_with("copilot-cli")
}

fn read_copilot_session_producer(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if event.get("type").and_then(Value::as_str) != Some("session.start") {
            continue;
        }
        return event
            .get("data")
            .and_then(|data| data.get("producer"))
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    None
}

fn open_copilot_store(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_millis(500))?;
    Ok(conn)
}

fn has_copilot_usage_store(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='assistant_usage_events')",
        [],
        |row| row.get(0),
    )
    .unwrap_or(false)
}

fn collect_copilot_store_session_files(
    path: &Path,
    cli_session_ids: &HashSet<String>,
) -> Vec<SessionFile> {
    if cli_session_ids.is_empty() || !path.is_file() {
        return Vec::new();
    }
    let Ok(conn) = open_copilot_store(path) else {
        return Vec::new();
    };
    if !has_copilot_usage_store(&conn) {
        return Vec::new();
    }
    let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT session_id FROM assistant_usage_events WHERE session_id IS NOT NULL AND session_id != ''",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) else {
        return Vec::new();
    };
    let Ok(metadata) = fs::metadata(path) else {
        return Vec::new();
    };
    let file_size = metadata.len();
    let last_modified = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let file_path = path.to_string_lossy().to_string();
    rows.filter_map(Result::ok)
        .filter(|session_id| cli_session_ids.contains(session_id))
        .map(|session_id| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            file_path.hash(&mut hasher);
            session_id.hash(&mut hasher);
            file_size.hash(&mut hasher);
            last_modified.hash(&mut hasher);
            SessionFile {
                session_id: format!("copilot_cli::{session_id}"),
                tool: TOOL_COPILOT.to_string(),
                project_path: session_id.clone(),
                file_path: format!("{file_path}#store:{session_id}"),
                transcript_paths: vec![file_path.clone()],
                file_size,
                last_modified,
                fingerprint: hasher.finish(),
            }
        })
        .collect()
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
        explicit_estimated_cost: None,
        scope: None,
        ..Default::default()
    };

    let file = match fs::File::open(&session.file_path) {
        Ok(file) => file,
        Err(_) => return (meta, Vec::new()),
    };

    let mut cwd_found: Option<String> = None;
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
            "user.message" => {}
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
                        let input_raw = usage
                            .get("inputTokens")
                            .and_then(parse_u64_from_value)
                            .unwrap_or(0);
                        let cache_read_tokens = usage
                            .get("cacheReadTokens")
                            .and_then(parse_u64_from_value)
                            .unwrap_or(0)
                            .min(input_raw);
                        let cache_create_tokens = usage
                            .get("cacheWriteTokens")
                            .and_then(parse_u64_from_value)
                            .unwrap_or(0)
                            .min(input_raw.saturating_sub(cache_read_tokens));
                        let output_tokens = usage
                            .get("outputTokens")
                            .and_then(parse_u64_from_value)
                            .unwrap_or(0);
                        let summary = ModelSummary {
                            request_count: requests,
                            input_tokens: input_raw
                                .saturating_sub(cache_read_tokens)
                                .saturating_sub(cache_create_tokens),
                            output_tokens,
                            cache_read_tokens,
                            cache_create_tokens,
                            reasoning_tokens: usage
                                .get("reasoningTokens")
                                .and_then(parse_u64_from_value)
                                .unwrap_or(0)
                                .min(output_tokens),
                        };
                        if summary.request_count > 0
                            || summary.input_tokens > 0
                            || summary.output_tokens > 0
                            || summary.reasoning_tokens > 0
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
                    shutdown.cache_create_tokens = token_details
                        .get("cache_write")
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
    meta.start_time = shutdown
        .start_time
        .or(earliest_timestamp)
        .unwrap_or(session.last_modified);
    meta.end_time = shutdown
        .end_time
        .or(latest_timestamp)
        .unwrap_or(session.last_modified);

    let shutdown_has_usage = shutdown.input_tokens > 0
        || shutdown.output_tokens > 0
        || shutdown.cache_read_tokens > 0
        || shutdown.cache_create_tokens > 0;
    let requests = if shutdown.request_count > 0 || shutdown_has_usage {
        build_requests_from_shutdown(session, &assistant_events, &shutdown)
    } else {
        build_requests_from_assistant_events(session, &assistant_events)
    };

    if shutdown_has_usage {
        meta.total_input_tokens = shutdown.input_tokens;
        meta.total_output_tokens = shutdown.output_tokens;
        meta.total_cache_read_tokens = shutdown.cache_read_tokens;
        meta.total_reasoning_tokens = shutdown.reasoning_tokens;
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
        meta.message_count = shutdown.request_count;
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

fn parse_copilot_store_session(session: &SessionFile) -> (SessionMeta, Vec<LocalRequestRecord>) {
    let Some((db_path, session_id)) = session.file_path.split_once("#store:") else {
        return (SessionMeta::default(), Vec::new());
    };
    let Ok(conn) = open_copilot_store(Path::new(db_path)) else {
        return (SessionMeta::default(), Vec::new());
    };
    if !has_copilot_usage_store(&conn) {
        return (SessionMeta::default(), Vec::new());
    }
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, model, input_tokens, output_tokens, cache_read_tokens,
                cache_write_tokens, reasoning_tokens, token_details_json, created_at
         FROM assistant_usage_events
         WHERE session_id = ?1
         ORDER BY created_at ASC, id ASC",
    ) else {
        return (SessionMeta::default(), Vec::new());
    };
    let Ok(rows) = stmt.query_map(params![session_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<i64>>(5)?,
            row.get::<_, Option<i64>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<String>>(8)?,
        ))
    }) else {
        return (SessionMeta::default(), Vec::new());
    };

    let mut requests = Vec::new();
    let mut models = BTreeSet::new();
    for row in rows.flatten() {
        let (
            id,
            model,
            raw_input,
            raw_output,
            raw_cache_read,
            raw_cache_write,
            raw_reasoning,
            details,
            created_at,
        ) = row;
        let input_raw = nonnegative(raw_input);
        let output = nonnegative(raw_output);
        let cache_read_raw = nonnegative(raw_cache_read);
        let cache_write_raw = nonnegative(raw_cache_write);
        let (input, cache_read, cache_write) = normalize_copilot_store_input(
            input_raw,
            cache_read_raw,
            cache_write_raw,
            details.as_deref(),
        );
        let reasoning = nonnegative(raw_reasoning).min(output);
        let timestamp = created_at
            .as_deref()
            .and_then(parse_copilot_timestamp)
            .unwrap_or(session.last_modified);
        let model = model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .unwrap_or("unknown")
            .to_string();
        if model != "unknown" {
            models.insert(model.clone());
        }
        let total_tokens = input + cache_read + cache_write + output;
        requests.push(LocalRequestRecord {
            provider_evidence: None,
            session_id: session.session_id.clone(),
            tool: TOOL_COPILOT.to_string(),
            timestamp,
            message_id: format!("copilot_store_{id}"),
            input_tokens: input,
            output_tokens: output,
            reasoning_tokens: reasoning,
            cache_create_tokens: cache_write,
            cache_read_tokens: cache_read,
            total_tokens,
            request_count: 1,
            model,
            is_subagent: false,
            request_key: None,
            explicit_estimated_cost: None,
            source_file_present: None,
        });
    }

    let total_input_tokens = requests.iter().map(|request| request.input_tokens).sum();
    let total_output_tokens = requests.iter().map(|request| request.output_tokens).sum();
    let total_cache_create_tokens = requests
        .iter()
        .map(|request| request.cache_create_tokens)
        .sum();
    let total_cache_read_tokens = requests
        .iter()
        .map(|request| request.cache_read_tokens)
        .sum();
    let total_reasoning_tokens = requests
        .iter()
        .map(|request| request.reasoning_tokens)
        .sum();
    let cwd = read_copilot_store_session_cwd(Path::new(db_path), session_id);
    let start_time = requests
        .iter()
        .map(|request| request.timestamp)
        .min()
        .unwrap_or(session.last_modified);
    let end_time = requests
        .iter()
        .map(|request| request.timestamp)
        .max()
        .unwrap_or(session.last_modified);
    let meta = SessionMeta {
        session_id: session.session_id.clone(),
        tool: TOOL_COPILOT.to_string(),
        cwd: cwd.clone(),
        project_name: cwd.as_deref().and_then(extract_project_name),
        topic: None,
        last_prompt: None,
        session_name: None,
        file_path: db_path.to_string(),
        file_size: session.file_size,
        last_modified: session.last_modified.max(end_time),
        total_input_tokens,
        total_output_tokens,
        total_cache_create_tokens,
        total_cache_read_tokens,
        total_reasoning_tokens,
        models: models.into_iter().collect(),
        message_count: requests.len() as u64,
        start_time,
        end_time,
        source: "copilot_cli_session_store".to_string(),
        message_ids: requests
            .iter()
            .map(|request| request.message_id.clone())
            .collect(),
        ..Default::default()
    };
    (meta, requests)
}

fn read_copilot_store_session_cwd(db_path: &Path, session_id: &str) -> Option<String> {
    let events_path = db_path
        .parent()?
        .join("session-state")
        .join(session_id)
        .join("events.jsonl");
    let file = fs::File::open(events_path).ok()?;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if event.get("type").and_then(Value::as_str) != Some("session.start") {
            continue;
        }
        return event
            .get("data")
            .and_then(|data| data.get("context"))
            .and_then(|context| context.get("cwd"))
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    None
}

fn nonnegative(value: Option<i64>) -> u64 {
    value.unwrap_or(0).max(0) as u64
}

fn normalize_copilot_store_input(
    input_raw: u64,
    cache_read_raw: u64,
    cache_write_raw: u64,
    token_details: Option<&str>,
) -> (u64, u64, u64) {
    if let Some((input, cache_read, cache_write)) = token_details
        .and_then(parse_copilot_store_token_details)
        .filter(|(input, cache_read, cache_write)| {
            input
                .saturating_add(*cache_read)
                .saturating_add(*cache_write)
                == input_raw
        })
    {
        return (input, cache_read, cache_write);
    }
    let cache_read = cache_read_raw.min(input_raw);
    let cache_write = cache_write_raw.min(input_raw.saturating_sub(cache_read));
    (
        input_raw
            .saturating_sub(cache_read)
            .saturating_sub(cache_write),
        cache_read,
        cache_write,
    )
}

fn parse_copilot_store_token_details(raw: &str) -> Option<(u64, u64, u64)> {
    let entries = serde_json::from_str::<Value>(raw).ok()?;
    let entries = entries.as_array()?;
    let mut input = 0u64;
    let mut cache_read = 0u64;
    let mut cache_write = 0u64;
    let mut found = false;
    for entry in entries {
        let count = entry
            .get("tokenCount")
            .and_then(parse_u64_from_value)
            .unwrap_or(0);
        match entry.get("tokenType").and_then(Value::as_str) {
            Some("input") => {
                input = input.saturating_add(count);
                found = true;
            }
            Some("cache_read") => {
                cache_read = cache_read.saturating_add(count);
                found = true;
            }
            Some("cache_write") => {
                cache_write = cache_write.saturating_add(count);
                found = true;
            }
            _ => {}
        }
    }
    found.then_some((input, cache_read, cache_write))
}

fn parse_copilot_timestamp(raw: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|timestamp| timestamp.timestamp())
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|timestamp| timestamp.and_utc().timestamp())
        })
}

fn build_requests_from_shutdown(
    session: &SessionFile,
    assistant_events: &[AssistantEvent],
    shutdown: &ShutdownSummary,
) -> Vec<LocalRequestRecord> {
    let timestamp = shutdown.end_time.unwrap_or(session.last_modified);
    let mut summaries: Vec<(&str, &ModelSummary)> = shutdown
        .model_metrics
        .iter()
        .map(|(model, summary)| (model.as_str(), summary))
        .collect();
    summaries.sort_by(|left, right| left.0.cmp(right.0));

    if summaries.is_empty() {
        if shutdown.input_tokens == 0
            && shutdown.output_tokens == 0
            && shutdown.cache_read_tokens == 0
            && shutdown.cache_create_tokens == 0
        {
            return build_requests_from_assistant_events(session, assistant_events);
        }
        let model = shutdown
            .current_model
            .as_deref()
            .filter(|model| !model.trim().is_empty())
            .unwrap_or("unknown");
        return vec![build_shutdown_summary_request(
            session,
            model,
            shutdown.request_count,
            shutdown.input_tokens,
            shutdown.output_tokens,
            shutdown.cache_read_tokens,
            shutdown.cache_create_tokens,
            shutdown.reasoning_tokens,
            timestamp,
        )];
    }

    summaries
        .into_iter()
        .map(|(model, summary)| {
            build_shutdown_summary_request(
                session,
                model,
                summary.request_count,
                summary.input_tokens,
                summary.output_tokens,
                summary.cache_read_tokens,
                summary.cache_create_tokens,
                summary.reasoning_tokens,
                timestamp,
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn build_shutdown_summary_request(
    session: &SessionFile,
    model: &str,
    request_count: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_create_tokens: u64,
    reasoning_tokens: u64,
    timestamp: i64,
) -> LocalRequestRecord {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    session.session_id.hash(&mut hasher);
    model.hash(&mut hasher);
    let message_id = format!("copilot_cli_summary_{:016x}", hasher.finish());
    LocalRequestRecord {
        provider_evidence: None,
        session_id: session.session_id.clone(),
        tool: TOOL_COPILOT.to_string(),
        timestamp,
        message_id,
        input_tokens,
        output_tokens,
        reasoning_tokens,
        cache_create_tokens,
        cache_read_tokens,
        total_tokens: input_tokens + output_tokens + cache_create_tokens + cache_read_tokens,
        request_count,
        model: model.to_string(),
        is_subagent: false,
        request_key: None,
        explicit_estimated_cost: None,
        source_file_present: None,
    }
}

fn build_requests_from_assistant_events(
    session: &SessionFile,
    assistant_events: &[AssistantEvent],
) -> Vec<LocalRequestRecord> {
    assistant_events
        .iter()
        .map(|event| LocalRequestRecord {
            provider_evidence: None,
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
            request_count: 1,
            model: event.model.clone(),
            is_subagent: false,
            request_key: event.interaction_id.clone(),
            explicit_estimated_cost: None,
            source_file_present: None,
        })
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
        assert_eq!(meta.topic, None);
        assert_eq!(meta.last_prompt, None);
        assert_eq!(meta.total_input_tokens, 90);
        assert_eq!(meta.total_output_tokens, 60);
        assert_eq!(meta.total_cache_read_tokens, 30);
        assert_eq!(meta.total_reasoning_tokens, 10);
        assert_eq!(meta.message_count, 2);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].request_count, 2);
        assert_eq!(requests[0].input_tokens, 90);
        assert_eq!(requests[0].cache_read_tokens, 30);
        assert_eq!(requests[0].total_tokens, 180);
        assert_eq!(requests[0].model, "gpt-5-mini");
    }

    #[test]
    fn parse_copilot_store_session_preserves_per_request_token_details() {
        let temp = tempdir().unwrap();
        let copilot_home = temp.path().join(".copilot");
        let db_path = copilot_home.join("session-store.db");
        let events_dir = copilot_home.join("session-state").join("session-1");
        fs::create_dir_all(&events_dir).unwrap();
        fs::write(
            events_dir.join("events.jsonl"),
            "{\"type\":\"session.start\",\"data\":{\"context\":{\"cwd\":\"/Users/test/project-a\"}}}\n",
        )
        .unwrap();
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE assistant_usage_events (
                id INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL,
                model TEXT NOT NULL,
                input_tokens INTEGER,
                output_tokens INTEGER,
                cache_read_tokens INTEGER,
                cache_write_tokens INTEGER,
                reasoning_tokens INTEGER,
                token_details_json TEXT,
                created_at TEXT
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO assistant_usage_events VALUES (
                1, 'session-1', 'gpt-5-mini', 120, 60, 30, 0, 10,
                ?1, '2026-06-12T12:00:10Z'
            )",
            [r#"[{"tokenType":"input","tokenCount":90},{"tokenType":"cache_read","tokenCount":30},{"tokenType":"output","tokenCount":60}]"#],
        )
        .unwrap();
        drop(conn);

        let db_file = db_path.to_string_lossy();
        let session = SessionFile {
            session_id: "copilot_cli::session-1".to_string(),
            tool: TOOL_COPILOT.to_string(),
            project_path: "session-1".to_string(),
            file_path: format!("{db_file}#store:session-1"),
            transcript_paths: vec![db_file.to_string()],
            file_size: fs::metadata(&db_path).unwrap().len(),
            last_modified: 1_781_265_900,
            fingerprint: 1,
        };
        let (meta, requests) = parse_copilot_store_session(&session);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].input_tokens, 90);
        assert_eq!(requests[0].cache_read_tokens, 30);
        assert_eq!(requests[0].output_tokens, 60);
        assert_eq!(requests[0].reasoning_tokens, 10);
        assert_eq!(requests[0].total_tokens, 180);
        assert_eq!(meta.total_reasoning_tokens, 10);
        assert_eq!(meta.cwd.as_deref(), Some("/Users/test/project-a"));
        assert_eq!(meta.project_name.as_deref(), Some("project-a"));
    }

    #[test]
    fn copilot_producer_filter_excludes_non_cli_clients() {
        assert!(is_copilot_cli_producer("copilot-agent"));
        assert!(is_copilot_cli_producer("copilot-cli"));
        assert!(!is_copilot_cli_producer("copilot-chat"));
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
