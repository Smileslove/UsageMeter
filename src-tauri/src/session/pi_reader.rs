//! Pi Agent local session reader.

use super::constants::TOOL_PI;
use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::shared::{
    extract_project_name, extract_timestamp, parse_u64_from_value, truncate_string,
};
use super::source::{ParsedSessionData, SessionSource, SourceSnapshot, SourceUpdateMode};
use serde_json::Value;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub(super) struct PiSource;

impl SessionSource for PiSource {
    fn tool_id(&self) -> &'static str {
        TOOL_PI
    }

    fn scan(&self) -> SourceSnapshot {
        let sessions = scan_pi_session_files();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for session in &sessions {
            session.session_id.hash(&mut hasher);
            session.fingerprint.hash(&mut hasher);
        }
        SourceSnapshot {
            source_id: TOOL_PI,
            update_mode: SourceUpdateMode::PerSession,
            sessions,
            scan_fingerprint: hasher.finish(),
        }
    }

    fn parse(&self, session: &SessionFile) -> Result<ParsedSessionData, String> {
        Ok(parse_pi_session_file(session))
    }
}

fn pi_sessions_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".pi").join("agent").join("sessions"))
}

fn session_header(path: &Path) -> Option<Value> {
    let file = fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines().map_while(Result::ok).take(20) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("session") {
            return Some(value);
        }
    }
    None
}

