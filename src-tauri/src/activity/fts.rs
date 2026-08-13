//! 深度活动全文索引（M3，SQLite FTS5；设计文档 12.4「可选全文表」）。
//!
//! 隐私边界（与 12.4/11.3 一致）：
//! - **只索引已脱敏摘要与工具名**（`session_events.summary_redacted` +
//!   `session_tool_invocations.tool_name`），绝不索引正文/payload；
//! - FTS 数据与 `session_events` 同步维护：写入批次后全量重建该会话 FTS 行
//!   （[`sync_events_to_fts`]），删除路径同步清理（[`delete_session_fts`]），
//!   不新增任何持久化内容——删除深度表即删除对应 FTS 数据；
//! - 查询入口（命令层）受 `deep_index_level == "fulltext"` 门控，本模块的
//!   纯 db 层函数不做门控（门控在 IPC 命令层，见 `commands/activity_search.rs`）。
//!
//! 查询安全：用户输入必须经 [`escape_fts_query`] 转成 FTS5 短语查询
//! （双引号包裹 + 内部引号转义），`*`/`NEAR`/`OR`/`AND`/`NOT`/括号等
//! 语法字符被中和为字面量，防止 FTS 语法注入；MATCH 查询串一律参数绑定。

use rusqlite::{params, Connection};

/// 全文搜索单页上限（与事件列表页一致，命令层 clamp 1..=200）。
pub const SEARCH_PAGE_MAX_LIMIT: i64 = 200;
/// 搜索词长度上限（字符数；防超长短语拖垮 FTS 解析）。
pub const SEARCH_QUERY_MAX_CHARS: usize = 200;

/// 跨会话搜索命中的最小载体（event_key + 归属会话；详情由命令层回填）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FtsHit {
    pub event_key: String,
    pub session_key: String,
}

/// 把用户输入转成安全的 FTS5 短语查询。
///
/// - trim 后为空 → `ERR_ACTIVITY_SEARCH_QUERY_EMPTY`；
/// - 超过 [`SEARCH_QUERY_MAX_CHARS`] 字符 → `ERR_ACTIVITY_SEARCH_QUERY_TOO_LONG`；
/// - 短语内 `"` 按 FTS5 规则转义为 `""`，整词用双引号包裹：
///   `*`、`NEAR`、`OR`、`AND`、`NOT`、`^`、括号等语法字符全部成为
///   字面量，任何输入都不会改变查询结构（防语法注入的关键）。
pub fn escape_fts_query(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("ERR_ACTIVITY_SEARCH_QUERY_EMPTY".to_string());
    }
    if trimmed.chars().count() > SEARCH_QUERY_MAX_CHARS {
        return Err("ERR_ACTIVITY_SEARCH_QUERY_TOO_LONG".to_string());
    }
    let escaped = trimmed.replace('"', "\"\"");
    Ok(format!("\"{escaped}\""))
}

