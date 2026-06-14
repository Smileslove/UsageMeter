//! Gemini CLI 本地会话读取模块
//!
//! 解析 `~/.gemini/tmp/<project_hash>/chats/session-*.json` 格式的 Gemini CLI 会话日志。
//! 与 Claude / Codex 不同，Gemini 会话文件是单个 JSON 对象（非 JSONL），顶层含
//! `sessionId` 与 `messages` 数组。每条 `type == "gemini"` 的消息自带独立 `tokens`
//! 用量（per-message 独立值，不需累计差分），按 message `id` 去重。

use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::shared::{
    extract_project_name, extract_timestamp, parse_u64_from_value, truncate_string,
};
use super::source::{ParsedSessionData, SessionSource, SourceSnapshot, SourceUpdateMode};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

pub(super) struct GeminiSource;

impl SessionSource for GeminiSource {
    fn tool_id(&self) -> &'static str {
        super::constants::TOOL_GEMINI
    }

    fn scan(&self) -> SourceSnapshot {
        let mut sessions = Vec::new();
        if let Some(home) = dirs::home_dir() {
            let gemini_tmp = home.join(".gemini").join("tmp");
            if gemini_tmp.exists() {
                sessions.extend(collect_gemini_session_files(&gemini_tmp));
            }
        }
        // 额外扫描 WSL 发行版内的 Gemini sessions（仅 Windows，且 wslScan 开启时）。
        #[cfg(windows)]
        if let Some(cfg) = super::wsl::scan_config_if_enabled() {
            for root in super::wsl::gemini_tmp_roots(&cfg) {
                if root.exists() {
                    sessions.extend(collect_gemini_session_files(&root));
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
        let (meta, requests) = parse_gemini_session_file(session);
        Ok(ParsedSessionData { meta, requests })
    }
}

/// 扫描 `~/.gemini/tmp` 下所有 `session-*.json` 文件，按 sessionId 分组为 SessionFile。
///
/// session_id 直接从文件名（`session-<id>.json`）派生，避免在 scan 阶段解析整个
/// JSON（Gemini 会话文件是单个大对象，逐个全量解析代价高）。
pub(super) fn collect_gemini_session_files(root: &Path) -> Vec<SessionFile> {
    #[derive(Default)]
    struct SessionGroupBuilder {
        session_id: String,
        primary_file_path: Option<String>,
        transcript_paths: Vec<String>,
        file_size: u64,
        last_modified: i64,
        fingerprint: u64,
    }

    let mut groups: HashMap<String, SessionGroupBuilder> = HashMap::new();

    for path in collect_gemini_session_paths(root) {
        let Some(raw_session_id) = derive_gemini_session_id(&path) else {
            continue;
        };

        let metadata = fs::metadata(path.as_path()).ok();
        let file_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        if file_size < 10 {
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

        let unique_id = format!("{}::{}", super::constants::TOOL_GEMINI, raw_session_id);
        let group = groups
            .entry(unique_id.clone())
            .or_insert_with(|| SessionGroupBuilder {
                session_id: unique_id.clone(),
                ..Default::default()
            });

        let path_string = path.to_string_lossy().to_string();
        if group.primary_file_path.is_none() {
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

    groups
        .into_values()
        .map(|mut group| {
            group.transcript_paths.sort();
            SessionFile {
                session_id: group.session_id,
                tool: super::constants::TOOL_GEMINI.to_string(),
                project_path: String::new(),
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
        .collect()
}

/// 解析一个 Gemini 会话文件，返回会话元数据和 per-message 请求事实。
pub(super) fn parse_gemini_session_file(
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
        source: "gemini_session".to_string(),
        message_ids: Vec::new(),
        scope: None,
    };

    let mut first_user_message: Option<String> = None;
    let mut last_user_message: Option<String> = None;
    let mut cwd_found: Option<String> = None;
    let mut models_set: BTreeSet<String> = BTreeSet::new();
    let mut earliest_timestamp: Option<i64> = None;
    let mut latest_timestamp: Option<i64> = None;
    let mut request_map: HashMap<String, LocalRequestRecord> = HashMap::new();
    let mut synthetic_index: u64 = 0;

    for transcript_path in &session.transcript_paths {
        let Ok(raw) = fs::read_to_string(transcript_path) else {
            continue;
        };
        let Ok(root) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };

        if cwd_found.is_none() {
            cwd_found = extract_gemini_cwd(&root);
        }

        let Some(messages) = root.get("messages").and_then(|value| value.as_array()) else {
            continue;
        };

        for message in messages {
            if let Some(ts) = extract_timestamp(message) {
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

            let msg_type = message.get("type").and_then(|value| value.as_str());

            if msg_type == Some("user") || msg_type == Some("human") {
                if let Some(text) = extract_gemini_user_text(message) {
                    if !is_gemini_system_message(&text) {
                        if first_user_message.is_none() {
                            first_user_message = Some(text.clone());
                        }
                        last_user_message = Some(text);
                    }
                }
                continue;
            }

            if msg_type != Some("gemini") {
                continue;
            }

            let Some(tokens) = message
                .get("tokens")
                .filter(|value| value.is_object())
                .and_then(extract_gemini_tokens)
            else {
                continue;
            };
            let total_tokens =
                tokens.input + tokens.output + tokens.cache_read + tokens.cache_create;
            if total_tokens == 0 {
                continue;
            }

            let model = message
                .get("model")
                .and_then(|value| value.as_str())
                .map(normalize_gemini_model)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            if model != "unknown" {
                models_set.insert(model.clone());
            }

            let timestamp = extract_timestamp(message).unwrap_or(session.last_modified);

            let message_id = message
                .get("id")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| {
                    synthetic_index += 1;
                    format!("gemini:{}:{}", session.session_id, synthetic_index)
                });

            let record = LocalRequestRecord {
                session_id: session.session_id.clone(),
                tool: session.tool.clone(),
                timestamp,
                message_id: message_id.clone(),
                input_tokens: tokens.input,
                output_tokens: tokens.output,
                reasoning_tokens: tokens.reasoning,
                cache_create_tokens: tokens.cache_create,
                cache_read_tokens: tokens.cache_read,
                total_tokens,
                model,
                is_subagent: false,
                ..Default::default()
            };

            request_map
                .entry(message_id)
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
    requests.sort_by_key(|request| request.timestamp);

    meta.cwd = cwd_found.clone();
    meta.project_name = cwd_found.as_ref().and_then(|cwd| extract_project_name(cwd));
    meta.topic = first_user_message.map(|text| truncate_string(&text, 50));
    meta.last_prompt = last_user_message.map(|text| truncate_string(&text, 100));
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

// ── 文件枚举 ──────────────────────────────────────────────────────────────────

fn collect_gemini_session_paths(root: &Path) -> Vec<PathBuf> {
    let mut queue = VecDeque::from([root.to_path_buf()]);
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

            let is_session = entry_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("session-") && name.ends_with(".json"))
                .unwrap_or(false);

            if is_session {
                files.push(entry_path);
            }
        }
    }

    files
}

fn derive_gemini_session_id(path: &Path) -> Option<String> {
    let file_stem = path.file_stem()?.to_string_lossy();
    if let Some(raw) = file_stem.strip_prefix("session-") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let trimmed = file_stem.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

// ── 字段提取 ──────────────────────────────────────────────────────────────────

/// Gemini 会话文件可能在顶层携带工作目录信息，键名因 CLI 版本而异。
fn extract_gemini_cwd(root: &serde_json::Value) -> Option<String> {
    for key in [
        "cwd",
        "projectRoot",
        "projectPath",
        "workspacePath",
        "workspace",
    ] {
        let value = root
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(value) = value {
            return Some(value.to_string());
        }
    }
    None
}

struct GeminiTokens {
    input: u64,
    output: u64,
    reasoning: u64,
    cache_create: u64,
    cache_read: u64,
}

/// 归一化 Gemini per-message token 用量。
///
/// Gemini 的 `input` 为 cache-inclusive 总 prompt，`cached` 是其中命中缓存的子集；
/// 为避免 total 双计，将 `cached` 从 `input` 扣出单独记为 cache_read（与 Codex 口径一致）。
/// `thoughts`（推理）与 `tool`（工具）token 计入输出；`thoughts` 另单独保留为 reasoning。
/// Gemini 日志不提供 cache creation，固定为 0。
fn extract_gemini_tokens(tokens: &serde_json::Value) -> Option<GeminiTokens> {
    let input_total = tokens
        .get("input")
        .and_then(parse_u64_from_value)
        .unwrap_or(0);
    let output = tokens
        .get("output")
        .and_then(parse_u64_from_value)
        .unwrap_or(0);
    let cached = tokens
        .get("cached")
        .and_then(parse_u64_from_value)
        .unwrap_or(0);
    let thoughts = tokens
        .get("thoughts")
        .and_then(parse_u64_from_value)
        .unwrap_or(0);
    let tool = tokens
        .get("tool")
        .and_then(parse_u64_from_value)
        .unwrap_or(0);

    if input_total == 0 && output == 0 && cached == 0 && thoughts == 0 && tool == 0 {
        return None;
    }

    let cache_read = cached.min(input_total);
    Some(GeminiTokens {
        input: input_total.saturating_sub(cache_read),
        output: output + thoughts + tool,
        reasoning: thoughts,
        cache_create: 0,
        cache_read,
    })
}

fn normalize_gemini_model(raw: &str) -> String {
    let mut name = raw.trim().to_lowercase();
    if let Some(pos) = name.rfind('/') {
        name = name[pos + 1..].to_string();
    }
    name
}

fn extract_gemini_user_text(message: &serde_json::Value) -> Option<String> {
    for key in ["text", "content", "message"] {
        if let Some(text) = message.get(key).and_then(|value| value.as_str()) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        if let Some(items) = message.get(key).and_then(|value| value.as_array()) {
            let mut parts = Vec::new();
            for item in items {
                if let Some(text) = item.as_str() {
                    parts.push(text.to_string());
                } else if let Some(text) = item.get("text").and_then(|value| value.as_str()) {
                    parts.push(text.to_string());
                }
            }
            let joined = parts.join("\n");
            if !joined.trim().is_empty() {
                return Some(joined.trim().to_string());
            }
        }
    }
    None
}

fn is_gemini_system_message(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with("# AGENTS.md")
        || trimmed.starts_with("<environment_context>")
        || trimmed.starts_with("<system-reminder>")
        || trimmed.chars().count() < 3
}

// ── 测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::constants::TOOL_GEMINI;
    use std::io::Write;
    use tempfile::tempdir;

    fn make_gemini_session(session_id: &str, path: String) -> SessionFile {
        SessionFile {
            session_id: session_id.to_string(),
            tool: TOOL_GEMINI.to_string(),
            project_path: String::new(),
            file_path: path.clone(),
            transcript_paths: vec![path],
            file_size: 0,
            last_modified: 0,
            fingerprint: 0,
        }
    }

    #[test]
    fn test_parse_gemini_session_per_message_tokens_and_meta() {
        let temp = tempdir().unwrap();
        let chats = temp.path().join("project-hash").join("chats");
        fs::create_dir_all(&chats).unwrap();
        let session_path = chats.join("session-abc.json");

        let doc = serde_json::json!({
            "sessionId": "abc",
            "cwd": "/Users/test/work/project-alpha",
            "messages": [
                {"type":"user","id":"u1","timestamp":"2026-05-09T10:00:00Z","content":"Fix the login bug"},
                {"type":"gemini","id":"g1","timestamp":"2026-05-09T10:00:01Z","model":"gemini-2.5-pro",
                 "tokens":{"input":100,"cached":40,"output":30,"thoughts":10}},
                {"type":"gemini","id":"g2","timestamp":"2026-05-09T10:00:02Z","model":"gemini-2.5-pro",
                 "tokens":{"input":50,"cached":5,"output":20,"thoughts":0}}
            ]
        });
        {
            let mut file = fs::File::create(&session_path).unwrap();
            write!(file, "{}", doc).unwrap();
        }

        let path = session_path.to_string_lossy().to_string();
        let session = make_gemini_session("gemini::abc", path);
        let (meta, requests) = parse_gemini_session_file(&session);

        assert_eq!(meta.tool, "gemini");
        assert_eq!(meta.source, "gemini_session");
        assert_eq!(meta.project_name, Some("project-alpha".to_string()));
        assert_eq!(meta.topic, Some("Fix the login bug".to_string()));
        assert_eq!(meta.models, vec!["gemini-2.5-pro".to_string()]);
        assert_eq!(meta.message_count, 2);

        assert_eq!(requests.len(), 2);
        // g1: cache_read=40, input=100-40=60, output=30+10=40, total=60+40+0+40=140
        assert_eq!(requests[0].input_tokens, 60);
        assert_eq!(requests[0].cache_read_tokens, 40);
        assert_eq!(requests[0].output_tokens, 40);
        assert_eq!(requests[0].reasoning_tokens, 10);
        assert_eq!(requests[0].cache_create_tokens, 0);
        assert_eq!(requests[0].total_tokens, 140);
        // g2: cache_read=5, input=45, output=20, total=70
        assert_eq!(requests[1].input_tokens, 45);
        assert_eq!(requests[1].cache_read_tokens, 5);
        assert_eq!(requests[1].output_tokens, 20);
        assert_eq!(requests[1].total_tokens, 70);

        assert_eq!(meta.total_input_tokens, 105);
        assert_eq!(meta.total_cache_read_tokens, 45);
        assert_eq!(meta.total_output_tokens, 60);
    }

    #[test]
    fn test_parse_gemini_skips_zero_and_non_gemini_messages() {
        let temp = tempdir().unwrap();
        let chats = temp.path().join("h").join("chats");
        fs::create_dir_all(&chats).unwrap();
        let session_path = chats.join("session-zero.json");

        let doc = serde_json::json!({
            "sessionId": "zero",
            "messages": [
                {"type":"gemini","id":"g1","model":"gemini-2.5-flash",
                 "tokens":{"input":0,"cached":0,"output":0,"thoughts":0}},
                {"type":"system","id":"s1","tokens":{"input":10,"output":10}},
                {"type":"gemini","id":"g2","model":"gemini-2.5-flash",
                 "tokens":{"input":12,"cached":0,"output":8}}
            ]
        });
        {
            let mut file = fs::File::create(&session_path).unwrap();
            write!(file, "{}", doc).unwrap();
        }

        let path = session_path.to_string_lossy().to_string();
        let session = make_gemini_session("gemini::zero", path);
        let (meta, requests) = parse_gemini_session_file(&session);

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].message_id, "g2");
        assert_eq!(requests[0].input_tokens, 12);
        assert_eq!(requests[0].output_tokens, 8);
        assert_eq!(meta.message_count, 1);
    }

    #[test]
    fn test_parse_gemini_dedupes_by_message_id_keeping_largest() {
        let temp = tempdir().unwrap();
        let chats = temp.path().join("h").join("chats");
        fs::create_dir_all(&chats).unwrap();
        let session_path = chats.join("session-dup.json");

        let doc = serde_json::json!({
            "sessionId": "dup",
            "messages": [
                {"type":"gemini","id":"g1","timestamp":"2026-05-09T10:00:01Z","model":"gemini-2.5-pro",
                 "tokens":{"input":10,"output":5}},
                {"type":"gemini","id":"g1","timestamp":"2026-05-09T10:00:02Z","model":"gemini-2.5-pro",
                 "tokens":{"input":10,"output":12}}
            ]
        });
        {
            let mut file = fs::File::create(&session_path).unwrap();
            write!(file, "{}", doc).unwrap();
        }

        let path = session_path.to_string_lossy().to_string();
        let session = make_gemini_session("gemini::dup", path);
        let (_, requests) = parse_gemini_session_file(&session);

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].output_tokens, 12);
    }

    #[test]
    fn test_collect_gemini_session_files_derives_id_from_filename() {
        let temp = tempdir().unwrap();
        let chats = temp.path().join("phash").join("chats");
        fs::create_dir_all(&chats).unwrap();
        let session_path = chats.join("session-xyz.json");
        {
            let mut file = fs::File::create(&session_path).unwrap();
            write!(
                file,
                "{}",
                serde_json::json!({"sessionId":"xyz","messages":[]})
            )
            .unwrap();
        }

        let sessions = collect_gemini_session_files(temp.path());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "gemini::xyz");
        assert_eq!(sessions[0].tool, "gemini");
    }
}
