//! Qoder CLI 本地 JSONL 会话读取模块
//!
//! 扫描 `~/.qoder/projects/<encoded-cwd>/transcript/<session_id>.jsonl`
//! 每个 JSONL 文件对应一个会话，解析 type=="assistant" 事件的 usage 字段。

use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::shared::{
    extract_project_name, extract_timestamp, extract_u64_by_keys, truncate_string,
};
use super::source::{ParsedSessionData, SessionSource, SourceSnapshot, SourceUpdateMode};
use std::collections::BTreeSet;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

const QODER_CLI_SOURCE_KIND: &str = "qoder_cli_jsonl";

pub(super) struct QoderCliSource;

impl SessionSource for QoderCliSource {
    fn tool_id(&self) -> &'static str {
        super::constants::TOOL_QODER_CLI
    }

    fn scan(&self) -> SourceSnapshot {
        let sessions = collect_qoder_cli_session_files();
        let scan_fingerprint = {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            for s in &sessions {
                s.session_id.hash(&mut hasher);
                s.fingerprint.hash(&mut hasher);
            }
            hasher.finish()
        };
        SourceSnapshot {
            source_id: self.tool_id(),
            update_mode: SourceUpdateMode::PerSession,
            sessions,
            scan_fingerprint,
        }
    }

    fn parse(&self, session: &SessionFile) -> Result<ParsedSessionData, String> {
        let (meta, requests) = parse_qoder_cli_session(session);
        Ok(ParsedSessionData { meta, requests })
    }
}

fn collect_qoder_cli_session_files() -> Vec<SessionFile> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mut sessions = Vec::new();
    for (root_name, projects_root) in [
        ("qoder", home.join(".qoder").join("projects")),
        ("qoder-cn", home.join(".qoder-cn").join("projects")),
    ] {
        sessions.extend(collect_qoder_cli_sessions_from_root(
            &projects_root,
            root_name,
        ));
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.last_modified));
    sessions
}

fn collect_qoder_cli_sessions_from_root(projects_root: &Path, root_name: &str) -> Vec<SessionFile> {
    let Ok(project_entries) = fs::read_dir(projects_root) else {
        return Vec::new();
    };
    let mut sessions = Vec::new();
    for entry in project_entries.flatten() {
        let project_dir = entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let encoded_cwd = project_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if encoded_cwd.is_empty() {
            continue;
        }

        for jsonl_path in collect_qoder_cli_jsonl_files(&project_dir) {
            if jsonl_path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("jsonl"))
                != Some(true)
            {
                continue;
            }

            let metadata = fs::metadata(&jsonl_path).ok();
            let file_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
            if file_size < 50 {
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

            let session_stem = jsonl_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if session_stem.is_empty() {
                continue;
            }

            let relative = jsonl_path.strip_prefix(&project_dir).unwrap_or(&jsonl_path);
            let relative_key = relative
                .with_extension("")
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "::");
            let old_layout =
                root_name == "qoder" && relative.parent() == Some(Path::new("transcript"));
            let session_id = if old_layout {
                format!("qoder_cli::{}::{}", encoded_cwd, session_stem)
            } else {
                format!(
                    "qoder_cli::{}::{}::{}",
                    root_name, encoded_cwd, relative_key
                )
            };
            let file_path_str = jsonl_path.to_string_lossy().to_string();

            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            file_path_str.hash(&mut hasher);
            file_size.hash(&mut hasher);
            last_modified.hash(&mut hasher);
            let fingerprint = hasher.finish();

            sessions.push(SessionFile {
                session_id,
                tool: super::constants::TOOL_QODER_CLI.to_string(),
                project_path: encoded_cwd.clone(),
                file_path: file_path_str.clone(),
                transcript_paths: vec![file_path_str],
                file_size,
                last_modified,
                fingerprint,
            });
        }
    }
    sessions
}

