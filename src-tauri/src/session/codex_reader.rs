//! Codex 会话文件解析器
//!
//! 解析 `~/.codex/sessions/**/rollout-*.jsonl` 格式的 Codex 会话日志，
//! 提取 token_count 差分事实和会话元信息。

use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::shared::{
    extract_project_name, extract_timestamp, parse_u64_from_value, truncate_string,
};
use super::source::{ParsedSessionData, SessionSource, SourceSnapshot, SourceUpdateMode};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub(super) struct CodexSource;

impl SessionSource for CodexSource {
    fn tool_id(&self) -> &'static str {
        super::constants::TOOL_CODEX
    }

    fn scan(&self) -> SourceSnapshot {
        let mut sessions = Vec::new();
        if let Some(home) = dirs::home_dir() {
            let codex_root = home.join(".codex").join("sessions");
            if codex_root.exists() {
                sessions.extend(collect_codex_session_files(&codex_root));
            }
        }
        // 额外扫描 WSL 发行版内的 Codex sessions（仅 Windows，且 wslScan 开启时）。
        #[cfg(windows)]
        if let Some(cfg) = super::wsl::scan_config_if_enabled() {
            for root in super::wsl::codex_session_roots(&cfg) {
                if root.exists() {
                    sessions.extend(collect_codex_session_files(&root));
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
        let data = parse_codex_session_file(session);
        Ok(ParsedSessionData {
            meta: data.meta,
            requests: data.requests,
        })
    }
}

/// Codex 解析结果，供 scanner.rs 消费。
pub(super) struct CodexParsedData {
    pub(super) meta: SessionMeta,
    pub(super) requests: Vec<LocalRequestRecord>,
}

#[derive(Clone, Debug, Default)]
struct CodexCumulativeTokens {
    input: u64,
    output: u64,
    cache_create: u64,
    cache_read: u64,
}

#[derive(Clone, Debug)]
pub(super) struct CodexRolloutIdentity {
    pub(super) root_session_id: String,
    pub(super) cwd: Option<String>,
    pub(super) is_subagent: bool,
    /// When this rollout is a fork, the Unix timestamp (seconds) at which the fork was
    /// created. All replayed events from the original session carry this same outer
    /// timestamp, so we can skip them to avoid double-counting.
    pub(super) fork_start_ts: Option<i64>,
}

pub(super) fn collect_codex_session_files(root: &Path) -> Vec<SessionFile> {
    #[derive(Default)]
    struct SessionGroupBuilder {
        session_id: String,
        project_path: String,
        primary_file_path: Option<String>,
        transcript_paths: Vec<String>,
        file_size: u64,
        last_modified: i64,
        fingerprint: u64,
    }

    let mut groups: HashMap<String, SessionGroupBuilder> = HashMap::new();

    for path in collect_codex_rollout_files(root) {
        let Some(identity) = inspect_codex_rollout_identity(&path) else {
            continue;
        };

        let metadata = std::fs::metadata(path.as_path()).ok();
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

        let unique_id = format!(
            "{}::{}",
            super::constants::TOOL_CODEX,
            identity.root_session_id
        );
        let project_name = identity
            .cwd
            .as_deref()
            .and_then(extract_project_name)
            .unwrap_or_default();
        let group = groups
            .entry(unique_id.clone())
            .or_insert_with(|| SessionGroupBuilder {
                session_id: unique_id.clone(),
                project_path: project_name.to_string(),
                ..Default::default()
            });

        let path_string = path.to_string_lossy().to_string();
        if group.primary_file_path.is_none() || !identity.is_subagent {
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
                tool: super::constants::TOOL_CODEX.to_string(),
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
        .collect()
}

/// 解析一个 Codex 会话（包括所有关联 rollout 文件），返回元数据和请求事实。
pub(super) fn parse_codex_session_file(session: &SessionFile) -> CodexParsedData {
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
        source: "codex_rollout".to_string(),
        message_ids: Vec::new(),
        scope: None,
    };

    let mut first_user_message: Option<String> = None;
    let mut last_message_summary: Option<String> = None;
    let mut cwd_found: Option<String> = None;
    let mut session_name_found: Option<String> = None;
    let mut models_set: BTreeSet<String> = BTreeSet::new();
    let mut earliest_timestamp: Option<i64> = None;
    let mut latest_timestamp: Option<i64> = None;
    let mut requests: Vec<LocalRequestRecord> = Vec::new();
    let mut event_index: u64 = 0;
    let mut first_user_message_ts: Option<i64> = None;
    let mut last_message_summary_ts: Option<i64> = None;

    for transcript_path in &session.transcript_paths {
        let file_identity = inspect_codex_rollout_identity(Path::new(transcript_path));
        let is_subagent_file = file_identity
            .as_ref()
            .map(|identity| identity.is_subagent)
            .unwrap_or(false);
        // For forked rollouts, all events with ts < fork_start_ts are replays of the
        // original session's history and must be skipped to avoid double-counting.
        let fork_start_ts: Option<i64> = file_identity
            .as_ref()
            .and_then(|identity| identity.fork_start_ts);
        let mut prev_total: Option<CodexCumulativeTokens> = None;
        let mut current_model: String = "unknown".to_string();

        let file_handle = match fs::File::open(transcript_path) {
            Ok(file) => file,
            Err(_) => continue,
        };
        let reader = BufReader::new(file_handle);

        for line in reader.lines().map_while(Result::ok) {
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };

            let ts = extract_timestamp(&json).unwrap_or(session.last_modified);
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

            let event_type = json
                .get("type")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            match event_type {
                "session_meta" => {
                    if let Some(payload) = json.get("payload") {
                        if is_codex_subagent(payload.get("source")) {
                            continue;
                        }
                        if session_name_found.is_none() && !is_subagent_file {
                            session_name_found = extract_codex_session_name(payload);
                        }
                        if cwd_found.is_none() {
                            cwd_found = payload
                                .get("cwd")
                                .and_then(|value| value.as_str())
                                .map(|value| value.to_string());
                        }
                        if let Some(model) = payload
                            .get("model")
                            .or_else(|| payload.get("model_provider"))
                            .and_then(|value| value.as_str())
                        {
                            let normalized = normalize_model_name(model);
                            if !normalized.is_empty() {
                                models_set.insert(normalized);
                            }
                        }
                    }
                }
                "turn_context" => {
                    if let Some(payload) = json.get("payload") {
                        if cwd_found.is_none() {
                            cwd_found = payload
                                .get("cwd")
                                .and_then(|value| value.as_str())
                                .map(|value| value.to_string());
                        }
                        if let Some(model) = payload
                            .get("model")
                            .or_else(|| payload.get("info").and_then(|info| info.get("model")))
                            .and_then(|value| value.as_str())
                        {
                            let normalized = normalize_model_name(model);
                            if !normalized.is_empty() {
                                current_model = normalized.clone();
                                models_set.insert(normalized);
                            }
                        }
                    }
                }
                "response_item" => {
                    if let Some(payload) = json.get("payload") {
                        if payload.get("type").and_then(|value| value.as_str()) == Some("message") {
                            let role = payload.get("role").and_then(|value| value.as_str());
                            let text =
                                extract_codex_response_item_text(payload).unwrap_or_default();
                            if !text.trim().is_empty() {
                                if role == Some("user")
                                    && !is_subagent_file
                                    && !is_codex_system_message(&text)
                                    && first_user_message_ts
                                        .map(|current| ts < current)
                                        .unwrap_or(true)
                                {
                                    first_user_message = Some(text.clone());
                                    first_user_message_ts = Some(ts);
                                }
                                if !is_subagent_file
                                    && !is_codex_system_message(&text)
                                    && last_message_summary_ts
                                        .map(|current| ts >= current)
                                        .unwrap_or(true)
                                {
                                    last_message_summary = Some(text);
                                    last_message_summary_ts = Some(ts);
                                }
                            }
                        }
                    }
                }
                "event_msg" => {
                    let Some(payload) = json.get("payload") else {
                        continue;
                    };
                    if payload.get("type").and_then(|value| value.as_str()) != Some("token_count") {
                        continue;
                    }
                    let Some(info) = payload.get("info") else {
                        continue;
                    };
                    if info.is_null() {
                        continue;
                    }

                    if let Some(model) = info
                        .get("model")
                        .or_else(|| info.get("model_name"))
                        .or_else(|| payload.get("model"))
                        .and_then(|value| value.as_str())
                    {
                        let normalized = normalize_model_name(model);
                        if !normalized.is_empty() {
                            current_model = normalized.clone();
                            models_set.insert(normalized);
                        }
                    }

                    let total_usage = info.get("total_token_usage");
                    let last_usage = info.get("last_token_usage");
                    let total = total_usage.and_then(parse_codex_cumulative_tokens);
                    let last = last_usage.and_then(parse_codex_cumulative_tokens);

                    // Fork replay handling: events at or before fork_start_ts are history
                    // replayed from the original session. Advance prev_total to establish
                    // the correct baseline so the first genuinely new event's delta is
                    // computed accurately — but do NOT emit a request record for them.
                    let is_fork_replay =
                        fork_start_ts.map(|fork_ts| ts <= fork_ts).unwrap_or(false);
                    if is_fork_replay {
                        if let Some(current_total) = total {
                            prev_total = Some(current_total);
                        }
                        continue;
                    }

                    let delta = if let Some(current_total) = total.clone() {
                        let delta = if let Some(previous_total) = prev_total.as_ref() {
                            if codex_total_rolled_back(previous_total, &current_total) {
                                last.clone().unwrap_or(current_total.clone())
                            } else {
                                compute_codex_delta(Some(previous_total), &current_total)
                            }
                        } else {
                            last.clone().unwrap_or(current_total.clone())
                        };
                        prev_total = Some(current_total);
                        Some(delta)
                    } else {
                        last.clone()
                    };

                    let Some(delta) = delta else {
                        continue;
                    };

                    let normalized = normalize_codex_delta(delta);
                    let total_tokens = normalized.input
                        + normalized.output
                        + normalized.cache_read
                        + normalized.cache_create;
                    if total_tokens == 0 {
                        continue;
                    }

                    event_index += 1;

                    requests.push(LocalRequestRecord {
                        session_id: session.session_id.clone(),
                        tool: session.tool.clone(),
                        timestamp: ts,
                        message_id: format!("codex:{}:{}", session.session_id, event_index),
                        input_tokens: normalized.input,
                        output_tokens: normalized.output,
                        cache_create_tokens: normalized.cache_create,
                        cache_read_tokens: normalized.cache_read,
                        total_tokens,
                        model: current_model.clone(),
                        is_subagent: false,
                        ..Default::default()
                    });
                }
                _ => {}
            }
        }
    }

    meta.cwd = cwd_found.clone();
    meta.project_name = cwd_found
        .as_ref()
        .and_then(|cwd| extract_project_name(cwd))
        .or_else(|| Some(session.project_path.clone()).filter(|value| !value.is_empty()));
    meta.topic = first_user_message.map(|text| truncate_string(&text, 50));
    meta.last_prompt = last_message_summary.map(|text| truncate_string(&text, 100));
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

    CodexParsedData { meta, requests }
}

// ── 文件枚举 ──────────────────────────────────────────────────────────────────

pub(super) fn collect_codex_rollout_files(root: &Path) -> Vec<PathBuf> {
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

            let is_rollout = entry_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
                .unwrap_or(false);

            if is_rollout {
                files.push(entry_path);
            }
        }
    }

    files
}

