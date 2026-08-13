//! 深度活动索引的 SQLite 读写辅助（`local_usage.db`，设计文档 12.4）。
//!
//! 表结构 DDL 集中在本文件（[`ACTIVITY_TABLES_DDL`]），`schema.rs` 的
//! `create_activity_tables` 与 v28 迁移共用同一常量，避免建表 SQL 漂移。
//!
//! 幂等策略：适配器每次产出“会话级全量快照”，事件写入按 session_key
//! 事务内先 DELETE 旧行再批量 INSERT（重复索引不产生重复事件、单会话原子）；
//! 代理/工具调用/请求关联使用 UPSERT（`ON CONFLICT DO UPDATE`）。

use crate::activity::adapter::{
    ActivityIndexBatch, NewAgentNode, NewEventRequestLink, NewSessionEvent, NewToolInvocation,
};
use crate::activity::model::{
    AgentNodeDto, AgentRelationLevel, ContentState, EventStatus, EventsPage, RequestEventLink,
    RequestLinkStrength, SafeSourceRef, SessionActivityCapability, SessionActivitySummary,
    SessionEventFilter, SessionEventKind, SessionEventListItem, ToolInvocationSummary,
    ToolSummaryRow,
};
use crate::activity::redact_home_path;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use std::collections::HashMap;

/// 深度活动索引 5 张表的建表 SQL（schema.rs 与 v28 迁移共用，保持单一来源）。
pub const ACTIVITY_TABLES_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS session_activity_index (
    session_key TEXT PRIMARY KEY,
    tool TEXT NOT NULL,
    capability_json TEXT NOT NULL,
    event_count INTEGER NOT NULL DEFAULT 0,
    tool_call_count INTEGER NOT NULL DEFAULT 0,
    agent_count INTEGER NOT NULL DEFAULT 0,
    source_fingerprint TEXT,
    parser_id TEXT NOT NULL DEFAULT '',
    parser_version INTEGER NOT NULL DEFAULT 0,
    indexed_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_session_activity_index_tool
    ON session_activity_index(tool);

CREATE TABLE IF NOT EXISTS session_events (
    event_key TEXT PRIMARY KEY,
    session_key TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    timestamp_ms INTEGER,
    kind TEXT NOT NULL,
    status TEXT,
    actor_agent_key TEXT,
    parent_event_key TEXT,
    summary_redacted TEXT,
    content_state TEXT NOT NULL DEFAULT 'none',
    source_file_id INTEGER,
    source_file_path TEXT NOT NULL,
    source_offset INTEGER,
    payload_hash TEXT,
    raw_event_kind TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_session_events_session_sequence
    ON session_events(session_key, sequence);
CREATE INDEX IF NOT EXISTS idx_session_events_session_timestamp
    ON session_events(session_key, timestamp_ms);
CREATE INDEX IF NOT EXISTS idx_session_events_actor_agent
    ON session_events(actor_agent_key);
CREATE INDEX IF NOT EXISTS idx_session_events_kind
    ON session_events(kind);

CREATE TABLE IF NOT EXISTS session_tool_invocations (
    invocation_key TEXT PRIMARY KEY,
    event_key TEXT NOT NULL,
    session_key TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    family TEXT NOT NULL DEFAULT '',
    status TEXT,
    duration_ms INTEGER,
    input_bytes INTEGER,
    output_bytes INTEGER,
    input_keys_json TEXT NOT NULL DEFAULT '[]',
    result_kind TEXT
);
CREATE INDEX IF NOT EXISTS idx_session_tool_invocations_session
    ON session_tool_invocations(session_key);
CREATE INDEX IF NOT EXISTS idx_session_tool_invocations_event
    ON session_tool_invocations(event_key);

CREATE TABLE IF NOT EXISTS session_agents (
    agent_key TEXT PRIMARY KEY,
    session_key TEXT NOT NULL,
    parent_agent_key TEXT,
    display_kind TEXT,
    task_summary_redacted TEXT,
    started_at_ms INTEGER,
    ended_at_ms INTEGER,
    status TEXT,
    relation_level TEXT NOT NULL DEFAULT 'none',
    request_count INTEGER,
    total_tokens INTEGER,
    estimated_cost REAL,
    child_count INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_session_agents_session
    ON session_agents(session_key);
CREATE INDEX IF NOT EXISTS idx_session_agents_parent
    ON session_agents(parent_agent_key);

CREATE TABLE IF NOT EXISTS session_event_request_links (
    event_key TEXT NOT NULL,
    request_key TEXT NOT NULL,
    strength TEXT NOT NULL,
    matcher_id TEXT,
    PRIMARY KEY(event_key, request_key)
);
"#;

/// `session_activity_index` 行（写入口）。
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityIndexEntry {
    pub session_key: String,
    pub tool: String,
    pub capability: SessionActivityCapability,
    pub event_count: i64,
    pub tool_call_count: i64,
    pub agent_count: i64,
    pub source_fingerprint: Option<String>,
    pub parser_id: String,
    pub parser_version: i64,
    pub indexed_at: i64,
    pub updated_at: i64,
}

/// 摘要截断上限（编码规范：summary_redacted ≤ 500 字符 + 脱敏）。
const SUMMARY_MAX_CHARS: usize = 500;

fn truncate_summary(summary: &str) -> String {
    if summary.chars().count() <= SUMMARY_MAX_CHARS {
        return summary.to_string();
    }
    let mut truncated: String = summary.chars().take(SUMMARY_MAX_CHARS).collect();
    truncated.push('…');
    truncated
}

fn status_str(status: Option<EventStatus>) -> Option<&'static str> {
    status.map(|s| s.as_str())
}

// ---------------------------------------------------------------------------
// 写
// ---------------------------------------------------------------------------

/// UPSERT 单会话活动索引行。
pub fn upsert_session_activity_index(
    conn: &Connection,
    entry: &ActivityIndexEntry,
) -> Result<(), String> {
    let capability_json = serde_json::to_string(&entry.capability)
        .map_err(|e| format!("ERR_ACTIVITY_SERIALIZE_CAPABILITY: {e}"))?;
    conn.execute(
        "INSERT INTO session_activity_index (
            session_key, tool, capability_json, event_count, tool_call_count, agent_count,
            source_fingerprint, parser_id, parser_version, indexed_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(session_key) DO UPDATE SET
            tool = excluded.tool,
            capability_json = excluded.capability_json,
            event_count = excluded.event_count,
            tool_call_count = excluded.tool_call_count,
            agent_count = excluded.agent_count,
            source_fingerprint = excluded.source_fingerprint,
            parser_id = excluded.parser_id,
            parser_version = excluded.parser_version,
            updated_at = excluded.updated_at",
        params![
            entry.session_key,
            entry.tool,
            capability_json,
            entry.event_count,
            entry.tool_call_count,
            entry.agent_count,
            entry.source_fingerprint,
            entry.parser_id,
            entry.parser_version,
            entry.indexed_at,
            entry.updated_at,
        ],
    )
    .map_err(|e| format!("ERR_ACTIVITY_UPSERT_INDEX: {e}"))?;
    Ok(())
}

/// 替换单会话全部事件：事务内先删除该 session_key 旧事件，再批量插入新批次。
///
/// 幂等：适配器每次产出全量快照，重复索引不产生重复事件；单会话写入原子。
pub fn replace_session_events(
    conn: &Connection,
    session_key: &str,
    events: &[NewSessionEvent],
) -> Result<(), String> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("ERR_ACTIVITY_TX_START: {e}"))?;
    tx.execute(
        "DELETE FROM session_events WHERE session_key = ?1",
        params![session_key],
    )
    .map_err(|e| format!("ERR_ACTIVITY_DELETE_EVENTS: {e}"))?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO session_events (
                    event_key, session_key, sequence, timestamp_ms, kind, status,
                    actor_agent_key, parent_event_key, summary_redacted, content_state,
                    source_file_id, source_file_path, source_offset, payload_hash, raw_event_kind
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            )
            .map_err(|e| format!("ERR_ACTIVITY_PREPARE_INSERT_EVENT: {e}"))?;
        for event in events {
            let summary = event.summary_redacted.as_deref().map(truncate_summary);
            stmt.execute(params![
                event.event_key,
                session_key,
                event.sequence,
                event.timestamp_ms,
                event.kind.as_str(),
                status_str(event.status),
                event.actor_agent_key,
                event.parent_event_key,
                summary,
                event.content_state.as_str(),
                // source_file_id：M2 按 source_file_path 定位，后续解析层回填
                None::<i64>,
                event.source_file_path,
                event.source_offset,
                event.payload_hash,
                event.raw_event_kind,
            ])
            .map_err(|e| format!("ERR_ACTIVITY_INSERT_EVENT: {e}"))?;
        }
    }
    tx.commit()
        .map_err(|e| format!("ERR_ACTIVITY_TX_COMMIT: {e}"))?;
    Ok(())
}