fn collect_qoder_cli_jsonl_files(project_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![project_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("jsonl"))
                == Some(true)
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn parse_qoder_cli_session(session: &SessionFile) -> (SessionMeta, Vec<LocalRequestRecord>) {
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
        source: QODER_CLI_SOURCE_KIND.to_string(),
        message_ids: Vec::new(),
        explicit_estimated_cost: None,
        scope: None,
        ..Default::default()
    };

    let file_handle = match fs::File::open(&session.file_path) {
        Ok(f) => f,
        Err(_) => return (meta, Vec::new()),
    };

    let mut cwd_found: Option<String> = None;
    let mut first_user_text: Option<String> = None;
    let mut last_user_text: Option<String> = None;
    let mut models_set: BTreeSet<String> = BTreeSet::new();
    let mut earliest_ts: Option<i64> = None;
    let mut latest_ts: Option<i64> = None;

    // Keyed by message_id (uuid) to deduplicate repeated events
    let mut request_map: std::collections::HashMap<String, LocalRequestRecord> =
        std::collections::HashMap::new();

    let reader = BufReader::new(file_handle);
    for (line_idx, line) in reader.lines().map_while(Result::ok).enumerate() {
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };

        if let Some(ts) = extract_timestamp(&json) {
            earliest_ts = Some(earliest_ts.map(|cur| cur.min(ts)).unwrap_or(ts));
            latest_ts = Some(latest_ts.map(|cur| cur.max(ts)).unwrap_or(ts));
        }

        // cwd is present on most events at top level
        if cwd_found.is_none() {
            if let Some(cwd) = json.get("cwd").and_then(|v| v.as_str()) {
                if !cwd.is_empty() {
                    cwd_found = Some(cwd.to_string());
                }
            }
        }

        let event_type = json.get("type").and_then(|v| v.as_str());

        if event_type == Some("user") {
            if let Some(text) = extract_cli_user_text(&json) {
                if first_user_text.is_none() {
                    first_user_text = Some(text.clone());
                }
                last_user_text = Some(text);
            }
        }

        if event_type == Some("assistant") {
            if let Some(model) = extract_cli_model(&json) {
                models_set.insert(model);
            }

            let Some(record) = extract_cli_request_record(&json, session, line_idx as u64) else {
                continue;
            };
            let key = record.message_id.clone();
            request_map
                .entry(key)
                .and_modify(|existing| {
                    if record.total_tokens > existing.total_tokens
                        || (record.total_tokens == existing.total_tokens
                            && record.timestamp > existing.timestamp)
                    {
                        *existing = record.clone();
                    }
                })
                .or_insert(record);
        }
    }

    let mut requests: Vec<LocalRequestRecord> = request_map.into_values().collect();
    requests.sort_by_key(|r| r.timestamp);

    meta.cwd = cwd_found.clone();
    meta.project_name = cwd_found
        .as_deref()
        .and_then(extract_project_name)
        .or_else(|| decode_qoder_project_name(&session.project_path));
    meta.topic = first_user_text.as_deref().map(|t| truncate_string(t, 50));
    meta.last_prompt = last_user_text.as_deref().map(|t| truncate_string(t, 100));
    meta.models = models_set.into_iter().collect();
    meta.start_time = earliest_ts.unwrap_or(session.last_modified);
    meta.end_time = latest_ts.unwrap_or(session.last_modified);
    meta.message_count = requests.len() as u64;
    meta.message_ids = requests.iter().map(|r| r.message_id.clone()).collect();

    for r in &requests {
        meta.total_input_tokens += r.input_tokens;
        meta.total_output_tokens += r.output_tokens;
        meta.total_cache_create_tokens += r.cache_create_tokens;
        meta.total_cache_read_tokens += r.cache_read_tokens;
    }

    (meta, requests)
}

fn extract_cli_request_record(
    json: &serde_json::Value,
    session: &SessionFile,
    line_idx: u64,
) -> Option<LocalRequestRecord> {
    let usage = extract_cli_token_usage(json).unwrap_or_default();
    let total = usage.input + usage.output + usage.cache_create + usage.cache_read;

    // Use the per-event uuid as message_id; fall back to a timestamp+token hash
    let message_id = json
        .get("uuid")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            let ts = extract_timestamp(json).unwrap_or(session.last_modified);
            format!("cli_{}_{}_{}", ts, total, line_idx)
        });

    let timestamp = extract_timestamp(json).unwrap_or(session.last_modified);
    let model = extract_cli_model(json).unwrap_or_else(|| "unknown".to_string());

    Some(LocalRequestRecord {
        session_id: session.session_id.clone(),
        tool: super::constants::TOOL_QODER_CLI.to_string(),
        timestamp,
        message_id,
        input_tokens: usage.input,
        output_tokens: usage.output,
        reasoning_tokens: usage.reasoning,
        cache_create_tokens: usage.cache_create,
        cache_read_tokens: usage.cache_read,
        total_tokens: total,
        request_count: 1,
        model,
        is_subagent: false,
        request_key: None,
        explicit_estimated_cost: None,
        source_file_present: None,
    })
}

#[derive(Default)]
struct CliTokenUsage {
    input: u64,
    output: u64,
    cache_create: u64,
    cache_read: u64,
    reasoning: u64,
}

