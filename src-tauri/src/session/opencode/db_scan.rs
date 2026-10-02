use crate::session::opencode_reader::{
    OpenCodeDbCacheState, OpenCodeDbCheckpoint, OpenCodeMessageSnapshot, OpenCodePathSignature,
    OpenCodeSchemaMode, OpenCodeStorageRoot, OpenCodeStorageSignature,
    OPENCODE_DB_FULL_RECONCILE_INTERVAL_SECS, REQUIRED_MESSAGE_COLUMNS, REQUIRED_SESSION_COLUMNS,
    REQUIRED_V2_MESSAGE_COLUMNS,
};
use rusqlite::{params, Connection, OpenFlags};
use serde_json::Value;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;

#[derive(Debug, Clone)]
struct DbMessageRow {
    rowid: i64,
    id: String,
    session_id: String,
    time_updated_ms: i64,
    time_created_ms: i64,
    message_type: Option<String>,
    data: Value,
}

#[allow(dead_code)]
pub(in crate::session) fn refresh_db_messages(
    state: &mut OpenCodeDbCacheState,
) -> HashMap<String, OpenCodeMessageSnapshot> {
    let Some(db_path) = crate::session::opencode_reader::find_opencode_db() else {
        clear_missing_db_state(state);
        return state.messages.clone();
    };
    let home = db_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    let root = OpenCodeStorageRoot {
        id: "native".to_string(),
        message_root: home.join("storage").join("message"),
        home,
        db_path,
    };
    refresh_db_messages_for_path(state, &root)
}

