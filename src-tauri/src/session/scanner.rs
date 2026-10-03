//! 会话文件扫描器
//!
//! 统一扫描 Claude Code / Codex / OpenCode 本地 transcript，并构建两类缓存：
//! - session 级聚合结果（会话列表 / 详情 / 项目统计）
//! - request 级事实记录（概览 / 趋势 / 活动图）

use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::registry::all_sources;
use super::source::{ParsedSessionData, SourceSnapshot, SourceUpdateMode, UsageSource};
use crate::models::ToolFilter;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

struct CacheEntry {
    data: Vec<SessionMeta>,
    requests: Vec<LocalRequestRecord>,
    message_to_session: HashMap<String, String>,
    session_fingerprints: HashMap<String, u64>,
    source_scan_fingerprints: HashMap<String, u64>,
    source_session_ids: HashMap<String, HashSet<String>>,
}

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

struct CacheSnapshot {
    data: Vec<SessionMeta>,
    requests: Vec<LocalRequestRecord>,
}

fn ensure_cache_ready() -> CacheSnapshot {
    let cache = get_cache();

    {
        let cache_guard = cache.lock().unwrap_or_else(|err| err.into_inner());
        if cache_guard.is_some() {
            drop(cache_guard);
            return incremental_update_cache();
        }
    }

    full_scan_and_cache()
}

fn full_scan_and_cache() -> CacheSnapshot {
    let cache = get_cache();

    let mut data = Vec::new();
    let mut requests = Vec::new();
    let mut message_to_session = HashMap::new();
    let mut session_fingerprints = HashMap::new();
    let mut source_scan_fingerprints = HashMap::new();
    let mut source_session_ids = HashMap::new();

    for source in all_sources() {
        let descriptor = source.descriptor();
        let snapshot = match source.collect() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                eprintln!("[session] source collection failed: {error}");
                continue;
            }
        };
        debug_assert_eq!(descriptor.tool_id, snapshot.source_id);
        let source_id = snapshot.source_id.to_string();
        source_scan_fingerprints.insert(source_id.clone(), snapshot.scan_fingerprint);

        // 最旧会话优先：确保 fork 前的原始会话先占据 message_id 所有权，
        // fork 会话只贡献其新增消息。
        let mut sorted_sessions = snapshot.sessions;
        sorted_sessions.sort_by(|a, b| {
            a.last_modified
                .cmp(&b.last_modified)
                .then(a.session_id.cmp(&b.session_id))
        });

        let mut session_ids = HashSet::new();
        for session_file in &sorted_sessions {
            let _ = load_parsed_session(
                &mut data,
                &mut requests,
                &mut message_to_session,
                &mut session_fingerprints,
                session_file,
            );
            session_ids.insert(session_file.session_id.clone());
        }

        source_session_ids.insert(source_id, session_ids);
    }

    sort_cache_vectors(&mut data, &mut requests);

    {
        let mut cache_guard = cache.lock().unwrap_or_else(|err| err.into_inner());
        *cache_guard = Some(CacheEntry {
            data: data.clone(),
            requests: requests.clone(),
            message_to_session,
            session_fingerprints,
            source_scan_fingerprints,
            source_session_ids,
        });
    }

    CacheSnapshot { data, requests }
}

fn incremental_update_cache() -> CacheSnapshot {
    let cache = get_cache();
    let snapshots: Vec<SourceSnapshot> = all_sources()
        .into_iter()
        .filter_map(|source| source.collect().ok())
        .collect();

    let mut cache_guard = cache.lock().unwrap_or_else(|err| err.into_inner());
    let entry = match cache_guard.as_mut() {
        Some(entry) => entry,
        None => {
            // 缓存未初始化：先释放锁再全量扫描，避免与 full_scan_and_cache 内部的
            // lock() 形成不可重入死锁（与 ensure_cache_ready 的释放顺序保持一致）。
            drop(cache_guard);
            return full_scan_and_cache();
        }
    };

    let has_changes = snapshots.iter().any(|snapshot| {
        entry
            .source_scan_fingerprints
            .get(snapshot.source_id)
            .copied()
            .unwrap_or_default()
            != snapshot.scan_fingerprint
    });

    if !has_changes {
        return CacheSnapshot {
            data: entry.data.clone(),
            requests: entry.requests.clone(),
        };
    }

    for snapshot in snapshots {
        let previous = entry
            .source_scan_fingerprints
            .get(snapshot.source_id)
            .copied()
            .unwrap_or_default();
        if previous == snapshot.scan_fingerprint {
            continue;
        }
        apply_source_snapshot(entry, snapshot);
    }

    sort_cache_vectors(&mut entry.data, &mut entry.requests);

    CacheSnapshot {
        data: entry.data.clone(),
        requests: entry.requests.clone(),
    }
}

