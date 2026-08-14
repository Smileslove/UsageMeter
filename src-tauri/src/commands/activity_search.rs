//! 深度活动全文搜索（M3，可选 FTS）IPC 命令。
//!
//! 与 `doc/桌面主应用窗口详细设计.md` 9.8（搜索）、12.4（可选全文表）、
//! 21.3（FTS 需用户明确开启）对齐：
//! - **隐私门控**：`settings.deep_index_level != "fulltext"` 时两个搜索命令
//!   都返回空分页（不触发任何 FTS 查询/写入），前端根据自身档位提示
//!   「全文索引未开启」；
//! - 查询串全部经 [`fts::escape_fts_query`] 转成安全短语，参数化绑定；
//! - 会话内搜索复用 `db::query_events_by_keys` 回填 [`EventsPage`]（保持
//!   FTS 的 bm25 相关性顺序），跨会话搜索回填 [`GlobalSearchPage`]；
//! - 错误码统一 `ERR_ACTIVITY_SEARCH_*`，不含路径/密钥/正文。

use crate::activity::db;
use crate::activity::fts;
use crate::activity::indexer;
use crate::activity::model::EventsPage;
use crate::models::AppSettings;
use std::collections::HashMap;

/// 全文搜索单页上限（与事件列表页一致；命令层 clamp 1..=200）。
const SEARCH_PAGE_MAX_LIMIT: i64 = 200;

/// 跨会话全文搜索命中项（camelCase 序列化）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalSearchHit {
    pub session_key: String,
    /// 会话标题：`local_sessions.topic` 或 `session_name`，取非空者。
    pub session_title: Option<String>,
    pub event_key: String,
    /// `SessionEventKind::as_str()` 值（原始字符串，避免枚举解析失败丢数据）。
    pub kind: String,
    pub status: Option<String>,
    pub summary: Option<String>,
    pub timestamp_ms: Option<i64>,
    pub tool_name: Option<String>,
}

/// 跨会话全文搜索分页结果。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalSearchPage {
    pub items: Vec<GlobalSearchHit>,
    pub total: i64,
    pub has_more: bool,
}

fn activity_db<R>(
    f: impl FnOnce(&mut rusqlite::Connection) -> Result<R, String>,
) -> Result<R, String> {
    let db = crate::local_usage::get_local_usage_db()?;
    db.with_conn(f)
}

/// FTS 门控：只有「本地全文索引」档位（`"fulltext"`）允许全文搜索；
/// `"off"`/`"structured"`/`"ondemand"` 一律返回空结果（设计 21.3
/// 用户显式开启 + 11.3 档位语义）。
///
/// 档位取**持久化设置**（[`crate::settings::service::persisted_deep_index_level`]），
/// 不信任 IPC 传入的 settings——任意调用方不得以 `"fulltext"` 伪造开启；
/// 非法值经归一后落在 `"off"`，同样关闭。`_settings` 参数仅保留历史
/// 签名兼容（外部测试仍按旧签名调用），其值不参与判定。
pub fn fulltext_disabled(_settings: &AppSettings) -> bool {
    crate::settings::persisted_deep_index_level() != "fulltext"
}

fn empty_events_page() -> EventsPage {
    EventsPage {
        items: Vec::new(),
        total: 0,
        has_more: false,
    }
}

fn empty_global_page() -> GlobalSearchPage {
    GlobalSearchPage {
        items: Vec::new(),
        total: 0,
        has_more: false,
    }
}

/// 查询路径的懒索引（与 `commands/activity.rs` 行为一致）：同步执行单会话
/// 索引（含 FTS 填充），失败不阻断查询——「深度解析失败不影响既有统计」。
fn lazy_ensure_indexed(conn: &mut rusqlite::Connection, session_key: &str) {
    match indexer::find_source_ref(conn, session_key) {
        Ok(Some(source)) => {
            if let Err(error) = indexer::ensure_session_indexed(conn, &source) {
                eprintln!("[UsageMeter] Deep index lazy sync failed: {error}");
            }
        }
        Ok(None) => {}
        Err(error) => eprintln!("[UsageMeter] Deep index lazy source lookup failed: {error}"),
    }
}