/// UPSERT 单会话代理节点（agent_key 冲突时更新；孤儿代理由上层重建时
/// 通过全量 replace 处理——M2 骨架按任务约定使用 UPSERT）。
pub fn upsert_agents(
    conn: &Connection,
    session_key: &str,
    agents: &[NewAgentNode],
) -> Result<(), String> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("ERR_ACTIVITY_TX_START: {e}"))?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO session_agents (
                    agent_key, session_key, parent_agent_key, display_kind,
                    task_summary_redacted, started_at_ms, ended_at_ms, status, relation_level
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(agent_key) DO UPDATE SET
                    session_key = excluded.session_key,
                    parent_agent_key = excluded.parent_agent_key,
                    display_kind = excluded.display_kind,
                    task_summary_redacted = excluded.task_summary_redacted,
                    started_at_ms = excluded.started_at_ms,
                    ended_at_ms = excluded.ended_at_ms,
                    status = excluded.status,
                    relation_level = excluded.relation_level",
            )
            .map_err(|e| format!("ERR_ACTIVITY_PREPARE_UPSERT_AGENT: {e}"))?;
        for agent in agents {
            let summary = agent.task_summary_redacted.as_deref().map(truncate_summary);
            stmt.execute(params![
                agent.agent_key,
                session_key,
                agent.parent_agent_key,
                agent.display_kind,
                summary,
                agent.started_at_ms,
                agent.ended_at_ms,
                status_str(agent.status),
                agent.relation_level.as_str(),
            ])
            .map_err(|e| format!("ERR_ACTIVITY_UPSERT_AGENT: {e}"))?;
        }
    }
    tx.commit()
        .map_err(|e| format!("ERR_ACTIVITY_TX_COMMIT: {e}"))?;
    Ok(())
}

