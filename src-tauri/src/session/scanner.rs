//! 会话文件扫描器
//!
//! 统一扫描 Claude Code / Codex 本地 transcript，并构建两类缓存：
//! - session 级聚合结果（会话列表 / 详情 / 项目统计）
//! - request 级事实记录（概览 / 趋势 / 活动图）
//!
//! 关键原则：
//! - Claude 以 assistant `message.id` 为基础主键
//! - Codex 以 rollout token_count 事件主键为基础事实
//! - Claude 子代理 transcript 合并到所属主 session
//! - 所有页面从同一批去重后的 request 事实层聚合

use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use crate::models::ToolFilter;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

const TOOL_CLAUDE_CODE: &str = "claude_code";
const TOOL_CODEX: &str = "codex";

/// 缓存条目
struct CacheEntry {
    /// 会话级聚合结果
    data: Vec<SessionMeta>,
    /// 去重后的请求事实
    requests: Vec<LocalRequestRecord>,
    /// message_id -> session_id 索引（用于快速查找）
    message_to_session: HashMap<String, String>,
    /// session_id -> 内容指纹（用于增量更新检测）
    session_fingerprints: HashMap<String, u64>,
}

/// 解析后的单个会话结果
pub(crate) struct ParsedSessionData {
    meta: SessionMeta,
    requests: Vec<LocalRequestRecord>,
}

/// 全局会话元数据缓存
static SESSION_CACHE: OnceLock<Arc<Mutex<Option<CacheEntry>>>> = OnceLock::new();

fn get_cache() -> &'static Arc<Mutex<Option<CacheEntry>>> {
    SESSION_CACHE.get_or_init(|| Arc::new(Mutex::new(None)))
}

pub fn get_all_session_meta_cached() -> Vec<SessionMeta> {
    ensure_cache_ready().data
}

#[allow(dead_code)]
pub fn get_all_local_request_records_cached() -> Vec<LocalRequestRecord> {
    ensure_cache_ready().requests
}

#[allow(dead_code)]
pub fn get_local_request_records_by_session_cached(session_id: &str) -> Vec<LocalRequestRecord> {
    ensure_cache_ready()
        .requests
        .into_iter()
        .filter(|record| record.session_id == session_id)
        .collect()
}

#[allow(dead_code)]
struct CacheSnapshot {
    data: Vec<SessionMeta>,
    requests: Vec<LocalRequestRecord>,
}

fn ensure_cache_ready() -> CacheSnapshot {
    let cache = get_cache();

    {
        let cache_guard = cache.lock().unwrap();
        if cache_guard.is_some() {
            drop(cache_guard);
            return incremental_update_cache();
        }
    }

    full_scan_and_cache()
}

fn full_scan_and_cache() -> CacheSnapshot {
    let cache = get_cache();
    let session_files = scan_session_files();

    let mut data = Vec::new();
    let mut requests = Vec::new();
    let mut message_to_session = HashMap::new();
    let mut session_fingerprints = HashMap::new();

    for session_file in &session_files {
        let parsed = parse_session_file(session_file);
        for request in &parsed.requests {
            message_to_session.insert(request.message_id.clone(), request.session_id.clone());
        }
        session_fingerprints.insert(session_file.session_id.clone(), session_file.fingerprint);
        requests.extend(parsed.requests);
        data.push(parsed.meta);
    }

    data.sort_by_key(|meta| std::cmp::Reverse(meta.last_modified));
    requests.sort_by_key(|record| record.timestamp);

    {
        let mut cache_guard = cache.lock().unwrap();
        *cache_guard = Some(CacheEntry {
            data: data.clone(),
            requests: requests.clone(),
            message_to_session,
            session_fingerprints,
        });
    }

    CacheSnapshot { data, requests }
}

