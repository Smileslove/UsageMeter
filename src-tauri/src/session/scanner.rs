//! 会话文件扫描器
//!
//! 统一扫描 Claude Code / Codex / OpenCode 本地 transcript，并构建两类缓存：
//! - session 级聚合结果（会话列表 / 详情 / 项目统计）
//! - request 级事实记录（概览 / 趋势 / 活动图）
//!
//! 关键原则：
//! - Claude 以 assistant `message.id` 为基础主键
//! - Codex 以 rollout token_count 事件主键为基础事实
//! - OpenCode 以 SQLite message.id 为基础主键（直接读取 opencode.db）
//! - Claude 子代理 transcript 合并到所属主 session
//! - 所有页面从同一批去重后的 request 事实层聚合

use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use crate::models::ToolFilter;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

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
    /// OpenCode 全量扫描指纹（覆盖 SQLite + legacy 文件来源）
    opencode_scan_fingerprint: u64,
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
    let session_files = super::registry::scan_session_files();

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

    // 加载 OpenCode SQLite 数据（与 JSONL 扫描并行，互不干扰）
    let opencode_sessions = super::opencode_reader::scan_opencode_sessions();
    let opencode_scan_fingerprint =
        super::opencode_reader::compute_opencode_scan_fingerprint(&opencode_sessions);
    for session_data in opencode_sessions {
        for request in &session_data.requests {
            let canonical_raw_key = format!("{}:{}", request.tool, request.message_id);
            if request
                .request_key
                .as_deref()
                .map(|key| key == canonical_raw_key)
                .unwrap_or(true)
            {
                message_to_session.insert(
                    request.message_id.clone(),
                    session_data.meta.session_id.clone(),
                );
            }
        }
        // OpenCode 走独立扫描缓存；scanner 层只在全量指纹变化时整体替换。
        session_fingerprints.insert(
            session_data.meta.session_id.clone(),
            session_data.fingerprint,
        );
        requests.extend(session_data.requests);
        data.push(session_data.meta);
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
            opencode_scan_fingerprint,
        });
    }

    CacheSnapshot { data, requests }
}

fn incremental_update_cache() -> CacheSnapshot {
    let cache = get_cache();
    let current_files = super::registry::scan_session_files();
    let current_file_map: HashMap<String, SessionFile> = current_files
        .into_iter()
        .map(|file| (file.session_id.clone(), file))
        .collect();
    let current_fingerprints: HashMap<String, u64> = current_file_map
        .iter()
        .map(|(session_id, file)| (session_id.clone(), file.fingerprint))
        .collect();

    let current_opencode_sessions = super::opencode_reader::scan_opencode_sessions();
    let current_opencode_scan_fingerprint =
        super::opencode_reader::compute_opencode_scan_fingerprint(&current_opencode_sessions);

    let mut cache_guard = cache.lock().unwrap();
    let entry = match cache_guard.as_mut() {
        Some(entry) => entry,
        None => return full_scan_and_cache(),
    };

    // 过滤 OpenCode session 指纹，仅对 JSONL 类工具做增量比对
    let cached_ids: HashSet<String> = entry
        .session_fingerprints
        .keys()
        .filter(|id| !id.starts_with("opencode::"))
        .cloned()
        .collect();
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
    let opencode_changed = current_opencode_scan_fingerprint != entry.opencode_scan_fingerprint;

    if changed_ids.is_empty() && new_ids.is_empty() && !opencode_changed {
        return CacheSnapshot {
            data: entry.data.clone(),
            requests: entry.requests.clone(),
        };
    }

    entry.data.retain(|meta| {
        !(changed_ids.contains(&meta.session_id)
            || new_ids.contains(&meta.session_id)
            || opencode_changed && meta.tool == "opencode")
    });
    entry.requests.retain(|record| {
        !(changed_ids.contains(&record.session_id)
            || new_ids.contains(&record.session_id)
            || opencode_changed && record.tool == "opencode")
    });
    entry.message_to_session.retain(|_, session_id| {
        !(changed_ids.contains(session_id)
            || new_ids.contains(session_id)
            || opencode_changed && session_id.starts_with("opencode::"))
    });
    entry.session_fingerprints.retain(|session_id, _| {
        !(changed_ids.contains(session_id)
            || new_ids.contains(session_id)
            || opencode_changed && session_id.starts_with("opencode::"))
    });

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

    // 如果 OpenCode DB 有变更，重新加载所有 OpenCode sessions
    if opencode_changed {
        for session_data in current_opencode_sessions {
            for request in &session_data.requests {
                let canonical_raw_key = format!("{}:{}", request.tool, request.message_id);
                if request
                    .request_key
                    .as_deref()
                    .map(|key| key == canonical_raw_key)
                    .unwrap_or(true)
                {
                    entry.message_to_session.insert(
                        request.message_id.clone(),
                        session_data.meta.session_id.clone(),
                    );
                }
            }
            entry.session_fingerprints.insert(
                session_data.meta.session_id.clone(),
                session_data.fingerprint,
            );
            entry.requests.extend(session_data.requests);
            entry.data.push(session_data.meta);
        }
        entry.opencode_scan_fingerprint = current_opencode_scan_fingerprint;
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

pub(crate) fn parse_session_file(session: &SessionFile) -> ParsedSessionData {
    let (meta, requests) = super::registry::parse_session_file_for_storage(session);
    ParsedSessionData { meta, requests }
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
}
