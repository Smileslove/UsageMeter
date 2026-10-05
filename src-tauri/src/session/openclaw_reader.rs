//! OpenClaw 本地 transcript 读取模块
//!
//! 扫描 `~/.openclaw/agents/*/sessions/*.jsonl*`（兼容历史目录），解析会话摘要与请求事实。

use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::shared::{
    extract_project_name, extract_timestamp, parse_u64_from_value, truncate_string,
};
use super::source::{ParsedSessionData, SessionSource, SourceSnapshot, SourceUpdateMode};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

const OPENCLAW_SQLITE_LOCATOR_PREFIX: &str = "openclaw-sqlite:";

pub(super) struct OpenClawSource;

impl SessionSource for OpenClawSource {
    fn tool_id(&self) -> &'static str {
        super::constants::TOOL_OPENCLAW
    }

    fn scan(&self) -> SourceSnapshot {
        let mut sessions = Vec::new();
        let mut seen_roots: HashSet<PathBuf> = HashSet::new();

        for root in openclaw_agent_roots() {
            if !root.exists() {
                continue;
            }
            let canonical = fs::canonicalize(&root).unwrap_or(root.clone());
            if seen_roots.insert(canonical) {
                sessions.extend(collect_openclaw_session_files_from_root(&root));
            }
        }

        #[cfg(windows)]
        if let Some(cfg) = super::wsl::scan_config_if_enabled() {
            for root in super::wsl::openclaw_agent_roots(&cfg) {
                if !root.exists() {
                    continue;
                }
                let canonical = fs::canonicalize(&root).unwrap_or(root.clone());
                if seen_roots.insert(canonical) {
                    sessions.extend(collect_openclaw_session_files_from_root(&root));
                }
            }
        }

        sessions.sort_by_key(|session| std::cmp::Reverse(session.last_modified));

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for session in &sessions {
            session.session_id.hash(&mut hasher);
            session.fingerprint.hash(&mut hasher);
        }

        SourceSnapshot {
            source_id: self.tool_id(),
            update_mode: SourceUpdateMode::PerSession,
            sessions,
            scan_fingerprint: hasher.finish(),
        }
    }

    fn parse(&self, session: &SessionFile) -> Result<ParsedSessionData, String> {
        let (meta, requests) = parse_openclaw_session_file(session);
        Ok(ParsedSessionData { meta, requests })
    }
}

fn openclaw_agent_roots() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    [".openclaw", ".clawdbot", ".moltbot", ".moldbot"]
        .into_iter()
        .map(|dir| home.join(dir).join("agents"))
        .collect()
}