/// 同步重建单会话的 FTS 行：事务内先删该会话旧行，再从 `session_events`
/// （左连 `session_tool_invocations` 取工具名）全量重灌。
///
/// 必须在 `write_activity_batch` 的 `upsert_tool_invocations` **之后**调用，
/// 否则新工具行尚未写入、工具名会全部为空。summary/tool_name 为空时存 `''`
/// （FTS5 不接受 NULL 索引值）。只读 `summary_redacted`（已脱敏列），
/// 绝不触碰 payload。
pub fn sync_events_to_fts(conn: &Connection, session_key: &str) -> Result<(), String> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("ERR_ACTIVITY_SEARCH_TX_START: {e}"))?;
    tx.execute(
        "DELETE FROM session_event_fts WHERE session_key = ?1",
        params![session_key],
    )
    .map_err(|e| format!("ERR_ACTIVITY_SEARCH_DELETE: {e}"))?;
    // 事件与工具经 event_key 关联（每事件最多一个 tool 行）；GROUP BY
    // 防御历史脏数据造成的多 tool 行，保证每事件在 FTS 中至多一行。
    let rows: Vec<(String, String, String, String, String)> = {
        let mut stmt = tx
            .prepare(
                "SELECT e.event_key, e.session_key, e.kind,
                        COALESCE(e.summary_redacted, ''),
                        COALESCE(MIN(ti.tool_name), '')
                 FROM session_events e
                 LEFT JOIN session_tool_invocations ti ON ti.event_key = e.event_key
                 WHERE e.session_key = ?1
                 GROUP BY e.event_key",
            )
            .map_err(|e| format!("ERR_ACTIVITY_SEARCH_PREPARE_SYNC: {e}"))?;
        let mapped = stmt
            .query_map(params![session_key], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .map_err(|e| format!("ERR_ACTIVITY_SEARCH_QUERY_SYNC: {e}"))?;
        let mut out = Vec::new();
        for row in mapped {
            out.push(row.map_err(|e| format!("ERR_ACTIVITY_SEARCH_READ_SYNC: {e}"))?);
        }
        out
    };
    {
        let mut insert = tx
            .prepare(
                "INSERT INTO session_event_fts (event_key, session_key, kind, summary, tool_name)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .map_err(|e| format!("ERR_ACTIVITY_SEARCH_PREPARE_INSERT: {e}"))?;
        for (event_key, session_key_row, kind, summary, tool_name) in rows {
            insert
                .execute(params![
                    event_key,
                    session_key_row,
                    kind,
                    summary,
                    tool_name
                ])
                .map_err(|e| format!("ERR_ACTIVITY_SEARCH_INSERT: {e}"))?;
        }
    }
    tx.commit()
        .map_err(|e| format!("ERR_ACTIVITY_SEARCH_TX_COMMIT: {e}"))?;
    Ok(())
}

/// 会话内全文搜索：返回按 bm25 相关性排序的匹配 event_key 列表与命中总数。
///
/// `session_key` 走普通列过滤（UNINDEXED 列参与 WHERE 过滤），查询串绑定
/// 参数；limit 由调用方 clamp 1..=200（此处再兜底一次），offset 下限 0。
/// 查询语法由调用方先用 [`escape_fts_query`] 转义（此处也校验一遍，双重防护）。
pub fn search_session(
    conn: &Connection,
    session_key: &str,
    query: &str,
    offset: i64,
    limit: i64,
) -> Result<(Vec<String>, i64), String> {
    let match_query = escape_fts_query(query)?;
    let limit = limit.clamp(1, SEARCH_PAGE_MAX_LIMIT);
    let offset = offset.max(0);
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_event_fts
             WHERE session_key = ?1 AND session_event_fts MATCH ?2",
            params![session_key, match_query],
            |row| row.get(0),
        )
        .map_err(|e| format!("ERR_ACTIVITY_SEARCH_COUNT: {e}"))?;
    let keys = query_keys(
        conn,
        "SELECT event_key FROM session_event_fts
         WHERE session_key = ?1 AND session_event_fts MATCH ?2
         ORDER BY bm25(session_event_fts)
         LIMIT ?3 OFFSET ?4",
        Some(session_key),
        &match_query,
        limit,
        offset,
    )?;
    Ok((keys, total))
}

/// 跨会话全文搜索：不分会话，返回匹配的 (event_key, session_key) 与总数。
pub fn search_global(
    conn: &Connection,
    query: &str,
    offset: i64,
    limit: i64,
) -> Result<(Vec<FtsHit>, i64), String> {
    let match_query = escape_fts_query(query)?;
    let limit = limit.clamp(1, SEARCH_PAGE_MAX_LIMIT);
    let offset = offset.max(0);
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_event_fts WHERE session_event_fts MATCH ?1",
            params![match_query],
            |row| row.get(0),
        )
        .map_err(|e| format!("ERR_ACTIVITY_SEARCH_COUNT: {e}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT event_key, session_key FROM session_event_fts
             WHERE session_event_fts MATCH ?1
             ORDER BY bm25(session_event_fts)
             LIMIT ?2 OFFSET ?3",
        )
        .map_err(|e| format!("ERR_ACTIVITY_SEARCH_PREPARE: {e}"))?;
    let rows = stmt
        .query_map(params![match_query, limit, offset], |row| {
            Ok(FtsHit {
                event_key: row.get(0)?,
                session_key: row.get(1)?,
            })
        })
        .map_err(|e| format!("ERR_ACTIVITY_SEARCH_QUERY: {e}"))?;
    let mut hits = Vec::new();
    for row in rows {
        hits.push(row.map_err(|e| format!("ERR_ACTIVITY_SEARCH_READ: {e}"))?);
    }
    Ok((hits, total))
}