/// UPSERT 单会话工具调用（invocation_key 冲突时更新）。
pub fn upsert_tool_invocations(
    conn: &Connection,
    session_key: &str,
    invocations: &[NewToolInvocation],
) -> Result<(), String> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("ERR_ACTIVITY_TX_START: {e}"))?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO session_tool_invocations (
                    invocation_key, event_key, session_key, tool_name, family, status,
                    duration_ms, input_bytes, output_bytes, input_keys_json, result_kind
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(invocation_key) DO UPDATE SET
                    event_key = excluded.event_key,
                    session_key = excluded.session_key,
                    tool_name = excluded.tool_name,
                    family = excluded.family,
                    status = excluded.status,
                    duration_ms = excluded.duration_ms,
                    input_bytes = excluded.input_bytes,
                    output_bytes = excluded.output_bytes,
                    input_keys_json = excluded.input_keys_json,
                    result_kind = excluded.result_kind",
            )
            .map_err(|e| format!("ERR_ACTIVITY_PREPARE_UPSERT_TOOL: {e}"))?;
        for invocation in invocations {
            let input_keys_json = serde_json::to_string(&invocation.input_keys)
                .map_err(|e| format!("ERR_ACTIVITY_SERIALIZE_INPUT_KEYS: {e}"))?;
            stmt.execute(params![
                invocation.invocation_key,
                invocation.event_key,
                session_key,
                invocation.tool_name,
                invocation.family,
                status_str(invocation.status),
                invocation.duration_ms,
                invocation.input_bytes,
                invocation.output_bytes,
                input_keys_json,
                invocation.result_kind,
            ])
            .map_err(|e| format!("ERR_ACTIVITY_UPSERT_TOOL: {e}"))?;
        }
    }
    tx.commit()
        .map_err(|e| format!("ERR_ACTIVITY_TX_COMMIT: {e}"))?;
    Ok(())
}

/// UPSERT 事件-请求关联（复合主键冲突时更新 strength/matcher_id）。
pub fn upsert_event_request_links(
    conn: &Connection,
    session_key: &str,
    links: &[NewEventRequestLink],
) -> Result<(), String> {
    // 先清掉该会话旧关联：全量快照语义下，若新批次不再包含某关联，
    // 旧行不应残留（本表无自有 session_key 列，按事件归属清理）。
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("ERR_ACTIVITY_TX_START: {e}"))?;
    tx.execute(
        "DELETE FROM session_event_request_links
         WHERE event_key IN (SELECT event_key FROM session_events WHERE session_key = ?1)",
        params![session_key],
    )
    .map_err(|e| format!("ERR_ACTIVITY_DELETE_LINKS: {e}"))?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO session_event_request_links (event_key, request_key, strength, matcher_id)
                 VALUES (?1, ?2, ?3, NULL)
                 ON CONFLICT(event_key, request_key) DO UPDATE SET
                    strength = excluded.strength",
            )
            .map_err(|e| format!("ERR_ACTIVITY_PREPARE_UPSERT_LINK: {e}"))?;
        for link in links {
            stmt.execute(params![
                link.event_key,
                link.request_key,
                link.strength.as_str(),
            ])
            .map_err(|e| format!("ERR_ACTIVITY_UPSERT_LINK: {e}"))?;
        }
    }
    tx.commit()
        .map_err(|e| format!("ERR_ACTIVITY_TX_COMMIT: {e}"))?;
    Ok(())
}

/// 写入一个完整索引批次（index 行 + 事件 + 代理 + 工具调用 + 关联）。
/// 事件替换、代理/工具 UPSERT、关联清理各自独立事务（骨架阶段保持简单；
/// 单会话原子性由 replace_session_events 保证）。
pub fn write_activity_batch(
    conn: &Connection,
    entry: &ActivityIndexEntry,
    batch: &ActivityIndexBatch,
) -> Result<(), String> {
    // 全量替换语义：先清该会话旧的 agents/tools 派生行，避免源文件删除后
    // 残留幽灵代理/工具行；links 由 upsert_event_request_links 内部清理，
    // events 由 replace_session_events 全量替换。各步独立提交：中途失败时
    // 库可能处于部分替换的中间态，但索引任务可重试收敛（幂等）。
    clear_session_derived_rows(conn, &batch.session_key)?;
    upsert_session_activity_index(conn, entry)?;
    replace_session_events(conn, &batch.session_key, &batch.events)?;
    upsert_agents(conn, &batch.session_key, &batch.agents)?;
    upsert_tool_invocations(conn, &batch.session_key, &batch.tool_invocations)?;
    upsert_event_request_links(conn, &batch.session_key, &batch.request_links)?;
    Ok(())
}

