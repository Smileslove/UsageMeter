//! 深度活动索引服务（M2 最后一环）：懒索引、rebuild、purge 与指纹管理。
//!
//! 职责边界：
//! - **不触碰计量表**：只读写 `session_activity_index` 等 5 张深度表与
//!   `local_sessions`/`local_source_files`（只读会话定位），不改
//!   `local_request_facts` 等既有用量口径；
//! - **幂等**：适配器产出会话级全量快照，指纹（源文件 mtime+size 摘要，
//!   不读全文）未变化时跳过写入；
//! - **单会话失败隔离**：rebuild 遍历时单个会话解析/写入失败只记录错误，
//!   不影响其他会话（设计 14.4）；
//! - **错误信息不含路径/密钥/正文**：文件系统错误只报稳定错误码与类别。
//!
//! 关联文件发现（subagents 方案，见 `discover_related_files`）：
//! `local_source_files` 中 Claude/Codex 会话每会话只有 primary 一行
//! （`file_role = "session_group"`，subagents 文件不落该表），因此无法
//! 按 session_id 从表内收集全部关联文件。改为方案 B'：由 indexer 从
//! primary 文件路径按工具目录规则推导兄弟文件，再逐个调用适配器
//! `index_session`（适配器接口只接收单文件），最后合并批次。

use crate::activity::adapter::{
    ActivityIndexBatch, NewEventRequestLink, NewSessionEvent, NewToolInvocation,
    SessionActivityAdapter, SessionSourceRef,
};
use crate::activity::db::{self, ActivityIndexEntry, ActivityPersistencePolicy};
use crate::activity::model::RebuildResult;
use crate::activity::registry::AdapterRegistry;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// rebuild 遍历用的可索引会话（只读 local_sessions 的必要列）。
#[derive(Debug, Clone, PartialEq)]
pub struct IndexableSession {
    pub session_id: String,
    pub tool: String,
    pub primary_file_path: String,
}

/// 单文件 (mtime, size) 摘要。错误信息不携带路径，防止敏感路径进入日志。
fn file_fingerprint(path: &str) -> Result<String, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|_| "ERR_ACTIVITY_IO: unable to stat source file".to_string())?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| (duration.as_secs(), duration.subsec_nanos()))
        .unwrap_or((0, 0));
    Ok(format!("{}.{:09}:{}", mtime.0, mtime.1, metadata.len()))
}

/// 会话级源指纹：primary + 全部关联文件的 (mtime,size) 摘要拼接。
/// 任一文件 stat 失败（被删除/截断）→ 报错，由调用方按会话隔离处理。
fn compute_source_fingerprint(files: &[String]) -> Result<String, String> {
    let mut parts = Vec::with_capacity(files.len());
    for file in files {
        parts.push(file_fingerprint(file)?);
    }
    Ok(parts.join("|"))
}

/// 从 primary 文件路径发现同会话的关联源文件（返回列表首位恒为 primary，
/// 其余按路径排序，顺序稳定 → 重复索引产出完全一致的批次）。
///
/// - `claude_code`：subagent transcript 位于 primary 同名子目录的
///   `subagents/` 下（`~/.claude/projects/{project}/{root}/subagents/*.jsonl`，
///   归组规则与 `session::claude_reader::derive_root_session_id` 的目录布局一致）；
/// - `codex`：同一日期目录下按时间段拆分的多个 `rollout-*.jsonl`，但只
///   收集 `session_meta` 中 root session id 与 primary 相同的文件。跨目录的
///   subagent 线程（独立 uuid）M2 不追踪：适配器能力只声明
///   [`AgentRelationLevel::FlagOnly`]，不承诺跨目录归组。
/// - 其他工具：仅 primary（M2 未注册对应适配器，正常不会走到）。
fn discover_related_files(tool: &str, primary_file_path: &str) -> Vec<String> {
    let primary = Path::new(primary_file_path);
    let mut extra: Vec<String> = Vec::new();
    match tool {
        crate::session::constants::TOOL_CLAUDE_CODE => {
            if let (Some(parent), Some(stem)) = (primary.parent(), primary.file_stem()) {
                let subdir = parent.join(stem).join("subagents");
                if let Ok(entries) = std::fs::read_dir(&subdir) {
                    extra.extend(
                        entries
                            .flatten()
                            .filter(|entry| {
                                entry.path().extension().is_some_and(|ext| ext == "jsonl")
                            })
                            .map(|entry| entry.path().to_string_lossy().to_string()),
                    );
                }
            }
        }
        crate::session::constants::TOOL_CODEX => {
            if let Some(parent) = primary.parent() {
                let primary_root =
                    crate::session::codex_reader::codex_rollout_root_session_id(primary);
                if let Ok(entries) = std::fs::read_dir(parent) {
                    extra.extend(
                        entries
                            .flatten()
                            .filter(|entry| {
                                let name = entry.file_name().to_string_lossy().to_string();
                                if !(name.starts_with("rollout-") && name.ends_with(".jsonl")) {
                                    return false;
                                }
                                let candidate = entry.path();
                                match (
                                    &primary_root,
                                    crate::session::codex_reader::codex_rollout_root_session_id(
                                        &candidate,
                                    ),
                                ) {
                                    (Some(root), Some(candidate_root)) => root == &candidate_root,
                                    _ => false,
                                }
                            })
                            .map(|entry| entry.path().to_string_lossy().to_string()),
                    );
                }
            }
        }
        _ => {}
    }
    extra.sort();
    extra.dedup();
    let mut files = vec![primary_file_path.to_string()];
    files.extend(extra.into_iter().filter(|file| file != primary_file_path));
    files
}