pub(super) fn collect_openclaw_session_files_from_root(root: &Path) -> Vec<SessionFile> {
    #[derive(Default)]
    struct SessionGroupBuilder {
        project_path: String,
        session_id: String,
        primary_file_path: Option<String>,
        transcript_paths: Vec<String>,
        file_size: u64,
        last_modified: i64,
        fingerprint: u64,
    }

    let Ok(agent_entries) = fs::read_dir(root) else {
        return Vec::new();
    };

    let mut groups: HashMap<String, SessionGroupBuilder> = HashMap::new();

    for agent_entry in agent_entries.flatten() {
        let agent_path = agent_entry.path();
        if !agent_path.is_dir() {
            continue;
        }

        let Some(agent_id) = agent_path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let sessions_dir = agent_path.join("sessions");
        for path in collect_openclaw_jsonl_files(&sessions_dir) {
            let Some(raw_session_id) = derive_openclaw_raw_session_id(&path) else {
                continue;
            };

            let metadata = fs::metadata(&path).ok();
            let file_size = metadata.as_ref().map(|value| value.len()).unwrap_or(0);
            if file_size < 2 {
                continue;
            }

            let last_modified = metadata
                .and_then(|value| value.modified().ok())
                .map(|time| {
                    time.duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64
                })
                .unwrap_or(0);

            let unique_id = format!(
                "{}::{}::{}",
                super::constants::TOOL_OPENCLAW,
                agent_id,
                raw_session_id
            );
            let group = groups
                .entry(unique_id.clone())
                .or_insert_with(|| SessionGroupBuilder {
                    project_path: agent_id.to_string(),
                    session_id: unique_id.clone(),
                    ..Default::default()
                });

            let path_string = path.to_string_lossy().to_string();
            if is_openclaw_primary_transcript(&path) {
                group.primary_file_path = Some(path_string.clone());
            }
            group.transcript_paths.push(path_string.clone());
            group.file_size += file_size;
            group.last_modified = group.last_modified.max(last_modified);

            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            path_string.hash(&mut hasher);
            file_size.hash(&mut hasher);
            last_modified.hash(&mut hasher);
            group.fingerprint ^= hasher.finish();
        }

        let db_path = agent_path.join("agent").join("openclaw-agent.sqlite");
        for raw_session_id in openclaw_sqlite_session_ids(&db_path) {
            let unique_id = format!(
                "{}::{}::{}",
                super::constants::TOOL_OPENCLAW,
                agent_id,
                raw_session_id
            );
            let locator = openclaw_sqlite_locator(&db_path, &raw_session_id);
            let group = groups
                .entry(unique_id.clone())
                .or_insert_with(|| SessionGroupBuilder {
                    project_path: agent_id.to_string(),
                    session_id: unique_id,
                    ..Default::default()
                });
            if !group.transcript_paths.contains(&locator) {
                group.transcript_paths.push(locator.clone());
                let db_metadata = fs::metadata(&db_path).ok();
                let wal_path = db_path.with_file_name(format!(
                    "{}-wal",
                    db_path.file_name().unwrap_or_default().to_string_lossy()
                ));
                let wal_metadata = fs::metadata(&wal_path).ok();
                let db_size = db_metadata
                    .as_ref()
                    .map(|metadata| metadata.len())
                    .unwrap_or(0)
                    .saturating_add(
                        wal_metadata
                            .as_ref()
                            .map(|metadata| metadata.len())
                            .unwrap_or(0),
                    );
                let db_mtime = db_metadata
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_secs() as i64)
                    .unwrap_or(0)
                    .max(
                        wal_metadata
                            .as_ref()
                            .and_then(|metadata| metadata.modified().ok())
                            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|duration| duration.as_secs() as i64)
                            .unwrap_or(0),
                    );
                group.file_size = group.file_size.saturating_add(db_size);
                group.last_modified = group.last_modified.max(db_mtime);
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                locator.hash(&mut hasher);
                db_size.hash(&mut hasher);
                db_mtime.hash(&mut hasher);
                wal_metadata
                    .as_ref()
                    .map(|metadata| metadata.len())
                    .unwrap_or(0)
                    .hash(&mut hasher);
                group.fingerprint ^= hasher.finish();
            }
        }
    }

    groups
        .into_values()
        .map(|mut group| {
            group.transcript_paths.sort();
            SessionFile {
                session_id: group.session_id,
                tool: super::constants::TOOL_OPENCLAW.to_string(),
                project_path: group.project_path,
                file_path: group
                    .primary_file_path
                    .or_else(|| {
                        group.transcript_paths.first().map(|path| {
                            parse_openclaw_sqlite_locator(path)
                                .map(|(db_path, _)| db_path.to_string_lossy().to_string())
                                .unwrap_or_else(|| path.clone())
                        })
                    })
                    .unwrap_or_default(),
                transcript_paths: group.transcript_paths,
                file_size: group.file_size,
                last_modified: group.last_modified,
                fingerprint: group.fingerprint,
            }
        })
        .filter(|session| !session.file_path.is_empty() && !session.transcript_paths.is_empty())
        .collect()
}