pub(super) fn derive_codex_session_id(path: &Path) -> Option<String> {
    let file_stem = path.file_stem()?.to_string_lossy();
    if let Some(raw) = file_stem.strip_prefix("rollout-") {
        return Some(raw.to_string());
    }
    Some(file_stem.to_string())
}

pub(super) fn inspect_codex_rollout_identity(path: &Path) -> Option<CodexRolloutIdentity> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    for line in reader.lines().map_while(Result::ok).take(20) {
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if json.get("type").and_then(|value| value.as_str()) != Some("session_meta") {
            continue;
        }
        let payload = json.get("payload")?;
        let session_id = payload
            .get("id")
            .or_else(|| payload.get("session_id"))
            .or_else(|| payload.get("sessionId"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .or_else(|| derive_codex_session_id(path))?;
        let is_subagent = is_codex_subagent(payload.get("source"));
        let root_session_id = payload
            .get("source")
            .and_then(|source| source.get("subagent"))
            .and_then(|subagent| subagent.get("thread_spawn"))
            .and_then(|spawn| spawn.get("parent_thread_id"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .unwrap_or_else(|| session_id.clone());
        let cwd = payload
            .get("cwd")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());

        // When the rollout was forked from another session, Codex replays the entire
        // original session history into the new file, stamping every replayed event
        // with the fork-creation timestamp. We capture that timestamp so the parser
        // can skip those events and avoid double-counting.
        let fork_start_ts = if payload.get("forked_from_id").is_some() {
            extract_timestamp(&json)
        } else {
            None
        };

        return Some(CodexRolloutIdentity {
            root_session_id,
            cwd,
            is_subagent,
            fork_start_ts,
        });
    }

    derive_codex_session_id(path).map(|session_id| CodexRolloutIdentity {
        root_session_id: session_id,
        cwd: None,
        is_subagent: false,
        fork_start_ts: None,
    })
}

// ── 文本提取 ──────────────────────────────────────────────────────────────────

fn extract_codex_session_name(payload: &serde_json::Value) -> Option<String> {
    for key in ["title", "name", "summary", "slug"] {
        let value = payload
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(value) = value {
            return Some(value.to_string());
        }
    }

    let id_value = payload
        .get("id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    if looks_like_uuid(id_value) {
        return None;
    }
    Some(id_value.to_string())
}

fn looks_like_uuid(value: &str) -> bool {
    let segments: Vec<&str> = value.split('-').collect();
    if segments.len() != 5 {
        return false;
    }
    let expected = [8, 4, 4, 4, 12];
    segments.iter().zip(expected.iter()).all(|(segment, len)| {
        segment.len() == *len && segment.chars().all(|ch| ch.is_ascii_hexdigit())
    })
}

fn extract_codex_response_item_text(payload: &serde_json::Value) -> Option<String> {
    if let Some(content) = payload.get("content").and_then(|value| value.as_str()) {
        return Some(content.to_string());
    }

    let content = payload.get("content")?.as_array()?;
    let mut text_parts = Vec::new();
    for item in content {
        let item_type = item.get("type").and_then(|value| value.as_str());
        match item_type {
            Some("output_text") | Some("text") => {
                if let Some(text) = item.get("text").and_then(|value| value.as_str()) {
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

fn is_codex_system_message(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with("# AGENTS.md") || trimmed.starts_with("<environment_context>")
}

fn is_codex_subagent(source: Option<&serde_json::Value>) -> bool {
    source
        .and_then(|value| value.as_object())
        .map(|source| source.contains_key("subagent"))
        .unwrap_or(false)
}

// ── 模型名称归一化 ────────────────────────────────────────────────────────────

fn normalize_model_name(raw: &str) -> String {
    let mut name = raw.trim().to_lowercase();
    if let Some(pos) = name.rfind('/') {
        name = name[pos + 1..].to_string();
    }

    if name.len() > 11 {
        let suffix = &name[name.len() - 11..];
        let bytes = suffix.as_bytes();
        if bytes.len() == 11
            && bytes[0] == b'-'
            && suffix[1..5].chars().all(|c| c.is_ascii_digit())
            && bytes[5] == b'-'
            && suffix[6..8].chars().all(|c| c.is_ascii_digit())
            && bytes[8] == b'-'
            && suffix[9..11].chars().all(|c| c.is_ascii_digit())
        {
            name.truncate(name.len() - 11);
        }
    }

    if name.len() > 9 {
        let parts: Vec<&str> = name.rsplitn(2, '-').collect();
        if parts.len() == 2 {
            let suffix = parts[0];
            if suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_digit()) {
                name = parts[1].to_string();
            }
        }
    }

    name
}

// ── Token 差分计算 ────────────────────────────────────────────────────────────

fn parse_codex_cumulative_tokens(value: &serde_json::Value) -> Option<CodexCumulativeTokens> {
    if !value.is_object() {
        return None;
    }

    Some(CodexCumulativeTokens {
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

fn codex_total_rolled_back(prev: &CodexCumulativeTokens, current: &CodexCumulativeTokens) -> bool {
    current.input < prev.input
        || current.output < prev.output
        || current.cache_create < prev.cache_create
        || current.cache_read < prev.cache_read
}

fn compute_codex_delta(
    prev: Option<&CodexCumulativeTokens>,
    current: &CodexCumulativeTokens,
) -> CodexCumulativeTokens {
    match prev {
        Some(previous) => CodexCumulativeTokens {
            input: current.input.saturating_sub(previous.input),
            output: current.output.saturating_sub(previous.output),
            cache_create: current.cache_create.saturating_sub(previous.cache_create),
            cache_read: current.cache_read.saturating_sub(previous.cache_read),
        },
        None => current.clone(),
    }
}

fn normalize_codex_delta(delta: CodexCumulativeTokens) -> CodexCumulativeTokens {
    let cache_read = delta.cache_read.min(delta.input);
    CodexCumulativeTokens {
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
    use crate::session::meta::SessionFile;
    use std::io::Write;
    use tempfile::tempdir;

    fn make_codex_session(session_id: &str, project_path: &str, paths: Vec<String>) -> SessionFile {
        SessionFile {
            session_id: session_id.to_string(),
            tool: TOOL_CODEX.to_string(),
            project_path: project_path.to_string(),
            file_path: paths.first().cloned().unwrap_or_default(),
            transcript_paths: paths,
            file_size: 0,
            last_modified: 0,
            fingerprint: 0,
        }
    }

    #[test]
    fn test_extract_codex_session_name_ignores_uuid_id() {
        let payload = serde_json::json!({
            "id": "019e1048-37a3-72b2-983f-37bb2abd16f6"
        });
        assert_eq!(extract_codex_session_name(&payload), None);
    }

    #[test]
    fn test_parse_codex_session_file_extracts_delta_requests_and_meta() {
        let temp = tempdir().unwrap();
        let codex_dir = temp.path().join("2026").join("05").join("09");
        fs::create_dir_all(&codex_dir).unwrap();
        let rollout_path = codex_dir.join("rollout-session-1.jsonl");

        {
            let mut file = fs::File::create(&rollout_path).unwrap();
            for line in [
                serde_json::json!({"timestamp":"2026-05-09T10:00:00Z","type":"session_meta","payload":{"id":"session-1","title":"Login bug triage","cwd":"/Users/test/work/project-alpha"}}),
                serde_json::json!({"timestamp":"2026-05-09T10:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":"Fix the login bug"}}),
                serde_json::json!({"timestamp":"2026-05-09T10:00:02Z","type":"turn_context","payload":{"model":"openai/gpt-5.4-2026-03-05"}}),
                serde_json::json!({"timestamp":"2026-05-09T10:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":30}}}}),
                serde_json::json!({"timestamp":"2026-05-09T10:00:04Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":160,"cached_input_tokens":50,"output_tokens":55}}}}),
            ] {
                writeln!(file, "{}", line).unwrap();
            }
        }

        let path = rollout_path.to_string_lossy().to_string();
        let session = make_codex_session("project-alpha::session-1", "project-alpha", vec![path]);
        let data = parse_codex_session_file(&session);

        assert_eq!(data.meta.tool, "codex");
        assert_eq!(data.meta.project_name, Some("project-alpha".to_string()));
        assert_eq!(data.meta.session_name, Some("Login bug triage".to_string()));
        assert_eq!(data.meta.topic, Some("Fix the login bug".to_string()));
        assert_eq!(data.meta.models, vec!["gpt-5.4".to_string()]);
        assert_eq!(data.meta.message_count, 2);
        assert_eq!(data.requests.len(), 2);
        assert_eq!(data.requests[0].input_tokens, 60);
        assert_eq!(data.requests[0].cache_read_tokens, 40);
        assert_eq!(data.requests[0].output_tokens, 30);
        assert_eq!(data.requests[0].total_tokens, 130);
        assert_eq!(data.requests[1].input_tokens, 50);
        assert_eq!(data.requests[1].cache_read_tokens, 10);
        assert_eq!(data.requests[1].output_tokens, 25);
        assert_eq!(data.requests[1].total_tokens, 85);
        assert_eq!(data.meta.total_input_tokens, 110);
        assert_eq!(data.meta.total_cache_read_tokens, 50);
        assert_eq!(data.meta.total_output_tokens, 55);
    }

    #[test]
    fn test_parse_codex_session_file_falls_back_to_last_token_usage_after_reset() {
        let temp = tempdir().unwrap();
        let codex_dir = temp.path().join("2026").join("05").join("09");
        fs::create_dir_all(&codex_dir).unwrap();
        let rollout_path = codex_dir.join("rollout-session-reset.jsonl");

        {
            let mut file = fs::File::create(&rollout_path).unwrap();
            for line in [
                serde_json::json!({"timestamp":"2026-05-09T10:00:00Z","type":"session_meta","payload":{"id":"session-reset","cwd":"/Users/test/work/project-beta"}}),
                serde_json::json!({"timestamp":"2026-05-09T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":80,"cached_input_tokens":20,"output_tokens":10}}}}),
                serde_json::json!({"timestamp":"2026-05-09T10:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":30,"cached_input_tokens":5,"output_tokens":4},"last_token_usage":{"input_tokens":30,"cached_input_tokens":5,"output_tokens":4}}}}),
            ] {
                writeln!(file, "{}", line).unwrap();
            }
        }

        let path = rollout_path.to_string_lossy().to_string();
        let session = make_codex_session("project-beta::session-reset", "project-beta", vec![path]);
        let data = parse_codex_session_file(&session);

        assert_eq!(data.requests.len(), 2);
        assert_eq!(data.requests[1].input_tokens, 25);
        assert_eq!(data.requests[1].cache_read_tokens, 5);
        assert_eq!(data.requests[1].output_tokens, 4);
    }

    #[test]
    fn test_parse_codex_session_file_keeps_each_rollout_independent() {
        let temp = tempdir().unwrap();
        let codex_dir = temp.path().join("2026").join("05").join("09");
        fs::create_dir_all(&codex_dir).unwrap();
        let rollout_a = codex_dir.join("rollout-a.jsonl");
        let rollout_b = codex_dir.join("rollout-b.jsonl");

        {
            let mut file = fs::File::create(&rollout_a).unwrap();
            writeln!(file, "{}", serde_json::json!({"timestamp":"2026-05-09T10:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10}}}})).unwrap();
            writeln!(file, "{}", serde_json::json!({"timestamp":"2026-05-09T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":150,"cached_input_tokens":30,"output_tokens":20}}}})).unwrap();
        }
        {
            let mut file = fs::File::create(&rollout_b).unwrap();
            writeln!(file, "{}", serde_json::json!({"timestamp":"2026-05-09T10:01:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":50,"cached_input_tokens":10,"output_tokens":5}}}})).unwrap();
        }

        let path_a = rollout_a.to_string_lossy().to_string();
        let path_b = rollout_b.to_string_lossy().to_string();
        let session = make_codex_session(
            "project-x::session-multi",
            "project-x",
            vec![path_a, path_b],
        );
        let data = parse_codex_session_file(&session);

        // rollout-a: event1 uses last (100-20=80 input, 10 out), event2 uses delta (50-10=40 input, 10 out)
        // rollout-b: event1 uses last (50-10=40 input, 5 out)
        assert_eq!(data.requests.len(), 3);
        let total_input: u64 = data.requests.iter().map(|r| r.input_tokens).sum();
        let total_output: u64 = data.requests.iter().map(|r| r.output_tokens).sum();
        // Each rollout resets prev_total, so they're independent
        assert!(total_input > 0);
        assert!(total_output > 0);
    }

    #[test]
    fn test_parse_codex_forked_session_skips_replayed_history() {
        // Validates the fork-replay fix:
        // - The fork rollout begins with the original session's full history (outer ts = fork time).
        // - Only events with ts > fork_start_ts are genuinely new and should be counted.
        // - prev_total must be advanced through the replay so the first new event delta is correct
        //   even when last_token_usage is absent.
        let temp = tempdir().unwrap();
        let dir = temp.path().join("2026").join("06").join("16");
        fs::create_dir_all(&dir).unwrap();

        let fork_ts = "2026-06-16T10:00:00Z"; // fork creation time
        let new_ts  = "2026-06-16T10:05:00Z"; // a real new turn, clearly after fork

        let rollout_path = dir.join("rollout-fork-session.jsonl");
        {
            let mut f = fs::File::create(&rollout_path).unwrap();

            // Fork's own session_meta (has forked_from_id, outer ts = fork_ts)
            writeln!(f, "{}", serde_json::json!({
                "timestamp": fork_ts,
                "type": "session_meta",
                "payload": {
                    "id": "fork-session-id",
                    "forked_from_id": "original-session-id",
                    "cwd": "/work/project"
                }
            })).unwrap();

            // Replayed history event 1: cumulative 100 input, 30 output (outer ts = fork_ts)
            writeln!(f, "{}", serde_json::json!({
                "timestamp": fork_ts,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": { "input_tokens": 100, "output_tokens": 30 }
                    }
                }
            })).unwrap();

            // Replayed history event 2: cumulative 200 input, 60 output (outer ts = fork_ts)
            writeln!(f, "{}", serde_json::json!({
                "timestamp": fork_ts,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": { "input_tokens": 200, "output_tokens": 60 }
                    }
                }
            })).unwrap();

            // First genuine new turn after fork: cumulative grows by 50 input, 20 output.
            // No last_token_usage — the fix must derive delta via prev_total baseline.
            writeln!(f, "{}", serde_json::json!({
                "timestamp": new_ts,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": { "input_tokens": 250, "output_tokens": 80 }
                    }
                }
            })).unwrap();
        }

        let path = rollout_path.to_string_lossy().to_string();
        let session = make_codex_session(
            &format!("{}::fork-session-id", TOOL_CODEX),
            "project",
            vec![path],
        );
        let data = parse_codex_session_file(&session);

        // Only the one genuinely new event should produce a record.
        assert_eq!(data.requests.len(), 1,
            "expected exactly 1 request (the new turn), got {}", data.requests.len());

        // Delta = 250-200=50 input (net), 80-60=20 output.
        // normalize_codex_delta may reclassify some input as cache_read; sum them all.
        let total_input = data.requests[0].input_tokens + data.requests[0].cache_read_tokens;
        let total_output = data.requests[0].output_tokens;
        assert_eq!(total_input, 50, "input delta should be 50, got {total_input}");
        assert_eq!(total_output, 20, "output delta should be 20, got {total_output}");
    }
}