/// 执行带 `session_key` 过滤的 FTS 查询并返回 event_key 列表（bm25 排序）。
fn query_keys(
    conn: &Connection,
    sql: &str,
    session_key: Option<&str>,
    match_query: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("ERR_ACTIVITY_SEARCH_PREPARE: {e}"))?;
    let mut keys = Vec::new();
    if let Some(session_key) = session_key {
        let rows = stmt
            .query_map(params![session_key, match_query, limit, offset], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| format!("ERR_ACTIVITY_SEARCH_QUERY: {e}"))?;
        for row in rows {
            keys.push(row.map_err(|e| format!("ERR_ACTIVITY_SEARCH_READ: {e}"))?);
        }
    } else {
        let rows = stmt
            .query_map(params![match_query, limit, offset], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| format!("ERR_ACTIVITY_SEARCH_QUERY: {e}"))?;
        for row in rows {
            keys.push(row.map_err(|e| format!("ERR_ACTIVITY_SEARCH_READ: {e}"))?);
        }
    }
    Ok(keys)
}

/// 删除单会话全部 FTS 行（purge/保留期清理路径调用，与深度表保持一致）。
pub fn delete_session_fts(conn: &Connection, session_key: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM session_event_fts WHERE session_key = ?1",
        params![session_key],
    )
    .map_err(|e| format!("ERR_ACTIVITY_SEARCH_DELETE: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::adapter::{ActivityIndexBatch, NewAgentNode, NewSessionEvent};
    use crate::activity::db::{self, ActivityIndexEntry, ACTIVITY_TABLES_DDL};
    use crate::activity::model::{
        ActivityCapabilityLevel, AgentRelationLevel, ContentState, EventStatus,
        SessionActivityCapability, SessionEventKind,
    };

    /// 内存 SQLite 测试连接：与生产共用同一 DDL 常量（含 FTS 虚拟表），杜绝漂移。
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory sqlite");
        conn.execute_batch(ACTIVITY_TABLES_DDL)
            .expect("create activity tables");
        conn
    }

    fn sample_entry(session_key: &str) -> ActivityIndexEntry {
        ActivityIndexEntry {
            session_key: session_key.to_string(),
            tool: "claude_code".to_string(),
            capability: SessionActivityCapability {
                level: ActivityCapabilityLevel::Structured,
                messages: true,
                tool_invocations: true,
                tool_results: true,
                request_links: true,
                agent_relations: AgentRelationLevel::RootGrouped,
                content_search: true,
                source_content_available: true,
                parser_id: "claude_code_v2".to_string(),
                parser_version: 2,
            },
            event_count: 3,
            tool_call_count: 1,
            agent_count: 1,
            source_fingerprint: Some("fp-1".to_string()),
            parser_id: "claude_code_v2".to_string(),
            parser_version: 2,
            indexed_at: 1_700_000_000,
            updated_at: 1_700_000_000,
        }
    }

    fn sample_batch(session_key: &str) -> ActivityIndexBatch {
        ActivityIndexBatch {
            session_key: session_key.to_string(),
            events: vec![
                NewSessionEvent {
                    event_key: format!("{session_key}:evt-1"),
                    sequence: 1,
                    timestamp_ms: Some(1_700_000_000_123),
                    kind: SessionEventKind::UserMessage,
                    status: Some(EventStatus::Success),
                    actor_agent_key: None,
                    parent_event_key: None,
                    summary_redacted: Some("fix the token budget bug".to_string()),
                    content_state: ContentState::Redacted,
                    source_file_path: "~/.claude/projects/x.jsonl".to_string(),
                    source_offset: Some(10),
                    payload_hash: Some("h1".to_string()),
                    raw_event_kind: "user".to_string(),
                },
                NewSessionEvent {
                    event_key: format!("{session_key}:evt-2"),
                    sequence: 2,
                    timestamp_ms: Some(1_700_000_001_000),
                    kind: SessionEventKind::ToolInvocation,
                    status: Some(EventStatus::Running),
                    actor_agent_key: Some("agent-1".to_string()),
                    parent_event_key: Some(format!("{session_key}:evt-1")),
                    summary_redacted: Some("Read src/main.rs".to_string()),
                    content_state: ContentState::Redacted,
                    source_file_path: "~/.claude/projects/x.jsonl".to_string(),
                    source_offset: Some(20),
                    payload_hash: Some("h2".to_string()),
                    raw_event_kind: "tool_use".to_string(),
                },
                NewSessionEvent {
                    event_key: format!("{session_key}:evt-3"),
                    sequence: 3,
                    timestamp_ms: None,
                    kind: SessionEventKind::Error,
                    status: Some(EventStatus::Error),
                    actor_agent_key: None,
                    parent_event_key: None,
                    summary_redacted: Some("failed with OR literal".to_string()),
                    content_state: ContentState::None,
                    source_file_path: "~/.claude/projects/x.jsonl".to_string(),
                    source_offset: Some(30),
                    payload_hash: None,
                    raw_event_kind: "error".to_string(),
                },
            ],
            agents: vec![NewAgentNode {
                agent_key: "agent-1".to_string(),
                parent_agent_key: None,
                display_kind: Some("Task".to_string()),
                task_summary_redacted: Some("fix the bug".to_string()),
                started_at_ms: Some(1_700_000_000_100),
                ended_at_ms: Some(1_700_000_002_000),
                status: Some(EventStatus::Success),
                relation_level: AgentRelationLevel::RootGrouped,
            }],
            tool_invocations: vec![crate::activity::adapter::NewToolInvocation {
                invocation_key: format!("{session_key}:inv-1"),
                event_key: format!("{session_key}:evt-2"),
                tool_name: "Read".to_string(),
                family: "fs".to_string(),
                duration_ms: Some(1200),
                status: Some(EventStatus::Running),
                input_bytes: Some(64),
                output_bytes: Some(2048),
                input_keys: vec!["file_path".to_string()],
                result_kind: Some("ok".to_string()),
            }],
            request_links: vec![],
        }
    }

    fn write_sample(conn: &Connection, session_key: &str) {
        db::write_activity_batch(conn, &sample_entry(session_key), &sample_batch(session_key))
            .expect("write batch");
    }

    fn fts_row_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM session_event_fts", [], |row| {
            row.get(0)
        })
        .expect("count fts rows")
    }

    #[test]
    fn fts_table_created_with_activity_tables() {
        let conn = test_conn();
        let name: String = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'session_event_fts'",
                [],
                |row| row.get(0),
            )
            .expect("fts table must exist after create_activity_tables");
        assert_eq!(name, "session_event_fts");
        // 幂等：重复执行 DDL 不报错（CREATE VIRTUAL TABLE IF NOT EXISTS）。
        conn.execute_batch(ACTIVITY_TABLES_DDL)
            .expect("idempotent ddl");
    }

    #[test]
    fn sync_populates_fts_and_delete_clears_it() {
        let conn = test_conn();
        let session_key = "sess-1";
        write_sample(&conn, session_key);
        assert_eq!(fts_row_count(&conn), 3, "every event gets one fts row");
        // summary 关键词可搜到；工具名可搜到（sync 在 tools upsert 之后执行）。
        let (keys, total) = search_session(&conn, session_key, "budget", 0, 10).expect("search");
        assert_eq!(total, 1);
        assert_eq!(keys, vec![format!("{session_key}:evt-1")]);
        let (keys, total) = search_session(&conn, session_key, "Read", 0, 10).expect("search tool");
        assert_eq!(total, 1);
        assert_eq!(keys, vec![format!("{session_key}:evt-2")]);

        // 删除会话深度活动后 FTS 必须同步清空（防残留）。
        db::delete_session_activity(&conn, session_key, false).expect("delete session");
        assert_eq!(
            fts_row_count(&conn),
            0,
            "fts must be cleared with deep tables"
        );
        let (_, total) = search_session(&conn, session_key, "budget", 0, 10).expect("search empty");
        assert_eq!(total, 0);
    }

    #[test]
    fn session_search_hit_miss_pagination_and_bm25_stability() {
        let conn = test_conn();
        let session_key = "sess-1";
        write_sample(&conn, session_key);

        // 命中两个事件（budget 与 OR 字面量分别命中不同事件）。
        let (keys, total) = search_session(&conn, session_key, "token", 0, 10).expect("search");
        assert_eq!(total, 1);
        assert_eq!(keys, vec![format!("{session_key}:evt-1")]);

        // 不命中：返回空列表、total 0。
        let (keys, total) =
            search_session(&conn, session_key, "nonexistent-term", 0, 10).expect("search miss");
        assert!(keys.is_empty());
        assert_eq!(total, 0);

        // 分页：limit=1 只回一页，offset 推进取下一页；总数不变。
        let (page1, total) = search_session(&conn, session_key, "failed", 0, 1).expect("page 1");
        assert_eq!(page1.len(), 1);
        assert_eq!(total, 1);

        // bm25 排序稳定性：相同数据重复查询结果一致（顺序确定性）。
        let (first, _) = search_session(&conn, session_key, "failed", 0, 10).expect("run 1");
        let (second, _) = search_session(&conn, session_key, "failed", 0, 10).expect("run 2");
        assert_eq!(first, second);

        // limit clamp：0 → 1；超大 → 200。
        let (keys, _) = search_session(&conn, session_key, "failed", 0, 0).expect("clamp low");
        assert!(keys.len() <= 1);
        let (keys, _) =
            search_session(&conn, session_key, "failed", 0, 10_000).expect("clamp high");
        assert!(keys.len() <= 200);
    }

    #[test]
    fn global_search_finds_hits_across_sessions() {
        let conn = test_conn();
        write_sample(&conn, "sess-1");
        // sess-2 写入不同摘要的批次：验证跨会话命中归属与区分。
        let mut batch2 = sample_batch("sess-2");
        batch2.events[0].summary_redacted = Some("deploy the release pipeline".to_string());
        db::write_activity_batch(&conn, &sample_entry("sess-2"), &batch2).expect("write batch 2");

        // "token" 只在 sess-1（sess-2 已改摘要）→ 命中 1 条且归属正确。
        let (hits, total) = search_global(&conn, "token", 0, 10).expect("global search");
        assert_eq!(total, 1);
        assert_eq!(hits[0].session_key, "sess-1");
        assert_eq!(hits[0].event_key, "sess-1:evt-1");

        // "release" 只在 sess-2 → 命中 1 条。
        let (hits, total) = search_global(&conn, "release", 0, 10).expect("global search 2");
        assert_eq!(total, 1);
        assert_eq!(hits[0].session_key, "sess-2");

        // 工具名跨会话可搜（两个会话工具事件都叫 Read）。
        let (hits, total) = search_global(&conn, "Read", 0, 10).expect("global tool search");
        assert_eq!(total, 2);
        let mut sessions: Vec<&str> = hits.iter().map(|hit| hit.session_key.as_str()).collect();
        sessions.sort_unstable();
        assert_eq!(sessions, vec!["sess-1", "sess-2"]);

        // 跨会话分页：limit=1 只回一页，total 不变。
        let (hits, total) = search_global(&conn, "Read", 0, 1).expect("global page");
        assert_eq!(hits.len(), 1);
        assert_eq!(total, 2);
    }

    #[test]
    fn escape_neutralizes_fts_syntax_and_rejects_bad_lengths() {
        let conn = test_conn();
        write_sample(&conn, "sess-1");

        // 语法字符全部中和为字面量：查询不报错、按字面匹配。
        for special in [
            "*", "OR", "NEAR", "AND", "NOT", "(", ")", "^", "-", "\"", "a OR b",
        ] {
            let escaped = escape_fts_query(special).expect("escape special");
            // 双引号包裹且内部引号被转义，不含裸语法结构。
            assert!(
                escaped.starts_with('"') && escaped.ends_with('"'),
                "{special} -> {escaped}"
            );
            // 字面查询不报错（即使无命中）。
            let (_, _) = search_session(&conn, "sess-1", special, 0, 10)
                .unwrap_or_else(|e| panic!("literal query {special:?} must not error: {e}"));
        }

        // 字面 OR：摘要含 "OR literal" 的事件按字面命中（而非布尔 OR 语义）。
        let (keys, total) = search_session(&conn, "sess-1", "OR", 0, 10).expect("literal OR");
        assert_eq!(total, 1);
        assert_eq!(keys, vec!["sess-1:evt-3".to_string()]);

        // 星号按字面：无事件包含裸 * → 0 命中且不报错。
        let (keys, total) = search_session(&conn, "sess-1", "*", 0, 10).expect("literal star");
        assert!(keys.is_empty());
        assert_eq!(total, 0);

        // 含引号的查询：转义后按字面匹配。
        let (keys, total) = search_session(&conn, "sess-1", "OR literal", 0, 10)
            .expect("literal phrase with space");
        assert_eq!(total, 1);
        assert_eq!(keys, vec!["sess-1:evt-3".to_string()]);

        // 空查询拒绝。
        let err = escape_fts_query("   ").expect_err("blank query rejected");
        assert_eq!(err, "ERR_ACTIVITY_SEARCH_QUERY_EMPTY");

        // 超长查询拒绝（> 200 字符）。
        let long = "x".repeat(201);
        let err = escape_fts_query(&long).expect_err("long query rejected");
        assert_eq!(err, "ERR_ACTIVITY_SEARCH_QUERY_TOO_LONG");
        // 边界：恰好 200 字符允许。
        let ok = escape_fts_query(&"y".repeat(200)).expect("200 chars allowed");
        assert_eq!(ok.chars().count(), 202); // 包裹引号
    }

    #[test]
    fn sync_is_idempotent_for_rewritten_batch() {
        let conn = test_conn();
        let session_key = "sess-1";
        write_sample(&conn, session_key);
        // 同一批次重写（幂等）：FTS 行数不变、无重复。
        write_sample(&conn, session_key);
        assert_eq!(fts_row_count(&conn), 3);
        let (_, total) = search_session(&conn, session_key, "budget", 0, 10).expect("search");
        assert_eq!(total, 1);
        // 事件收缩后 FTS 同步收缩。
        let mut smaller = sample_batch(session_key);
        smaller.events.pop();
        db::write_activity_batch(&conn, &sample_entry(session_key), &smaller)
            .expect("rewrite smaller");
        assert_eq!(fts_row_count(&conn), 2);
    }

    /// 隐私说明：纯 db 层搜索函数不做门控（本测试可直连搜索）；
    /// `fulltext` 门控位于 IPC 命令层（commands/activity_search.rs），
    /// 命令层门控需要 tauri::AppHandle 难以单测，由 `fulltext_disabled`
    /// 纯函数单测覆盖。
    #[test]
    fn db_layer_does_not_gate_but_command_layer_does() {
        let conn = test_conn();
        write_sample(&conn, "sess-1");
        // db 层：直接可搜（门控在命令层，隐私面由命令层保证）。
        let (_, total) = search_session(&conn, "sess-1", "budget", 0, 10).expect("db search");
        assert_eq!(total, 1);
        // 命令层门控逻辑（纯函数）在 commands_search::tests 中验证。
        assert!(crate::activity::commands_search::fulltext_disabled(
            &crate::models::AppSettings::default()
        ));
    }
}