pub(in crate::session) fn refresh_db_messages_for_path(
    state: &mut OpenCodeDbCacheState,
    root: &OpenCodeStorageRoot,
) -> HashMap<String, OpenCodeMessageSnapshot> {
    if !root.db_path.exists() {
        clear_missing_db_state(state);
        return state.messages.clone();
    }

    let storage_signature = compute_opencode_db_storage_signature(&root.db_path);
    let storage_signature_hash = storage_signature.hash();
    let storage_signature_changed = storage_signature_hash != state.storage_signature_hash;

    let file_size = storage_signature.db.size;
    let conn = match open_opencode_db_read_only(&root.db_path) {
        Ok(c) => c,
        Err(_) => return state.messages.clone(),
    };

    let previous_schema_mode = state.schema_mode;
    let (schema_mode, _) = super::schema::detect_schema_mode(
        &conn,
        REQUIRED_SESSION_COLUMNS,
        REQUIRED_MESSAGE_COLUMNS,
    );
    if schema_mode == OpenCodeSchemaMode::Incompatible {
        state.messages.clear();
        state.storage_signature_hash = storage_signature_hash;
        state.file_size = file_size;
        state.schema_fingerprint = compute_schema_fingerprint(&conn);
        state.assistant_row_count = 0;
        state.last_time_updated_ms = 0;
        state.last_rowid = 0;
        state.schema_mode = schema_mode;
        return state.messages.clone();
    }

    let checkpoint = read_db_checkpoint(&conn);
    let v2_has_rows = has_v2_message_rows(&conn);
    let checkpoint_unchanged = checkpoint.schema_fingerprint == state.schema_fingerprint
        && checkpoint.assistant_row_count == state.assistant_row_count
        && checkpoint.max_rowid == state.last_rowid
        && checkpoint.max_time_updated_ms == state.last_time_updated_ms;
    if !storage_signature_changed
        && checkpoint_unchanged
        && previous_schema_mode == schema_mode
        && !state.messages.is_empty()
    {
        return state.messages.clone();
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let checkpoint_rewound = checkpoint.assistant_row_count < state.assistant_row_count
        || checkpoint.max_rowid < state.last_rowid
        || checkpoint.max_time_updated_ms < state.last_time_updated_ms;
    let checkpoint_advanced = checkpoint.assistant_row_count > state.assistant_row_count
        || checkpoint.max_rowid > state.last_rowid
        || checkpoint.max_time_updated_ms > state.last_time_updated_ms;
    let should_full_reconcile = state.messages.is_empty()
        || checkpoint_rewound
        || state.last_full_reconcile_at_ms == 0
        || previous_schema_mode != schema_mode
        || checkpoint.schema_fingerprint != state.schema_fingerprint
        || (storage_signature_changed && !checkpoint_advanced)
        || v2_has_rows
        || now_ms.saturating_sub(state.last_full_reconcile_at_ms)
            >= OPENCODE_DB_FULL_RECONCILE_INTERVAL_SECS * 1000;

    let rows = if should_full_reconcile {
        query_db_message_rows(&conn, None, None)
    } else {
        query_db_message_rows(
            &conn,
            Some(state.last_time_updated_ms),
            Some(state.last_rowid),
        )
    };
    let v2_rows = if should_full_reconcile && v2_has_rows {
        query_v2_message_rows(&conn)
    } else {
        Vec::new()
    };

    if should_full_reconcile {
        state.messages.clear();
    }

    let mut max_time_updated_ms = if should_full_reconcile {
        0
    } else {
        state.last_time_updated_ms
    };
    let mut max_rowid = if should_full_reconcile {
        0
    } else {
        state.last_rowid
    };
    for row in rows {
        max_time_updated_ms = max_time_updated_ms.max(row.time_updated_ms);
        max_rowid = max_rowid.max(row.rowid);
        if let Some(snapshot) = super::message::parse_message_snapshot(
            &root.id,
            &root.db_path.to_string_lossy(),
            &row.session_id,
            &row.id,
            &row.data,
            row.time_updated_ms,
            "opencode_db",
        ) {
            state
                .messages
                .insert(snapshot.message_identity_key(), snapshot);
        }
    }
    for row in v2_rows {
        if let Some(snapshot) = super::message::parse_v2_message_snapshot(
            &root.id,
            &root.db_path.to_string_lossy(),
            &row.session_id,
            &row.id,
            row.message_type.as_deref().unwrap_or_default(),
            &row.data,
            row.time_updated_ms.max(row.time_created_ms),
            "opencode_db",
        ) {
            state
                .messages
                .insert(snapshot.message_identity_key(), snapshot);
        }
    }

    state.storage_signature_hash = storage_signature_hash;
    state.file_size = file_size;
    state.schema_fingerprint = checkpoint.schema_fingerprint;
    state.assistant_row_count = checkpoint.assistant_row_count;
    state.last_time_updated_ms = checkpoint.max_time_updated_ms.max(max_time_updated_ms);
    state.last_rowid = checkpoint.max_rowid.max(max_rowid);
    state.schema_mode = schema_mode;
    if should_full_reconcile {
        state.last_full_reconcile_at_ms = now_ms;
    }

    state.messages.clone()
}

fn clear_missing_db_state(state: &mut OpenCodeDbCacheState) {
    state.messages.clear();
    state.storage_signature_hash = 0;
    state.file_size = 0;
    state.schema_fingerprint = 0;
    state.assistant_row_count = 0;
    state.last_time_updated_ms = 0;
    state.last_rowid = 0;
    state.last_full_reconcile_at_ms = 0;
    state.schema_mode = OpenCodeSchemaMode::Incompatible;
}

/// 判断指定表是否存在指定列（表不存在时返回 false）。
fn table_has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let sql = format!("PRAGMA table_info({})", table);
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return false;
    };
    stmt.query_map([], |row| row.get::<_, String>(1))
        .map(|rows| rows.flatten().any(|name| name == column))
        .unwrap_or(false)
}