fn incremental_update_cache() -> CacheSnapshot {
    let cache = get_cache();
    let current_files = scan_session_files();
    let current_file_map: HashMap<String, SessionFile> = current_files
        .into_iter()
        .map(|file| (file.session_id.clone(), file))
        .collect();
    let current_fingerprints: HashMap<String, u64> = current_file_map
        .iter()
        .map(|(session_id, file)| (session_id.clone(), file.fingerprint))
        .collect();

    let mut cache_guard = cache.lock().unwrap();
    let entry = match cache_guard.as_mut() {
        Some(entry) => entry,
        None => return full_scan_and_cache(),
    };

    let cached_ids: HashSet<String> = entry.session_fingerprints.keys().cloned().collect();
    let current_ids: HashSet<String> = current_fingerprints.keys().cloned().collect();

    let deleted_ids: HashSet<String> = cached_ids.difference(&current_ids).cloned().collect();
    let mut changed_ids: HashSet<String> = deleted_ids.clone();

    for session_id in current_ids.intersection(&cached_ids) {
        let current = current_fingerprints
            .get(session_id)
            .copied()
            .unwrap_or_default();
        let cached = entry
            .session_fingerprints
            .get(session_id)
            .copied()
            .unwrap_or_default();
        if current != cached {
            changed_ids.insert(session_id.clone());
        }
    }

    let new_ids: Vec<String> = current_ids.difference(&cached_ids).cloned().collect();

    if changed_ids.is_empty() && new_ids.is_empty() {
        return CacheSnapshot {
            data: entry.data.clone(),
            requests: entry.requests.clone(),
        };
    }

    entry.data.retain(|meta| {
        !changed_ids.contains(&meta.session_id) && !new_ids.contains(&meta.session_id)
    });
    entry.requests.retain(|record| {
        !changed_ids.contains(&record.session_id) && !new_ids.contains(&record.session_id)
    });
    entry
        .message_to_session
        .retain(|_, session_id| !changed_ids.contains(session_id) && !new_ids.contains(session_id));
    entry
        .session_fingerprints
        .retain(|session_id, _| !changed_ids.contains(session_id) && !new_ids.contains(session_id));

    for session_id in changed_ids.into_iter().chain(new_ids.into_iter()) {
        let Some(file) = current_file_map.get(&session_id) else {
            continue;
        };
        let parsed = parse_session_file(file);
        for request in &parsed.requests {
            entry
                .message_to_session
                .insert(request.message_id.clone(), request.session_id.clone());
        }
        entry
            .session_fingerprints
            .insert(session_id, file.fingerprint);
        entry.requests.extend(parsed.requests);
        entry.data.push(parsed.meta);
    }

    entry
        .data
        .sort_by_key(|meta| std::cmp::Reverse(meta.last_modified));
    entry.requests.sort_by_key(|record| record.timestamp);

    CacheSnapshot {
        data: entry.data.clone(),
        requests: entry.requests.clone(),
    }
}

pub fn find_session_id_by_message_id(message_id: &str) -> Option<String> {
    let cache = get_cache();

    {
        let cache_guard = cache.lock().unwrap();
        if cache_guard.is_none() {
            drop(cache_guard);
            let _ = ensure_cache_ready();
        }
    }

    let cache_guard = cache.lock().unwrap();
    cache_guard
        .as_ref()
        .and_then(|entry| entry.message_to_session.get(message_id).cloned())
}

#[allow(dead_code)]
pub fn invalidate_cache() {
    let cache = get_cache();
    let mut cache_guard = cache.lock().unwrap();
    *cache_guard = None;
}