/// 会话内全文搜索：FTS 命中（bm25 相关性排序）→ 按 event_key 回填
/// `session_events` 详情，返回与活动页一致的 [`EventsPage`]。
///
/// 全文索引未开启（`deep_index_level != "fulltext"`）时返回空页；
/// 开启时对目标会话做一次单会话懒索引（与 `get_session_events` 一致的
/// 查询路径语义，不会静默全盘扫描）。
#[tauri::command]
pub async fn search_session_activity(
    _app: tauri::AppHandle,
    session_key: String,
    query: String,
    offset: i64,
    limit: i64,
    settings: AppSettings,
) -> Result<EventsPage, String> {
    if fulltext_disabled(&settings) {
        return Ok(empty_events_page());
    }
    let limit = limit.clamp(1, SEARCH_PAGE_MAX_LIMIT);
    let offset = offset.max(0);
    activity_db(|conn| {
        lazy_ensure_indexed(conn, &session_key);
        let (keys, total) = fts::search_session(conn, &session_key, &query, offset, limit)?;
        let items = db::query_events_by_keys(conn, &keys)?;
        let has_more = (offset as usize) + items.len() < total as usize;
        Ok(EventsPage {
            items,
            total,
            has_more,
        })
    })
}

/// 跨会话全文搜索：不分会话，返回命中事件 + 会话标题（topic/session_name）
/// 等摘要详情。不做懒索引（避免静默全盘扫描，设计 9.8），只搜已入 FTS
/// 的内容；全文索引未开启时返回空页。
#[tauri::command]
pub async fn search_activity_global(
    _app: tauri::AppHandle,
    query: String,
    offset: i64,
    limit: i64,
    settings: AppSettings,
) -> Result<GlobalSearchPage, String> {
    if fulltext_disabled(&settings) {
        return Ok(empty_global_page());
    }
    let limit = limit.clamp(1, SEARCH_PAGE_MAX_LIMIT);
    let offset = offset.max(0);
    activity_db(|conn| {
        let (hits, total) = fts::search_global(conn, &query, offset, limit)?;
        let items = hydrate_global_hits(conn, &hits)?;
        let has_more = (offset as usize) + items.len() < total as usize;
        Ok(GlobalSearchPage {
            items,
            total,
            has_more,
        })
    })
}

/// 按 FTS 命中顺序回填跨会话搜索详情：事件行 + 工具名 + 会话标题。
/// 一次批量 IN 查询（避免 N+1），再按 hits 顺序组装。
fn hydrate_global_hits(
    conn: &rusqlite::Connection,
    hits: &[fts::FtsHit],
) -> Result<Vec<GlobalSearchHit>, String> {
    if hits.is_empty() {
        return Ok(Vec::new());
    }
    let keys: Vec<&str> = hits.iter().map(|hit| hit.event_key.as_str()).collect();
    let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let sql = format!(
        "SELECT e.event_key, e.session_key, e.kind, e.status, e.summary_redacted,
                e.timestamp_ms, MIN(ti.tool_name),
                COALESCE(NULLIF(ls.topic, ''), NULLIF(ls.session_name, ''))
         FROM session_events e
         LEFT JOIN session_tool_invocations ti ON ti.event_key = e.event_key
         LEFT JOIN local_sessions ls ON ls.session_id = e.session_key
         WHERE e.event_key IN ({placeholders})
         GROUP BY e.event_key"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("ERR_ACTIVITY_SEARCH_PREPARE_HYDRATE: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(keys.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                GlobalSearchHit {
                    session_key: row.get(1)?,
                    kind: row.get(2)?,
                    status: row.get(3)?,
                    summary: row.get(4)?,
                    timestamp_ms: row.get(5)?,
                    tool_name: row.get(6)?,
                    session_title: row.get(7)?,
                    event_key: String::new(), // 占位，组装时补
                },
            ))
        })
        .map_err(|e| format!("ERR_ACTIVITY_SEARCH_QUERY_HYDRATE: {e}"))?;
    let mut by_key: HashMap<String, GlobalSearchHit> = HashMap::new();
    for row in rows {
        let (event_key, mut hit) = row.map_err(|e| format!("ERR_ACTIVITY_SEARCH_READ: {e}"))?;
        hit.event_key = event_key.clone();
        by_key.insert(event_key, hit);
    }
    // 按 FTS 命中顺序（bm25 相关性）组装；缺失键防御性跳过。
    let mut items = Vec::with_capacity(hits.len());
    for hit in hits {
        if let Some(detail) = by_key.remove(hit.event_key.as_str()) {
            items.push(detail);
        }
    }
    Ok(items)
}

