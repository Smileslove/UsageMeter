//! Claude Code 本地 transcript 读取模块
//!
//! 负责发现 `~/.claude/projects` 下的会话文件组，并解析为统一会话摘要与请求事实。

use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::shared::{
    extract_model, extract_project_name, extract_timestamp, extract_u64_by_keys, truncate_string,
};
use super::source::{ParsedSessionData, SessionSource, SourceSnapshot, SourceUpdateMode};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub(super) struct ClaudeSource;

impl SessionSource for ClaudeSource {
    fn tool_id(&self) -> &'static str {
        super::constants::TOOL_CLAUDE_CODE
    }

    fn scan(&self) -> SourceSnapshot {
        let mut sessions = Vec::new();
        if let Some(home) = dirs::home_dir() {
            for root in [
                home.join(".claude").join("projects"),
                home.join(".config").join("claude").join("projects"),
            ] {
                if root.exists() {
                    sessions.extend(collect_claude_session_files_from_root(&root));
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
        let (meta, requests) = parse_claude_session_file(session);
        Ok(ParsedSessionData { meta, requests })
    }
}

pub(super) fn collect_claude_session_files_from_root(projects_root: &Path) -> Vec<SessionFile> {
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

    let Ok(entries) = fs::read_dir(projects_root) else {
        return Vec::new();
    };

    let mut groups: HashMap<String, SessionGroupBuilder> = HashMap::new();

    for entry in entries.flatten() {
        let project_path = entry.path();
        if !project_path.is_dir() {
            continue;
        }

        let project_name = project_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let jsonl_files = collect_jsonl_files(&project_path);

        for path in jsonl_files {
            let Some(raw_session_id) = derive_root_session_id(&project_path, &path) else {
                continue;
            };

            let metadata = fs::metadata(&path).ok();
            let file_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
            if file_size < 100 {
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

            let unique_id = format!("{project_name}::{raw_session_id}");
            let group = groups
                .entry(unique_id.clone())
                .or_insert_with(|| SessionGroupBuilder {
                    project_path: project_name.to_string(),
                    session_id: unique_id.clone(),
                    ..Default::default()
                });

            let path_string = path.to_string_lossy().to_string();
            if is_primary_transcript(&project_path, &path) {
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
    }

    let mut sessions: Vec<SessionFile> = groups
        .into_values()
        .map(|mut group| {
            group.transcript_paths.sort();
            SessionFile {
                session_id: group.session_id,
                tool: super::constants::TOOL_CLAUDE_CODE.to_string(),
                project_path: group.project_path,
                file_path: group
                    .primary_file_path
                    .or_else(|| group.transcript_paths.first().cloned())
                    .unwrap_or_default(),
                transcript_paths: group.transcript_paths,
                file_size: group.file_size,
                last_modified: group.last_modified,
                fingerprint: group.fingerprint,
            }
        })
        .filter(|session| !session.file_path.is_empty() && !session.transcript_paths.is_empty())
        .collect();

    sessions.sort_by_key(|session| std::cmp::Reverse(session.last_modified));
    sessions
}

pub(super) fn parse_claude_session_file(
    session: &SessionFile,
) -> (SessionMeta, Vec<LocalRequestRecord>) {
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
        source: "jsonl".to_string(),
        message_ids: Vec::new(),
    };

    let mut first_user_message: Option<String> = None;
    let mut last_user_message: Option<String> = None;
    let mut cwd_found: Option<String> = None;
    let mut session_name_found: Option<String> = None;
    let mut models_set: BTreeSet<String> = BTreeSet::new();
    let mut earliest_timestamp: Option<i64> = None;
    let mut latest_timestamp: Option<i64> = None;
    let mut request_map: HashMap<String, LocalRequestRecord> = HashMap::new();

    for transcript_path in &session.transcript_paths {
        let file_handle = match fs::File::open(transcript_path) {
            Ok(file) => file,
            Err(_) => continue,
        };
        let reader = BufReader::new(file_handle);
        let is_primary = transcript_path == &session.file_path;
        let is_subagent = is_subagent_transcript(session, transcript_path);

        for line in reader.lines().map_while(Result::ok) {
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };

            if let Some(ts) = extract_timestamp(&json) {
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

            if is_primary {
                if cwd_found.is_none() {
                    if let Some(cwd) = json.get("cwd").and_then(|value| value.as_str()) {
                        cwd_found = Some(cwd.to_string());
                    }
                }

                if session_name_found.is_none() {
                    if let Some(slug) = json.get("slug").and_then(|value| value.as_str()) {
                        session_name_found = Some(slug.to_string());
                    }
                    if let Some(title) = json.get("customTitle").and_then(|value| value.as_str()) {
                        session_name_found = Some(title.to_string());
                    }
                }

                let msg_type = json.get("type").and_then(|value| value.as_str());
                if msg_type == Some("user") || msg_type == Some("human") {
                    if let Some(text) = extract_user_text(&json) {
                        if first_user_message.is_none() && !is_system_message(&text) {
                            first_user_message = Some(text.clone());
                        }
                        if !is_system_message(&text) {
                            last_user_message = Some(text);
                        }
                    }
                }
            }

            if let Some(model) = extract_model(&json) {
                models_set.insert(model);
            }

            let Some(request) = extract_request_record(&json, session, is_subagent) else {
                continue;
            };
            let request_key = request.message_id.clone();
            request_map
                .entry(request_key)
                .and_modify(|existing| {
                    if request.total_tokens > existing.total_tokens
                        || (request.total_tokens == existing.total_tokens
                            && request.timestamp > existing.timestamp)
                    {
                        *existing = request.clone();
                    }
                })
                .or_insert(request);
        }
    }

    let mut requests: Vec<LocalRequestRecord> = request_map.into_values().collect();
    requests.sort_by_key(|request| request.timestamp);

    meta.cwd = cwd_found.clone();
    meta.project_name = cwd_found
        .as_ref()
        .and_then(|cwd| extract_project_name(cwd))
        .or_else(|| Some(session.project_path.clone()).filter(|value| !value.is_empty()));
    meta.topic = first_user_message.map(|text| truncate_string(&text, 50));
    meta.last_prompt = last_user_message.map(|text| truncate_string(&text, 100));
    meta.session_name = session_name_found;
    meta.models = models_set.into_iter().collect();
    meta.start_time = earliest_timestamp.unwrap_or(session.last_modified);
    meta.end_time = latest_timestamp.unwrap_or(session.last_modified);
    meta.message_count = requests.len() as u64;
    meta.message_ids = requests
        .iter()
        .map(|request| request.message_id.clone())
        .collect();

    for request in &requests {
        meta.total_input_tokens += request.input_tokens;
        meta.total_output_tokens += request.output_tokens;
        meta.total_cache_create_tokens += request.cache_create_tokens;
        meta.total_cache_read_tokens += request.cache_read_tokens;
    }

    (meta, requests)
}

fn collect_jsonl_files(project_path: &Path) -> Vec<PathBuf> {
    let mut queue = VecDeque::from([project_path.to_path_buf()]);
    let mut files = Vec::new();

    while let Some(path) = queue.pop_front() {
        let Ok(read_dir) = fs::read_dir(&path) else {
            continue;
        };

        for entry in read_dir.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                queue.push_back(entry_path);
                continue;
            }

            if entry_path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("jsonl"))
                .unwrap_or(false)
            {
                files.push(entry_path);
            }
        }
    }

    files
}

fn derive_root_session_id(project_path: &Path, transcript_path: &Path) -> Option<String> {
    let relative = transcript_path.strip_prefix(project_path).ok()?;
    let components: Vec<String> = relative
        .components()
        .map(|comp| comp.as_os_str().to_string_lossy().to_string())
        .collect();

    if components.len() == 1 {
        return Path::new(&components[0])
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(|stem| stem.to_string());
    }

    if let Some(subagent_index) = components
        .iter()
        .position(|component| component == "subagents")
    {
        if subagent_index >= 1 {
            return Some(components[subagent_index - 1].clone());
        }
    }

    None
}

fn is_primary_transcript(project_path: &Path, transcript_path: &Path) -> bool {
    transcript_path
        .strip_prefix(project_path)
        .ok()
        .map(|relative| relative.components().count() == 1)
        .unwrap_or(false)
}

fn is_subagent_transcript(session: &SessionFile, transcript_path: &str) -> bool {
    transcript_path != session.file_path
}

fn extract_request_record(
    json: &serde_json::Value,
    session: &SessionFile,
    is_subagent: bool,
) -> Option<LocalRequestRecord> {
    if json.get("type").and_then(|value| value.as_str()) != Some("assistant") {
        return None;
    }

    let message_id = json
        .get("message")
        .and_then(|message| message.get("id"))
        .and_then(|value| value.as_str())?
        .to_string();

    let usage = extract_token_usage(json)?;
    let total_tokens = usage.input + usage.output + usage.cache_create + usage.cache_read;
    if total_tokens == 0 {
        return None;
    }

    let timestamp = extract_timestamp(json).unwrap_or(session.last_modified);

    Some(LocalRequestRecord {
        session_id: session.session_id.clone(),
        tool: session.tool.clone(),
        timestamp,
        message_id,
        input_tokens: usage.input,
        output_tokens: usage.output,
        cache_create_tokens: usage.cache_create,
        cache_read_tokens: usage.cache_read,
        total_tokens,
        model: extract_model(json).unwrap_or_else(|| "unknown".to_string()),
        is_subagent,
        ..Default::default()
    })
}

fn is_system_message(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.starts_with("<local-command-caveat>") || trimmed.contains("<local-command-caveat>") {
        return true;
    }
    if trimmed.starts_with("<command-name>") || trimmed.contains("<command-name>") {
        return true;
    }
    if trimmed.starts_with("<local-command-stdout>") || trimmed.contains("<local-command-stdout>") {
        return true;
    }
    if trimmed.starts_with("<system-reminder>") || trimmed.contains("<system-reminder>") {
        return true;
    }
    trimmed.chars().count() < 3
}

fn extract_user_text(json: &serde_json::Value) -> Option<String> {
    if let Some(message) = json.get("message") {
        if let Some(content) = message.get("content").and_then(|value| value.as_str()) {
            return Some(content.to_string());
        }

        if let Some(content_arr) = message.get("content").and_then(|value| value.as_array()) {
            for item in content_arr {
                if item.get("type").and_then(|value| value.as_str()) == Some("text") {
                    if let Some(text) = item.get("text").and_then(|value| value.as_str()) {
                        return Some(text.to_string());
                    }
                }
            }
        }
    }

    None
}

struct TokenUsage {
    input: u64,
    output: u64,
    cache_create: u64,
    cache_read: u64,
}

fn extract_token_usage(json: &serde_json::Value) -> Option<TokenUsage> {
    let usage = json
        .get("message")
        .and_then(|message| message.get("usage"))
        .or_else(|| json.get("usage"));

    usage
        .and_then(|value| extract_token_usage_from_value(value, true))
        .or_else(|| find_nested_token_usage(json))
}

fn extract_token_usage_from_value(
    value: &serde_json::Value,
    allow_short_aliases: bool,
) -> Option<TokenUsage> {
    let mut input_keys = vec!["input_tokens", "inputTokens"];
    let mut output_keys = vec!["output_tokens", "outputTokens"];
    if allow_short_aliases {
        input_keys.push("input");
        output_keys.push("output");
    }

    let input = extract_u64_by_keys(value, &input_keys);
    let output = extract_u64_by_keys(value, &output_keys);
    let cache_create = extract_u64_by_keys(
        value,
        &[
            "cache_creation_input_tokens",
            "cacheCreationInputTokens",
            "cache_create_tokens",
            "cacheCreateTokens",
        ],
    );
    let cache_read = extract_u64_by_keys(
        value,
        &[
            "cache_read_input_tokens",
            "cacheReadInputTokens",
            "cache_read_tokens",
            "cacheReadTokens",
        ],
    );

    if input > 0 || output > 0 || cache_create > 0 || cache_read > 0 {
        Some(TokenUsage {
            input,
            output,
            cache_create,
            cache_read,
        })
    } else {
        None
    }
}

fn find_nested_token_usage(value: &serde_json::Value) -> Option<TokenUsage> {
    if let Some(usage) = extract_token_usage_from_value(value, false) {
        return Some(usage);
    }

    match value {
        serde_json::Value::Array(items) => items.iter().find_map(find_nested_token_usage),
        serde_json::Value::Object(map) => map.values().find_map(find_nested_token_usage),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::constants::TOOL_CLAUDE_CODE;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_derive_root_session_id() {
        let project = Path::new("/tmp/projects/my-project");
        let top_level = Path::new("/tmp/projects/my-project/abc-session.jsonl");
        let subagent = Path::new("/tmp/projects/my-project/abc-session/subagents/agent-1.jsonl");

        assert_eq!(
            derive_root_session_id(project, top_level),
            Some("abc-session".to_string())
        );
        assert_eq!(
            derive_root_session_id(project, subagent),
            Some("abc-session".to_string())
        );
    }

    #[test]
    fn test_extract_token_usage_supports_multiple_key_styles() {
        let json = serde_json::json!({
            "message": {
                "usage": {
                    "inputTokens": 10,
                    "outputTokens": 20,
                    "cacheCreationInputTokens": 3,
                    "cacheReadTokens": 4
                }
            }
        });

        let usage = extract_token_usage(&json).unwrap();
        assert_eq!(usage.input, 10);
        assert_eq!(usage.output, 20);
        assert_eq!(usage.cache_create, 3);
        assert_eq!(usage.cache_read, 4);
    }

    #[test]
    fn test_extract_token_usage_falls_back_to_nested_payload_tokens() {
        let json = serde_json::json!({
            "type": "assistant",
            "message": {
                "id": "msg_123"
            },
            "payload": {
                "metrics": {
                    "input_tokens": 42,
                    "output_tokens": 24,
                    "cache_creation_input_tokens": 5,
                    "cache_read_input_tokens": 6
                }
            }
        });

        let usage = extract_token_usage(&json).unwrap();
        assert_eq!(usage.input, 42);
        assert_eq!(usage.output, 24);
        assert_eq!(usage.cache_create, 5);
        assert_eq!(usage.cache_read, 6);
    }

    #[test]
    fn test_extract_token_usage_prefers_nested_payload_when_standard_usage_is_empty() {
        let json = serde_json::json!({
            "message": {
                "usage": {
                    "input_tokens": 0,
                    "output_tokens": 0
                }
            },
            "payload": {
                "usage_breakdown": {
                    "input_tokens": 11,
                    "output_tokens": 7
                }
            }
        });

        let usage = extract_token_usage(&json).unwrap();
        assert_eq!(usage.input, 11);
        assert_eq!(usage.output, 7);
        assert_eq!(usage.cache_create, 0);
        assert_eq!(usage.cache_read, 0);
    }

    #[test]
    fn test_extract_token_usage_does_not_treat_generic_input_output_fields_as_usage() {
        let json = serde_json::json!({
            "payload": {
                "stats": {
                    "input": 10,
                    "output": 20
                }
            }
        });

        assert!(extract_token_usage(&json).is_none());
    }

    #[test]
    fn test_parse_claude_session_file_keeps_latest_request_variant() {
        let tmpdir = tempdir().unwrap();
        let project_dir = tmpdir.path().join("project-a");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("session-1.jsonl");
        let mut file = fs::File::create(&session_path).unwrap();

        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "assistant",
                "timestamp": "2026-01-01T00:00:01Z",
                "message": {
                    "id": "msg-1",
                    "model": "claude-sonnet-4",
                    "usage": { "inputTokens": 10, "outputTokens": 5 }
                }
            })
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({
                "type": "assistant",
                "timestamp": "2026-01-01T00:00:02Z",
                "message": {
                    "id": "msg-1",
                    "model": "claude-sonnet-4",
                    "usage": { "inputTokens": 10, "outputTokens": 8 }
                }
            })
        )
        .unwrap();

        let session = SessionFile {
            session_id: "project-a::session-1".to_string(),
            tool: TOOL_CLAUDE_CODE.to_string(),
            project_path: "project-a".to_string(),
            file_path: session_path.to_string_lossy().to_string(),
            transcript_paths: vec![session_path.to_string_lossy().to_string()],
            file_size: fs::metadata(&session_path).unwrap().len(),
            last_modified: 1_700_000_000,
            fingerprint: 1,
        };

        let (meta, requests) = parse_claude_session_file(&session);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].output_tokens, 8);
        assert_eq!(meta.message_count, 1);
    }
}