pub fn find_session_id_by_message_id(message_id: &str) -> Option<String> {
    let cache = get_cache();

    {
        let cache_guard = cache.lock().unwrap_or_else(|err| err.into_inner());
        if cache_guard.is_none() {
            drop(cache_guard);
            let _ = ensure_cache_ready();
        }
    }

    let cache_guard = cache.lock().unwrap_or_else(|err| err.into_inner());
    cache_guard
        .as_ref()
        .and_then(|entry| entry.message_to_session.get(message_id).cloned())
}

#[allow(dead_code)]
pub fn invalidate_cache() {
    let cache = get_cache();
    let mut cache_guard = cache.lock().unwrap_or_else(|err| err.into_inner());
    *cache_guard = None;
}

pub(crate) fn parse_session_file(session: &SessionFile) -> Result<ParsedSessionData, String> {
    super::registry::parse_session_file(session)
}

fn load_parsed_session(
    data: &mut Vec<SessionMeta>,
    requests: &mut Vec<LocalRequestRecord>,
    message_to_session: &mut HashMap<String, String>,
    session_fingerprints: &mut HashMap<String, u64>,
    session_file: &SessionFile,
) -> bool {
    match parse_session_file(session_file) {
        Ok(parsed) => {
            merge_parsed_session(
                data,
                requests,
                message_to_session,
                session_fingerprints,
                session_file,
                parsed,
            );
            true
        }
        Err(err) => {
            log_parse_error(session_file, &err);
            false
        }
    }
}

fn log_parse_error(session_file: &SessionFile, error: &str) {
    if session_file.tool == super::constants::TOOL_DEEPSEEK_HARNESS {
        eprintln!("[UsageMeter] Failed to parse DeepSeek Harness session: {error}");
    } else {
        eprintln!(
            "[UsageMeter] Failed to parse session {} ({}): {}",
            session_file.session_id, session_file.file_path, error
        );
    }
}

fn apply_source_snapshot(entry: &mut CacheEntry, snapshot: SourceSnapshot) {
    let source_id = snapshot.source_id.to_string();
    let current_file_map: HashMap<String, SessionFile> = snapshot
        .sessions
        .into_iter()
        .map(|file| (file.session_id.clone(), file))
        .collect();
    let current_ids: HashSet<String> = current_file_map.keys().cloned().collect();
    let previous_ids = entry
        .source_session_ids
        .get(&source_id)
        .cloned()
        .unwrap_or_default();

    let removed_ids: HashSet<String> = match snapshot.update_mode {
        SourceUpdateMode::ReplaceAll => previous_ids.clone(),
        SourceUpdateMode::PerSession => previous_ids.difference(&current_ids).cloned().collect(),
    };
    remove_sessions(entry, &removed_ids);

    let changed_or_new_ids: Vec<String> = match snapshot.update_mode {
        SourceUpdateMode::ReplaceAll => current_ids.iter().cloned().collect(),
        SourceUpdateMode::PerSession => current_ids
            .iter()
            .filter(|session_id| {
                current_file_map
                    .get(*session_id)
                    .map(|file| {
                        entry
                            .session_fingerprints
                            .get(*session_id)
                            .copied()
                            .unwrap_or_default()
                            != file.fingerprint
                    })
                    .unwrap_or(false)
            })
            .cloned()
            .collect(),
    };
    // 最旧会话优先，保证原始会话先占据 message_id，fork 会话只贡献新增消息。
    let mut to_parse: Vec<&SessionFile> = changed_or_new_ids
        .iter()
        .filter_map(|id| current_file_map.get(id))
        .collect();
    to_parse.sort_by(|a, b| {
        a.last_modified
            .cmp(&b.last_modified)
            .then(a.session_id.cmp(&b.session_id))
    });

    let mut parsed = Vec::new();
    let mut had_parse_error = false;
    for file in to_parse {
        match parse_session_file(file) {
            Ok(data) => parsed.push((file, data)),
            Err(error) => {
                had_parse_error = true;
                log_parse_error(file, &error);
            }
        }
    }
    let parsed_ids: HashSet<String> = parsed
        .iter()
        .map(|(file, _)| file.session_id.clone())
        .collect();
    remove_sessions(entry, &parsed_ids);
    for (file, data) in parsed {
        merge_parsed_session(
            &mut entry.data,
            &mut entry.requests,
            &mut entry.message_to_session,
            &mut entry.session_fingerprints,
            file,
            data,
        );
    }

    if !had_parse_error {
        entry
            .source_scan_fingerprints
            .insert(source_id.clone(), snapshot.scan_fingerprint);
    }
    entry.source_session_ids.insert(source_id, current_ids);
}

fn remove_sessions(entry: &mut CacheEntry, session_ids: &HashSet<String>) {
    if session_ids.is_empty() {
        return;
    }
    entry
        .data
        .retain(|meta| !session_ids.contains(&meta.session_id));
    entry
        .requests
        .retain(|record| !session_ids.contains(&record.session_id));
    entry
        .message_to_session
        .retain(|_, session_id| !session_ids.contains(session_id));
    entry
        .session_fingerprints
        .retain(|session_id, _| !session_ids.contains(session_id));
}