fn query_db_message_rows(
    conn: &Connection,
    last_time_updated_ms: Option<i64>,
    last_rowid: Option<i64>,
) -> Vec<DbMessageRow> {
    // LEFT JOIN session to filter out fork-replayed messages: a forked session copies historical
    // messages with their original time_created (earlier than the fork session's time_created).
    // Any message.time_created < session.time_created is a replayed copy and must be excluded.
    //
    // fork 过滤依赖 message.time_created 与 session.time_created 两列；
    // REQUIRED_MESSAGE_COLUMNS 并不要求 time_created，且 message-only 模式下
    // session 表可能缺列甚至不存在。缺少任一依赖时退化为不做 fork 过滤，
    // 避免 SQL prepare 失败导致整个扫描静默返回空结果（丢数据）。
    let fork_filter_supported = table_has_column(conn, "message", "time_created")
        && table_has_column(conn, "session", "time_created");
    let (fork_join, fork_cond) = if fork_filter_supported {
        (
            "\n             LEFT JOIN session s ON s.id = m.session_id",
            "\n               AND (s.time_created IS NULL OR m.time_created >= s.time_created)",
        )
    } else {
        ("", "")
    };

    let (sql, params_vec): (String, Vec<i64>) = if let (Some(last_time), Some(last_rowid)) =
        (last_time_updated_ms, last_rowid)
    {
        (
            format!(
                "SELECT m.rowid, m.id, m.session_id, COALESCE(m.time_updated, 0), m.data
             FROM message m{fork_join}
             WHERE json_extract(m.data, '$.role') = 'assistant'{fork_cond}
               AND (COALESCE(m.time_updated, 0) > ?1 OR (COALESCE(m.time_updated, 0) = ?1 AND m.rowid > ?2))
             ORDER BY COALESCE(m.time_updated, 0) ASC, m.rowid ASC"
            ),
            vec![last_time, last_rowid],
        )
    } else {
        (
            format!(
                "SELECT m.rowid, m.id, m.session_id, COALESCE(m.time_updated, 0), m.data
             FROM message m{fork_join}
             WHERE json_extract(m.data, '$.role') = 'assistant'{fork_cond}
             ORDER BY COALESCE(m.time_updated, 0) ASC, m.rowid ASC"
            ),
            Vec::new(),
        )
    };

    let mut stmt = match conn.prepare(&sql) {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };
    let mut rows = if params_vec.is_empty() {
        match stmt.query([]) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        }
    } else {
        match stmt.query(params![params_vec[0], params_vec[1]]) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        }
    };

    let mut out = Vec::new();
    while let Ok(Some(row)) = rows.next() {
        let data_str: String = match row.get(4) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let data = match serde_json::from_str::<Value>(&data_str) {
            Ok(v) => v,
            Err(_) => continue,
        };
        out.push(DbMessageRow {
            rowid: row.get(0).unwrap_or(0),
            id: row.get::<_, String>(1).unwrap_or_default(),
            session_id: row.get::<_, String>(2).unwrap_or_default(),
            time_updated_ms: row.get::<_, i64>(3).unwrap_or(0),
            time_created_ms: 0,
            message_type: None,
            data,
        });
    }
    out
}

fn has_v2_message_rows(conn: &Connection) -> bool {
    if !REQUIRED_V2_MESSAGE_COLUMNS
        .iter()
        .all(|column| table_has_column(conn, "session_message", column))
    {
        return false;
    }
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM session_message
            WHERE type IN ('assistant', 'compaction')
        )",
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .unwrap_or(false)
}