pub(super) fn parse_openclaw_session_file(
    session: &SessionFile,
) -> (SessionMeta, Vec<LocalRequestRecord>) {
    let mut meta = SessionMeta {
        session_id: session.session_id.clone(),
        tool: session.tool.clone(),
        cwd: None,
        project_name: None,
        topic: None,
        last_prompt: None,
        session_name: openclaw_display_name_for_session(session),
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
        source: "openclaw_session".to_string(),
        message_ids: Vec::new(),
        explicit_estimated_cost: None,
        scope: None,
        ..Default::default()
    };

    let mut first_user_message: Option<String> = None;
    let mut last_user_message: Option<String> = None;
    let mut cwd_found: Option<String> = None;
    let mut models_set: BTreeSet<String> = BTreeSet::new();
    let mut earliest_timestamp: Option<i64> = None;
    let mut latest_timestamp: Option<i64> = None;
    let mut requests = Vec::new();

    let mut transcript_paths = session.transcript_paths.clone();
    transcript_paths.sort_by_key(|path| {
        if path.starts_with(OPENCLAW_SQLITE_LOCATOR_PREFIX) {
            0
        } else if path == &session.file_path {
            1
        } else if is_openclaw_primary_transcript(Path::new(path)) {
            2
        } else {
            3
        }
    });

    let mut seen_event_ids = HashSet::new();
    for transcript_path in &transcript_paths {
        let Some(contents) = read_openclaw_transcript(transcript_path) else {
            continue;
        };
        let file_timestamp =
            openclaw_path_timestamp(transcript_path).unwrap_or(session.last_modified);
        let mut current_model: Option<String> = None;
        let transcript_token = openclaw_transcript_token(transcript_path);

        for (line_idx, line) in contents.lines().enumerate() {
            let Ok(json) = serde_json::from_str::<Value>(line) else {
                continue;
            };

            let timestamp = extract_openclaw_timestamp(&json).unwrap_or(file_timestamp);
            earliest_timestamp = Some(
                earliest_timestamp
                    .map(|current| current.min(timestamp))
                    .unwrap_or(timestamp),
            );
            latest_timestamp = Some(
                latest_timestamp
                    .map(|current| current.max(timestamp))
                    .unwrap_or(timestamp),
            );

            let event_type = json
                .get("type")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            match event_type {
                "session" => {
                    if cwd_found.is_none() {
                        cwd_found = json
                            .get("cwd")
                            .and_then(|value| value.as_str())
                            .map(|value| value.to_string());
                    }
                }
                "model_change" => {
                    if let Some(model) = json
                        .get("modelId")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.trim().is_empty())
                    {
                        current_model = Some(model.to_string());
                    }
                }
                "custom" => {
                    if json.get("customType").and_then(|value| value.as_str())
                        != Some("model-snapshot")
                    {
                        continue;
                    }
                    if let Some(model) = json
                        .get("data")
                        .and_then(|value| value.get("modelId"))
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.trim().is_empty())
                    {
                        current_model = Some(model.to_string());
                    }
                }
                "message" => {
                    let Some(message) = json.get("message") else {
                        continue;
                    };
                    let role = message
                        .get("role")
                        .and_then(|value| value.as_str())
                        .unwrap_or("");

                    if role == "user" {
                        if let Some(text) = extract_openclaw_message_text(message) {
                            if first_user_message.is_none() {
                                first_user_message = Some(text.clone());
                            }
                            last_user_message = Some(text);
                        }
                        continue;
                    }

                    if role != "assistant" {
                        continue;
                    }

                    let usage = message.get("usage").or_else(|| json.get("usage"));
                    let Some(usage) = usage else {
                        continue;
                    };

                    let cache_read =
                        parse_u64_from_value(usage.get("cacheRead").unwrap_or(&Value::Null))
                            .unwrap_or(0);
                    let cache_create =
                        parse_u64_from_value(usage.get("cacheWrite").unwrap_or(&Value::Null))
                            .unwrap_or(0);
                    let raw_input =
                        parse_u64_from_value(usage.get("input").unwrap_or(&Value::Null))
                            .unwrap_or(0);
                    let output = parse_u64_from_value(usage.get("output").unwrap_or(&Value::Null))
                        .unwrap_or(0);
                    let input = raw_input;
                    let computed_total = input
                        .saturating_add(cache_create)
                        .saturating_add(cache_read)
                        .saturating_add(output);
                    let explicit_total =
                        parse_u64_from_value(usage.get("totalTokens").unwrap_or(&Value::Null))
                            .unwrap_or(0);
                    let total_tokens = computed_total.max(explicit_total);
                    if total_tokens == 0 {
                        continue;
                    }

                    let model = message
                        .get("model")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.trim().is_empty())
                        .map(|value| value.to_string())
                        .or_else(|| current_model.clone())
                        .unwrap_or_else(|| "unknown".to_string());
                    models_set.insert(model.clone());
                    current_model = Some(model.clone());

                    let event_id = json
                        .get("id")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.trim().is_empty());
                    if let Some(event_id) = event_id {
                        if !seen_event_ids.insert(event_id.to_string()) {
                            continue;
                        }
                    }
                    let message_id = event_id
                        .map(str::to_string)
                        .or_else(|| {
                            message
                                .get("id")
                                .and_then(|value| value.as_str())
                                .filter(|value| !value.trim().is_empty())
                                .map(|value| value.to_string())
                        })
                        .unwrap_or_else(|| {
                            format!(
                                "openclaw:{}:{}:{}",
                                session.session_id, transcript_token, line_idx
                            )
                        });

                    meta.total_input_tokens += input;
                    meta.total_output_tokens += output;
                    meta.total_cache_create_tokens += cache_create;
                    meta.total_cache_read_tokens += cache_read;
                    meta.message_ids.push(message_id.clone());

                    requests.push(LocalRequestRecord {
                        provider_evidence: None,
                        session_id: session.session_id.clone(),
                        tool: session.tool.clone(),
                        timestamp,
                        message_id,
                        input_tokens: input,
                        output_tokens: output,
                        reasoning_tokens: 0,
                        cache_create_tokens: cache_create,
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
                _ => {}
            }
        }
    }

    requests.sort_by_key(|request| request.timestamp);
    meta.cwd = cwd_found.clone();
    meta.project_name = cwd_found
        .as_deref()
        .and_then(extract_project_name)
        .or_else(|| {
            (!session.project_path.trim().is_empty()).then(|| session.project_path.clone())
        });
    meta.topic = first_user_message
        .as_deref()
        .map(|value| truncate_string(value, 50));
    meta.last_prompt = last_user_message
        .as_deref()
        .map(|value| truncate_string(value, 100));
    meta.models = models_set.into_iter().collect();
    meta.message_count = requests.len() as u64;
    meta.start_time = earliest_timestamp.unwrap_or(session.last_modified);
    meta.end_time = latest_timestamp.unwrap_or(session.last_modified);

    (meta, requests)
}