/// 合并多文件的 `ActivityIndexBatch` 为单个会话批次。
///
/// 两个必须处理的冲突：
/// 1. **event_key 冲突**：各文件独立产出事件键。Claude 的键基于全局唯一
///    message id（跨文件不冲突）；Codex 的键基于行号
///    （`codex:{session}:{line}`），同一会话多个 rollout 文件必然冲突。
///    对冲突文件（file_idx > 0 且键已出现过）的键追加 `#f{file_idx}` 后缀，
///    并同步重写该文件内的 `parent_event_key`/工具调用/请求关联引用
///    （引用只发生在文件内部，跨文件无引用）。文件发现顺序稳定，故
///    重复索引产出一致批次。
/// 2. **sequence 冲突**：各文件 sequence 都从 1 开始。合并后按事件时间、
///    来源文件、来源行号和原 sequence 做稳定排序，再重写为全局 1..N，
///    避免多 rollout 交错时按文件拼接造成时间线倒序。
fn merge_batches(session_key: &str, batches: Vec<ActivityIndexBatch>) -> ActivityIndexBatch {
    let mut out = ActivityIndexBatch {
        session_key: session_key.to_string(),
        ..ActivityIndexBatch::default()
    };
    let mut seen_keys: HashSet<String> = HashSet::new();
    let mut seen_invocations: HashSet<String> = HashSet::new();
    let mut seen_agents: HashSet<String> = HashSet::new();
    // 单文件内旧键 → 新键（仅冲突文件有映射）。
    let mut remap: HashMap<String, String> = HashMap::new();
    for (file_idx, batch) in batches.iter().enumerate() {
        remap.clear();
        let mut agent_remap: HashMap<String, String> = HashMap::new();
        for event in &batch.events {
            let key = &event.event_key;
            if file_idx > 0 && seen_keys.contains(key) {
                remap.insert(key.clone(), format!("{key}#f{file_idx}"));
            }
            seen_keys.insert(key.clone());
        }
        let rewrite =
            |key: &str| -> String { remap.get(key).cloned().unwrap_or_else(|| key.to_string()) };
        for agent in &batch.agents {
            let mut agent = agent.clone();
            let original_agent_key = agent.agent_key.clone();
            if !seen_agents.insert(agent.agent_key.clone()) {
                agent.agent_key = format!("{}#f{file_idx}", agent.agent_key);
                if let Some(parent) = agent.parent_agent_key.as_mut() {
                    *parent = format!("{}#f{file_idx}", parent);
                }
            }
            agent_remap.insert(original_agent_key, agent.agent_key.clone());
            out.agents.push(agent);
        }
        for event in &batch.events {
            let actor_agent_key = event
                .actor_agent_key
                .as_deref()
                .and_then(|key| agent_remap.get(key))
                .cloned()
                .or_else(|| event.actor_agent_key.clone());
            out.events.push(NewSessionEvent {
                event_key: rewrite(&event.event_key),
                parent_event_key: event
                    .parent_event_key
                    .as_deref()
                    .map(rewrite)
                    .or_else(|| event.parent_event_key.clone()),
                actor_agent_key,
                ..event.clone()
            });
        }
        for invocation in &batch.tool_invocations {
            let mut invocation_key = invocation.invocation_key.clone();
            if !seen_invocations.insert(invocation_key.clone()) {
                invocation_key = format!("{}#f{file_idx}", invocation_key);
            }
            out.tool_invocations.push(NewToolInvocation {
                event_key: rewrite(&invocation.event_key),
                invocation_key,
                ..invocation.clone()
            });
        }
        for link in &batch.request_links {
            out.request_links.push(NewEventRequestLink {
                event_key: rewrite(&link.event_key),
                ..link.clone()
            });
        }
    }
    out.events.sort_by(|left, right| {
        left.timestamp_ms
            .is_none()
            .cmp(&right.timestamp_ms.is_none())
            .then_with(|| left.timestamp_ms.cmp(&right.timestamp_ms))
            .then_with(|| left.source_file_path.cmp(&right.source_file_path))
            .then_with(|| left.source_offset.cmp(&right.source_offset))
            .then_with(|| left.sequence.cmp(&right.sequence))
    });
    for (index, event) in out.events.iter_mut().enumerate() {
        event.sequence = (index + 1) as i64;
    }
    out
}

/// 逐文件调用适配器 `index_session` 并合并批次。
fn index_session_files(
    adapter: &'static dyn SessionActivityAdapter,
    source: &SessionSourceRef,
    files: &[String],
) -> Result<ActivityIndexBatch, String> {
    let mut batches = Vec::with_capacity(files.len());
    for file in files {
        let file_source = SessionSourceRef {
            session_id: source.session_id.clone(),
            tool: source.tool.clone(),
            primary_file_path: file.clone(),
            source_file_id: None,
        };
        let mut batch = adapter
            .index_session(&file_source)
            .map_err(|error| error.to_string())?;
        if let Ok(fingerprint) = file_fingerprint(file) {
            for event in &mut batch.events {
                event.payload_hash = Some(fingerprint.clone());
            }
        }
        batches.push(batch);
    }
    Ok(merge_batches(&source.session_id, batches))
}