/// 删除单会话的 agents/tools 派生行（index/events/links 不受影响）。
fn clear_session_derived_rows(conn: &Connection, session_key: &str) -> Result<(), String> {
    for table in ["session_tool_invocations", "session_agents"] {
        conn.execute(
            &format!("DELETE FROM {table} WHERE session_key = ?1"),
            params![session_key],
        )
        .map_err(|e| format!("ERR_ACTIVITY_CLEAR_ROWS: {e}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 查
// ---------------------------------------------------------------------------

struct EventQuery {
    where_sql: String,
    params: Vec<String>,
}

fn build_event_where(filter: Option<&SessionEventFilter>) -> EventQuery {
    let mut conditions = vec!["e.session_key = ?1".to_string()];
    let mut params: Vec<String> = vec![];
    if let Some(filter) = filter {
        if let Some(kinds) = &filter.kinds {
            if !kinds.is_empty() {
                let placeholders = kinds.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
                conditions.push(format!("e.kind IN ({placeholders})"));
                params.extend(kinds.iter().map(|k| k.as_str().to_string()));
            }
        }
        if let Some(statuses) = &filter.statuses {
            if !statuses.is_empty() {
                let placeholders = statuses.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
                conditions.push(format!("e.status IN ({placeholders})"));
                params.extend(statuses.iter().map(|s| s.as_str().to_string()));
            }
        }
        if let Some(agents) = &filter.agents {
            if !agents.is_empty() {
                let placeholders = agents.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
                conditions.push(format!("e.actor_agent_key IN ({placeholders})"));
                params.extend(agents.iter().cloned());
            }
        }
        if let Some(tool_names) = &filter.tool_names {
            if !tool_names.is_empty() {
                let placeholders = tool_names
                    .iter()
                    .map(|_| "?")
                    .collect::<Vec<_>>()
                    .join(", ");
                conditions.push(format!(
                    "EXISTS (SELECT 1 FROM session_tool_invocations ti
                             WHERE ti.event_key = e.event_key AND ti.tool_name IN ({placeholders}))"
                ));
                params.extend(tool_names.iter().cloned());
            }
        }
        if let Some(search) = &filter.search {
            let trimmed = search.trim();
            if !trimmed.is_empty() {
                conditions.push("e.summary_redacted LIKE ?".to_string());
                params.push(format!("%{trimmed}%"));
            }
        }
        if let Some(min_sequence) = filter.min_sequence {
            conditions.push("e.sequence >= ?".to_string());
            params.push(min_sequence.to_string());
        }
        if let Some(max_sequence) = filter.max_sequence {
            conditions.push("e.sequence <= ?".to_string());
            params.push(max_sequence.to_string());
        }
    }
    EventQuery {
        where_sql: conditions.join(" AND "),
        params,
    }
}

fn event_query_values(
    query: &EventQuery,
    session_key: &str,
    tail: &[rusqlite::types::Value],
) -> Vec<rusqlite::types::Value> {
    let mut values: Vec<rusqlite::types::Value> =
        Vec::with_capacity(1 + query.params.len() + tail.len());
    values.push(rusqlite::types::Value::Text(session_key.to_string()));
    values.extend(
        query
            .params
            .iter()
            .cloned()
            .map(rusqlite::types::Value::Text),
    );
    values.extend(tail.iter().cloned());
    values
}

fn parse_input_keys(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

/// 分页查询会话事件（按 sequence 升序；limit 由调用方 clamp 1..=200）。
pub fn query_events(
    conn: &Connection,
    session_key: &str,
    filter: Option<&SessionEventFilter>,
    offset: i64,
    limit: i64,
) -> Result<EventsPage, String> {
    let query = build_event_where(filter);

    let total: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM session_events e WHERE {}",
                query.where_sql
            ),
            params_from_iter(event_query_values(&query, session_key, &[]).iter()),
            |row| row.get(0),
        )
        .map_err(|e| format!("ERR_ACTIVITY_COUNT_EVENTS: {e}"))?;

    let sql = format!(
        "SELECT e.event_key, e.session_key, e.sequence, e.timestamp_ms, e.kind, e.status,
                e.actor_agent_key, e.parent_event_key, e.summary_redacted, e.content_state,
                e.source_file_id, e.source_file_path, e.source_offset, e.payload_hash,
                ti.invocation_key, ti.tool_name, ti.family, ti.duration_ms, ti.input_bytes,
                ti.output_bytes, ti.input_keys_json, ti.result_kind
         FROM session_events e
         LEFT JOIN session_tool_invocations ti ON ti.event_key = e.event_key
         WHERE {}
         ORDER BY e.sequence ASC
         LIMIT ? OFFSET ?",
        query.where_sql
    );
    let mut values = event_query_values(&query, session_key, &[]);
    values.push(rusqlite::types::Value::Integer(limit));
    values.push(rusqlite::types::Value::Integer(offset));

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("ERR_ACTIVITY_PREPARE_QUERY_EVENTS: {e}"))?;
    let rows = stmt
        .query_map(params_from_iter(values.iter()), |row| {
            let tool_invocation_key: Option<String> = row.get(14)?;
            let tool = match &tool_invocation_key {
                Some(invocation_key) => {
                    let raw_name: String = row.get(15)?;
                    let family: String = row.get(16)?;
                    let input_keys: String = row.get(20)?;
                    Some(ToolInvocationSummary {
                        invocation_key: invocation_key.clone(),
                        // M2 schema 单列 tool_name：raw 与 normalized 暂同值，
                        // 规范化映射由后续解析层（M3）细化。
                        raw_name: raw_name.clone(),
                        normalized_name: raw_name,
                        family,
                        duration_ms: row.get(17)?,
                        input_bytes: row.get(18)?,
                        output_bytes: row.get(19)?,
                        input_keys: parse_input_keys(&input_keys),
                        result_kind: row.get(21)?,
                    })
                }
                None => None,
            };
            Ok(SessionEventListItem {
                event_key: row.get(0)?,
                session_key: row.get(1)?,
                sequence: row.get(2)?,
                timestamp_ms: row.get(3)?,
                kind: SessionEventKind::parse_db(&row.get::<_, String>(4)?),
                status: row
                    .get::<_, Option<String>>(5)?
                    .as_deref()
                    .map(EventStatus::parse_db),
                actor_agent_key: row.get(6)?,
                parent_event_key: row.get(7)?,
                summary: row.get(8)?,
                content_state: ContentState::parse_db(&row.get::<_, String>(9)?),
                tool,
                request_links: Vec::new(),
                source_ref: SafeSourceRef {
                    source_file_id: row.get(10)?,
                    // 查询出口脱敏用户名（P2 路径脱敏）；on-demand 读取用 DB 原值，不受影响。
                    source_file_path: redact_home_path(&row.get::<_, String>(11)?),
                    source_offset: row.get(12)?,
                    fingerprint: row.get(13)?,
                },
            })
        })
        .map_err(|e| format!("ERR_ACTIVITY_QUERY_EVENTS: {e}"))?;

    let mut items: Vec<SessionEventListItem> = Vec::new();
    for row in rows {
        items.push(row.map_err(|e| format!("ERR_ACTIVITY_READ_EVENT: {e}"))?);
    }

    // 批量加载本页 request links（一次 IN 查询，避免 N+1）。
    if !items.is_empty() {
        let keys: Vec<String> = items.iter().map(|item| item.event_key.clone()).collect();
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let link_sql = format!(
            "SELECT event_key, request_key, strength FROM session_event_request_links
             WHERE event_key IN ({placeholders})"
        );
        let mut stmt = conn
            .prepare(&link_sql)
            .map_err(|e| format!("ERR_ACTIVITY_PREPARE_QUERY_LINKS: {e}"))?;
        let link_rows = stmt
            .query_map(params_from_iter(keys.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    RequestEventLink {
                        request_key: row.get(1)?,
                        strength: RequestLinkStrength::parse_db(&row.get::<_, String>(2)?),
                    },
                ))
            })
            .map_err(|e| format!("ERR_ACTIVITY_QUERY_LINKS: {e}"))?;
        let mut by_event: HashMap<String, Vec<RequestEventLink>> = HashMap::new();
        for link_row in link_rows {
            let (event_key, link) = link_row.map_err(|e| format!("ERR_ACTIVITY_READ_LINK: {e}"))?;
            by_event.entry(event_key).or_default().push(link);
        }
        for item in &mut items {
            item.request_links = by_event.remove(&item.event_key).unwrap_or_default();
        }
    }

    let has_more = (offset as usize) + items.len() < total as usize;
    Ok(EventsPage {
        items,
        total,
        has_more,
    })
}