fn extract_cli_token_usage(json: &serde_json::Value) -> Option<CliTokenUsage> {
    // Qoder CLI: usage is at message.usage (same as Claude Code)
    let usage = json
        .get("message")
        .and_then(|m| m.get("usage"))
        .or_else(|| json.get("usage"))?;

    let input = extract_u64_by_keys(usage, &["input_tokens", "inputTokens"]);
    let output = extract_u64_by_keys(usage, &["output_tokens", "outputTokens"]);
    let cache_create = extract_u64_by_keys(
        usage,
        &[
            "cache_creation_input_tokens",
            "cacheCreationInputTokens",
            "cache_create_tokens",
        ],
    );
    let cache_read = extract_u64_by_keys(
        usage,
        &[
            "cache_read_input_tokens",
            "cacheReadInputTokens",
            "cache_read_tokens",
        ],
    );
    let reasoning = extract_u64_by_keys(
        usage,
        &["reasoning_tokens", "reasoningTokens", "thinking_tokens"],
    );

    Some(CliTokenUsage {
        input,
        output,
        cache_create,
        cache_read,
        reasoning,
    })
}

fn extract_cli_model(json: &serde_json::Value) -> Option<String> {
    let model = json
        .get("message")
        .and_then(|m| m.get("model"))
        .and_then(|v| v.as_str())
        .or_else(|| json.get("model").and_then(|v| v.as_str()))?;
    if model.is_empty() || model == "unknown" {
        return None;
    }
    Some(model.to_string())
}