fn query_v2_message_rows(conn: &Connection) -> Vec<DbMessageRow> {
    let session_table = if table_has_column(conn, "session_v2", "id") {
        Some("session_v2")
    } else if table_has_column(conn, "session", "id") {
        Some("session")
    } else {
        None
    };
    let (join, fork_filter) = match session_table {
        Some(table) if table_has_column(conn, table, "time_created") => (
            format!(" LEFT JOIN {table} s ON s.id = m.session_id"),
            " AND (s.time_created IS NULL OR m.time_created >= s.time_created)",
        ),
        _ => (String::new(), ""),
    };
    let sql = format!(
        "SELECT m.rowid, m.id, m.session_id, COALESCE(m.time_updated, 0),
                COALESCE(m.time_created, 0), m.type, m.data
         FROM session_message m{join}
         WHERE m.type IN ('assistant', 'compaction'){fork_filter}
         ORDER BY COALESCE(m.time_created, 0) ASC, m.seq ASC, m.rowid ASC"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let Ok(mut rows) = stmt.query([]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(row)) = rows.next() {
        let data_str: String = match row.get(6) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let Ok(data) = serde_json::from_str::<Value>(&data_str) else {
            continue;
        };
        out.push(DbMessageRow {
            rowid: row.get(0).unwrap_or(0),
            id: row.get::<_, String>(1).unwrap_or_default(),
            session_id: row.get::<_, String>(2).unwrap_or_default(),
            time_updated_ms: row.get::<_, i64>(3).unwrap_or(0),
            time_created_ms: row.get::<_, i64>(4).unwrap_or(0),
            message_type: row.get::<_, String>(5).ok(),
            data,
        });
    }
    out
}

pub(in crate::session) fn compute_opencode_db_storage_signature(
    db_path: &Path,
) -> OpenCodeStorageSignature {
    OpenCodeStorageSignature {
        db_path: db_path.to_string_lossy().to_string(),
        db: read_path_signature(db_path),
        wal: read_path_signature(&db_path.with_extension("db-wal")),
        shm: read_path_signature(&db_path.with_extension("db-shm")),
    }
}

fn read_path_signature(path: &Path) -> OpenCodePathSignature {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return OpenCodePathSignature::default(),
    };
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    OpenCodePathSignature {
        exists: true,
        size: meta.len(),
        mtime_ns,
    }
}

fn open_opencode_db_read_only(db_path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let _ = conn.execute_batch("PRAGMA busy_timeout=3000;");
    Ok(conn)
}

fn read_db_checkpoint(conn: &Connection) -> OpenCodeDbCheckpoint {
    let mut checkpoint = OpenCodeDbCheckpoint {
        schema_fingerprint: compute_schema_fingerprint(conn),
        ..Default::default()
    };
    let mut values = (0_i64, 0_i64, 0_i64);
    if table_has_column(conn, "message", "data") {
        values = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(MAX(rowid), 0),
                        COALESCE(MAX(COALESCE(time_updated, 0)), 0)
                 FROM message
                 WHERE json_extract(data, '$.role') = 'assistant'",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0).unwrap_or(0),
                        row.get::<_, i64>(1).unwrap_or(0),
                        row.get::<_, i64>(2).unwrap_or(0),
                    ))
                },
            )
            .unwrap_or(values);
    }
    if REQUIRED_V2_MESSAGE_COLUMNS
        .iter()
        .all(|column| table_has_column(conn, "session_message", column))
    {
        let v2 = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(MAX(rowid), 0),
                        COALESCE(MAX(COALESCE(time_updated, 0)), 0)
                 FROM session_message
                 WHERE type IN ('assistant', 'compaction')",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0).unwrap_or(0),
                        row.get::<_, i64>(1).unwrap_or(0),
                        row.get::<_, i64>(2).unwrap_or(0),
                    ))
                },
            )
            .unwrap_or((0, 0, 0));
        values.0 += v2.0;
        values.1 = values.1.max(v2.1);
        values.2 = values.2.max(v2.2);
    }
    checkpoint.assistant_row_count = values.0.max(0) as u64;
    checkpoint.max_rowid = values.1.max(0);
    checkpoint.max_time_updated_ms = values.2.max(0);
    checkpoint
}