/// 查询会话代理树节点（按 started_at_ms 升序，child_count 由子查询计算）。
pub fn query_agents(conn: &Connection, session_key: &str) -> Result<Vec<AgentNodeDto>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT a.agent_key, a.session_key, a.parent_agent_key, a.relation_level,
                    a.display_kind, a.task_summary_redacted, a.started_at_ms, a.ended_at_ms,
                    a.status, a.request_count, a.total_tokens, a.estimated_cost,
                    (SELECT COUNT(*) FROM session_agents c
                      WHERE c.session_key = a.session_key AND c.parent_agent_key = a.agent_key)
             FROM session_agents a
             WHERE a.session_key = ?1
             ORDER BY COALESCE(a.started_at_ms, 0) ASC",
        )
        .map_err(|e| format!("ERR_ACTIVITY_PREPARE_QUERY_AGENTS: {e}"))?;
    let rows = stmt
        .query_map(params![session_key], |row| {
            Ok(AgentNodeDto {
                agent_key: row.get(0)?,
                session_key: row.get(1)?,
                parent_agent_key: row.get(2)?,
                relation_level: AgentRelationLevel::parse_db(&row.get::<_, String>(3)?),
                display_kind: row.get(4)?,
                task_summary: row.get(5)?,
                started_at_ms: row.get(6)?,
                ended_at_ms: row.get(7)?,
                status: row
                    .get::<_, Option<String>>(8)?
                    .as_deref()
                    .map_or(EventStatus::Unknown, EventStatus::parse_db),
                request_count: row.get(9)?,
                total_tokens: row.get(10)?,
                estimated_cost: row.get(11)?,
                child_count: row.get(12)?,
            })
        })
        .map_err(|e| format!("ERR_ACTIVITY_QUERY_AGENTS: {e}"))?;
    let mut agents = Vec::new();
    for row in rows {
        agents.push(row.map_err(|e| format!("ERR_ACTIVITY_READ_AGENT: {e}"))?);
    }
    Ok(agents)
}

/// 查询会话工具调用汇总（按调用数倒序）。
pub fn query_tool_summary(
    conn: &Connection,
    session_key: &str,
) -> Result<Vec<ToolSummaryRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT tool_name, family,
                    COUNT(*) AS invocation_count,
                    COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0) AS success_count,
                    COALESCE(SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END), 0) AS error_count,
                    COALESCE(SUM(duration_ms), 0) AS total_duration_ms,
                    COALESCE(AVG(duration_ms), 0) AS avg_duration_ms,
                    COALESCE(SUM(input_bytes), 0) AS input_bytes_total,
                    COALESCE(SUM(output_bytes), 0) AS output_bytes_total
             FROM session_tool_invocations
             WHERE session_key = ?1
             GROUP BY tool_name, family
             ORDER BY invocation_count DESC, tool_name ASC",
        )
        .map_err(|e| format!("ERR_ACTIVITY_PREPARE_QUERY_TOOL_SUMMARY: {e}"))?;
    let rows = stmt
        .query_map(params![session_key], |row| {
            let avg_duration_ms: f64 = row.get(6)?;
            Ok(ToolSummaryRow {
                tool_name: row.get(0)?,
                family: row.get(1)?,
                invocation_count: row.get(2)?,
                success_count: row.get(3)?,
                error_count: row.get(4)?,
                total_duration_ms: row.get(5)?,
                avg_duration_ms: avg_duration_ms.round() as i64,
                input_bytes_total: row.get(7)?,
                output_bytes_total: row.get(8)?,
            })
        })
        .map_err(|e| format!("ERR_ACTIVITY_QUERY_TOOL_SUMMARY: {e}"))?;
    let mut rows_out = Vec::new();
    for row in rows {
        rows_out.push(row.map_err(|e| format!("ERR_ACTIVITY_READ_TOOL_SUMMARY: {e}"))?);
    }
    Ok(rows_out)
}

/// session_activity_index 行的查询元组（tool, capability_json, event_count,
/// tool_call_count, agent_count, source_fingerprint, parser_id, parser_version）。
type ActivityIndexRow = (String, String, i64, i64, i64, Option<String>, String, i64);

