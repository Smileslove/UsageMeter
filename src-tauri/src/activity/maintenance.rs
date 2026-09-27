//! 深度活动索引自动清理（M3「索引保留和清理」）。
//!
//! 口径说明：`db::delete_activity_older_than_days` 按
//! `session_activity_index.indexed_at`（秒）判定保留期——该语义服务于手动
//! purge 的 `older_than_days` scope；本模块的自动清理改按**事件最新
//! timestamp** 判定（`session_events.timestamp_ms` 毫秒），因为保留期面向
//! 用户可见的活动时间线，而不是索引建立时间。判定口径差异在代码注释中
//! 显式声明，避免两处清理语义被误认为一致。
//!
//! FTS 一致性：逐会话删除经由 `db::delete_session_activity`，该函数在
//! db.rs 中已接入 `session_event_fts` 同步删除（M3 FTS 并行子代理负责），
//! 本模块不重复调用 fts 模块，避免双重清理。

use crate::activity::db;
use rusqlite::{params, Connection};
use std::sync::atomic::{AtomicI64, Ordering};

/// 自动清理节流间隔（秒）：两次执行至少间隔 10 分钟（查询路径惰性触发，
/// 不因频繁清理拖慢查询）。
const AUTO_PURGE_MIN_INTERVAL_SECS: i64 = 600;
/// 上次自动清理执行的 unix 秒（进程内惰性节流；0 = 从未执行）。
static LAST_AUTO_PURGE_AT: AtomicI64 = AtomicI64::new(0);

/// 自动清理入口（查询路径调用）：
/// - `retention_days <= 0` 跳过（不设保留期，永不自动清理）；
/// - 距上次执行不足 [`AUTO_PURGE_MIN_INTERVAL_SECS`] 秒时跳过（惰性节流）；
/// - 执行失败仅 eprintln 记录，不向调用方返回错误（查询路径不因清理
///   失败而阻断）。
pub fn maybe_auto_purge(conn: &Connection, retention_days: i64) -> Result<(), String> {
    if retention_days <= 0 {
        return Ok(());
    }
    let now = chrono::Utc::now().timestamp();
    let last = LAST_AUTO_PURGE_AT.load(Ordering::Relaxed);
    if last != 0 && now - last < AUTO_PURGE_MIN_INTERVAL_SECS {
        return Ok(());
    }
    // 乐观抢占：CAS 失败说明并发路径刚执行过，直接跳过。
    if LAST_AUTO_PURGE_AT
        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return Ok(());
    }
    let result = purge_sessions_older_than(conn, retention_days);
    if let Err(error) = &result {
        eprintln!("[UsageMeter] Auto activity purge failed: {error}");
    }
    result
}

