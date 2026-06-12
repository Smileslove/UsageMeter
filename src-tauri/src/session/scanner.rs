//! 会话文件扫描器
//!
//! 统一扫描 Claude Code / Codex / OpenCode 本地 transcript，并构建两类缓存：
//! - session 级聚合结果（会话列表 / 详情 / 项目统计）
//! - request 级事实记录（概览 / 趋势 / 活动图）

use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::registry::all_sources;
use super::source::{ParsedSessionData, SourceSnapshot, SourceUpdateMode};
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

    let mut data = Vec::new();
    let mut requests = Vec::new();
    let mut message_to_session = HashMap::new();
    let mut session_fingerprints = HashMap::new();
    let mut source_scan_fingerprints = HashMap::new();
    let mut source_session_ids = HashMap::new();

    for source in all_sources() {
        let snapshot = source.scan();
        let source_id = snapshot.source_id.to_string();
        source_scan_fingerprints.insert(source_id.clone(), snapshot.scan_fingerprint);

        let mut session_ids = HashSet::new();
        for session_file in &snapshot.sessions {
            let parsed = parse_session_file(session_file);
            merge_parsed_session(
                &mut data,
                &mut requests,
                &mut message_to_session,
                &mut session_fingerprints,
                session_file,
                parsed,
            );
            session_ids.insert(session_file.session_id.clone());
        }

        source_session_ids.insert(source_id, session_ids);
    }

    sort_cache_vectors(&mut data, &mut requests);

    {
        let mut cache_guard = cache.lock().unwrap();
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
        .map(|source| source.scan())
        .collect();

    let mut cache_guard = cache.lock().unwrap();
    let entry = match cache_guard.as_mut() {
        Some(entry) => entry,
        None => return full_scan_and_cache(),
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

pub(crate) fn parse_session_file(session: &SessionFile) -> ParsedSessionData {
    super::registry::parse_session_file(session)
        .unwrap_or_else(|err| panic!("failed to parse session {}: {err}", session.session_id))
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
    let changed_set: HashSet<String> = changed_or_new_ids.iter().cloned().collect();
    remove_sessions(entry, &changed_set);

    for session_id in changed_or_new_ids {
        let Some(file) = current_file_map.get(&session_id) else {
            continue;
        };
        let parsed = parse_session_file(file);
        merge_parsed_session(
            &mut entry.data,
            &mut entry.requests,
            &mut entry.message_to_session,
            &mut entry.session_fingerprints,
            file,
            parsed,
        );
    }

    entry
        .source_scan_fingerprints
        .insert(source_id.clone(), snapshot.scan_fingerprint);
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
    parsed: ParsedSessionData,
) {
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

        let parsed = parse_session_file(&session);
        assert_eq!(parsed.meta.start_time, 100);
        assert_eq!(parsed.meta.end_time, 500);
    }
}
