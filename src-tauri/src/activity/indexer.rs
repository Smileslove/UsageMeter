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
use crate::activity::db::{self, ActivityIndexEntry};
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
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    Ok(format!("{}:{}", mtime, metadata.len()))
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
/// - `codex`：同一会话 uuid 目录下按时间段拆分的多个 `rollout-*.jsonl`
///   （`~/.codex/sessions/{uuid}/rollout-*.jsonl`）都属于同一会话。
///   跨目录的 subagent 线程（独立 uuid）M2 不追踪：适配器能力只声明
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
                if let Ok(entries) = std::fs::read_dir(parent) {
                    extra.extend(
                        entries
                            .flatten()
                            .filter(|entry| {
                                let name = entry.file_name().to_string_lossy().to_string();
                                name.starts_with("rollout-") && name.ends_with(".jsonl")
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
/// 2. **sequence 冲突**：各文件 sequence 都从 1 开始。合并后按
///    （文件顺序，文件内原 sequence）稳定排序并重写为全局 1..N——
///    `sort_by_key` 稳定，同 sequence 元素保持文件间先后，文件内相对
///    顺序不变，事件顺序稳定可重放。
fn merge_batches(session_key: &str, batches: Vec<ActivityIndexBatch>) -> ActivityIndexBatch {
    let mut out = ActivityIndexBatch {
        session_key: session_key.to_string(),
        ..ActivityIndexBatch::default()
    };
    let mut seen_keys: HashSet<String> = HashSet::new();
    // 单文件内旧键 → 新键（仅冲突文件有映射）。
    let mut remap: HashMap<String, String> = HashMap::new();
    for (file_idx, batch) in batches.iter().enumerate() {
        remap.clear();
        for event in &batch.events {
            let key = &event.event_key;
            if file_idx > 0 && seen_keys.contains(key) {
                remap.insert(key.clone(), format!("{key}#f{file_idx}"));
            }
            seen_keys.insert(key.clone());
        }
        let rewrite =
            |key: &str| -> String { remap.get(key).cloned().unwrap_or_else(|| key.to_string()) };
        for event in &batch.events {
            out.events.push(NewSessionEvent {
                event_key: rewrite(&event.event_key),
                parent_event_key: event
                    .parent_event_key
                    .as_deref()
                    .map(rewrite)
                    .or_else(|| event.parent_event_key.clone()),
                ..event.clone()
            });
        }
        for agent in &batch.agents {
            out.agents.push(agent.clone());
        }
        for invocation in &batch.tool_invocations {
            out.tool_invocations.push(NewToolInvocation {
                event_key: rewrite(&invocation.event_key),
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
    // 保持文件顺序（primary → 后续文件），sequence 全局重排；
    // 时间线顺序由写入层的 (timestamp_ms, source_offset, event_index) 排序键保证。
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
        let batch = adapter
            .index_session(&file_source)
            .map_err(|error| error.to_string())?;
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

/// 核心索引步骤：指纹比对 → 过期则解析并写入；返回本次写入的事件数
/// （`None` = 指纹未变化，未写入）。
///
/// 注意：本函数在持有 `conn` 借用期间完成文件解析（懒索引路径单会话文件
/// 小、通常 <100ms，可接受；rebuild 由用户显式触发，spawn_blocking 执行）。
fn index_session_if_stale(
    conn: &Connection,
    source: &SessionSourceRef,
) -> Result<Option<usize>, String> {
    let registry = AdapterRegistry::new();
    let adapter = registry.get_adapter(&source.tool).ok_or_else(|| {
        format!(
            "ERR_ACTIVITY_UNSUPPORTED: no adapter registered for tool {}",
            source.tool
        )
    })?;
    let files = discover_related_files(&source.tool, &source.primary_file_path);
    let fingerprint = compute_source_fingerprint(&files)?;
    let existing: Option<String> = conn
        .query_row(
            "SELECT source_fingerprint FROM session_activity_index WHERE session_key = ?1",
            params![source.session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("ERR_ACTIVITY_QUERY_INDEX: {error}"))?;
    if existing.as_deref() == Some(fingerprint.as_str()) {
        return Ok(None);
    }
    let batch = index_session_files(adapter, source, &files)?;
    let entry = build_index_entry(adapter, source, &batch, &fingerprint);
    db::write_activity_batch(conn, &entry, &batch)?;
    Ok(Some(batch.events.len()))
}

/// 懒索引：单会话指纹比对 + 按需重建；返回是否发生了写入（新建或更新）。
pub fn ensure_session_indexed(
    conn: &Connection,
    source: &SessionSourceRef,
) -> Result<bool, String> {
    Ok(index_session_if_stale(conn, source)?.is_some())
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
pub fn rebuild_scope(conn: &Connection, scope: &str) -> Result<RebuildResult, String> {
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
            match index_session_if_stale(conn, &source) {
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
        if let Some(written) = index_session_if_stale(conn, &source)? {
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
        assert_eq!(merged.events[1].event_key, "codex:sess-1:2");
        assert_eq!(merged.events[2].event_key, "codex:sess-1:1#f1");
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