/// 查询单会话活动汇总；无索引返回 None。
pub fn query_activity_summary(
    conn: &Connection,
    session_key: &str,
) -> Result<Option<SessionActivitySummary>, String> {
    let entry: Option<ActivityIndexRow> = conn
        .query_row(
            "SELECT tool, capability_json, event_count, tool_call_count, agent_count,
                    source_fingerprint, parser_id, parser_version
             FROM session_activity_index WHERE session_key = ?1",
            params![session_key],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .optional()
        .map_err(|e| format!("ERR_ACTIVITY_QUERY_INDEX: {e}"))?;
    let Some((_, capability_json, _, _, _, _, _, _)) = entry else {
        return Ok(None);
    };
    let capability: SessionActivityCapability = serde_json::from_str(&capability_json)
        .map_err(|e| format!("ERR_ACTIVITY_PARSE_CAPABILITY: {e}"))?;

    let mut event_counts: HashMap<String, i64> = HashMap::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT kind, COUNT(*) FROM session_events
                 WHERE session_key = ?1 GROUP BY kind",
            )
            .map_err(|e| format!("ERR_ACTIVITY_PREPARE_EVENT_COUNTS: {e}"))?;
        let rows = stmt
            .query_map(params![session_key], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| format!("ERR_ACTIVITY_QUERY_EVENT_COUNTS: {e}"))?;
        for row in rows {
            let (kind, count) = row.map_err(|e| format!("ERR_ACTIVITY_READ_EVENT_COUNTS: {e}"))?;
            event_counts.insert(kind, count);
        }
    }

    let mut agent_counts: HashMap<String, i64> = HashMap::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT COALESCE(status, 'unknown'), COUNT(*) FROM session_agents
                 WHERE session_key = ?1 GROUP BY status",
            )
            .map_err(|e| format!("ERR_ACTIVITY_PREPARE_AGENT_COUNTS: {e}"))?;
        let rows = stmt
            .query_map(params![session_key], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| format!("ERR_ACTIVITY_QUERY_AGENT_COUNTS: {e}"))?;
        for row in rows {
            let (status, count) =
                row.map_err(|e| format!("ERR_ACTIVITY_READ_AGENT_COUNTS: {e}"))?;
            agent_counts.insert(status, count);
        }
    }

    let mut tool_counts: Vec<(String, i64)> = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT tool_name, COUNT(*) FROM session_tool_invocations
                 WHERE session_key = ?1 GROUP BY tool_name ORDER BY 2 DESC",
            )
            .map_err(|e| format!("ERR_ACTIVITY_PREPARE_TOOL_COUNTS: {e}"))?;
        let rows = stmt
            .query_map(params![session_key], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| format!("ERR_ACTIVITY_QUERY_TOOL_COUNTS: {e}"))?;
        for row in rows {
            let (tool, count) = row.map_err(|e| format!("ERR_ACTIVITY_READ_TOOL_COUNTS: {e}"))?;
            tool_counts.push((tool, count));
        }
    }

    Ok(Some(SessionActivitySummary {
        session_key: session_key.to_string(),
        capability: capability.clone(),
        event_counts,
        agent_counts,
        tool_counts,
        // M2 覆盖度 = 能力级别；M3 按实际事件覆盖细化。
        coverage: capability.level.as_str().to_string(),
    }))
}

// ---------------------------------------------------------------------------
// 清理
// ---------------------------------------------------------------------------

/// 删除单会话全部深度活动数据；返回受影响行数（5 表之和）。
/// `purge_payload`：M2 payload 不落库，该参数为 M3 “同时删除源文件内容”预留语义，
/// 当前对深度表删除无差别。
pub fn delete_session_activity(
    conn: &Connection,
    session_key: &str,
    _purge_payload: bool,
) -> Result<usize, String> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("ERR_ACTIVITY_TX_START: {e}"))?;
    let mut removed = 0usize;
    // 先删 request links（依赖 events 子查询定位；须在 events 删除之前执行）。
    removed += tx
        .execute(
            "DELETE FROM session_event_request_links
             WHERE event_key IN (SELECT event_key FROM session_events
                                 WHERE session_key = ?1)",
            params![session_key],
        )
        .map_err(|e| format!("ERR_ACTIVITY_DELETE_LINKS: {e}"))?;
    removed += tx
        .execute(
            "DELETE FROM session_events WHERE session_key = ?1",
            params![session_key],
        )
        .map_err(|e| format!("ERR_ACTIVITY_DELETE_EVENTS: {e}"))?;
    removed += tx
        .execute(
            "DELETE FROM session_tool_invocations WHERE session_key = ?1",
            params![session_key],
        )
        .map_err(|e| format!("ERR_ACTIVITY_DELETE_TOOLS: {e}"))?;
    removed += tx
        .execute(
            "DELETE FROM session_agents WHERE session_key = ?1",
            params![session_key],
        )
        .map_err(|e| format!("ERR_ACTIVITY_DELETE_AGENTS: {e}"))?;
    removed += tx
        .execute(
            "DELETE FROM session_activity_index WHERE session_key = ?1",
            params![session_key],
        )
        .map_err(|e| format!("ERR_ACTIVITY_DELETE_INDEX: {e}"))?;
    tx.commit()
        .map_err(|e| format!("ERR_ACTIVITY_TX_COMMIT: {e}"))?;
    Ok(removed)
}

/// 删除超过保留期限的会话深度活动（按 indexed_at 判定）；返回受影响行数。
pub fn delete_activity_older_than_days(
    conn: &Connection,
    days: i64,
    purge_payload: bool,
) -> Result<usize, String> {
    let cutoff = chrono::Utc::now().timestamp() - days * 86_400;
    let session_keys: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT session_key FROM session_activity_index WHERE indexed_at < ?1")
            .map_err(|e| format!("ERR_ACTIVITY_PREPARE_OLD_SESSIONS: {e}"))?;
        let rows = stmt
            .query_map(params![cutoff], |row| row.get::<_, String>(0))
            .map_err(|e| format!("ERR_ACTIVITY_QUERY_OLD_SESSIONS: {e}"))?;
        let mut keys = Vec::new();
        for row in rows {
            keys.push(row.map_err(|e| format!("ERR_ACTIVITY_READ_OLD_SESSION: {e}"))?);
        }
        keys
    };
    let mut removed = 0usize;
    for session_key in session_keys {
        removed += delete_session_activity(conn, &session_key, purge_payload)?;
    }
    Ok(removed)
}