fn collect_openclaw_jsonl_files(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .map(is_openclaw_session_file_name)
                .unwrap_or(false)
        })
        .collect()
}

fn is_openclaw_session_file_name(file_name: &str) -> bool {
    file_name.ends_with(".jsonl")
        || file_name.contains(".jsonl.deleted.")
        || file_name.contains(".jsonl.reset.")
}

fn is_openclaw_primary_transcript(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.ends_with(".jsonl"))
        .unwrap_or(false)
}

fn derive_openclaw_raw_session_id(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .and_then(|value| value.split_once(".jsonl").map(|(raw, _)| raw.to_string()))
        .filter(|value| !value.trim().is_empty())
}

fn openclaw_display_name_for_session(session: &SessionFile) -> Option<String> {
    let sessions_dir = Path::new(&session.file_path).parent()?;
    let raw_session_id = derive_openclaw_raw_session_id(Path::new(&session.file_path))?;
    load_openclaw_display_names(sessions_dir)
        .remove(&raw_session_id)
        .filter(|value| !value.trim().is_empty())
}

fn load_openclaw_display_names(sessions_dir: &Path) -> HashMap<String, String> {
    let content = match fs::read_to_string(sessions_dir.join("sessions.json")) {
        Ok(content) => content,
        Err(_) => return HashMap::new(),
    };
    let Ok(index) = serde_json::from_str::<serde_json::Map<String, Value>>(&content) else {
        return HashMap::new();
    };

    let mut display_names = HashMap::new();
    for entry in index.values() {
        let Some(session_id) = entry.get("sessionId").and_then(|value| value.as_str()) else {
            continue;
        };
        let Some(display_name) = entry.get("displayName").and_then(|value| value.as_str()) else {
            continue;
        };
        if !display_name.trim().is_empty() {
            display_names.insert(session_id.to_string(), display_name.to_string());
        }
    }
    display_names
}

fn extract_openclaw_timestamp(json: &Value) -> Option<i64> {
    extract_timestamp(json).or_else(|| {
        json.get("message").and_then(|message| {
            let ts = message.get("timestamp")?;
            if let Some(num) = ts.as_u64() {
                Some(if num > 10_000_000_000 {
                    (num / 1000) as i64
                } else {
                    num as i64
                })
            } else if let Some(text) = ts.as_str() {
                chrono::DateTime::parse_from_rfc3339(text)
                    .ok()
                    .map(|value| value.timestamp())
            } else {
                None
            }
        })
    })
}