fn merge_parsed_session(
    data: &mut Vec<SessionMeta>,
    requests: &mut Vec<LocalRequestRecord>,
    message_to_session: &mut HashMap<String, String>,
    session_fingerprints: &mut HashMap<String, u64>,
    session_file: &SessionFile,
    mut parsed: ParsedSessionData,
) {
    // Drop requests whose message_id is already owned by an earlier (original) session.
    // This prevents fork sessions from double-counting the copied history.
    parsed
        .requests
        .retain(|req| !message_to_session.contains_key(&req.message_id));

    // Recompute meta totals to match the filtered request set.
    parsed.meta.total_input_tokens = parsed.requests.iter().map(|r| r.input_tokens).sum();
    parsed.meta.total_output_tokens = parsed.requests.iter().map(|r| r.output_tokens).sum();
    parsed.meta.total_cache_create_tokens =
        parsed.requests.iter().map(|r| r.cache_create_tokens).sum();
    parsed.meta.total_cache_read_tokens = parsed.requests.iter().map(|r| r.cache_read_tokens).sum();
    parsed.meta.message_count = parsed.requests.len() as u64;
    parsed.meta.message_ids = parsed
        .requests
        .iter()
        .map(|r| r.message_id.clone())
        .collect();

    for request in &parsed.requests {
        message_to_session.insert(request.message_id.clone(), request.session_id.clone());
    }
    session_fingerprints.insert(session_file.session_id.clone(), session_file.fingerprint);
    requests.extend(parsed.requests);
    data.push(parsed.meta);
}