/// 删除全部深度活动数据；返回受影响行数。
pub fn delete_all_activity(conn: &Connection, purge_payload: bool) -> Result<usize, String> {
    let session_keys: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT session_key FROM session_activity_index")
            .map_err(|e| format!("ERR_ACTIVITY_PREPARE_ALL_SESSIONS: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("ERR_ACTIVITY_QUERY_ALL_SESSIONS: {e}"))?;
        let mut keys = Vec::new();
        for row in rows {
            keys.push(row.map_err(|e| format!("ERR_ACTIVITY_READ_ALL_SESSION: {e}"))?);
        }
        keys
    };
    let mut removed = 0usize;
    for session_key in session_keys {
        removed += delete_session_activity(conn, &session_key, purge_payload)?;
    }
    Ok(removed)
}

/// 统计已建立深度活动索引的会话数。
pub fn count_activity_sessions(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT COUNT(*) FROM session_activity_index", [], |row| {
        row.get(0)
    })
    .map_err(|e| format!("ERR_ACTIVITY_COUNT_SESSIONS: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::adapter::{ActivityIndexBatch, NewAgentNode, NewSessionEvent};

    /// 内存 SQLite 测试连接：建表 SQL 与生产共用同一 DDL 常量，杜绝漂移。
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
                level: crate::activity::model::ActivityCapabilityLevel::Structured,
                messages: true,
                tool_invocations: true,
                tool_results: true,
                request_links: true,
                agent_relations: AgentRelationLevel::RootGrouped,
                content_search: false,
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
                    summary_redacted: Some("fix the bug".to_string()),
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
                    summary_redacted: Some("failed".to_string()),
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
            tool_invocations: vec![NewToolInvocation {
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
            request_links: vec![NewEventRequestLink {
                event_key: format!("{session_key}:evt-1"),
                request_key: "req-1".to_string(),
                strength: RequestLinkStrength::Exact,
            }],
        }
    }

    fn count_events(conn: &Connection, session_key: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM session_events WHERE session_key = ?1",
            params![session_key],
            |row| row.get(0),
        )
        .expect("count events")
    }

    #[test]
    fn replace_events_is_idempotent() {
        let conn = test_conn();
        let session_key = "sess-1";
        let entry = sample_entry(session_key);
        let batch = sample_batch(session_key);
        write_activity_batch(&conn, &entry, &batch).expect("write batch");
        assert_eq!(count_events(&conn, session_key), 3);

        // 同一批次再写一次：事件数不变（幂等，不产生重复事件）。
        write_activity_batch(&conn, &entry, &batch).expect("rewrite batch");
        assert_eq!(count_events(&conn, session_key), 3);

        // 批次事件减少时旧事件被替换。
        let mut smaller = batch.clone();
        smaller.events.pop();
        write_activity_batch(&conn, &entry, &smaller).expect("rewrite smaller batch");
        assert_eq!(count_events(&conn, session_key), 2);
    }

    #[test]
    fn upsert_agents_and_tools_are_idempotent() {
        let conn = test_conn();
        let session_key = "sess-1";
        let entry = sample_entry(session_key);
        let batch = sample_batch(session_key);
        write_activity_batch(&conn, &entry, &batch).expect("write batch");
        write_activity_batch(&conn, &entry, &batch).expect("rewrite batch");

        let agent_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_agents WHERE session_key = ?1",
                params![session_key],
                |row| row.get(0),
            )
            .expect("count agents");
        assert_eq!(agent_count, 1);

        let tool_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_tool_invocations WHERE session_key = ?1",
                params![session_key],
                |row| row.get(0),
            )
            .expect("count tools");
        assert_eq!(tool_count, 1);

        let link_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_event_request_links",
                [],
                |row| row.get(0),
            )
            .expect("count links");
        assert_eq!(link_count, 1);
    }

    #[test]
    fn shrinking_batch_clears_ghost_agents_and_tools() {
        let conn = test_conn();
        let session_key = "sess-1";
        let entry = sample_entry(session_key);
        let mut big = sample_batch(session_key);
        // 构造含 2 个代理与 2 个工具调用的批次（工具事件额外 append 一个）。
        let mut second_agent = big.agents[0].clone();
        second_agent.agent_key = "agent-2".to_string();
        big.agents.push(second_agent);
        let mut second_inv = big.tool_invocations[0].clone();
        second_inv.invocation_key = "inv-2-1".to_string();
        second_inv.event_key = "evt-2".to_string();
        big.tool_invocations.push(second_inv);
        write_activity_batch(&conn, &entry, &big).expect("write big batch");
        assert_eq!(
            count_rows(&conn, "session_agents", session_key),
            2,
            "big batch has 2 agents"
        );
        assert_eq!(
            count_rows(&conn, "session_tool_invocations", session_key),
            2,
            "big batch has 2 tools"
        );

        // 缩小的批次（1 代理 1 工具）重写后，旧派生行必须被清除。
        let small = sample_batch(session_key);
        write_activity_batch(&conn, &entry, &small).expect("write small batch");
        assert_eq!(
            count_rows(&conn, "session_agents", session_key),
            1,
            "ghost agent must be cleared"
        );
        assert_eq!(
            count_rows(&conn, "session_tool_invocations", session_key),
            1,
            "ghost tool must be cleared"
        );
    }

    fn count_rows(conn: &Connection, table: &str, session_key: &str) -> i64 {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE session_key = ?1"),
            params![session_key],
            |row| row.get(0),
        )
        .expect("count rows")
    }

    #[test]
    fn query_events_filters_and_paginates() {
        let conn = test_conn();
        let session_key = "sess-1";
        write_activity_batch(
            &conn,
            &sample_entry(session_key),
            &sample_batch(session_key),
        )
        .expect("write batch");

        // 无过滤：3 条，按 sequence 升序。
        let page = query_events(&conn, session_key, None, 0, 10).expect("query events");
        assert_eq!(page.total, 3);
        assert!(!page.has_more);
        assert_eq!(page.items.len(), 3);
        assert_eq!(page.items[0].sequence, 1);
        assert_eq!(page.items[0].kind, SessionEventKind::UserMessage);
        assert_eq!(page.items[0].request_links.len(), 1);
        assert_eq!(
            page.items[0].request_links[0].strength,
            RequestLinkStrength::Exact
        );
        // 工具事件带 tool 摘要
        let tool_item = &page.items[1];
        let tool = tool_item.tool.as_ref().expect("tool summary");
        assert_eq!(tool.raw_name, "Read");
        assert_eq!(tool.input_keys, vec!["file_path".to_string()]);
        assert_eq!(tool_item.actor_agent_key.as_deref(), Some("agent-1"));

        // kind 过滤
        let filter = SessionEventFilter {
            kinds: Some(vec![SessionEventKind::ToolInvocation]),
            ..Default::default()
        };
        let page = query_events(&conn, session_key, Some(&filter), 0, 10).expect("query kinds");
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].kind, SessionEventKind::ToolInvocation);

        // 状态过滤 + 搜索
        let filter = SessionEventFilter {
            statuses: Some(vec![EventStatus::Error]),
            search: Some("failed".to_string()),
            ..Default::default()
        };
        let page = query_events(&conn, session_key, Some(&filter), 0, 10).expect("query status");
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].kind, SessionEventKind::Error);

        // 分页
        let page = query_events(&conn, session_key, None, 0, 2).expect("query page");
        assert_eq!(page.items.len(), 2);
        assert!(page.has_more);
        let page2 = query_events(&conn, session_key, None, 2, 2).expect("query page 2");
        assert_eq!(page2.items.len(), 1);
        assert!(!page2.has_more);
    }

    #[test]
    fn query_summary_agents_and_tool_summary() {
        let conn = test_conn();
        let session_key = "sess-1";
        write_activity_batch(
            &conn,
            &sample_entry(session_key),
            &sample_batch(session_key),
        )
        .expect("write batch");

        let summary = query_activity_summary(&conn, session_key)
            .expect("query summary")
            .expect("summary exists");
        assert_eq!(
            summary.capability.level,
            crate::activity::model::ActivityCapabilityLevel::Structured
        );
        assert_eq!(summary.event_counts.get("userMessage"), Some(&1));
        assert_eq!(summary.event_counts.get("toolInvocation"), Some(&1));
        assert_eq!(summary.tool_counts, vec![("Read".to_string(), 1)]);

        // 不存在的会话返回 None
        assert!(query_activity_summary(&conn, "missing")
            .expect("no summary")
            .is_none());

        let agents = query_agents(&conn, session_key).expect("query agents");
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent_key, "agent-1");
        assert_eq!(agents[0].child_count, 0);

        let tools = query_tool_summary(&conn, session_key).expect("query tool summary");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].invocation_count, 1);
        assert_eq!(tools[0].success_count, 0); // Running 不算 success
        assert_eq!(tools[0].total_duration_ms, 1200);
        assert_eq!(tools[0].output_bytes_total, 2048);
    }

    #[test]
    fn delete_session_activity_removes_all_rows() {
        let conn = test_conn();
        let session_key = "sess-1";
        write_activity_batch(
            &conn,
            &sample_entry(session_key),
            &sample_batch(session_key),
        )
        .expect("write batch");

        let removed = delete_session_activity(&conn, session_key, false).expect("delete");
        assert_eq!(removed, 7); // 3 events + 1 tool + 1 agent + 1 link + 1 index
        assert_eq!(count_events(&conn, session_key), 0);
        assert_eq!(count_activity_sessions(&conn).expect("count"), 0);
    }

    #[test]
    fn delete_all_and_older_than_days() {
        let conn = test_conn();
        for (i, session) in ["sess-1", "sess-2"].iter().enumerate() {
            let mut entry = sample_entry(session);
            entry.indexed_at = 1_700_000_000 + i as i64 * 100_000;
            write_activity_batch(&conn, &entry, &sample_batch(session)).expect("write batch");
        }
        assert_eq!(count_activity_sessions(&conn).expect("count"), 2);

        // 保留期 1 天：cutoff = now - 86400，两条 index 都远早于 cutoff → 全删。
        let removed = delete_activity_older_than_days(&conn, 1, false).expect("delete old");
        assert!(removed > 0);
        assert_eq!(count_activity_sessions(&conn).expect("count"), 0);

        // 重新写入后 delete_all
        for session in ["sess-1", "sess-2"] {
            write_activity_batch(&conn, &sample_entry(session), &sample_batch(session))
                .expect("write batch");
        }
        let removed = delete_all_activity(&conn, false).expect("delete all");
        assert!(removed > 0);
        assert_eq!(count_activity_sessions(&conn).expect("count"), 0);
    }

    #[test]
    fn summary_truncation_caps_at_500_chars() {
        let long = "x".repeat(600);
        let truncated = truncate_summary(&long);
        assert!(truncated.chars().count() <= SUMMARY_MAX_CHARS + 1); // 500 + 省略号
        let short = "short".to_string();
        assert_eq!(truncate_summary(&short), "short");
    }
}
