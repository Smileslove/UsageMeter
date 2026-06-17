use crate::session::opencode_reader::{
    OpenCodeDbCacheState, OpenCodeDbCheckpoint, OpenCodeMessageSnapshot, OpenCodePathSignature,
    OpenCodeSchemaMode, OpenCodeStorageRoot, OpenCodeStorageSignature,
    OPENCODE_DB_FULL_RECONCILE_INTERVAL_SECS, REQUIRED_MESSAGE_COLUMNS, REQUIRED_SESSION_COLUMNS,
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

fn query_db_message_rows(
    conn: &Connection,
    last_time_updated_ms: Option<i64>,
    last_rowid: Option<i64>,
) -> Vec<DbMessageRow> {
    // LEFT JOIN session to filter out fork-replayed messages: a forked session copies historical
    // messages with their original time_created (earlier than the fork session's time_created).
    // Any message.time_created < session.time_created is a replayed copy and must be excluded.
    let (sql, params_vec): (&str, Vec<i64>) = if let (Some(last_time), Some(last_rowid)) =
        (last_time_updated_ms, last_rowid)
    {
        (
            "SELECT m.rowid, m.id, m.session_id, COALESCE(m.time_updated, 0), m.data
             FROM message m
             LEFT JOIN session s ON s.id = m.session_id
             WHERE json_extract(m.data, '$.role') = 'assistant'
               AND (s.time_created IS NULL OR m.time_created >= s.time_created)
               AND (COALESCE(m.time_updated, 0) > ?1 OR (COALESCE(m.time_updated, 0) = ?1 AND m.rowid > ?2))
             ORDER BY COALESCE(m.time_updated, 0) ASC, m.rowid ASC",
            vec![last_time, last_rowid],
        )
    } else {
        (
            "SELECT m.rowid, m.id, m.session_id, COALESCE(m.time_updated, 0), m.data
             FROM message m
             LEFT JOIN session s ON s.id = m.session_id
             WHERE json_extract(m.data, '$.role') = 'assistant'
               AND (s.time_created IS NULL OR m.time_created >= s.time_created)
             ORDER BY COALESCE(m.time_updated, 0) ASC, m.rowid ASC",
            Vec::new(),
        )
    };

    let mut stmt = match conn.prepare(sql) {
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
    let values: rusqlite::Result<(i64, i64, i64)> = conn.query_row(
        "SELECT
            COUNT(*),
            COALESCE(MAX(rowid), 0),
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
    );
    if let Ok((count, max_rowid, max_time)) = values {
        checkpoint.assistant_row_count = count.max(0) as u64;
        checkpoint.max_rowid = max_rowid.max(0);
        checkpoint.max_time_updated_ms = max_time.max(0);
    }
    checkpoint
}

fn compute_schema_fingerprint(conn: &Connection) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut stmt = match conn.prepare(
        "SELECT name, COALESCE(sql, '')
         FROM sqlite_schema
         WHERE type = 'table' AND name IN ('message', 'session')
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