fn file_modified(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn scan_pi_session_files() -> Vec<SessionFile> {
    let Some(root) = pi_sessions_root() else {
        return Vec::new();
    };
    let Ok(project_dirs) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut sessions = Vec::new();
    let mut seen_session_ids = HashSet::new();
    for project_entry in project_dirs.flatten() {
        let project_dir = project_entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let project_path = project_dir
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string();
        let Ok(files) = fs::read_dir(&project_dir) else {
            continue;
        };
        for file_entry in files.flatten() {
            let path = file_entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(header) = session_header(&path) else {
                continue;
            };
            let Some(raw_id) = header
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
            else {
                continue;
            };
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if metadata.len() == 0 {
                continue;
            }
            let modified = file_modified(&path);
            let path_string = path.to_string_lossy().to_string();
            let mut fingerprint = std::collections::hash_map::DefaultHasher::new();
            path_string.hash(&mut fingerprint);
            metadata.len().hash(&mut fingerprint);
            modified.hash(&mut fingerprint);
            let base_id = format!("{TOOL_PI}::{raw_id}");
            let session_id = if seen_session_ids.insert(base_id.clone()) {
                base_id
            } else {
                let mut path_hasher = std::collections::hash_map::DefaultHasher::new();
                path_string.hash(&mut path_hasher);
                format!("{TOOL_PI}::{raw_id}::{:016x}", path_hasher.finish())
            };
            sessions.push(SessionFile {
                session_id,
                tool: TOOL_PI.to_string(),
                project_path: project_path.clone(),
                file_path: path_string.clone(),
                transcript_paths: vec![path_string],
                file_size: metadata.len(),
                last_modified: modified,
                fingerprint: fingerprint.finish(),
            });
        }
    }
    sessions.sort_by_key(|session| std::cmp::Reverse(session.last_modified));
    sessions
}

pub(super) fn parse_pi_session_file(session: &SessionFile) -> ParsedSessionData {
    let mut meta = SessionMeta {
        session_id: session.session_id.clone(),
        tool: TOOL_PI.to_string(),
        file_path: session.file_path.clone(),
        file_size: session.file_size,
        last_modified: session.last_modified,
        source: "pi_jsonl".to_string(),
        ..Default::default()
    };
    let mut requests = Vec::new();
    let mut models = BTreeSet::new();
    let mut current_provider = String::new();
    let mut current_model = String::new();
    let mut first_user = None;
    let mut last_user = None;
    let mut session_name = None;
    let mut earliest = None;
    let mut latest = None;
    let mut seen_message_ids = std::collections::HashSet::new();

    for path in &session.transcript_paths {
        let Ok(file) = fs::File::open(path) else {
            continue;
        };
        let file_timestamp = file_modified(Path::new(path)).max(session.last_modified);
        for (line_idx, line) in BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .enumerate()
        {
            let Ok(json) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let timestamp = extract_timestamp(&json)
                .or_else(|| json.get("message").and_then(extract_timestamp))
                .unwrap_or(file_timestamp);
            earliest = Some(earliest.map_or(timestamp, |value: i64| value.min(timestamp)));
            latest = Some(latest.map_or(timestamp, |value: i64| value.max(timestamp)));
            match json.get("type").and_then(Value::as_str).unwrap_or_default() {
                "session" => {
                    if meta.cwd.is_none() {
                        meta.cwd = json.get("cwd").and_then(Value::as_str).map(str::to_string);
                    }
                    if current_provider.is_empty() {
                        current_provider = json
                            .get("provider")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                    }
                    if current_model.is_empty() {
                        current_model = json
                            .get("modelId")
                            .and_then(Value::as_str)
                            .or_else(|| json.get("model").and_then(Value::as_str))
                            .unwrap_or_default()
                            .to_string();
                    }
                }
                "model_change" => {
                    current_provider = json
                        .get("provider")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    current_model = json
                        .get("modelId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                }
                "session_info" => {
                    if session_name.is_none() {
                        session_name = json
                            .get("name")
                            .and_then(Value::as_str)
                            .filter(|value| !value.trim().is_empty())
                            .map(str::to_string);
                    }
                }
                "message" => {
                    let Some(message) = json.get("message") else {
                        continue;
                    };
                    let role = message
                        .get("role")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if role == "user" {
                        if let Some(text) = message_text(message) {
                            if first_user.is_none() {
                                first_user = Some(text.clone());
                            }
                            last_user = Some(text);
                        }
                        continue;
                    }
                    if role != "assistant" {
                        continue;
                    }
                    if let Some(provider) = message
                        .get("provider")
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                    {
                        current_provider = provider.to_string();
                    }
                    if let Some(model) = message
                        .get("model")
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                    {
                        current_model = model.to_string();
                    }
                    let Some(usage) = message.get("usage") else {
                        continue;
                    };
                    let input = parse_u64_from_value(usage.get("input").unwrap_or(&Value::Null))
                        .unwrap_or(0);
                    let output = parse_u64_from_value(usage.get("output").unwrap_or(&Value::Null))
                        .unwrap_or(0);
                    let cache_read =
                        parse_u64_from_value(usage.get("cacheRead").unwrap_or(&Value::Null))
                            .unwrap_or(0);
                    let cache_create =
                        parse_u64_from_value(usage.get("cacheWrite").unwrap_or(&Value::Null))
                            .unwrap_or(0);
                    let reasoning =
                        parse_u64_from_value(usage.get("reasoning").unwrap_or(&Value::Null))
                            .unwrap_or(0)
                            .min(output);
                    if input == 0 && output == 0 && cache_read == 0 && cache_create == 0 {
                        continue;
                    }
                    let raw_model = if current_model.is_empty() {
                        "unknown".to_string()
                    } else {
                        current_model.clone()
                    };
                    let model = if current_provider.is_empty() || raw_model == "unknown" {
                        raw_model
                    } else {
                        format!("{}/{}", current_provider, raw_model)
                    };
                    models.insert(model.clone());
                    let message_id = message
                        .get("id")
                        .and_then(Value::as_str)
                        .or_else(|| json.get("id").and_then(Value::as_str))
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("pi:{}:{}", session.session_id, line_idx));
                    if !seen_message_ids.insert(message_id.clone()) {
                        continue;
                    }
                    let explicit_cost = usage
                        .get("cost")
                        .and_then(|cost| cost.get("total"))
                        .and_then(Value::as_f64)
                        .filter(|value| value.is_finite() && *value > 0.0);
                    let total_tokens = input + output + cache_read + cache_create;
                    meta.total_input_tokens += input;
                    meta.total_output_tokens += output;
                    meta.total_reasoning_tokens += reasoning;
                    meta.total_cache_read_tokens += cache_read;
                    meta.total_cache_create_tokens += cache_create;
                    meta.message_ids.push(message_id.clone());
                    requests.push(LocalRequestRecord {
                        session_id: session.session_id.clone(),
                        tool: TOOL_PI.to_string(),
                        timestamp,
                        message_id,
                        input_tokens: input,
                        output_tokens: output,
                        reasoning_tokens: reasoning,
                        cache_create_tokens: cache_create,
                        cache_read_tokens: cache_read,
                        total_tokens,
                        request_count: 1,
                        model,
                        is_subagent: false,
                        request_key: None,
                        explicit_estimated_cost: explicit_cost,
                        source_file_present: None,
                    });
                }
                _ => {}
            }
        }
    }
    requests.sort_by_key(|request| request.timestamp);
    meta.project_name = meta
        .cwd
        .as_deref()
        .and_then(extract_project_name)
        .or_else(|| (!session.project_path.is_empty()).then(|| session.project_path.clone()));
    meta.topic = first_user
        .as_deref()
        .map(|value| truncate_string(value, 50));
    meta.last_prompt = last_user
        .as_deref()
        .map(|value| truncate_string(value, 100));
    meta.session_name = session_name;
    meta.models = models.into_iter().collect();
    meta.message_count = requests.len() as u64;
    meta.start_time = earliest.unwrap_or(session.last_modified);
    meta.end_time = latest.unwrap_or(session.last_modified);
    ParsedSessionData { meta, requests }
}

fn message_text(message: &Value) -> Option<String> {
    let content = message.get("content")?;
    if let Some(text) = content.as_str() {
        return (!text.trim().is_empty()).then(|| text.to_string());
    }
    content
        .as_array()?
        .iter()
        .filter_map(|item| {
            (item.get("type").and_then(Value::as_str) == Some("text"))
                .then(|| item.get("text").and_then(Value::as_str))
                .flatten()
        })
        .map(str::to_string)
        .reduce(|left, right| format!("{left} {right}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    fn session(path: &Path) -> SessionFile {
        SessionFile {
            session_id: "pi::session-1".to_string(),
            tool: TOOL_PI.to_string(),
            project_path: "--repo--".to_string(),
            file_path: path.to_string_lossy().to_string(),
            transcript_paths: vec![path.to_string_lossy().to_string()],
            file_size: 1,
            last_modified: 1_700_000_000,
            fingerprint: 1,
        }
    }

    #[test]
    fn parses_usage_model_changes_cost_and_metadata() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("session.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({"type":"session","id":"session-1","cwd":"/tmp/pi-project"})
        )
        .unwrap();
        writeln!(file, "{}", serde_json::json!({"type":"message","timestamp":"2026-09-01T10:00:00Z","message":{"role":"user","content":"Fix the login"}})).unwrap();
        writeln!(file, "{}", serde_json::json!({"type":"model_change","provider":"anthropic","modelId":"claude-sonnet-4-6"})).unwrap();
        writeln!(file, "{}", serde_json::json!({"type":"message","id":"assistant-1","timestamp":"2026-09-01T10:00:01Z","message":{"role":"assistant","content":[],"usage":{"input":100,"output":20,"reasoning":5,"cacheRead":10,"cacheWrite":2,"cost":{"total":0.0033}}}})).unwrap();
        writeln!(file, "not-json").unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({"type":"session_info","name":"Login fix"})
        )
        .unwrap();
        let parsed = parse_pi_session_file(&session(&path));
        assert_eq!(parsed.meta.project_name.as_deref(), Some("pi-project"));
        assert_eq!(parsed.meta.topic.as_deref(), Some("Fix the login"));
        assert_eq!(parsed.meta.session_name.as_deref(), Some("Login fix"));
        assert_eq!(parsed.meta.message_count, 1);
        assert_eq!(parsed.meta.total_reasoning_tokens, 5);
        assert_eq!(parsed.requests[0].model, "anthropic/claude-sonnet-4-6");
        assert_eq!(parsed.requests[0].total_tokens, 132);
        assert_eq!(parsed.requests[0].explicit_estimated_cost, Some(0.0033));
    }

    #[test]
    fn session_header_skips_invalid_rows_before_session_record() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("invalid.jsonl");
        fs::write(&path, "not-json\n{\"type\":\"session\",\"id\":\"one\"}\n").unwrap();
        assert_eq!(session_header(&path).unwrap()["id"], "one");
    }

    #[test]
    fn duplicate_session_ids_get_stable_path_suffix() {
        let base = "pi::same";
        let path = "/tmp/pi/--other--/same.jsonl";
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        path.hash(&mut hasher);
        let derived = format!("{TOOL_PI}::same::{:016x}", hasher.finish());
        assert_ne!(base, derived);
        assert!(derived.starts_with("pi::same::"));
    }
}