fn extract_cli_user_text(json: &serde_json::Value) -> Option<String> {
    let message = json.get("message")?;
    if let Some(s) = message.get("content").and_then(|v| v.as_str()) {
        let trimmed = s.trim();
        if !trimmed.is_empty() && trimmed.len() >= 3 {
            return Some(trimmed.to_string());
        }
    }
    if let Some(arr) = message.get("content").and_then(|v| v.as_array()) {
        for item in arr {
            if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    let trimmed = text.trim();
                    if trimmed.len() >= 3 {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }
    }
    None
}

/// 将 Qoder CLI 的 encoded-cwd 目录名（形如 `-Users-foo-bar-myproject`）解码为项目名
fn decode_qoder_project_name(encoded_cwd: &str) -> Option<String> {
    // encoded-cwd 以 `-` 为分隔符，最后一段即为项目名
    let parts: Vec<&str> = encoded_cwd.split('-').filter(|s| !s.is_empty()).collect();
    parts.last().map(|s| {
        // 简单尝试从末尾重建中文路径，只取最后 1 个分段
        s.to_string()
    })
}

#[allow(dead_code)]
pub(crate) fn find_qoder_cli_projects_root() -> Option<PathBuf> {
    dirs::home_dir()
        .map(|home| home.join(".qoder").join("projects"))
        .filter(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::constants::TOOL_QODER_CLI;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn parse_qoder_cli_session_extracts_usage_and_cwd() {
        let tmpdir = tempdir().unwrap();
        let project_dir = tmpdir.path().join("-Users-test-myproject");
        let transcript_dir = project_dir.join("transcript");
        fs::create_dir_all(&transcript_dir).unwrap();
        let session_path = transcript_dir.join("abc-123.jsonl");
        let mut f = fs::File::create(&session_path).unwrap();

        writeln!(
            f,
            "{}",
            serde_json::json!({
                "type": "session_meta",
                "sessionId": "abc-123",
                "uuid": "uuid-meta",
                "timestamp": "2026-06-11T03:00:00Z",
                "cwd": "/Users/test/myproject",
                "data": {}
            })
        )
        .unwrap();
        writeln!(
            f,
            "{}",
            serde_json::json!({
                "type": "user",
                "sessionId": "abc-123",
                "uuid": "uuid-user",
                "timestamp": "2026-06-11T03:01:00Z",
                "cwd": "/Users/test/myproject",
                "message": {"role": "user", "content": "Hello Qoder"}
            })
        )
        .unwrap();
        writeln!(
            f,
            "{}",
            serde_json::json!({
                "type": "assistant",
                "sessionId": "abc-123",
                "uuid": "uuid-assistant-1",
                "timestamp": "2026-06-11T03:01:10Z",
                "cwd": "/Users/test/myproject",
                "message": {
                    "role": "assistant",
                    "model": "qwen-max",
                    "content": [],
                    "usage": {
                        "input_tokens": 100,
                        "output_tokens": 50,
                        "cache_creation_input_tokens": 10,
                        "cache_read_input_tokens": 20
                    }
                }
            })
        )
        .unwrap();

        let session = SessionFile {
            session_id: "qoder_cli::-Users-test-myproject::abc-123".to_string(),
            tool: TOOL_QODER_CLI.to_string(),
            project_path: "-Users-test-myproject".to_string(),
            file_path: session_path.to_string_lossy().to_string(),
            transcript_paths: vec![session_path.to_string_lossy().to_string()],
            file_size: fs::metadata(&session_path).unwrap().len(),
            last_modified: 1_781_000_000,
            fingerprint: 42,
        };

        let (meta, requests) = parse_qoder_cli_session(&session);
        assert_eq!(requests.len(), 1);
        assert_eq!(meta.cwd.as_deref(), Some("/Users/test/myproject"));
        assert_eq!(meta.project_name.as_deref(), Some("myproject"));
        assert_eq!(meta.topic.as_deref(), Some("Hello Qoder"));
        assert_eq!(requests[0].input_tokens, 100);
        assert_eq!(requests[0].output_tokens, 50);
        assert_eq!(requests[0].cache_create_tokens, 10);
        assert_eq!(requests[0].cache_read_tokens, 20);
        assert_eq!(requests[0].model, "qwen-max");
        assert_eq!(requests[0].message_id, "uuid-assistant-1");
    }

    #[test]
    fn parse_qoder_cli_session_keeps_missing_usage_requests() {
        let tmpdir = tempdir().unwrap();
        let session_path = tmpdir.path().join("abc-123.jsonl");
        let mut f = fs::File::create(&session_path).unwrap();

        writeln!(
            f,
            "{}",
            serde_json::json!({
                "type": "assistant",
                "sessionId": "abc-123",
                "uuid": "uuid-assistant-1",
                "timestamp": "2026-06-11T03:01:10Z",
                "message": {
                    "role": "assistant",
                    "model": "gm51model",
                    "content": []
                }
            })
        )
        .unwrap();

        let session = SessionFile {
            session_id: "qoder_cli::-Users-test-myproject::abc-123".to_string(),
            tool: TOOL_QODER_CLI.to_string(),
            project_path: "-Users-test-myproject".to_string(),
            file_path: session_path.to_string_lossy().to_string(),
            transcript_paths: vec![session_path.to_string_lossy().to_string()],
            file_size: fs::metadata(&session_path).unwrap().len(),
            last_modified: 1_781_000_000,
            fingerprint: 42,
        };

        let (meta, requests) = parse_qoder_cli_session(&session);
        assert_eq!(requests.len(), 1);
        assert_eq!(meta.message_count, 1);
        assert_eq!(requests[0].total_tokens, 0);
        assert_eq!(requests[0].request_count, 1);
        assert_eq!(requests[0].model, "gm51model");
    }

    #[test]
    fn qoder_cli_missing_uuid_fallback_keeps_same_second_requests_distinct() {
        let session = SessionFile {
            session_id: "qoder_cli::qoder::project::session".to_string(),
            tool: TOOL_QODER_CLI.to_string(),
            project_path: "project".to_string(),
            file_path: "/tmp/session.jsonl".to_string(),
            transcript_paths: vec![],
            file_size: 100,
            last_modified: 1_781_000_000,
            fingerprint: 1,
        };
        let event = serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-06-11T03:01:10Z",
            "message": {"usage": {"input_tokens": 10, "output_tokens": 5}}
        });
        let first = extract_cli_request_record(&event, &session, 2).unwrap();
        let second = extract_cli_request_record(&event, &session, 3).unwrap();
        assert_ne!(first.message_id, second.message_id);
    }

    #[test]
    fn decode_qoder_project_name_returns_last_segment() {
        assert_eq!(
            decode_qoder_project_name("-Users-test-myproject"),
            Some("myproject".to_string())
        );
        assert_eq!(decode_qoder_project_name(""), None);
    }

    #[test]
    fn qoder_cli_scan_covers_legacy_nested_and_cn_layouts() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("projects");
        let project = root.join("-Users-test-project");
        let cn_root = temp.path().join("cn-projects");
        let paths = [
            project.join("transcript/legacy.jsonl"),
            project.join("new.jsonl"),
            cn_root.join("-Users-test-project/nested/child.jsonl"),
        ];
        for path in &paths {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("{}\n", "x".repeat(60))).unwrap();
        }

        let intl = collect_qoder_cli_sessions_from_root(&root, "qoder");
        let cn = collect_qoder_cli_sessions_from_root(&cn_root, "qoder-cn");
        assert_eq!(intl.len(), 2);
        assert_eq!(cn.len(), 1);
        assert!(intl
            .iter()
            .any(|session| { session.session_id == "qoder_cli::-Users-test-project::legacy" }));
        assert!(intl
            .iter()
            .any(|session| session.session_id.contains("::qoder::")));
        assert!(cn
            .iter()
            .all(|session| session.session_id.contains("::qoder-cn::")));
    }
}