// ---------------------------------------------------------------------------
// 单元测试：门控逻辑与跨会话回填
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::db::ACTIVITY_TABLES_DDL;
    use crate::activity::model::AgentRelationLevel;

    /// 内存连接：深度表 + 迷你 local_sessions（只含跨会话回填所需列）。
    fn test_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
        conn.execute_batch(ACTIVITY_TABLES_DDL)
            .expect("create activity tables");
        conn.execute_batch(
            "CREATE TABLE local_sessions (
                session_id TEXT PRIMARY KEY,
                topic TEXT,
                session_name TEXT
            );",
        )
        .expect("create mini local_sessions");
        conn
    }

    fn sample_batch(session_key: &str) -> crate::activity::adapter::ActivityIndexBatch {
        use crate::activity::adapter::{NewAgentNode, NewSessionEvent, NewToolInvocation};
        use crate::activity::model::{ContentState, EventStatus, SessionEventKind};
        crate::activity::adapter::ActivityIndexBatch {
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
                    summary_redacted: Some("search the token budget".to_string()),
                    content_state: ContentState::Redacted,
                    source_file_path: "~/.claude/projects/x.jsonl".to_string(),
                    source_offset: Some(10),
                    payload_hash: None,
                    raw_event_kind: "user".to_string(),
                },
                NewSessionEvent {
                    event_key: format!("{session_key}:evt-2"),
                    sequence: 2,
                    timestamp_ms: Some(1_700_000_001_000),
                    kind: SessionEventKind::ToolInvocation,
                    status: Some(EventStatus::Running),
                    actor_agent_key: None,
                    parent_event_key: None,
                    summary_redacted: Some("Read config file".to_string()),
                    content_state: ContentState::Redacted,
                    source_file_path: "~/.claude/projects/x.jsonl".to_string(),
                    source_offset: Some(20),
                    payload_hash: None,
                    raw_event_kind: "tool_use".to_string(),
                },
            ],
            agents: vec![NewAgentNode {
                agent_key: format!("{session_key}:agent-1"),
                parent_agent_key: None,
                display_kind: Some("Task".to_string()),
                task_summary_redacted: Some("task".to_string()),
                started_at_ms: None,
                ended_at_ms: None,
                status: Some(EventStatus::Success),
                relation_level: AgentRelationLevel::RootGrouped,
            }],
            tool_invocations: vec![NewToolInvocation {
                invocation_key: format!("{session_key}:inv-1"),
                event_key: format!("{session_key}:evt-2"),
                tool_name: "Read".to_string(),
                family: "fs".to_string(),
                duration_ms: None,
                status: Some(EventStatus::Running),
                input_bytes: None,
                output_bytes: None,
                input_keys: vec![],
                result_kind: None,
            }],
            request_links: vec![],
        }
    }

    fn write_and_hydrate_setup(
        conn: &rusqlite::Connection,
        session_key: &str,
        title: Option<&str>,
    ) {
        let entry = crate::activity::db::ActivityIndexEntry {
            session_key: session_key.to_string(),
            tool: "claude_code".to_string(),
            capability: crate::activity::model::SessionActivityCapability {
                level: crate::activity::model::ActivityCapabilityLevel::Structured,
                messages: true,
                tool_invocations: true,
                tool_results: true,
                request_links: false,
                agent_relations: AgentRelationLevel::RootGrouped,
                content_search: true,
                source_content_available: true,
                parser_id: "claude_code_v2".to_string(),
                parser_version: 2,
            },
            event_count: 2,
            tool_call_count: 1,
            agent_count: 1,
            source_fingerprint: Some("fp".to_string()),
            parser_id: "claude_code_v2".to_string(),
            parser_version: 2,
            indexed_at: 1_700_000_000,
            updated_at: 1_700_000_000,
        };
        db::write_activity_batch(conn, &entry, &sample_batch(session_key)).expect("write batch");
        conn.execute(
            "INSERT INTO local_sessions (session_id, topic, session_name) VALUES (?1, ?2, ?3)",
            rusqlite::params![session_key, title, title.map(|t| format!("{t}-name"))],
        )
        .expect("insert session title");
    }

    #[test]
    fn fulltext_gate_uses_persisted_level_and_requires_fulltext() {
        use crate::test_support::env_lock;
        let _guard = env_lock();
        let previous_home = std::env::var_os("HOME");
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("HOME", dir.path());
        let settings_dir = dir.path().join(".usagemeter");
        std::fs::create_dir_all(&settings_dir).expect("create settings dir");
        let write_level = |level: &str| {
            std::fs::write(
                settings_dir.join("settings.json"),
                serde_json::json!({ "settingsVersion": 3, "deepIndexLevel": level }).to_string(),
            )
            .expect("write preferences");
        };

        // 无任何持久化设置：默认 off → 全文搜索关闭。
        assert!(fulltext_disabled(&AppSettings::default()));
        // off/structured/ondemand 一律关闭（语义不变：只有 fulltext 允许）。
        for level in ["off", "structured", "ondemand"] {
            write_level(level);
            assert!(
                fulltext_disabled(&AppSettings::default()),
                "level {level} must disable fulltext search"
            );
        }
        // 持久化档位 fulltext（偏好文件为权威落点）→ 开启。
        write_level("fulltext");
        assert!(!fulltext_disabled(&AppSettings::default()));
        // 非法值（如 "fullText"）归一为 off → 关闭，不误开全文。
        write_level("fullText");
        assert!(fulltext_disabled(&AppSettings::default()));

        match previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn hydrate_global_hits_preserves_fts_order_and_title() {
        let conn = test_conn();
        // sess-1 有 topic；sess-2 只有 session_name（topic 为空）。
        write_and_hydrate_setup(&conn, "sess-1", Some("Budget Fix"));
        write_and_hydrate_setup(&conn, "sess-2", None);
        conn.execute(
            "UPDATE local_sessions SET topic = NULL, session_name = 'Second Session' WHERE session_id = 'sess-2'",
            [],
        )
        .expect("set session_name only");

        let (hits, total) = fts::search_global(&conn, "token", 0, 10).expect("search");
        assert_eq!(total, 2);
        let items = hydrate_global_hits(&conn, &hits).expect("hydrate");
        // 顺序与 FTS 命中顺序一致（bm25 排序：两会话同分时顺序确定）。
        assert_eq!(items.len(), 2);
        let keys: Vec<&str> = items.iter().map(|hit| hit.event_key.as_str()).collect();
        let expected: Vec<&str> = hits.iter().map(|hit| hit.event_key.as_str()).collect();
        assert_eq!(keys, expected);
        let by_session: HashMap<&str, &GlobalSearchHit> = items
            .iter()
            .map(|hit| (hit.session_key.as_str(), hit))
            .collect();
        assert_eq!(
            by_session["sess-1"].session_title.as_deref(),
            Some("Budget Fix"),
            "topic wins when non-empty"
        );
        assert_eq!(
            by_session["sess-2"].session_title.as_deref(),
            Some("Second Session"),
            "session_name used when topic empty"
        );
        // 工具事件已入 FTS 且 tool_name 列被索引（供带工具过滤的搜索）。
        let tool_rows: Vec<String> = conn
            .prepare(
                "SELECT tool_name FROM session_event_fts
                 WHERE kind = 'toolInvocation' AND tool_name != ''",
            )
            .expect("prepare tool rows")
            .query_map([], |row| row.get(0))
            .expect("query tool rows")
            .filter_map(Result::ok)
            .collect();
        assert!(
            tool_rows.iter().any(|name| name == "Read"),
            "tool_name must be indexed in FTS: {tool_rows:?}"
        );
    }

    #[test]
    fn session_search_returns_events_page_in_bm25_order() {
        let conn = test_conn();
        write_and_hydrate_setup(&conn, "sess-1", Some("Budget Fix"));
        // 会话内搜索：命中 evt-1（摘要含 token），回填为 SessionEventListItem。
        let (keys, total) = fts::search_session(&conn, "sess-1", "token", 0, 10).expect("search");
        assert_eq!(total, 1);
        let items = db::query_events_by_keys(&conn, &keys).expect("hydrate events");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].event_key, "sess-1:evt-1");
        assert_eq!(items[0].summary.as_deref(), Some("search the token budget"));
        // 工具名命中：evt-2 的 tool_name=Read 可搜，回填带 tool 摘要。
        let (keys, total) = fts::search_session(&conn, "sess-1", "Read", 0, 10).expect("search");
        assert_eq!(total, 1);
        let items = db::query_events_by_keys(&conn, &keys).expect("hydrate tool event");
        assert_eq!(
            items[0].tool.as_ref().map(|t| t.raw_name.as_str()),
            Some("Read")
        );
    }
}