/// 按事件最新时间清理：查询各会话最新事件时间戳，
/// `MAX(timestamp_ms) < now - days * 86400 * 1000`（毫秒）的会话
/// 逐会话删除（复用 `db::delete_session_activity`，含 FTS 同步清理）。
///
/// `timestamp_ms` 全为 NULL 的会话不满足 HAVING 条件（NULL 比较不成立），
/// 不会被自动清理误删——保守方向，孤儿索引行仍可手动 purge。
fn purge_sessions_older_than(conn: &Connection, retention_days: i64) -> Result<(), String> {
    let cutoff_ms = chrono::Utc::now().timestamp_millis() - retention_days * 86_400 * 1_000;
    let session_keys: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT session_key FROM session_events
                 GROUP BY session_key
                 HAVING MAX(timestamp_ms) < ?1",
            )
            .map_err(|error| format!("ERR_ACTIVITY_PREPARE_OLD_SESSIONS: {error}"))?;
        let rows = stmt
            .query_map(params![cutoff_ms], |row| row.get::<_, String>(0))
            .map_err(|error| format!("ERR_ACTIVITY_QUERY_OLD_SESSIONS: {error}"))?;
        let mut keys = Vec::new();
        for row in rows {
            keys.push(row.map_err(|error| format!("ERR_ACTIVITY_READ_OLD_SESSION: {error}"))?);
        }
        keys
    };
    for session_key in session_keys {
        db::delete_session_activity(conn, &session_key, false)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::adapter::{ActivityIndexBatch, NewSessionEvent};
    use crate::activity::db::{self, ActivityIndexEntry, ACTIVITY_TABLES_DDL};
    use crate::activity::model::{
        ActivityCapabilityLevel, AgentRelationLevel, ContentState, EventStatus,
        SessionActivityCapability, SessionEventKind,
    };
    use rusqlite::Connection;
    use std::sync::{Mutex, MutexGuard};

    static THROTTLE_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// 内存 SQLite 测试连接（与生产共用同一 DDL 常量）。
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory sqlite");
        conn.execute_batch(ACTIVITY_TABLES_DDL)
            .expect("create activity tables");
        conn
    }

    fn reset_throttle() {
        LAST_AUTO_PURGE_AT.store(0, Ordering::Relaxed);
    }

    fn throttle_test_guard() -> MutexGuard<'static, ()> {
        THROTTLE_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn count_events(conn: &Connection, session_key: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM session_events WHERE session_key = ?1",
            params![session_key],
            |row| row.get(0),
        )
        .expect("count events")
    }

    /// 写入单事件会话（index 行 + events），事件时间戳可控。
    fn insert_session_events(conn: &Connection, session_key: &str, timestamp_ms: i64) {
        let entry = ActivityIndexEntry {
            session_key: session_key.to_string(),
            tool: "claude_code".to_string(),
            capability: SessionActivityCapability {
                level: ActivityCapabilityLevel::Structured,
                messages: true,
                tool_invocations: true,
                tool_results: true,
                request_links: true,
                agent_relations: AgentRelationLevel::RootGrouped,
                content_search: false,
                source_content_available: true,
                parser_id: "claude_code".to_string(),
                parser_version: 1,
            },
            event_count: 1,
            tool_call_count: 0,
            agent_count: 0,
            source_fingerprint: None,
            parser_id: "claude_code".to_string(),
            parser_version: 1,
            indexed_at: timestamp_ms / 1000,
            updated_at: timestamp_ms / 1000,
        };
        db::upsert_session_activity_index(conn, &entry).expect("upsert index");
        let batch = ActivityIndexBatch {
            session_key: session_key.to_string(),
            events: vec![NewSessionEvent {
                event_key: format!("{session_key}:evt-1"),
                sequence: 1,
                timestamp_ms: Some(timestamp_ms),
                kind: SessionEventKind::UserMessage,
                status: Some(EventStatus::Success),
                actor_agent_key: None,
                parent_event_key: None,
                summary_redacted: Some("hello".to_string()),
                content_state: ContentState::Available,
                source_file_path: "/tmp/x.jsonl".to_string(),
                source_offset: Some(1),
                payload_hash: None,
                raw_event_kind: "user".to_string(),
            }],
            agents: vec![],
            tool_invocations: vec![],
            request_links: vec![],
        };
        db::replace_session_events(conn, session_key, &batch.events).expect("replace events");
    }

    fn now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    #[test]
    fn retention_days_le_zero_skips_purge() {
        let _guard = throttle_test_guard();
        let conn = test_conn();
        reset_throttle();
        insert_session_events(&conn, "sess-old", now_ms() - 100 * 86_400 * 1_000);
        assert_eq!(count_events(&conn, "sess-old"), 1);

        maybe_auto_purge(&conn, 0).expect("retention 0 is a no-op");
        maybe_auto_purge(&conn, -7).expect("negative retention is a no-op");
        assert_eq!(
            count_events(&conn, "sess-old"),
            1,
            "no purge when retention <= 0"
        );
    }

    #[test]
    fn purges_sessions_whose_latest_event_is_older_than_retention() {
        let _guard = throttle_test_guard();
        let conn = test_conn();
        reset_throttle();
        insert_session_events(&conn, "sess-old", now_ms() - 100 * 86_400 * 1_000);
        insert_session_events(&conn, "sess-new", now_ms() - 1_000);

        maybe_auto_purge(&conn, 90).expect("auto purge");
        assert_eq!(count_events(&conn, "sess-old"), 0, "old session purged");
        assert_eq!(count_events(&conn, "sess-new"), 1, "recent session kept");
        // index 行一并清除（delete_session_activity 5 表联动）。
        assert_eq!(
            db::count_activity_sessions(&conn).expect("count"),
            1,
            "only recent session index remains"
        );
    }

    #[test]
    fn throttle_skips_second_call_within_ten_minutes() {
        let _guard = throttle_test_guard();
        let conn = test_conn();
        reset_throttle();
        insert_session_events(&conn, "sess-old-1", now_ms() - 100 * 86_400 * 1_000);
        maybe_auto_purge(&conn, 90).expect("first purge runs");
        assert_eq!(count_events(&conn, "sess-old-1"), 0);

        // 节流窗口内再次调用：跳过执行，老会话保留。
        insert_session_events(&conn, "sess-old-2", now_ms() - 100 * 86_400 * 1_000);
        maybe_auto_purge(&conn, 90).expect("throttled call returns ok");
        assert_eq!(
            count_events(&conn, "sess-old-2"),
            1,
            "second call throttled"
        );

        // 重置节流后执行。
        reset_throttle();
        maybe_auto_purge(&conn, 90).expect("purge after throttle reset");
        assert_eq!(count_events(&conn, "sess-old-2"), 0);
    }
}