fn extract_openclaw_message_text(message: &Value) -> Option<String> {
    if let Some(content) = message.get("content").and_then(|value| value.as_str()) {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    let content_items = message.get("content")?.as_array()?;
    for item in content_items {
        if item.get("type").and_then(|value| value.as_str()) == Some("text") {
            if let Some(text) = item.get("text").and_then(|value| value.as_str()) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }

    None
}

fn openclaw_transcript_token(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.replace('.', "_"))
        .unwrap_or_else(|| "session".to_string())
}

fn openclaw_sqlite_locator(db_path: &Path, session_id: &str) -> String {
    format!(
        "{}{}",
        OPENCLAW_SQLITE_LOCATOR_PREFIX,
        serde_json::json!([db_path.to_string_lossy(), session_id])
    )
}

fn parse_openclaw_sqlite_locator(locator: &str) -> Option<(PathBuf, String)> {
    let value: Value =
        serde_json::from_str(locator.strip_prefix(OPENCLAW_SQLITE_LOCATOR_PREFIX)?).ok()?;
    Some((
        PathBuf::from(value.get(0)?.as_str()?),
        value.get(1)?.as_str()?.to_string(),
    ))
}

fn openclaw_sqlite_session_ids(db_path: &Path) -> Vec<String> {
    let Ok(conn) = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    let Ok(mut statement) =
        conn.prepare("SELECT DISTINCT session_id FROM transcript_events ORDER BY session_id")
    else {
        return Vec::new();
    };
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|id| !id.trim().is_empty())
        .collect()
}

fn read_openclaw_transcript(locator: &str) -> Option<String> {
    if let Some((db_path, session_id)) = parse_openclaw_sqlite_locator(locator) {
        return read_openclaw_sqlite_session(&db_path, &session_id);
    }
    let path = Path::new(locator);
    let file = fs::File::open(path).ok()?;
    if path.extension().and_then(|value| value.to_str()) == Some("zst") {
        let mut decoder = zstd::stream::read::Decoder::new(file).ok()?;
        decoder.window_log_max(26).ok()?;
        let mut contents = String::new();
        decoder
            .take(64 * 1024 * 1024 + 1)
            .read_to_string(&mut contents)
            .ok()?;
        (contents.len() <= 64 * 1024 * 1024).then_some(contents)
    } else {
        fs::read_to_string(path).ok()
    }
}

fn openclaw_path_timestamp(locator: &str) -> Option<i64> {
    let path = parse_openclaw_sqlite_locator(locator)
        .map(|(path, _)| path)
        .unwrap_or_else(|| PathBuf::from(locator));
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

fn read_openclaw_sqlite_session(db_path: &Path, session_id: &str) -> Option<String> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    conn.busy_timeout(Duration::from_millis(500)).ok()?;
    let has_compressed = conn
        .prepare("SELECT event_zstd, event_utf8_bytes FROM transcript_events LIMIT 0")
        .is_ok();
    let has_created_at = conn
        .prepare("SELECT created_at FROM transcript_events LIMIT 0")
        .is_ok();
    let query = if has_compressed {
        if has_created_at {
            "SELECT event_json, event_zstd, event_utf8_bytes, created_at FROM transcript_events WHERE session_id = ?1 ORDER BY seq"
        } else {
            "SELECT event_json, event_zstd, event_utf8_bytes, NULL FROM transcript_events WHERE session_id = ?1 ORDER BY seq"
        }
    } else {
        if has_created_at {
            "SELECT event_json, NULL, NULL, created_at FROM transcript_events WHERE session_id = ?1 ORDER BY seq"
        } else {
            "SELECT event_json, NULL, NULL, NULL FROM transcript_events WHERE session_id = ?1 ORDER BY seq"
        }
    };
    let mut statement = conn.prepare(query).ok()?;
    let rows = statement
        .query_map([session_id], |row| {
            let text: Option<String> = row.get(0)?;
            if let Some(text) = text {
                return Ok(Some(text));
            }
            let blob: Option<Vec<u8>> = row.get(1)?;
            let raw_bytes: Option<i64> = row.get(2)?;
            let decoded = match (blob, raw_bytes) {
                (Some(blob), Some(size)) if (0..=64 * 1024 * 1024).contains(&size) => {
                    zstd::bulk::decompress(&blob, size as usize)
                        .ok()
                        .filter(|bytes| bytes.len() == size as usize)
                        .and_then(|bytes| String::from_utf8(bytes).ok())
                }
                _ => None,
            };
            let created_at: Option<f64> = row.get(3)?;
            Ok(decoded.map(|event| add_openclaw_row_timestamp(&event, created_at)))
        })
        .ok()?;
    let mut events = Vec::new();
    for row in rows {
        if let Some(event) = row.ok().flatten() {
            events.push(event);
        }
    }
    (!events.is_empty()).then(|| events.join("\n"))
}