fn sort_cache_vectors(data: &mut [SessionMeta], requests: &mut [LocalRequestRecord]) {
    data.sort_by_key(|meta| std::cmp::Reverse(meta.last_modified));
    requests.sort_by_key(|record| record.timestamp);
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
        ToolFilter::AnyOf(tools) => tools.iter().any(|t| tool == *t),
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
    use crate::session::registry::all_sources;
    use std::fs;
    use std::io::Write;
    use tempfile::tempdir;

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
            tool: super::super::constants::TOOL_CLAUDE_CODE.to_string(),
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

        let parsed = parse_session_file(&session).expect("claude session should parse");
        assert_eq!(parsed.meta.start_time, 100);
        assert_eq!(parsed.meta.end_time, 500);
    }

    #[test]
    fn test_parse_session_file_returns_error_for_unsupported_tool() {
        let session = SessionFile {
            session_id: "broken::session".to_string(),
            tool: "unsupported_tool".to_string(),
            project_path: "broken".to_string(),
            file_path: "/tmp/broken.jsonl".to_string(),
            transcript_paths: vec!["/tmp/broken.jsonl".to_string()],
            file_size: 0,
            last_modified: 0,
            fingerprint: 0,
        };

        let error = parse_session_file(&session).expect_err("unsupported tool should error");
        assert!(error.contains("unsupported session tool"));
    }

    #[test]
    fn test_all_sources_registers_qoder_work_variants() {
        let tool_ids: Vec<&str> = all_sources()
            .into_iter()
            .map(|source| source.tool_id())
            .collect();
        assert!(tool_ids.contains(&crate::session::constants::TOOL_QODER_WORK));
        assert!(tool_ids.contains(&crate::session::constants::TOOL_QODER_WORK_CN));
    }

    #[test]
    fn test_merge_parsed_session_deduplicates_fork_messages() {
        use crate::session::constants::TOOL_CLAUDE_CODE;
        use crate::session::meta::LocalRequestRecord;
        use crate::session::source::ParsedSessionData;

        let make_session = |id: &str, last_modified: i64| SessionFile {
            session_id: id.to_string(),
            tool: TOOL_CLAUDE_CODE.to_string(),
            project_path: "project".to_string(),
            file_path: format!("/tmp/{id}.jsonl"),
            transcript_paths: vec![format!("/tmp/{id}.jsonl")],
            file_size: 100,
            last_modified,
            fingerprint: last_modified as u64,
        };

        let make_request = |session_id: &str, msg_id: &str, tokens: u64| LocalRequestRecord {
            session_id: session_id.to_string(),
            tool: TOOL_CLAUDE_CODE.to_string(),
            timestamp: 1000,
            message_id: msg_id.to_string(),
            input_tokens: tokens,
            output_tokens: tokens,
            total_tokens: tokens * 2,
            ..Default::default()
        };

        let mut data: Vec<super::super::meta::SessionMeta> = Vec::new();
        let mut requests: Vec<LocalRequestRecord> = Vec::new();
        let mut message_to_session: HashMap<String, String> = HashMap::new();
        let mut session_fingerprints: HashMap<String, u64> = HashMap::new();

        // Original session: messages M1, M2, M3
        let orig_file = make_session("project::orig", 100);
        let orig_parsed = ParsedSessionData {
            meta: {
                let mut m = super::super::meta::SessionMeta::default();
                m.session_id = "project::orig".to_string();
                m.last_modified = 100;
                m.total_input_tokens = 30;
                m.total_output_tokens = 30;
                m.message_count = 3;
                m
            },
            requests: vec![
                make_request("project::orig", "msg_1", 10),
                make_request("project::orig", "msg_2", 10),
                make_request("project::orig", "msg_3", 10),
            ],
        };
        merge_parsed_session(
            &mut data,
            &mut requests,
            &mut message_to_session,
            &mut session_fingerprints,
            &orig_file,
            orig_parsed,
        );

        // Fork session: copies M1+M2+M3, adds M4+M5
        let fork_file = make_session("project::fork", 200);
        let fork_parsed = ParsedSessionData {
            meta: {
                let mut m = super::super::meta::SessionMeta::default();
                m.session_id = "project::fork".to_string();
                m.last_modified = 200;
                m.total_input_tokens = 50;
                m.total_output_tokens = 50;
                m.message_count = 5;
                m
            },
            requests: vec![
                make_request("project::fork", "msg_1", 10), // duplicate
                make_request("project::fork", "msg_2", 10), // duplicate
                make_request("project::fork", "msg_3", 10), // duplicate
                make_request("project::fork", "msg_4", 10), // new
                make_request("project::fork", "msg_5", 10), // new
            ],
        };
        merge_parsed_session(
            &mut data,
            &mut requests,
            &mut message_to_session,
            &mut session_fingerprints,
            &fork_file,
            fork_parsed,
        );

        // Global requests should have 3 (orig) + 2 (fork new) = 5, not 8
        assert_eq!(requests.len(), 5);

        // Fork session meta should reflect only its 2 new messages
        let fork_meta = data
            .iter()
            .find(|m| m.session_id == "project::fork")
            .unwrap();
        assert_eq!(fork_meta.message_count, 2);
        assert_eq!(fork_meta.total_input_tokens, 20);
        assert_eq!(fork_meta.total_output_tokens, 20);

        // Each message_id should appear exactly once in the global map
        let msg_1_owner = message_to_session.get("msg_1").unwrap();
        assert_eq!(msg_1_owner, "project::orig");
        let msg_4_owner = message_to_session.get("msg_4").unwrap();
        assert_eq!(msg_4_owner, "project::fork");

        // Fork meta.message_ids must also only list the 2 new messages
        let mut fork_ids = fork_meta.message_ids.clone();
        fork_ids.sort();
        assert_eq!(fork_ids, vec!["msg_4", "msg_5"]);
    }

    // ============================================================
    // apply_source_snapshot / remove_sessions / incremental_update_cache
    // ============================================================

    use crate::session::constants::TOOL_CLAUDE_CODE;
    use crate::session::source::{ParsedSessionData, SourceSnapshot, SourceUpdateMode};
    use crate::test_support::env_lock;
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn make_request_record(session_id: &str, msg_id: &str, tokens: u64) -> LocalRequestRecord {
        LocalRequestRecord {
            session_id: session_id.to_string(),
            tool: TOOL_CLAUDE_CODE.to_string(),
            timestamp: 1000,
            message_id: msg_id.to_string(),
            input_tokens: tokens,
            output_tokens: tokens,
            total_tokens: tokens * 2,
            ..Default::default()
        }
    }

    fn make_parsed_session(
        session_id: &str,
        msg_ids: &[&str],
        tokens: u64,
        last_modified: i64,
    ) -> ParsedSessionData {
        let requests: Vec<LocalRequestRecord> = msg_ids
            .iter()
            .map(|id| make_request_record(session_id, id, tokens))
            .collect();
        let meta = SessionMeta {
            session_id: session_id.to_string(),
            tool: TOOL_CLAUDE_CODE.to_string(),
            last_modified,
            message_count: requests.len() as u64,
            total_input_tokens: tokens * requests.len() as u64,
            total_output_tokens: tokens * requests.len() as u64,
            message_ids: requests.iter().map(|r| r.message_id.clone()).collect(),
            ..Default::default()
        };
        ParsedSessionData { meta, requests }
    }

    /// 写入一个可被 Claude source 解析的 transcript 文件（assistant 消息 JSONL）。
    fn write_claude_transcript(path: &Path, msgs: &[(&str, u64)]) -> u64 {
        let mut file = fs::File::create(path).unwrap();
        for (index, (msg_id, tokens)) in msgs.iter().enumerate() {
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "type": "assistant",
                    "timestamp": 1_700_000_000 + index as i64,
                    "message": {
                        "id": msg_id,
                        "model": "claude-3-7-sonnet",
                        "usage": { "input_tokens": tokens, "output_tokens": tokens }
                    }
                })
            )
            .unwrap();
        }
        file.flush().unwrap();
        fs::metadata(path).unwrap().len()
    }

    fn make_session_file_at(
        id: &str,
        path: &Path,
        last_modified: i64,
        fingerprint: u64,
    ) -> SessionFile {
        let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        SessionFile {
            session_id: id.to_string(),
            tool: TOOL_CLAUDE_CODE.to_string(),
            project_path: "project".to_string(),
            file_path: path.to_string_lossy().to_string(),
            transcript_paths: vec![path.to_string_lossy().to_string()],
            file_size: size,
            last_modified,
            fingerprint,
        }
    }

    fn make_snapshot(
        source_id: &'static str,
        mode: SourceUpdateMode,
        sessions: Vec<SessionFile>,
        scan_fingerprint: u64,
    ) -> SourceSnapshot {
        SourceSnapshot {
            source_id,
            update_mode: mode,
            sessions,
            scan_fingerprint,
        }
    }

    fn empty_cache_entry() -> CacheEntry {
        CacheEntry {
            data: Vec::new(),
            requests: Vec::new(),
            message_to_session: HashMap::new(),
            session_fingerprints: HashMap::new(),
            source_scan_fingerprints: HashMap::new(),
            source_session_ids: HashMap::new(),
        }
    }

    fn all_ids(entry: &CacheEntry) -> Vec<String> {
        let mut ids: Vec<String> = entry.data.iter().map(|m| m.session_id.clone()).collect();
        ids.sort();
        ids
    }

    /// 注入一条只存在于缓存中的幽灵会话，并登记到 claude source 名下。
    /// 若被测路径触发重扫，该幽灵必须被 remove_sessions 清掉。
    fn inject_ghost_session(entry: &mut CacheEntry) {
        entry.data.push(SessionMeta {
            session_id: "fake::ghost".to_string(),
            tool: TOOL_CLAUDE_CODE.to_string(),
            file_path: "/tmp/ghost.jsonl".to_string(),
            last_modified: 1,
            ..Default::default()
        });
        entry
            .requests
            .push(make_request_record("fake::ghost", "fake_msg", 5));
        entry
            .message_to_session
            .insert("fake_msg".to_string(), "fake::ghost".to_string());
        entry
            .session_fingerprints
            .insert("fake::ghost".to_string(), 999);
        entry
            .source_session_ids
            .entry(TOOL_CLAUDE_CODE.to_string())
            .or_default()
            .insert("fake::ghost".to_string());
    }

    /// 持有环境锁 + 全局 session cache 锁，串行化走真实 source 扫描的测试，
    /// 避免并行测试互相污染 SESSION_CACHE。
    fn scanner_scan_lock() -> MutexGuard<'static, ()> {
        static SCANNER_CACHE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        SCANNER_CACHE_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    /// 将扫描器可能读取的全部环境变量隔离到临时空目录，Drop 时恢复。
    /// 覆盖 session/ 各 reader 实际读取的变量（grep env::var 确认）：
    /// HOME / XDG_DATA_HOME / OPENCODE_HOME / OPENCODE_DATA_DIR / OPENCODE_DB / REASONIX_HOME /
    /// HERMES_HOME / DSH_HOME / LOCALAPPDATA。
    struct EnvGuard {
        /// 每个变量在隔离前的原始值，Drop 时据此恢复。
        vars: Vec<(&'static str, Option<OsString>)>,
    }

    impl EnvGuard {
        fn isolated(tmp: &Path) -> Self {
            let home_dir = tmp.join("home");
            let xdg_dir = tmp.join("xdg");
            fs::create_dir_all(&home_dir).unwrap();
            fs::create_dir_all(&xdg_dir).unwrap();
            let guard = EnvGuard {
                vars: vec![
                    ("HOME", std::env::var_os("HOME")),
                    ("XDG_DATA_HOME", std::env::var_os("XDG_DATA_HOME")),
                    ("OPENCODE_HOME", std::env::var_os("OPENCODE_HOME")),
                    ("OPENCODE_DATA_DIR", std::env::var_os("OPENCODE_DATA_DIR")),
                    ("OPENCODE_DB", std::env::var_os("OPENCODE_DB")),
                    ("REASONIX_HOME", std::env::var_os("REASONIX_HOME")),
                    ("HERMES_HOME", std::env::var_os("HERMES_HOME")),
                    ("DSH_HOME", std::env::var_os("DSH_HOME")),
                    ("LOCALAPPDATA", std::env::var_os("LOCALAPPDATA")),
                ],
            };
            // 统一先移除（避免残留的独立 *_HOME 变量被各 reader 扫到真实数据），
            // 再为 HOME / XDG_DATA_HOME 指向隔离空目录。
            for (name, _) in &guard.vars {
                std::env::remove_var(name);
            }
            std::env::set_var("HOME", &home_dir);
            std::env::set_var("XDG_DATA_HOME", &xdg_dir);
            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in &self.vars {
                match value {
                    Some(v) => std::env::set_var(name, v),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[test]
    fn apply_source_snapshot_per_session_adds_and_keeps_unchanged() {
        let tmp = tempdir().unwrap();
        let s1_path = tmp.path().join("s1.jsonl");
        write_claude_transcript(&s1_path, &[("m1", 10), ("m2", 20)]);
        let s2_path = tmp.path().join("s2.jsonl");
        write_claude_transcript(&s2_path, &[("m3", 30)]);

        let mut entry = empty_cache_entry();
        let s1 = make_session_file_at("proj::s1", &s1_path, 100, 10);
        apply_source_snapshot(
            &mut entry,
            make_snapshot(
                TOOL_CLAUDE_CODE,
                SourceUpdateMode::PerSession,
                vec![s1.clone()],
                1,
            ),
        );

        assert_eq!(all_ids(&entry), vec!["proj::s1"]);
        assert_eq!(entry.session_fingerprints.get("proj::s1"), Some(&10));
        let s1_meta = entry
            .data
            .iter()
            .find(|m| m.session_id == "proj::s1")
            .unwrap();
        assert_eq!(s1_meta.message_count, 2);
        assert_eq!(
            entry.message_to_session.get("m1").map(String::as_str),
            Some("proj::s1")
        );

        // 同 fingerprint 再次出现 → 不重解析；新增 s2 → 增量加入
        let s2 = make_session_file_at("proj::s2", &s2_path, 200, 20);
        apply_source_snapshot(
            &mut entry,
            make_snapshot(
                TOOL_CLAUDE_CODE,
                SourceUpdateMode::PerSession,
                vec![s1.clone(), s2.clone()],
                2,
            ),
        );

        assert_eq!(all_ids(&entry), vec!["proj::s1", "proj::s2"]);
        let s2_meta = entry
            .data
            .iter()
            .find(|m| m.session_id == "proj::s2")
            .unwrap();
        assert_eq!(s2_meta.message_count, 1);
        assert_eq!(
            entry.source_scan_fingerprints.get(TOOL_CLAUDE_CODE),
            Some(&2)
        );
        // s1 未重解析：指纹保持首次值
        assert_eq!(entry.session_fingerprints.get("proj::s1"), Some(&10));
    }

    #[test]
    fn changed_session_parse_failure_keeps_previous_cached_facts() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("s1.jsonl");
        write_claude_transcript(&path, &[("m1", 10)]);
        let mut entry = empty_cache_entry();
        let original = make_session_file_at("proj::s1", &path, 100, 10);
        apply_source_snapshot(
            &mut entry,
            make_snapshot(
                TOOL_CLAUDE_CODE,
                SourceUpdateMode::PerSession,
                vec![original],
                1,
            ),
        );
        assert_eq!(entry.requests.len(), 1);

        let mut changed = make_session_file_at("proj::s1", &path, 101, 11);
        changed.tool = "unsupported_tool".to_string();
        apply_source_snapshot(
            &mut entry,
            make_snapshot(
                TOOL_CLAUDE_CODE,
                SourceUpdateMode::PerSession,
                vec![changed],
                2,
            ),
        );
        assert_eq!(entry.requests.len(), 1);
        assert_eq!(entry.requests[0].message_id, "m1");
        assert_eq!(entry.session_fingerprints.get("proj::s1"), Some(&10));
        assert_eq!(
            entry.source_scan_fingerprints.get(TOOL_CLAUDE_CODE),
            Some(&1)
        );
    }

    #[test]
    fn apply_source_snapshot_per_session_updates_changed_and_removes_missing() {
        let tmp = tempdir().unwrap();
        let s1_path = tmp.path().join("s1.jsonl");
        write_claude_transcript(&s1_path, &[("m1", 10), ("m2", 20)]);
        let s2_path = tmp.path().join("s2.jsonl");
        write_claude_transcript(&s2_path, &[("m3", 30)]);

        let mut entry = empty_cache_entry();
        let s1 = make_session_file_at("proj::s1", &s1_path, 100, 10);
        let s2 = make_session_file_at("proj::s2", &s2_path, 200, 20);
        apply_source_snapshot(
            &mut entry,
            make_snapshot(
                TOOL_CLAUDE_CODE,
                SourceUpdateMode::PerSession,
                vec![s1.clone(), s2.clone()],
                1,
            ),
        );
        assert_eq!(all_ids(&entry), vec!["proj::s1", "proj::s2"]);

        // s1 内容变化（fingerprint 变化）→ 重解析；s2 从快照消失 → 移除；s3 新增
        let s1_v2_path = tmp.path().join("s1_v2.jsonl");
        write_claude_transcript(&s1_v2_path, &[("m1", 10), ("m2", 20), ("m3new", 15)]);
        let s3_path = tmp.path().join("s3.jsonl");
        write_claude_transcript(&s3_path, &[("m4", 40)]);
        let s1_changed = make_session_file_at("proj::s1", &s1_v2_path, 100, 11);
        let s3 = make_session_file_at("proj::s3", &s3_path, 300, 30);
        apply_source_snapshot(
            &mut entry,
            make_snapshot(
                TOOL_CLAUDE_CODE,
                SourceUpdateMode::PerSession,
                vec![s1_changed.clone(), s3.clone()],
                2,
            ),
        );

        assert_eq!(all_ids(&entry), vec!["proj::s1", "proj::s3"]);
        // s2 被整体移除：data / requests / 消息归属均不再出现
        assert!(!entry.requests.iter().any(|r| r.session_id == "proj::s2"));
        assert!(entry.message_to_session.get("m3").is_none());
        // s1 重解析成功：指纹推进，消息数反映新文件
        assert_eq!(entry.session_fingerprints.get("proj::s1"), Some(&11));
        let s1_meta = entry
            .data
            .iter()
            .find(|m| m.session_id == "proj::s1")
            .unwrap();
        assert_eq!(s1_meta.message_count, 3);
        // s3 新增
        assert_eq!(
            entry.message_to_session.get("m4").map(String::as_str),
            Some("proj::s3")
        );
    }

    #[test]
    fn apply_source_snapshot_replace_all_replaces_entire_source() {
        let tmp = tempdir().unwrap();
        let s1_path = tmp.path().join("s1.jsonl");
        write_claude_transcript(&s1_path, &[("m1", 10)]);
        let s2_path = tmp.path().join("s2.jsonl");
        write_claude_transcript(&s2_path, &[("m2", 20)]);

        let mut entry = empty_cache_entry();
        apply_source_snapshot(
            &mut entry,
            make_snapshot(
                TOOL_CLAUDE_CODE,
                SourceUpdateMode::ReplaceAll,
                vec![make_session_file_at("proj::s1", &s1_path, 100, 10)],
                1,
            ),
        );
        assert_eq!(all_ids(&entry), vec!["proj::s1"]);

        // ReplaceAll：即使 s1 仍存在，旧集合整体被 s2 替换
        apply_source_snapshot(
            &mut entry,
            make_snapshot(
                TOOL_CLAUDE_CODE,
                SourceUpdateMode::ReplaceAll,
                vec![make_session_file_at("proj::s2", &s2_path, 200, 20)],
                2,
            ),
        );
        assert_eq!(all_ids(&entry), vec!["proj::s2"]);
        assert!(entry.message_to_session.get("m1").is_none());
        assert_eq!(
            entry.message_to_session.get("m2").map(String::as_str),
            Some("proj::s2")
        );
        assert_eq!(
            entry
                .source_session_ids
                .get(TOOL_CLAUDE_CODE)
                .map(|s| s.len()),
            Some(1)
        );
    }

    #[test]
    fn apply_source_snapshot_preserves_other_sources_unchanged() {
        use crate::session::constants::TOOL_CODEX;

        let tmp = tempdir().unwrap();
        let s1_path = tmp.path().join("s1.jsonl");
        write_claude_transcript(&s1_path, &[("m1", 10)]);

        let mut entry = empty_cache_entry();
        apply_source_snapshot(
            &mut entry,
            make_snapshot(
                TOOL_CLAUDE_CODE,
                SourceUpdateMode::PerSession,
                vec![make_session_file_at("proj::s1", &s1_path, 100, 10)],
                1,
            ),
        );

        // 预置 codex 源会话（直接喂 merge_parsed_session，模拟已存在于缓存）
        let codex_file = SessionFile {
            session_id: "codex::s2".to_string(),
            tool: TOOL_CODEX.to_string(),
            project_path: "project".to_string(),
            file_path: "/tmp/codex_s2.jsonl".to_string(),
            transcript_paths: vec!["/tmp/codex_s2.jsonl".to_string()],
            file_size: 0,
            last_modified: 200,
            fingerprint: 99,
        };
        merge_parsed_session(
            &mut entry.data,
            &mut entry.requests,
            &mut entry.message_to_session,
            &mut entry.session_fingerprints,
            &codex_file,
            make_parsed_session("codex::s2", &["m2"], 20, 200),
        );
        entry.source_session_ids.insert(
            TOOL_CODEX.to_string(),
            HashSet::from(["codex::s2".to_string()]),
        );
        entry
            .source_scan_fingerprints
            .insert(TOOL_CODEX.to_string(), 7);

        // 只更新 claude 源：codex 源的状态与归属必须原样保留
        apply_source_snapshot(
            &mut entry,
            make_snapshot(
                TOOL_CLAUDE_CODE,
                SourceUpdateMode::PerSession,
                vec![make_session_file_at("proj::s1", &s1_path, 100, 11)],
                2,
            ),
        );
        assert_eq!(all_ids(&entry), vec!["codex::s2", "proj::s1"]);
        assert_eq!(entry.source_scan_fingerprints.get(TOOL_CODEX), Some(&7));
        assert_eq!(
            entry.message_to_session.get("m2").map(String::as_str),
            Some("codex::s2")
        );
        assert_eq!(entry.session_fingerprints.get("codex::s2"), Some(&99));
    }

    #[test]
    fn remove_sessions_clears_message_ownership_for_later_forks() {
        let mut entry = empty_cache_entry();
        let orig_file = make_session_file_at("proj::orig", Path::new("/tmp/orig.jsonl"), 100, 1);
        let fork_file = make_session_file_at("proj::fork", Path::new("/tmp/fork.jsonl"), 200, 2);

        merge_parsed_session(
            &mut entry.data,
            &mut entry.requests,
            &mut entry.message_to_session,
            &mut entry.session_fingerprints,
            &orig_file,
            make_parsed_session("proj::orig", &["m1", "m2", "m3"], 10, 100),
        );
        merge_parsed_session(
            &mut entry.data,
            &mut entry.requests,
            &mut entry.message_to_session,
            &mut entry.session_fingerprints,
            &fork_file,
            make_parsed_session("proj::fork", &["m1", "m2", "m3", "m4"], 10, 200),
        );
        // fork 会话只贡献新增消息 m4
        assert_eq!(entry.requests.len(), 4);
        assert_eq!(
            entry.message_to_session.get("m1").map(String::as_str),
            Some("proj::orig")
        );

        // 移除原始会话：m1/m2/m3 的所有权必须被清空
        remove_sessions(&mut entry, &HashSet::from(["proj::orig".to_string()]));
        assert_eq!(entry.requests.len(), 1);
        assert_eq!(entry.message_to_session.len(), 1);
        assert!(entry.message_to_session.get("m1").is_none());
        assert_eq!(
            entry.message_to_session.get("m4").map(String::as_str),
            Some("proj::fork")
        );

        // 后续包含 m1 的新 fork 会话应能继承 m1，而不是被残留映射丢弃
        let new_fork = make_session_file_at("proj::fork2", Path::new("/tmp/fork2.jsonl"), 300, 3);
        merge_parsed_session(
            &mut entry.data,
            &mut entry.requests,
            &mut entry.message_to_session,
            &mut entry.session_fingerprints,
            &new_fork,
            make_parsed_session("proj::fork2", &["m1", "m9"], 10, 300),
        );
        let fork2_meta = entry
            .data
            .iter()
            .find(|m| m.session_id == "proj::fork2")
            .unwrap();
        assert_eq!(fork2_meta.message_count, 2);
        assert_eq!(
            entry.message_to_session.get("m1").map(String::as_str),
            Some("proj::fork2")
        );
        assert_eq!(entry.requests.len(), 3);
    }

    #[test]
    fn incremental_update_cache_reuses_cache_when_scan_fingerprints_unchanged() {
        let _env = env_lock();
        let _cache = scanner_scan_lock();
        let tmp = tempdir().unwrap();
        let _env_guard = EnvGuard::isolated(tmp.path());
        invalidate_cache();

        // 首次全量扫描：隔离空环境，建立各 source 的空 fingerprint 基线
        let first = incremental_update_cache();
        assert!(first.data.is_empty());

        // 注入幽灵数据到缓存
        {
            let mut guard = get_cache().lock().unwrap_or_else(|err| err.into_inner());
            inject_ghost_session(guard.as_mut().unwrap());
        }

        // fingerprint 未变 → 直接复用缓存，幽灵数据必须原样保留（未被重扫清空）
        let second = incremental_update_cache();
        assert!(second.data.iter().any(|m| m.session_id == "fake::ghost"));
        assert!(second
            .requests
            .iter()
            .any(|r| r.session_id == "fake::ghost"));

        invalidate_cache();
    }

    #[test]
    fn incremental_update_cache_reparses_changed_source_and_drops_stale() {
        let _env = env_lock();
        let _cache = scanner_scan_lock();
        let tmp = tempdir().unwrap();
        let _env_guard = EnvGuard::isolated(tmp.path());
        invalidate_cache();

        // 在隔离 HOME 放置一个真实 Claude transcript
        let projects = tmp
            .path()
            .join("home")
            .join(".claude")
            .join("projects")
            .join("proj");
        fs::create_dir_all(&projects).unwrap();
        let s1_path = projects.join("s1.jsonl");
        write_claude_transcript(&s1_path, &[("m1", 10), ("m2", 20)]);

        let first = incremental_update_cache();
        assert!(first.data.iter().any(|m| m.session_id == "proj::s1"));
        assert!(first.requests.iter().any(|r| r.session_id == "proj::s1"));

        // 注入幽灵数据并登记到 claude source 名下
        {
            let mut guard = get_cache().lock().unwrap_or_else(|err| err.into_inner());
            inject_ghost_session(guard.as_mut().unwrap());
        }

        // 新增一个会话文件 → claude scan fingerprint 变化 → 增量重扫
        let s2_path = projects.join("s2.jsonl");
        write_claude_transcript(&s2_path, &[("m3", 30)]);

        let second = incremental_update_cache();
        assert!(
            !second.data.iter().any(|m| m.session_id == "fake::ghost"),
            "re-scan must drop stale ghost sessions"
        );
        assert!(second.data.iter().any(|m| m.session_id == "proj::s1"));
        assert!(second.data.iter().any(|m| m.session_id == "proj::s2"));
        assert!(second.requests.iter().any(|r| r.session_id == "proj::s2"));

        invalidate_cache();
    }
}