/// 扫描 Claude 项目目录中的所有会话 transcript 组
pub fn scan_session_files() -> Vec<SessionFile> {
    let mut roots: Vec<(&str, PathBuf)> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push((TOOL_CLAUDE_CODE, home.join(".claude").join("projects")));
        roots.push((
            TOOL_CLAUDE_CODE,
            home.join(".config").join("claude").join("projects"),
        ));
        roots.push((TOOL_CODEX, home.join(".codex").join("sessions")));
    }

    #[derive(Default)]
    struct SessionGroupBuilder {
        tool: String,
        project_path: String,
        session_id: String,
        primary_file_path: Option<String>,
        transcript_paths: Vec<String>,
        file_size: u64,
        last_modified: i64,
        fingerprint: u64,
    }

    let mut groups: HashMap<String, SessionGroupBuilder> = HashMap::new();

    for (tool, root) in roots {
        if !root.exists() {
            continue;
        }

        if tool == TOOL_CLAUDE_CODE {
            let Ok(entries) = fs::read_dir(&root) else {
                continue;
            };

            for entry in entries.flatten() {
                let project_path = entry.path();
                if !project_path.is_dir() {
                    continue;
                }

                let project_name = project_path
                    .file_name()
                    .and_then(|n| n.to_str())
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
                    let group =
                        groups
                            .entry(unique_id.clone())
                            .or_insert_with(|| SessionGroupBuilder {
                                tool: tool.to_string(),
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
        } else if tool == TOOL_CODEX {
            for path in collect_codex_rollout_files(&root) {
                let Some(identity) = inspect_codex_rollout_identity(&path) else {
                    continue;
                };

                let metadata = fs::metadata(&path).ok();
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

                let unique_id = format!("codex::{}", identity.root_session_id);
                let project_name = identity
                    .cwd
                    .as_deref()
                    .and_then(extract_project_name)
                    .unwrap_or_default();
                let group =
                    groups
                        .entry(unique_id.clone())
                        .or_insert_with(|| SessionGroupBuilder {
                            tool: tool.to_string(),
                            project_path: project_name.to_string(),
                            session_id: unique_id.clone(),
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
        }
    }

    let mut sessions: Vec<SessionFile> = groups
        .into_values()
        .map(|mut group| {
            group.transcript_paths.sort();
            SessionFile {
                session_id: group.session_id,
                tool: group.tool,
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

pub(crate) fn parse_session_file(session: &SessionFile) -> ParsedSessionData {
    if session.tool == TOOL_CODEX {
        return parse_codex_session_file(session);
    }

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

    ParsedSessionData { meta, requests }
}

pub fn parse_session_file_for_storage(
    session: &SessionFile,
) -> (SessionMeta, Vec<LocalRequestRecord>) {
    let parsed = parse_session_file(session);
    (parsed.meta, parsed.requests)
}

fn parse_codex_session_file(session: &SessionFile) -> ParsedSessionData {
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
                            session_name_found = payload
                                .get("id")
                                .and_then(|value| value.as_str())
                                .map(|value| value.to_string());
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

    ParsedSessionData { meta, requests }
}

#[derive(Clone, Debug, Default)]
struct CodexCumulativeTokens {
    input: u64,
    output: u64,
    cache_create: u64,
    cache_read: u64,
}

#[derive(Clone, Debug)]
struct CodexRolloutIdentity {
    root_session_id: String,
    cwd: Option<String>,
    is_subagent: bool,
}

fn collect_codex_rollout_files(root: &Path) -> Vec<PathBuf> {
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

fn derive_codex_session_id(path: &Path) -> Option<String> {
    let file_stem = path.file_stem()?.to_string_lossy();
    if let Some(raw) = file_stem.strip_prefix("rollout-") {
        return Some(raw.to_string());
    }
    Some(file_stem.to_string())
}

fn inspect_codex_rollout_identity(path: &Path) -> Option<CodexRolloutIdentity> {
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

        return Some(CodexRolloutIdentity {
            root_session_id,
            cwd,
            is_subagent,
        });
    }

    derive_codex_session_id(path).map(|session_id| CodexRolloutIdentity {
        root_session_id: session_id,
        cwd: None,
        is_subagent: false,
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

fn extract_project_name(cwd: &str) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }

    let normalized = cwd.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    parts.last().map(|value| value.to_string())
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

fn extract_u64_by_keys(value: &serde_json::Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(parse_u64_from_value))
        .unwrap_or(0)
}

fn parse_u64_from_value(value: &serde_json::Value) -> Option<u64> {
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

fn extract_model(json: &serde_json::Value) -> Option<String> {
    let model = json
        .get("message")
        .and_then(|message| message.get("model"))
        .and_then(|value| value.as_str())
        .or_else(|| json.get("model").and_then(|value| value.as_str()));

    let model = model?;
    if model.is_empty() || model == "unknown" {
        return None;
    }
    if model.starts_with('<') && model.ends_with('>') {
        return None;
    }
    Some(model.to_string())
}

fn extract_timestamp(json: &serde_json::Value) -> Option<i64> {
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

fn truncate_string(value: &str, max_len: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max_len {
        trimmed.to_string()
    } else {
        let truncated: String = trimmed.chars().take(max_len).collect();
        format!("{truncated}...")
    }
}

#[allow(dead_code)]
pub fn get_session_meta_by_id(session_id: &str) -> Option<SessionMeta> {
    let all_meta = get_all_session_meta_cached();
    all_meta
        .iter()
        .find(|meta| meta.session_id == session_id)
        .cloned()
        .or_else(|| {
            let id_suffix = session_id.split("::").last().unwrap_or(session_id);
            all_meta.into_iter().find(|meta| {
                meta.session_id
                    .split("::")
                    .last()
                    .unwrap_or(&meta.session_id)
                    == id_suffix
            })
        })
}

#[allow(dead_code)]
fn matches_tool(tool: &str, filter: &ToolFilter) -> bool {
    match filter {
        ToolFilter::All => true,
        ToolFilter::Tool(tool_filter) if tool_filter.trim().is_empty() => true,
        ToolFilter::Tool(tool_filter) => tool == *tool_filter,
    }
}

#[allow(dead_code)]
pub fn matches_tool_filter(meta: &SessionMeta, filter: &ToolFilter) -> bool {
    matches_tool(&meta.tool, filter)
}

#[allow(dead_code)]
pub fn matches_request_tool_filter(record: &LocalRequestRecord, filter: &ToolFilter) -> bool {
    matches_tool(&record.tool, filter)
}

#[allow(dead_code)]
pub fn get_all_session_meta(limit: usize) -> Vec<SessionMeta> {
    get_all_session_meta_cached()
        .into_iter()
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_truncate_string() {
        assert_eq!(truncate_string("hello", 10), "hello");
        assert_eq!(truncate_string("hello world", 5), "hello...");
    }

    #[test]
    fn test_is_system_message() {
        assert!(is_system_message("<local-command-caveat>some text"));
        assert!(is_system_message("prefix <local-command-caveat> content"));
        assert!(is_system_message("<command-name>/model</command-name>"));
        assert!(is_system_message(
            "<local-command-stdout>Set model to...</local-command-stdout>"
        ));
        assert!(is_system_message(
            "<system-reminder>Some reminder</system-reminder>"
        ));
        assert!(is_system_message("ab"));

        assert!(!is_system_message("请帮我分析这段代码"));
        assert!(!is_system_message("How do I fix this bug?"));
        assert!(!is_system_message("分析一下项目结构"));
    }

    #[test]
    fn test_extract_project_name() {
        assert_eq!(
            extract_project_name("/Users/test/projects/my-app"),
            Some("my-app".to_string())
        );
        assert_eq!(
            extract_project_name("/home/user/code/UsageMeter"),
            Some("UsageMeter".to_string())
        );
        assert_eq!(
            extract_project_name("C:\\Users\\test\\project"),
            Some("project".to_string())
        );
        assert_eq!(extract_project_name(""), None);
        assert_eq!(extract_project_name("/"), None);
    }

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
    fn test_parse_session_file_uses_min_max_timestamps_across_transcripts() {
        let temp = tempdir().unwrap();
        let project_dir = temp.path().join("project");
        let subagent_dir = project_dir.join("session-1").join("subagents");
        fs::create_dir_all(&subagent_dir).unwrap();

        let primary_path = project_dir.join("session-1.jsonl");
        let subagent_path = subagent_dir.join("agent-1.jsonl");

        {
            let mut file = fs::File::create(&primary_path).unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "type": "assistant",
                    "timestamp": 300,
                    "message": {
                        "id": "msg_primary",
                        "model": "claude-3-7-sonnet",
                        "usage": { "input_tokens": 10, "output_tokens": 5 }
                    }
                })
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "type": "assistant",
                    "timestamp": 100,
                    "message": {
                        "id": "msg_primary_early",
                        "model": "claude-3-7-sonnet",
                        "usage": { "input_tokens": 8, "output_tokens": 4 }
                    }
                })
            )
            .unwrap();
        }

        {
            let mut file = fs::File::create(&subagent_path).unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "type": "assistant",
                    "timestamp": 500,
                    "message": {
                        "id": "msg_subagent",
                        "model": "claude-3-7-sonnet",
                        "usage": { "input_tokens": 6, "output_tokens": 3 }
                    }
                })
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "type": "assistant",
                    "timestamp": 200,
                    "message": {
                        "id": "msg_subagent_mid",
                        "model": "claude-3-7-sonnet",
                        "usage": { "input_tokens": 4, "output_tokens": 2 }
                    }
                })
            )
            .unwrap();
        }

        let session = SessionFile {
            session_id: "project::session-1".to_string(),
            tool: "claude_code".to_string(),
            project_path: "project".to_string(),
            file_path: primary_path.to_string_lossy().to_string(),
            transcript_paths: vec![
                primary_path.to_string_lossy().to_string(),
                subagent_path.to_string_lossy().to_string(),
            ],
            file_size: 0,
            last_modified: 999,
            fingerprint: 0,
        };

        let parsed = parse_session_file(&session);
        assert_eq!(parsed.meta.start_time, 100);
        assert_eq!(parsed.meta.end_time, 500);
    }

    #[test]
    fn test_parse_codex_session_file_extracts_delta_requests_and_meta() {
        let temp = tempdir().unwrap();
        let codex_dir = temp.path().join("2026").join("05").join("09");
        fs::create_dir_all(&codex_dir).unwrap();
        let rollout_path = codex_dir.join("rollout-session-1.jsonl");

        {
            let mut file = fs::File::create(&rollout_path).unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "timestamp": "2026-05-09T10:00:00Z",
                    "type": "session_meta",
                    "payload": {
                        "id": "session-1",
                        "cwd": "/Users/test/work/project-alpha"
                    }
                })
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "timestamp": "2026-05-09T10:00:01Z",
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "user",
                        "content": "Fix the login bug"
                    }
                })
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "timestamp": "2026-05-09T10:00:02Z",
                    "type": "turn_context",
                    "payload": {
                        "model": "openai/gpt-5.4-2026-03-05"
                    }
                })
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "timestamp": "2026-05-09T10:00:03Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 100,
                                "cached_input_tokens": 40,
                                "output_tokens": 30
                            }
                        }
                    }
                })
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "timestamp": "2026-05-09T10:00:04Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 160,
                                "cached_input_tokens": 50,
                                "output_tokens": 55
                            }
                        }
                    }
                })
            )
            .unwrap();
        }

        let session = SessionFile {
            session_id: "project-alpha::session-1".to_string(),
            tool: TOOL_CODEX.to_string(),
            project_path: "project-alpha".to_string(),
            file_path: rollout_path.to_string_lossy().to_string(),
            transcript_paths: vec![rollout_path.to_string_lossy().to_string()],
            file_size: 0,
            last_modified: 0,
            fingerprint: 0,
        };

        let parsed = parse_session_file(&session);
        assert_eq!(parsed.meta.tool, TOOL_CODEX);
        assert_eq!(parsed.meta.project_name, Some("project-alpha".to_string()));
        assert_eq!(parsed.meta.topic, Some("Fix the login bug".to_string()));
        assert_eq!(parsed.meta.models, vec!["gpt-5.4".to_string()]);
        assert_eq!(parsed.meta.message_count, 2);
        assert_eq!(parsed.requests.len(), 2);

        assert_eq!(parsed.requests[0].input_tokens, 60);
        assert_eq!(parsed.requests[0].cache_read_tokens, 40);
        assert_eq!(parsed.requests[0].output_tokens, 30);
        assert_eq!(parsed.requests[0].total_tokens, 130);

        assert_eq!(parsed.requests[1].input_tokens, 50);
        assert_eq!(parsed.requests[1].cache_read_tokens, 10);
        assert_eq!(parsed.requests[1].output_tokens, 25);
        assert_eq!(parsed.requests[1].total_tokens, 85);

        assert_eq!(parsed.meta.total_input_tokens, 110);
        assert_eq!(parsed.meta.total_cache_read_tokens, 50);
        assert_eq!(parsed.meta.total_output_tokens, 55);
    }

    #[test]
    fn test_parse_codex_session_file_falls_back_to_last_token_usage_after_reset() {
        let temp = tempdir().unwrap();
        let codex_dir = temp.path().join("2026").join("05").join("09");
        fs::create_dir_all(&codex_dir).unwrap();
        let rollout_path = codex_dir.join("rollout-session-reset.jsonl");

        {
            let mut file = fs::File::create(&rollout_path).unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "timestamp": "2026-05-09T10:00:00Z",
                    "type": "session_meta",
                    "payload": {
                        "id": "session-reset",
                        "cwd": "/Users/test/work/project-beta"
                    }
                })
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "timestamp": "2026-05-09T10:00:01Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 80,
                                "cached_input_tokens": 20,
                                "output_tokens": 10
                            }
                        }
                    }
                })
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "timestamp": "2026-05-09T10:00:02Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 30,
                                "cached_input_tokens": 5,
                                "output_tokens": 4
                            },
                            "last_token_usage": {
                                "input_tokens": 30,
                                "cached_input_tokens": 5,
                                "output_tokens": 4
                            }
                        }
                    }
                })
            )
            .unwrap();
        }

        let session = SessionFile {
            session_id: "project-beta::session-reset".to_string(),
            tool: TOOL_CODEX.to_string(),
            project_path: "project-beta".to_string(),
            file_path: rollout_path.to_string_lossy().to_string(),
            transcript_paths: vec![rollout_path.to_string_lossy().to_string()],
            file_size: 0,
            last_modified: 0,
            fingerprint: 0,
        };

        let parsed = parse_session_file(&session);
        assert_eq!(parsed.requests.len(), 2);
        assert_eq!(parsed.requests[1].input_tokens, 25);
        assert_eq!(parsed.requests[1].cache_read_tokens, 5);
        assert_eq!(parsed.requests[1].output_tokens, 4);
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
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "timestamp": "2026-05-09T10:00:00Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 100,
                                "cached_input_tokens": 20,
                                "output_tokens": 10
                            }
                        }
                    }
                })
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "timestamp": "2026-05-09T10:00:01Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 120,
                                "cached_input_tokens": 30,
                                "output_tokens": 15
                            }
                        }
                    }
                })
            )
            .unwrap();
        }

        {
            let mut file = fs::File::create(&rollout_b).unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "timestamp": "2026-05-09T10:10:00Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 50,
                                "cached_input_tokens": 5,
                                "output_tokens": 7
                            }
                        }
                    }
                })
            )
            .unwrap();
        }

        let session = SessionFile {
            session_id: "project-gamma::session-multi".to_string(),
            tool: TOOL_CODEX.to_string(),
            project_path: "project-gamma".to_string(),
            file_path: rollout_a.to_string_lossy().to_string(),
            transcript_paths: vec![
                rollout_a.to_string_lossy().to_string(),
                rollout_b.to_string_lossy().to_string(),
            ],
            file_size: 0,
            last_modified: 0,
            fingerprint: 0,
        };

        let parsed = parse_session_file(&session);
        assert_eq!(parsed.requests.len(), 3);
        assert_eq!(parsed.requests[0].input_tokens, 80);
        assert_eq!(parsed.requests[1].input_tokens, 10);
        assert_eq!(parsed.requests[2].input_tokens, 45);
    }
}