fn compute_schema_fingerprint(conn: &Connection) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut stmt = match conn.prepare(
        "SELECT name, COALESCE(sql, '')
         FROM sqlite_schema
         WHERE type = 'table' AND name IN ('message', 'session', 'session_message', 'session_v2')
         ORDER BY name ASC",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return 0,
    };
    let rows = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0).unwrap_or_default(),
            row.get::<_, String>(1).unwrap_or_default(),
        ))
    }) {
        Ok(rows) => rows,
        Err(_) => return 0,
    };
    for row in rows.flatten() {
        row.0.hash(&mut hasher);
        row.1.hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};
    use std::fs;
    use tempfile::tempdir;

    fn create_message_schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE message (
               id TEXT,
               session_id TEXT,
               time_updated INTEGER,
               data TEXT
             );",
        )
        .unwrap();
    }

    fn insert_message(
        conn: &Connection,
        id: &str,
        session_id: &str,
        time_updated_ms: i64,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        conn.execute(
            "INSERT INTO message (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
            params![
                id,
                session_id,
                time_updated_ms,
                serde_json::json!({
                    "id": id,
                    "sessionID": session_id,
                    "modelID": "gpt-4o",
                    "path": { "cwd": "/tmp/project" },
                    "role": "assistant",
                    "time": { "created": time_updated_ms - 5000, "completed": time_updated_ms },
                    "tokens": { "input": input_tokens, "output": output_tokens, "reasoning": 0, "cache": { "read": 0, "write": 0 } }
                })
                .to_string()
            ],
        )
        .unwrap();
    }

    fn make_root(db_path: &Path) -> OpenCodeStorageRoot {
        let home = db_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        OpenCodeStorageRoot {
            id: "test".to_string(),
            home: home.clone(),
            message_root: home.join("storage").join("message"),
            db_path: db_path.to_path_buf(),
        }
    }

    fn message_keys(messages: &HashMap<String, OpenCodeMessageSnapshot>) -> Vec<String> {
        let mut keys: Vec<String> = messages.keys().cloned().collect();
        keys.sort();
        keys
    }

    fn ghost_snapshot(source_path: &str) -> OpenCodeMessageSnapshot {
        OpenCodeMessageSnapshot {
            source_path: source_path.to_string(),
            canonical_session_id: "opencode::test::ghost_sess".to_string(),
            raw_message_id: "ghost_msg".to_string(),
            timestamp_sec: 1,
            model: "gpt-4o".to_string(),
            cwd: None,
            title: None,
            input_tokens: 1,
            output_tokens: 1,
            reasoning_tokens: 0,
            cache_create_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: 2,
            explicit_cost: None,
            source_kind: "test",
        }
    }

    #[test]
    fn refresh_db_checkpoint_unchanged_returns_cached_messages_without_requery() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("opencode.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            create_message_schema(&conn);
            insert_message(&conn, "m1", "sess_1", 1_700_000_000_000, 10, 2);
        }
        let root = make_root(&db_path);
        let mut state = OpenCodeDbCacheState::default();

        let first = refresh_db_messages_for_path(&mut state, &root);
        assert_eq!(first.len(), 1);
        assert_eq!(state.assistant_row_count, 1);
        assert_eq!(state.last_rowid, 1);

        // 注入仅存在于缓存中的幽灵消息：若 checkpoint 未变直接复用缓存，它会保留
        let ghost = ghost_snapshot(&db_path.to_string_lossy());
        state.messages.insert(ghost.message_identity_key(), ghost);

        let second = refresh_db_messages_for_path(&mut state, &root);
        assert!(
            second.contains_key("opencode::test::ghost_sess|ghost_msg"),
            "unchanged checkpoint must return cached messages without re-query"
        );
        assert_eq!(second.len(), 2);
        assert!(second.contains_key("opencode::test::sess_1|m1"));
    }

    #[test]
    fn refresh_db_rewound_checkpoint_triggers_full_reconcile() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("opencode.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            create_message_schema(&conn);
            insert_message(&conn, "m1", "sess_1", 1_700_000_000_000, 10, 2);
            insert_message(&conn, "m2", "sess_1", 1_700_000_001_000, 10, 2);
            insert_message(&conn, "m3", "sess_1", 1_700_000_002_000, 10, 2);
        }
        let root = make_root(&db_path);
        let mut state = OpenCodeDbCacheState::default();

        let first = refresh_db_messages_for_path(&mut state, &root);
        assert_eq!(first.len(), 3);
        assert_eq!(state.assistant_row_count, 3);
        assert_eq!(state.last_rowid, 3);

        // 删除中间行：count 变小但 max_rowid / max_time 保持不变 → 仍判定为回滚
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute("DELETE FROM message WHERE id = 'm2'", [])
                .unwrap();
        }

        let second = refresh_db_messages_for_path(&mut state, &root);
        assert_eq!(
            message_keys(&second),
            vec![
                "opencode::test::sess_1|m1".to_string(),
                "opencode::test::sess_1|m3".to_string(),
            ]
        );
        assert_eq!(state.assistant_row_count, 2);
        assert_eq!(state.last_rowid, 3);
    }

    #[test]
    fn refresh_db_schema_fingerprint_change_triggers_full_reconcile() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("opencode.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            create_message_schema(&conn);
            insert_message(&conn, "m1", "sess_1", 1_700_000_000_000, 10, 2);
        }
        let root = make_root(&db_path);
        let mut state = OpenCodeDbCacheState::default();

        refresh_db_messages_for_path(&mut state, &root);
        let schema_before = state.schema_fingerprint;
        assert_ne!(schema_before, 0);

        // 修改 schema（加列）→ schema fingerprint 变化 → 全量 reconcile
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch("ALTER TABLE message ADD COLUMN extra INTEGER;")
                .unwrap();
        }

        let second = refresh_db_messages_for_path(&mut state, &root);
        assert_eq!(second.len(), 1);
        assert!(second.contains_key("opencode::test::sess_1|m1"));
        assert_ne!(state.schema_fingerprint, schema_before);
        // 全量 reconcile 后刷新了上次全量时间
        assert!(state.last_full_reconcile_at_ms > 0);
    }

    #[test]
    fn refresh_db_missing_db_clears_cached_state() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("opencode.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            create_message_schema(&conn);
            insert_message(&conn, "m1", "sess_1", 1_700_000_000_000, 10, 2);
        }
        let root = make_root(&db_path);
        let mut state = OpenCodeDbCacheState::default();

        let first = refresh_db_messages_for_path(&mut state, &root);
        assert_eq!(first.len(), 1);

        fs::remove_file(&db_path).unwrap();
        let _ = fs::remove_file(db_path.with_extension("db-wal"));
        let _ = fs::remove_file(db_path.with_extension("db-shm"));

        let second = refresh_db_messages_for_path(&mut state, &root);
        assert!(second.is_empty());
        assert!(state.messages.is_empty());
        assert_eq!(state.storage_signature_hash, 0);
        assert_eq!(state.schema_fingerprint, 0);
        assert_eq!(state.assistant_row_count, 0);
        assert_eq!(state.last_time_updated_ms, 0);
        assert_eq!(state.last_rowid, 0);
        assert_eq!(state.schema_mode, OpenCodeSchemaMode::Incompatible);
    }

    #[test]
    fn refresh_db_incremental_cursor_picks_same_time_higher_rowid() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("opencode.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            create_message_schema(&conn);
            insert_message(&conn, "m1", "sess_1", 1_700_000_000_000, 10, 2);
        }
        let root = make_root(&db_path);
        let mut state = OpenCodeDbCacheState::default();

        let first = refresh_db_messages_for_path(&mut state, &root);
        assert_eq!(first.len(), 1);
        assert_eq!(state.last_time_updated_ms, 1_700_000_000_000);
        assert_eq!(state.last_rowid, 1);

        // 追加一条 time_updated 相同但 rowid 更大的消息 → 增量游标必须取到
        {
            let conn = Connection::open(&db_path).unwrap();
            insert_message(&conn, "m2", "sess_1", 1_700_000_000_000, 5, 3);
        }

        let second = refresh_db_messages_for_path(&mut state, &root);
        assert!(
            second.contains_key("opencode::test::sess_1|m2"),
            "incremental cursor must include equal time_updated with larger rowid"
        );
        assert!(second.contains_key("opencode::test::sess_1|m1"));
        assert_eq!(state.last_rowid, 2);
        assert_eq!(state.assistant_row_count, 2);
    }

    #[test]
    fn refresh_db_reads_v2_session_messages_and_compaction_cost() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("opencode.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE session_v2 (id TEXT PRIMARY KEY, directory TEXT NOT NULL);
            CREATE TABLE session_message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                type TEXT NOT NULL,
                seq INTEGER NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            INSERT INTO session_v2 (id, directory) VALUES ('ses_v2', '/tmp/v2');
            INSERT INTO session_message
                (id, session_id, type, seq, time_created, time_updated, data)
            VALUES
                ('msg_v2', 'ses_v2', 'assistant', 1, 1700000001000, 1700000009000,
                 '{"id":"msg_v2","model":{"id":"claude-sonnet","providerID":"anthropic"},"time":{"created":1700000002000,"completed":1700000009000},"tokens":{"input":10,"output":2,"reasoning":1,"cache":{"read":3,"write":4}},"cost":0.25}'),
                ('cmp_v2', 'ses_v2', 'compaction', 2, 1700000010000, 1700000011000,
                 '{"id":"cmp_v2","model":{"id":"claude-sonnet","providerID":"anthropic"},"time":{"created":1700000010000},"cost":0.5}');
            "#,
        )
        .unwrap();

        let root = make_root(&db_path);
        let mut state = OpenCodeDbCacheState::default();
        let messages = refresh_db_messages_for_path(&mut state, &root);

        assert_eq!(messages.len(), 2);
        let assistant = messages
            .get("opencode::test::ses_v2|msg_v2")
            .expect("v2 assistant message");
        assert_eq!(assistant.timestamp_sec, 1_700_000_002);
        assert_eq!(assistant.model, "claude-sonnet");
        assert_eq!(assistant.total_tokens, 20);
        assert_eq!(assistant.explicit_cost, Some(0.25));
        let compaction = messages
            .get("opencode::test::ses_v2|cmp_v2")
            .expect("v2 compaction message");
        assert_eq!(compaction.explicit_cost, Some(0.5));
        assert_eq!(state.assistant_row_count, 2);
    }

    #[test]
    fn refresh_db_keeps_legacy_and_v2_rows_in_transitional_database() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("opencode.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE message (
                id TEXT,
                session_id TEXT,
                time_updated INTEGER,
                data TEXT
            );
            CREATE TABLE session_message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                type TEXT NOT NULL,
                seq INTEGER NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            INSERT INTO message (id, session_id, time_updated, data)
            VALUES ('legacy', 'ses_old', 1700000000000,
                    '{"role":"assistant","modelID":"gpt-4o","time":{"created":1700000000000},"tokens":{"input":1,"output":1}}');
            INSERT INTO session_message
                (id, session_id, type, seq, time_created, time_updated, data)
            VALUES ('new', 'ses_new', 'assistant', 1, 1700000010000, 1700000010000,
                    '{"model":{"id":"gpt-5","providerID":"openai"},"time":{"created":1700000010000},"tokens":{"input":2,"output":3}}');
            "#,
        )
        .unwrap();

        let root = make_root(&db_path);
        let mut state = OpenCodeDbCacheState::default();
        let messages = refresh_db_messages_for_path(&mut state, &root);
        assert!(messages.contains_key("opencode::test::ses_old|legacy"));
        assert!(messages.contains_key("opencode::test::ses_new|new"));
        assert_eq!(messages.len(), 2);
    }
}