/// 构造 `session_activity_index` 写入条目（能力快照取自适配器当前上报值）。
fn build_index_entry(
    adapter: &'static dyn SessionActivityAdapter,
    source: &SessionSourceRef,
    batch: &ActivityIndexBatch,
    fingerprint: &str,
) -> ActivityIndexEntry {
    let capability = adapter.capability();
    let now = chrono::Utc::now().timestamp();
    ActivityIndexEntry {
        session_key: batch.session_key.clone(),
        tool: source.tool.clone(),
        capability: capability.clone(),
        event_count: batch.events.len() as i64,
        tool_call_count: batch.tool_invocations.len() as i64,
        agent_count: batch.agents.len() as i64,
        source_fingerprint: Some(fingerprint.to_string()),
        parser_id: capability.parser_id.clone(),
        parser_version: capability.parser_version,
        indexed_at: now,
        updated_at: now,
    }
}

/// 锁外解析得到的单会话索引快照。`observed_fingerprint` 用于提交时检测
/// 并发索引或隐私档位变化，避免旧解析结果覆盖更新的数据。
pub(crate) struct PreparedSessionIndex {
    entry: ActivityIndexEntry,
    batch: ActivityIndexBatch,
    policy: ActivityPersistencePolicy,
    observed_fingerprint: Option<String>,
}

pub(crate) fn query_session_fingerprint(
    conn: &Connection,
    session_key: &str,
) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT source_fingerprint FROM session_activity_index WHERE session_key = ?1",
        params![session_key],
        |row| row.get(0),
    )
    .optional()
    .map_err(|error| format!("ERR_ACTIVITY_QUERY_INDEX: {error}"))
}

/// 只进行文件系统访问与解析，不持有 SQLite 连接锁。
pub(crate) fn prepare_session_index_if_changed(
    source: &SessionSourceRef,
    policy: ActivityPersistencePolicy,
    observed_fingerprint: Option<String>,
) -> Result<Option<PreparedSessionIndex>, String> {
    if !policy.indexing_enabled() {
        return Ok(None);
    }
    let registry = AdapterRegistry::new();
    let adapter = registry.get_adapter(&source.tool).ok_or_else(|| {
        format!(
            "ERR_ACTIVITY_UNSUPPORTED: no adapter registered for tool {}",
            source.tool
        )
    })?;
    let files = discover_related_files(&source.tool, &source.primary_file_path);
    let fingerprint = compute_source_fingerprint(&files)?;
    if observed_fingerprint.as_deref() == Some(fingerprint.as_str()) {
        return Ok(None);
    }
    let batch = index_session_files(adapter, source, &files)?;
    // 文件在解析期间变化时不提交混合快照；下一次查询会自然重试。
    if compute_source_fingerprint(&files)? != fingerprint {
        return Err("ERR_ACTIVITY_SOURCE_CHANGED: source changed while indexing".to_string());
    }
    let entry = build_index_entry(adapter, source, &batch, &fingerprint);
    Ok(Some(PreparedSessionIndex {
        entry,
        batch,
        policy,
        observed_fingerprint,
    }))
}

/// 在短事务内提交锁外解析结果。若另一个任务已经提交，或数据库指纹自准备
/// 开始后发生变化，则丢弃当前快照，由后续查询按最新状态决定是否重建。
pub(crate) fn commit_prepared_session_index(
    conn: &Connection,
    prepared: PreparedSessionIndex,
) -> Result<Option<usize>, String> {
    let current = query_session_fingerprint(conn, &prepared.entry.session_key)?;
    if current == prepared.entry.source_fingerprint {
        return Ok(None);
    }
    if current != prepared.observed_fingerprint {
        return Ok(None);
    }
    let event_count = prepared.batch.events.len();
    db::write_activity_batch_with_policy(conn, &prepared.entry, &prepared.batch, prepared.policy)?;
    Ok(Some(event_count))
}

/// 核心索引步骤：指纹比对 → 过期则解析并写入；返回本次写入的事件数
/// （`None` = 指纹未变化，未写入）。保留该同步入口供纯数据库单元测试使用；
/// IPC 路径使用 prepare/commit 两阶段以避免解析期间占用全局 DB 锁。
fn index_session_if_stale(
    conn: &Connection,
    source: &SessionSourceRef,
    policy: ActivityPersistencePolicy,
) -> Result<Option<usize>, String> {
    let existing = query_session_fingerprint(conn, &source.session_id)?;
    let Some(prepared) = prepare_session_index_if_changed(source, policy, existing)? else {
        return Ok(None);
    };
    commit_prepared_session_index(conn, prepared)
}

/// 懒索引：单会话指纹比对 + 按需重建；返回是否发生了写入（新建或更新）。
pub fn ensure_session_indexed_with_policy(
    conn: &Connection,
    source: &SessionSourceRef,
    policy: ActivityPersistencePolicy,
) -> Result<bool, String> {
    Ok(index_session_if_stale(conn, source, policy)?.is_some())
}

#[cfg(test)]
pub fn ensure_session_indexed(
    conn: &Connection,
    source: &SessionSourceRef,
) -> Result<bool, String> {
    ensure_session_indexed_with_policy(conn, source, ActivityPersistencePolicy::FullText)
}