fn add_openclaw_row_timestamp(event: &str, created_at: Option<f64>) -> String {
    let Some(timestamp) = created_at.filter(|timestamp| *timestamp > 0.0) else {
        return event.to_string();
    };
    let Ok(mut value) = serde_json::from_str::<Value>(event) else {
        return event.to_string();
    };
    let has_timestamp = value.get("timestamp").is_some_and(|value| !value.is_null())
        || value
            .get("message")
            .and_then(|message| message.get("timestamp"))
            .is_some_and(|value| !value.is_null());
    if has_timestamp {
        return event.to_string();
    }
    if let Some(object) = value.as_object_mut() {
        object.insert("timestamp".to_string(), Value::from(timestamp as i64));
        serde_json::to_string(&value).unwrap_or_else(|_| event.to_string())
    } else {
        event.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::constants::TOOL_OPENCLAW;
    use std::io::Write;
    use tempfile::tempdir;

    fn make_openclaw_session(session_id: &str, paths: Vec<String>) -> SessionFile {
        SessionFile {
            session_id: session_id.to_string(),
            tool: TOOL_OPENCLAW.to_string(),
            project_path: "agent-main".to_string(),
            file_path: paths.first().cloned().unwrap_or_default(),
            transcript_paths: paths,
            file_size: 0,
            last_modified: 1_746_784_000,
            fingerprint: 1,
        }
    }

    #[test]
    fn collect_openclaw_session_files_groups_archived_variants() {
        let temp = tempdir().unwrap();
        let sessions_dir = temp.path().join("agent-main").join("sessions");
        fs::create_dir_all(&sessions_dir).unwrap();

        fs::write(sessions_dir.join("session-1.jsonl"), "{}\n").unwrap();
        fs::write(
            sessions_dir.join("session-1.jsonl.reset.2026-06-13T12-00-00Z"),
            "{}\n",
        )
        .unwrap();
        fs::write(
            sessions_dir.join("session-1.jsonl.deleted.1740000000"),
            "{}\n",
        )
        .unwrap();

        let sessions = collect_openclaw_session_files_from_root(temp.path());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].tool, TOOL_OPENCLAW);
        assert_eq!(sessions[0].transcript_paths.len(), 3);
        assert!(sessions[0].file_path.ends_with("session-1.jsonl"));
    }

    #[test]
    fn parse_openclaw_session_file_extracts_normalized_usage_and_meta() {
        let temp = tempdir().unwrap();
        let sessions_dir = temp.path().join("agent-main").join("sessions");
        fs::create_dir_all(&sessions_dir).unwrap();

        let primary_path = sessions_dir.join("session-1.jsonl");
        let archive_path = sessions_dir.join("session-1.jsonl.reset.2026-06-13T12-00-00Z");

        let mut primary = fs::File::create(&primary_path).unwrap();
        writeln!(
            primary,
            "{}",
            serde_json::json!({
                "type": "session",
                "id": "session-1",
                "cwd": "/tmp/project-alpha",
                "timestamp": "2026-06-13T09:00:00Z"
            })
        )
        .unwrap();
        writeln!(
            primary,
            "{}",
            serde_json::json!({
                "type": "message",
                "timestamp": "2026-06-13T09:00:05Z",
                "message": {
                    "role": "user",
                    "content": "Fix the login redirect loop"
                }
            })
        )
        .unwrap();
        writeln!(
            primary,
            "{}",
            serde_json::json!({
                "type": "model_change",
                "provider": "anthropic",
                "modelId": "claude-opus-4-6"
            })
        )
        .unwrap();
        writeln!(
            primary,
            "{}",
            serde_json::json!({
                "type": "message",
                "message": {
                    "role": "assistant",
                    "model": "claude-sonnet-4-6",
                    "usage": {
                        "input": 120,
                        "output": 30,
                        "cacheRead": 20,
                        "cacheWrite": 5
                    },
                    "timestamp": 1769753935279_i64
                }
            })
        )
        .unwrap();

        let mut archive = fs::File::create(&archive_path).unwrap();
        writeln!(
            archive,
            "{}",
            serde_json::json!({
                "type": "custom",
                "customType": "model-snapshot",
                "data": {
                    "provider": "anthropic",
                    "modelId": "claude-opus-4-6"
                }
            })
        )
        .unwrap();
        writeln!(
            archive,
            "{}",
            serde_json::json!({
                "type": "message",
                "timestamp": "2026-06-13T09:05:00Z",
                "message": {
                    "role": "assistant",
                    "usage": {
                        "input": 15,
                        "output": 12,
                        "cacheRead": 20,
                        "cacheWrite": 3
                    }
                }
            })
        )
        .unwrap();

        fs::write(
            sessions_dir.join("sessions.json"),
            serde_json::to_string(&serde_json::json!({
                "agent:main:main": {
                    "sessionId": "session-1",
                    "displayName": "登录重构"
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let session = make_openclaw_session(
            "openclaw::agent-main::session-1",
            vec![
                primary_path.to_string_lossy().to_string(),
                archive_path.to_string_lossy().to_string(),
            ],
        );

        let (meta, requests) = parse_openclaw_session_file(&session);
        assert_eq!(meta.tool, TOOL_OPENCLAW);
        assert_eq!(meta.project_name.as_deref(), Some("project-alpha"));
        assert_eq!(meta.session_name.as_deref(), Some("登录重构"));
        assert_eq!(meta.topic.as_deref(), Some("Fix the login redirect loop"));
        assert_eq!(meta.message_count, 2);
        assert_eq!(meta.total_input_tokens, 135);
        assert_eq!(meta.total_output_tokens, 42);
        assert_eq!(meta.total_cache_create_tokens, 8);
        assert_eq!(meta.total_cache_read_tokens, 40);
        assert_eq!(
            meta.models,
            vec![
                "claude-opus-4-6".to_string(),
                "claude-sonnet-4-6".to_string()
            ]
        );

        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].input_tokens, 120);
        assert_eq!(requests[0].total_tokens, 175);
        assert_eq!(requests[0].model, "claude-sonnet-4-6");
        assert_eq!(requests[1].input_tokens, 15);
        assert_eq!(requests[1].cache_read_tokens, 20);
        assert_eq!(requests[1].total_tokens, 50);
        assert_eq!(requests[1].model, "claude-opus-4-6");
    }

    #[test]
    fn sqlite_and_zstd_archives_use_outer_event_ids_and_deduplicate() {
        let temp = tempdir().unwrap();
        let agent = temp.path().join("agent-main");
        let sessions_dir = agent.join("sessions");
        let db_path = agent.join("agent").join("openclaw-agent.sqlite");
        fs::create_dir_all(&sessions_dir).unwrap();
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();

        let event = serde_json::json!({
            "type": "message",
            "id": "event-stable-1",
            "message": {"role": "assistant", "model": "model-a", "usage": {
                "input": 10, "output": 4, "cacheRead": 3, "totalTokens": 25
            }}
        });
        let compressed_path = sessions_dir.join("sess-1.jsonl.deleted.1.zst");
        let compressed = zstd::bulk::compress(format!("{}\n", event).as_bytes(), 1).unwrap();
        fs::write(&compressed_path, compressed).unwrap();

        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE transcript_events (session_id TEXT, seq INTEGER, event_json TEXT, event_zstd BLOB, event_utf8_bytes INTEGER, created_at INTEGER);",
        )
        .unwrap();
        let event_text = event.to_string();
        let event_zstd = zstd::bulk::compress(event_text.as_bytes(), 1).unwrap();
        conn.execute(
            "INSERT INTO transcript_events VALUES (?1, ?2, NULL, ?3, ?4, ?5)",
            rusqlite::params![
                "sess-1",
                1_i64,
                event_zstd,
                event_text.len() as i64,
                1_760_000_000_123_i64
            ],
        )
        .unwrap();
        drop(conn);

        let sessions = collect_openclaw_session_files_from_root(temp.path());
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0]
            .transcript_paths
            .iter()
            .any(|p| p.starts_with(OPENCLAW_SQLITE_LOCATOR_PREFIX)));
        let (meta, requests) = parse_openclaw_session_file(&sessions[0]);
        assert_eq!(meta.message_count, 1);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].message_id, "event-stable-1");
        assert_eq!(requests[0].timestamp, 1_760_000_000);
        assert_eq!(requests[0].input_tokens, 10);
        assert_eq!(requests[0].cache_read_tokens, 3);
        assert_eq!(requests[0].total_tokens, 25);
    }
}