/// 按 session_key 定位会话源引用（供懒索引查询路径使用）。
///
/// 以 `local_sessions` 为准（会话聚合行，含 primary_file_path）；工具未注册
/// 适配器时返回 `None`（查询继续走既有深度表，不伪造能力）。
pub fn find_source_ref(
    conn: &Connection,
    session_key: &str,
) -> Result<Option<SessionSourceRef>, String> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT tool, primary_file_path FROM local_sessions WHERE session_id = ?1",
            params![session_key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| format!("ERR_ACTIVITY_QUERY_SESSION: {error}"))?;
    let Some((tool, Some(primary_file_path))) = row else {
        return Ok(None);
    };
    if primary_file_path.trim().is_empty() {
        return Ok(None);
    }
    if AdapterRegistry::new().get_adapter(&tool).is_none() {
        return Ok(None);
    }
    Ok(Some(SessionSourceRef {
        session_id: session_key.to_string(),
        tool,
        primary_file_path,
        source_file_id: None,
    }))
}

/// 列出可深度索引的会话（工具在适配器注册表中、有 primary 文件）。
///
/// 遍历 `local_sessions` 而非 `local_source_files`：Claude/Codex 的
/// 关联文件在 `local_source_files` 中不逐文件落表（每会话仅 primary 一行，
/// `file_role = "session_group"`），以会话表为主键遍历并配合
/// `discover_related_files` 推导关联文件。
pub fn list_indexable_sessions(
    conn: &Connection,
    registry: &AdapterRegistry,
) -> Result<Vec<IndexableSession>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT session_id, tool, primary_file_path FROM local_sessions
             WHERE primary_file_path IS NOT NULL AND primary_file_path != ''",
        )
        .map_err(|error| format!("ERR_ACTIVITY_PREPARE_LIST_SESSIONS: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(IndexableSession {
                session_id: row.get(0)?,
                tool: row.get(1)?,
                primary_file_path: row.get(2)?,
            })
        })
        .map_err(|error| format!("ERR_ACTIVITY_QUERY_LIST_SESSIONS: {error}"))?;
    let mut sessions = Vec::new();
    for row in rows {
        let session = row.map_err(|error| format!("ERR_ACTIVITY_READ_LIST_SESSION: {error}"))?;
        if registry.get_adapter(&session.tool).is_some() {
            sessions.push(session);
        }
    }
    sessions.sort_by(|left, right| left.session_id.cmp(&right.session_id));
    Ok(sessions)
}

/// 重建深度活动索引。
///
/// scope 支持：
/// - `"all"`：遍历全部可索引会话，逐个 `index_session_if_stale`
///   （指纹未变化自动跳过；单会话失败记录 errors 继续，设计 14.4 隔离）；
/// - `"session:<session_key>"`：单会话；
/// - `"older_than_days:<n>"`：仅 purge 支持的清理 scope，此处返回明确错误。
pub fn rebuild_scope_with_policy(
    conn: &Connection,
    scope: &str,
    policy: ActivityPersistencePolicy,
) -> Result<RebuildResult, String> {
    if !policy.indexing_enabled() {
        return Ok(RebuildResult::default());
    }
    let registry = AdapterRegistry::new();
    let mut result = RebuildResult::default();

    if scope == "all" {
        for session in list_indexable_sessions(conn, &registry)? {
            let source = SessionSourceRef {
                session_id: session.session_id.clone(),
                tool: session.tool.clone(),
                primary_file_path: session.primary_file_path.clone(),
                source_file_id: None,
            };
            match index_session_if_stale(conn, &source, policy) {
                Ok(Some(written)) => {
                    result.sessions_indexed += 1;
                    result.events_written += written as i64;
                }
                Ok(None) => {}
                Err(error) => {
                    result.sessions_failed += 1;
                    result.errors.push(error);
                }
            }
        }
        return Ok(result);
    }

    if let Some(session_key) = scope.strip_prefix("session:") {
        let session_key = session_key.trim();
        if session_key.is_empty() {
            return Err("ERR_ACTIVITY_INVALID_SCOPE: empty session key".to_string());
        }
        let source = find_source_ref(conn, session_key)?.ok_or_else(|| {
            "ERR_ACTIVITY_SESSION_NOT_FOUND: session not found in local sessions".to_string()
        })?;
        if let Some(written) = index_session_if_stale(conn, &source, policy)? {
            result.sessions_indexed = 1;
            result.events_written = written as i64;
        }
        return Ok(result);
    }

    if scope.starts_with("older_than_days:") {
        return Err(
            "ERR_ACTIVITY_UNSUPPORTED_SCOPE: older_than_days is a purge-only scope".to_string(),
        );
    }

    Err(format!(
        "ERR_ACTIVITY_INVALID_SCOPE: unsupported scope: {scope}"
    ))
}

#[cfg(test)]
pub fn rebuild_scope(conn: &Connection, scope: &str) -> Result<RebuildResult, String> {
    rebuild_scope_with_policy(conn, scope, ActivityPersistencePolicy::FullText)
}

/// 清理深度活动内容（scope："all" | "session:<session_key>" |
/// "older_than_days:<n>"）；返回删除的行数（5 张深度表之和）。
///
/// 只影响深度索引表，不删除原工具会话文件、不影响 `local_request_facts`
/// 等既有用量表口径。
pub fn purge_scope(conn: &Connection, scope: &str) -> Result<usize, String> {
    if scope == "all" {
        return db::delete_all_activity(conn, false);
    }
    if let Some(session_key) = scope.strip_prefix("session:") {
        let session_key = session_key.trim();
        if session_key.is_empty() {
            return Err("ERR_ACTIVITY_INVALID_SCOPE: empty session key".to_string());
        }
        return db::delete_session_activity(conn, session_key, false);
    }
    if let Some(days_str) = scope.strip_prefix("older_than_days:") {
        let days: i64 = days_str.trim().parse().map_err(|_| {
            format!("ERR_ACTIVITY_INVALID_SCOPE: invalid days in scope: {days_str}")
        })?;
        if days <= 0 {
            return Err("ERR_ACTIVITY_INVALID_SCOPE: days must be positive".to_string());
        }
        return db::delete_activity_older_than_days(conn, days, false);
    }
    Err(format!(
        "ERR_ACTIVITY_INVALID_SCOPE: unsupported scope: {scope}"
    ))
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::adapter::NewAgentNode;
    use crate::activity::model::{
        ContentState, EventStatus, SessionActivityCapability, SessionEventKind,
    };
    use crate::session::constants::TOOL_CLAUDE_CODE;
    use serde_json::{json, Value};
    use std::io::Write;

    /// 内存测试连接：深度表 + 精简 local_sessions（只含本服务用到的列）。
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory sqlite");
        conn.execute_batch(crate::activity::db::ACTIVITY_TABLES_DDL)
            .expect("create activity tables");
        conn.execute_batch(
            "CREATE TABLE local_sessions (
                session_id TEXT PRIMARY KEY,
                tool TEXT NOT NULL,
                primary_file_path TEXT,
                end_time INTEGER NOT NULL DEFAULT 0
            );",
        )
        .expect("create local_sessions");
        conn
    }

    fn insert_session(conn: &Connection, session_id: &str, tool: &str, primary: &str) {
        conn.execute(
            "INSERT INTO local_sessions (session_id, tool, primary_file_path) VALUES (?1, ?2, ?3)",
            params![session_id, tool, primary],
        )
        .expect("insert local session");
    }

    /// Claude fixture：与 `adapters/claude.rs` 测试同构的简化版，4 个事件
    /// （user 文本 / assistant 文本 / tool_use / tool_result）。
    fn claude_fixture_lines() -> Vec<Value> {
        vec![
            json!({"timestamp": 1747000000.0, "type": "user", "message": {
                "role": "user", "id": "msg-user-1",
                "content": [{"type": "text", "text": "fix the login bug"}]
            }}),
            json!({"timestamp": 1747000001.5, "type": "assistant", "message": {
                "role": "assistant", "id": "msg-assistant-1",
                "content": [{"type": "text", "text": "let me look"}],
                "usage": {"input_tokens": 100, "output_tokens": 50}
            }}),
            json!({"timestamp": 1747000002.0, "type": "assistant", "message": {
                "role": "assistant", "id": "msg-assistant-2",
                "content": [{"type": "tool_use", "id": "toolu_01", "name": "Bash",
                    "input": {"command": "pwd", "cwd": "/tmp"}}]
            }}),
            json!({"timestamp": 1747000003.0, "type": "user", "message": {
                "role": "user", "id": "msg-user-2",
                "content": [{"type": "tool_result", "tool_use_id": "toolu_01",
                    "content": "/tmp\n", "is_error": false}]
            }}),
        ]
    }

    fn write_claude_fixture(lines: &[Value]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("claude-test.jsonl");
        let mut file = std::fs::File::create(&path).expect("create fixture");
        for line in lines {
            writeln!(file, "{line}").expect("write fixture line");
        }
        (dir, path.to_string_lossy().to_string())
    }

    fn count_events(conn: &Connection, session_key: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM session_events WHERE session_key = ?1",
            params![session_key],
            |row| row.get(0),
        )
        .expect("count events")
    }

    fn sample_capability() -> SessionActivityCapability {
        SessionActivityCapability {
            level: crate::activity::model::ActivityCapabilityLevel::Structured,
            messages: true,
            tool_invocations: true,
            tool_results: true,
            request_links: true,
            agent_relations: crate::activity::model::AgentRelationLevel::RootGrouped,
            content_search: false,
            source_content_available: true,
            parser_id: "claude_code".to_string(),
            parser_version: 1,
        }
    }

    /// 构造单文件批次（sequence 从 1 起，与真实适配器行为一致）。
    fn sample_batch(session_key: &str, line_base: i64) -> ActivityIndexBatch {
        ActivityIndexBatch {
            session_key: session_key.to_string(),
            events: vec![
                NewSessionEvent {
                    event_key: format!("evt-{line_base}-1"),
                    sequence: 1,
                    timestamp_ms: Some(1_700_000_000_000),
                    kind: SessionEventKind::UserMessage,
                    status: None,
                    actor_agent_key: None,
                    parent_event_key: None,
                    summary_redacted: Some("hello".to_string()),
                    content_state: ContentState::Available,
                    source_file_path: format!("file-{line_base}.jsonl"),
                    source_offset: Some(1),
                    payload_hash: None,
                    raw_event_kind: "user".to_string(),
                },
                NewSessionEvent {
                    event_key: format!("evt-{line_base}-2"),
                    sequence: 2,
                    timestamp_ms: Some(1_700_000_001_000),
                    kind: SessionEventKind::ToolInvocation,
                    status: Some(EventStatus::Success),
                    actor_agent_key: None,
                    parent_event_key: None,
                    summary_redacted: Some("run".to_string()),
                    content_state: ContentState::Available,
                    source_file_path: format!("file-{line_base}.jsonl"),
                    source_offset: Some(2),
                    payload_hash: None,
                    raw_event_kind: "tool_use".to_string(),
                },
            ],
            agents: vec![NewAgentNode {
                agent_key: format!("agent-{line_base}"),
                parent_agent_key: None,
                display_kind: Some("Task".to_string()),
                task_summary_redacted: Some("task".to_string()),
                started_at_ms: None,
                ended_at_ms: None,
                status: Some(EventStatus::Success),
                relation_level: crate::activity::model::AgentRelationLevel::RootGrouped,
            }],
            tool_invocations: vec![NewToolInvocation {
                invocation_key: format!("inv-{line_base}-1"),
                event_key: format!("evt-{line_base}-2"),
                tool_name: "Bash".to_string(),
                family: "shell".to_string(),
                duration_ms: None,
                status: Some(EventStatus::Success),
                input_bytes: None,
                output_bytes: None,
                input_keys: vec![],
                result_kind: None,
            }],
            request_links: vec![],
        }
    }

    #[test]
    fn merge_batches_renumbers_sequence_and_resolves_key_conflicts() {
        // 两个文件各自从 sequence=1 开始、事件键前缀相同（模拟 codex 多 rollout）。
        let mut first = sample_batch("sess-1", 1);
        first.events[0].event_key = "codex:sess-1:1".to_string();
        first.events[1].event_key = "codex:sess-1:2".to_string();
        first.tool_invocations[0].event_key = "codex:sess-1:2".to_string();

        let mut second = sample_batch("sess-1", 2);
        second.events[0].event_key = "codex:sess-1:1".to_string();
        second.events[1].event_key = "codex:sess-1:2".to_string();
        second.events[1].parent_event_key = Some("codex:sess-1:1".to_string());
        second.tool_invocations[0].event_key = "codex:sess-1:2".to_string();

        let merged = merge_batches("sess-1", vec![first, second]);

        assert_eq!(merged.events.len(), 4);
        // sequence 全局重排 1..4，文件内相对顺序保持。
        let sequences: Vec<i64> = merged.events.iter().map(|event| event.sequence).collect();
        assert_eq!(sequences, vec![1, 2, 3, 4]);
        // 冲突键重写：第二文件的重复键追加 #f1，引用同步重写。
        assert_eq!(merged.events[0].event_key, "codex:sess-1:1");
        assert_eq!(merged.events[1].event_key, "codex:sess-1:1#f1");
        assert_eq!(merged.events[2].event_key, "codex:sess-1:2");
        assert_eq!(merged.events[3].event_key, "codex:sess-1:2#f1");
        assert_eq!(
            merged.events[3].parent_event_key.as_deref(),
            Some("codex:sess-1:1#f1")
        );
        assert_eq!(merged.tool_invocations.len(), 2);
        assert_eq!(merged.tool_invocations[1].event_key, "codex:sess-1:2#f1");
    }

    #[test]
    fn discover_related_files_finds_claude_subagents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let primary = dir.path().join("proj").join("sess-root.jsonl");
        std::fs::create_dir_all(dir.path().join("proj").join("sess-root").join("subagents"))
            .expect("create subagents dir");
        let sub1 = dir
            .path()
            .join("proj")
            .join("sess-root")
            .join("subagents")
            .join("sub-1.jsonl");
        let sub2 = dir
            .path()
            .join("proj")
            .join("sess-root")
            .join("subagents")
            .join("sub-2.jsonl");
        for file in [&primary, &sub1, &sub2] {
            std::fs::write(file, b"{}").expect("write fixture");
        }

        let files = discover_related_files(TOOL_CLAUDE_CODE, &primary.to_string_lossy());
        assert_eq!(files.len(), 3);
        assert_eq!(files[0], primary.to_string_lossy().to_string());
        assert_eq!(files[1], sub1.to_string_lossy().to_string());
        assert_eq!(files[2], sub2.to_string_lossy().to_string());

        // 无 subagents 目录时只返回 primary。
        let bare = dir.path().join("bare.jsonl");
        std::fs::write(&bare, b"{}").expect("write bare");
        let files = discover_related_files(TOOL_CLAUDE_CODE, &bare.to_string_lossy());
        assert_eq!(files, vec![bare.to_string_lossy().to_string()]);
    }

    #[test]
    fn discover_related_files_keeps_codex_rollouts_in_same_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let primary = dir.path().join("rollout-primary.jsonl");
        let same = dir.path().join("rollout-same.jsonl");
        let other = dir.path().join("rollout-other.jsonl");
        let meta = |id: &str| {
            format!(
                "{}\n{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"hello\"}}}}\n",
                json!({"type":"session_meta","payload":{"id":id}})
            )
        };
        std::fs::write(&primary, meta("root-1")).expect("write primary");
        std::fs::write(&same, meta("root-1")).expect("write same");
        std::fs::write(&other, meta("root-2")).expect("write other");

        let files = discover_related_files(
            crate::session::constants::TOOL_CODEX,
            &primary.to_string_lossy(),
        );
        assert_eq!(
            files,
            vec![
                primary.to_string_lossy().to_string(),
                same.to_string_lossy().to_string(),
            ]
        );
    }

    #[test]
    fn lazy_index_is_idempotent_and_rebuilds_on_fingerprint_change() {
        let conn = test_conn();
        let (_dir, path) = write_claude_fixture(&claude_fixture_lines());
        let session_key = "proj::sess-1";
        insert_session(&conn, session_key, TOOL_CLAUDE_CODE, &path);

        let source = find_source_ref(&conn, session_key)
            .expect("find source")
            .expect("source exists");
        assert_eq!(source.primary_file_path, path);

        // 首次：新建索引。
        assert!(ensure_session_indexed(&conn, &source).expect("first index"));
        assert_eq!(count_events(&conn, session_key), 4);
        let indexed_at_first: i64 = conn
            .query_row(
                "SELECT indexed_at FROM session_activity_index WHERE session_key = ?1",
                params![session_key],
                |row| row.get(0),
            )
            .expect("read indexed_at");

        // 二次：指纹未变化，幂等（不写入、事件数不变）。
        assert!(!ensure_session_indexed(&conn, &source).expect("second index"));
        assert_eq!(count_events(&conn, session_key), 4);

        // 指纹变化（追加内容 → 文件 size 变化）后重建。
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open fixture append");
        writeln!(
            file,
            "{}",
            json!({"timestamp": 1747000004.0, "type": "user", "message": {
                "role": "user", "id": "msg-user-3",
                "content": [{"type": "text", "text": "one more turn"}]
            }})
        )
        .expect("append fixture line");
        drop(file);

        assert!(ensure_session_indexed(&conn, &source).expect("rebuild after change"));
        assert_eq!(count_events(&conn, session_key), 5);
        let indexed_at_second: i64 = conn
            .query_row(
                "SELECT indexed_at FROM session_activity_index WHERE session_key = ?1",
                params![session_key],
                |row| row.get(0),
            )
            .expect("read indexed_at");
        // 首次索引时间保留（保留期语义按数据首次建立索引计算）。
        assert_eq!(indexed_at_first, indexed_at_second);
    }

    #[test]
    fn prepared_index_does_not_overwrite_a_concurrent_fingerprint_change() {
        let conn = test_conn();
        let (_dir, path) = write_claude_fixture(&claude_fixture_lines());
        let session_key = "proj::concurrent";
        insert_session(&conn, session_key, TOOL_CLAUDE_CODE, &path);
        let source = find_source_ref(&conn, session_key)
            .expect("find source")
            .expect("source exists");
        let prepared =
            prepare_session_index_if_changed(&source, ActivityPersistencePolicy::FullText, None)
                .expect("prepare index")
                .expect("changed source");

        let mut concurrent_entry = prepared.entry.clone();
        concurrent_entry.source_fingerprint = Some("newer-fingerprint".to_string());
        db::upsert_session_activity_index(&conn, &concurrent_entry)
            .expect("write concurrent fingerprint");

        assert_eq!(
            commit_prepared_session_index(&conn, prepared).expect("commit prepared"),
            None
        );
        assert_eq!(
            query_session_fingerprint(&conn, session_key).expect("read fingerprint"),
            Some("newer-fingerprint".to_string())
        );
        assert_eq!(count_events(&conn, session_key), 0);
    }

    #[test]
    fn rebuild_all_isolates_single_session_failure() {
        let conn = test_conn();
        let (_dir, good1) = write_claude_fixture(&claude_fixture_lines());
        let (_dir2, good2) = write_claude_fixture(&claude_fixture_lines());
        insert_session(&conn, "proj::good-1", TOOL_CLAUDE_CODE, &good1);
        insert_session(&conn, "proj::good-2", TOOL_CLAUDE_CODE, &good2);
        // 坏会话：primary 文件不存在 → 解析失败。
        insert_session(
            &conn,
            "proj::broken",
            TOOL_CLAUDE_CODE,
            "/nonexistent/claude-broken.jsonl",
        );
        // 无适配器工具：不应出现在可索引列表，也不计失败。
        insert_session(&conn, "proj::opencode-x", "opencode", "/tmp/x.jsonl");

        let result = rebuild_scope(&conn, "all").expect("rebuild all");
        assert_eq!(result.sessions_indexed, 2);
        assert_eq!(result.sessions_failed, 1);
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].starts_with("ERR_ACTIVITY_IO"));
        assert!(result.events_written > 0);
        assert_eq!(count_events(&conn, "proj::good-1"), 4);
        assert_eq!(count_events(&conn, "proj::good-2"), 4);
        assert_eq!(count_events(&conn, "proj::broken"), 0);

        // 再次 rebuild：指纹未变化，全部跳过。
        let again = rebuild_scope(&conn, "all").expect("rebuild all again");
        assert_eq!(again.sessions_indexed, 0);
        assert_eq!(again.sessions_failed, 1);
    }

    #[test]
    fn rebuild_session_scope_and_unsupported_scopes() {
        let conn = test_conn();
        let (_dir, path) = write_claude_fixture(&claude_fixture_lines());
        insert_session(&conn, "proj::sess-1", TOOL_CLAUDE_CODE, &path);

        let result = rebuild_scope(&conn, "session:proj::sess-1").expect("rebuild session");
        assert_eq!(result.sessions_indexed, 1);
        assert_eq!(count_events(&conn, "proj::sess-1"), 4);

        // 不存在的会话 → 明确错误。
        let err = rebuild_scope(&conn, "session:missing").expect_err("missing session");
        assert!(err.starts_with("ERR_ACTIVITY_SESSION_NOT_FOUND"));

        // rebuild 不支持 older_than_days。
        let err = rebuild_scope(&conn, "older_than_days:30").expect_err("purge-only scope");
        assert!(err.starts_with("ERR_ACTIVITY_UNSUPPORTED_SCOPE"));

        // 非法 scope。
        let err = rebuild_scope(&conn, "bogus").expect_err("invalid scope");
        assert!(err.starts_with("ERR_ACTIVITY_INVALID_SCOPE"));
    }

    #[test]
    fn purge_single_all_and_older_than_days() {
        let conn = test_conn();
        let (_dir, path) = write_claude_fixture(&claude_fixture_lines());
        insert_session(&conn, "proj::sess-1", TOOL_CLAUDE_CODE, &path);
        let source = find_source_ref(&conn, "proj::sess-1")
            .expect("find source")
            .expect("source exists");
        ensure_session_indexed(&conn, &source).expect("index");

        // 单会话 purge。
        let removed = purge_scope(&conn, "session:proj::sess-1").expect("purge session");
        assert!(removed > 0);
        assert_eq!(count_events(&conn, "proj::sess-1"), 0);

        // 重新索引后 older_than_days：把 indexed_at 改到很久以前 → 全部删除。
        ensure_session_indexed(&conn, &source).expect("reindex");
        conn.execute(
            "UPDATE session_activity_index SET indexed_at = 1 WHERE session_key = ?1",
            params!["proj::sess-1"],
        )
        .expect("age index row");
        let removed = purge_scope(&conn, "older_than_days:1").expect("purge old");
        assert!(removed > 0);
        assert_eq!(count_events(&conn, "proj::sess-1"), 0);

        // 重新索引后 purge all。
        ensure_session_indexed(&conn, &source).expect("reindex again");
        let removed = purge_scope(&conn, "all").expect("purge all");
        assert!(removed > 0);
        assert_eq!(count_events(&conn, "proj::sess-1"), 0);

        // 非法 scope。
        let err = purge_scope(&conn, "bogus").expect_err("invalid scope");
        assert!(err.starts_with("ERR_ACTIVITY_INVALID_SCOPE"));
    }

    #[test]
    fn index_entry_carries_adapter_capability_and_counts() {
        let conn = test_conn();
        let (_dir, path) = write_claude_fixture(&claude_fixture_lines());
        let session_key = "proj::sess-cap";
        insert_session(&conn, session_key, TOOL_CLAUDE_CODE, &path);
        let source = find_source_ref(&conn, session_key)
            .expect("find source")
            .expect("source exists");
        ensure_session_indexed(&conn, &source).expect("index");

        let (capability_json, event_count, tool_call_count, agent_count): (String, i64, i64, i64) =
            conn.query_row(
                "SELECT capability_json, event_count, tool_call_count, agent_count
                 FROM session_activity_index WHERE session_key = ?1",
                params![session_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read index entry");
        let capability: SessionActivityCapability =
            serde_json::from_str(&capability_json).expect("parse capability");
        assert_eq!(capability, sample_capability());
        assert_eq!(event_count, 4);
        assert_eq!(tool_call_count, 1);
        assert_eq!(agent_count, 0);
    }

    #[test]
    fn find_source_ref_returns_none_for_unknown_session_or_tool() {
        let conn = test_conn();
        assert!(find_source_ref(&conn, "missing")
            .expect("no session")
            .is_none());
        insert_session(&conn, "proj::opencode-x", "opencode", "/tmp/x.jsonl");
        assert!(find_source_ref(&conn, "proj::opencode-x")
            .expect("no adapter")
            .is_none());
    }

    #[test]
    fn list_indexable_sessions_filters_by_registry() {
        let conn = test_conn();
        insert_session(&conn, "proj::sess-1", TOOL_CLAUDE_CODE, "/tmp/a.jsonl");
        insert_session(&conn, "proj::sess-2", "codex", "/tmp/b.jsonl");
        insert_session(&conn, "proj::sess-3", "opencode", "/tmp/c.jsonl");
        let registry = AdapterRegistry::new();
        let sessions = list_indexable_sessions(&conn, &registry).expect("list sessions");
        let tools: Vec<&str> = sessions
            .iter()
            .map(|session| session.tool.as_str())
            .collect();
        assert_eq!(tools, vec!["claude_code", "codex"]);
    }

    #[test]
    fn error_messages_never_contain_file_paths() {
        let conn = test_conn();
        insert_session(
            &conn,
            "proj::broken",
            TOOL_CLAUDE_CODE,
            "/secret/private-dir/claude-x.jsonl",
        );
        let source = find_source_ref(&conn, "proj::broken")
            .expect("find source")
            .expect("source exists");
        let err = ensure_session_indexed(&conn, &source).expect_err("io error");
        assert!(err.starts_with("ERR_ACTIVITY_IO"));
        assert!(!err.contains("secret"), "error must not leak path");
        assert!(!err.contains("private-dir"), "error must not leak path");
    }
}
