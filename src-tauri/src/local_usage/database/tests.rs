use super::*;
use crate::models::ToolFilter;
use crate::unified_usage;
use crate::unified_usage::{CoverageOrigin, MergedRequestFact};
use chrono::TimeZone;
use rusqlite::{params, Connection};
use std::fs;
use std::sync::{Mutex, MutexGuard, OnceLock};

fn opencode_test_guard() -> MutexGuard<'static, ()> {
    static OPENCODE_TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
    OPENCODE_TEST_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap()
}

fn temp_db() -> (tempfile::TempDir, LocalUsageDatabase) {
    let tmpdir = tempfile::tempdir().expect("create temp dir");
    let path = tmpdir.path().join("local_usage.db");
    let db = LocalUsageDatabase::new_with_path(&path).expect("open temp db");
    (tmpdir, db)
}

fn insert_request_fact(
    db: &LocalUsageDatabase,
    session_id: &str,
    message_id: &str,
    source_file_path: &str,
    present: bool,
    created_at: i64,
) {
    let conn = db.conn.lock().unwrap();
    let dedupe_key = format!("{}:{}", session_id, message_id);
    let request_id = format!("claude_code:{}", dedupe_key);
    let request_key = format!("claude_code:{}", message_id);
    conn.execute(
        "INSERT INTO local_request_facts (
            request_id, session_id, tool, project_key, timestamp, message_id, dedupe_key,
            request_key, model, input_tokens, output_tokens, cache_create_tokens,
            cache_read_tokens, total_tokens, source_file_path, source_file_present,
            created_at, raw_event_kind, sync_version, is_subagent
         ) VALUES (?1, ?2, 'claude_code', 'p', ?3, ?4, ?5, ?6, 'claude-3', 10, 20, 0, 0, 30,
                   ?7, ?8, ?9, 'request', 1, 0)",
        params![
            request_id,
            session_id,
            created_at,
            message_id,
            dedupe_key,
            request_key,
            source_file_path,
            if present { 1 } else { 0 },
            created_at
        ],
    )
    .expect("insert fact");
}

fn insert_source_file(
    db: &LocalUsageDatabase,
    session_id: &str,
    file_path: &str,
    deleted_at: Option<i64>,
) {
    let conn = db.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO local_source_files (
            tool, session_id, project_key, file_path, file_role, file_size,
            mtime_epoch, fingerprint, last_scanned_at, last_synced_at,
            sync_status, deleted_at, deletion_reason
         ) VALUES ('claude_code', ?1, 'p', ?2, 'session_group', 100, 0, 'fp', 0, 0,
                   'ready', ?3, ?4)",
        params![
            session_id,
            file_path,
            deleted_at,
            deleted_at.map(|_| "missing"),
        ],
    )
    .expect("insert source file");
}

#[test]
fn opencode_db_checkpoint_persists_across_reopen() {
    let _guard = opencode_test_guard();
    let tmpdir = tempfile::tempdir().expect("create temp dir");
    let data_home = tmpdir.path().join(".local").join("share");
    let opencode_home = data_home.join("opencode");
    fs::create_dir_all(&opencode_home).expect("create opencode home");

    let db_path = opencode_home.join("opencode.db");
    let conn = Connection::open(&db_path).expect("open opencode db");
    conn.execute_batch(
        "
        CREATE TABLE session (
          id TEXT PRIMARY KEY,
          directory TEXT,
          title TEXT,
          model TEXT,
          tokens_input INTEGER,
          tokens_output INTEGER,
          tokens_reasoning INTEGER,
          tokens_cache_read INTEGER,
          tokens_cache_write INTEGER,
          time_created INTEGER,
          time_updated INTEGER,
          time_archived INTEGER
        );
        CREATE TABLE message (
          id TEXT,
          session_id TEXT,
          time_updated INTEGER,
          data TEXT
        );
        INSERT INTO session (
          id, directory, title, model, tokens_input, tokens_output, tokens_reasoning,
          tokens_cache_read, tokens_cache_write, time_created, time_updated, time_archived
        ) VALUES (
          'sess_1', '/tmp/project', 'OpenCode', '{\"id\":\"gpt-4o\"}', 0, 0, 0, 0, 0, 1700000000000, 1700000005000, NULL
        );
        ",
    )
    .expect("create schema");
    conn.execute(
        "INSERT INTO message (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
        params![
            "msg_1",
            "sess_1",
            1700000005000_i64,
            serde_json::json!({
                "id": "msg_1",
                "sessionID": "sess_1",
                "modelID": "gpt-4o",
                "path": { "cwd": "/tmp/project" },
                "role": "assistant",
                "time": { "created": 1700000000000_i64, "completed": 1700000005000_i64 },
                "tokens": { "input": 10, "output": 2, "reasoning": 1, "cache": { "read": 0, "write": 0 } }
            })
            .to_string()
        ],
    )
    .expect("insert first message");
    drop(conn);

    let old_xdg_data_home = std::env::var_os("XDG_DATA_HOME");
    std::env::set_var("XDG_DATA_HOME", &data_home);

    let local_usage_path = tmpdir.path().join("local_usage.db");
    let db = LocalUsageDatabase::new_with_path(&local_usage_path).expect("open local usage db");
    db.sync_from_scanner().expect("sync once");

    let first_state = db.load_opencode_db_scan_state().expect("load scan state");
    assert!(first_state.last_rowid > 0);
    let first_states = db
        .load_opencode_db_scan_states()
        .expect("load v2 scan states");
    assert!(first_states.stores.contains_key("native"));

    let conn = Connection::open(&db_path).expect("reopen opencode db");
    conn.execute(
        "INSERT INTO message (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
        params![
            "msg_2",
            "sess_1",
            1700000010000_i64,
            serde_json::json!({
                "id": "msg_2",
                "sessionID": "sess_1",
                "model": "gpt-4o-mini",
                "path": { "cwd": "/tmp/project" },
                "role": "assistant",
                "time": { "created": 1700000009000_i64, "completed": 1700000010000_i64 },
                "tokens": { "input": 3, "output": 1, "reasoning": 0, "cache": { "read": 0, "write": 0 } }
            })
            .to_string()
        ],
    )
    .expect("insert second message");
    drop(conn);

    let reopened =
        LocalUsageDatabase::new_with_path(&local_usage_path).expect("reopen local usage db");
    reopened.sync_from_scanner().expect("sync twice");

    let second_state = reopened
        .load_opencode_db_scan_state()
        .expect("load scan state");
    assert!(second_state.last_rowid > first_state.last_rowid);

    let records = reopened
        .get_request_records_in_range(0, i64::MAX, &ToolFilter::Tool("opencode".to_string()))
        .expect("load opencode facts");
    assert_eq!(records.len(), 2);
    assert!(records
        .iter()
        .all(|record| record.session_id == "opencode::native::sess_1"));

    match old_xdg_data_home {
        Some(value) => std::env::set_var("XDG_DATA_HOME", value),
        None => std::env::remove_var("XDG_DATA_HOME"),
    }
}

#[test]
fn opencode_message_id_conflict_persists_conflict_state_and_composite_request_keys() {
    let _guard = opencode_test_guard();
    let tmpdir = tempfile::tempdir().expect("create temp dir");
    let data_home = tmpdir.path().join(".local").join("share");
    let opencode_home = data_home.join("opencode");
    fs::create_dir_all(&opencode_home).expect("create opencode home");

    let db_path = opencode_home.join("opencode.db");
    let conn = Connection::open(&db_path).expect("open opencode db");
    conn.execute_batch(
        "
        CREATE TABLE message (
          id TEXT,
          session_id TEXT,
          time_updated INTEGER,
          data TEXT
        );
        ",
    )
    .expect("create message schema");
    for session_id in ["sess_a", "sess_b"] {
        conn.execute(
            "INSERT INTO message (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
            params![
                "msg_dup",
                session_id,
                1700000010000_i64,
                serde_json::json!({
                    "id": "msg_dup",
                    "sessionID": session_id,
                    "modelID": "gpt-4o",
                    "path": { "cwd": format!("/tmp/{}", session_id) },
                    "role": "assistant",
                    "time": { "created": 1700000009000_i64, "completed": 1700000010000_i64 },
                    "tokens": { "input": 3, "output": 1, "reasoning": 0, "cache": { "read": 0, "write": 0 } }
                })
                .to_string()
            ],
        )
        .expect("insert duplicate message");
    }
    drop(conn);

    let old_xdg_data_home = std::env::var_os("XDG_DATA_HOME");
    std::env::set_var("XDG_DATA_HOME", &data_home);

    let local_usage_path = tmpdir.path().join("local_usage.db");
    let db = LocalUsageDatabase::new_with_path(&local_usage_path).expect("open local usage db");
    db.sync_from_scanner().expect("sync");

    let has_conflict = db
        .get_local_sync_state("opencode_message_id_conflict_has_conflict")
        .expect("read conflict state")
        .unwrap_or_default();
    assert_eq!(has_conflict, "1");

    let records = db
        .get_request_records_in_range(0, i64::MAX, &ToolFilter::Tool("opencode".to_string()))
        .expect("load opencode facts");
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| {
        record
            .request_key
            .as_deref()
            .map(|key| key.contains("|msg_dup"))
            .unwrap_or(false)
    }));

    match old_xdg_data_home {
        Some(value) => std::env::set_var("XDG_DATA_HOME", value),
        None => std::env::remove_var("XDG_DATA_HOME"),
    }
}

#[test]
fn opencode_db_message_rewrite_updates_existing_fact() {
    let _guard = opencode_test_guard();
    let tmpdir = tempfile::tempdir().expect("create temp dir");
    let data_home = tmpdir.path().join(".local").join("share");
    let opencode_home = data_home.join("opencode");
    fs::create_dir_all(&opencode_home).expect("create opencode home");

    let db_path = opencode_home.join("opencode.db");
    let conn = Connection::open(&db_path).expect("open opencode db");
    conn.execute_batch(
        "
        CREATE TABLE message (
          id TEXT,
          session_id TEXT,
          time_updated INTEGER,
          data TEXT
        );
        ",
    )
    .expect("create message schema");
    conn.execute(
        "INSERT INTO message (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
        params![
            "msg_rewrite",
            "sess_rewrite",
            1700000005000_i64,
            serde_json::json!({
                "id": "msg_rewrite",
                "sessionID": "sess_rewrite",
                "modelID": "gpt-4o",
                "path": { "cwd": "/tmp/project" },
                "role": "assistant",
                "time": { "created": 1700000000000_i64, "completed": 1700000005000_i64 },
                "tokens": { "input": 4, "output": 1, "reasoning": 0, "cache": { "read": 0, "write": 0 } }
            })
            .to_string()
        ],
    )
    .expect("insert first version");
    drop(conn);

    let old_xdg_data_home = std::env::var_os("XDG_DATA_HOME");
    std::env::set_var("XDG_DATA_HOME", &data_home);

    let local_usage_path = tmpdir.path().join("local_usage.db");
    let db = LocalUsageDatabase::new_with_path(&local_usage_path).expect("open local usage db");
    db.sync_from_scanner().expect("sync once");

    let conn = Connection::open(&db_path).expect("reopen opencode db");
    conn.execute(
        "UPDATE message SET time_updated = ?1, data = ?2 WHERE id = 'msg_rewrite'",
        params![
            1700000015000_i64,
            serde_json::json!({
                "id": "msg_rewrite",
                "sessionID": "sess_rewrite",
                "modelID": "gpt-4o",
                "path": { "cwd": "/tmp/project" },
                "role": "assistant",
                "time": { "created": 1700000000000_i64, "completed": 1700000015000_i64 },
                "tokens": { "input": 9, "output": 2, "reasoning": 1, "cache": { "read": 0, "write": 0 } }
            })
            .to_string()
        ],
    )
    .expect("rewrite message");
    drop(conn);

    let reopened =
        LocalUsageDatabase::new_with_path(&local_usage_path).expect("reopen local usage db");
    reopened.sync_from_scanner().expect("sync twice");

    let records = reopened
        .get_request_records_in_range(0, i64::MAX, &ToolFilter::Tool("opencode".to_string()))
        .expect("load opencode facts");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].input_tokens, 9);
    assert_eq!(records[0].output_tokens, 3);
    assert_eq!(records[0].total_tokens, 12);

    match old_xdg_data_home {
        Some(value) => std::env::set_var("XDG_DATA_HOME", value),
        None => std::env::remove_var("XDG_DATA_HOME"),
    }
}

#[test]
fn opencode_schema_mode_transition_persists_message_only_and_recovers_to_full() {
    let _guard = opencode_test_guard();
    let tmpdir = tempfile::tempdir().expect("create temp dir");
    let data_home = tmpdir.path().join(".local").join("share");
    let opencode_home = data_home.join("opencode");
    fs::create_dir_all(&opencode_home).expect("create opencode home");

    let db_path = opencode_home.join("opencode.db");
    let conn = Connection::open(&db_path).expect("open opencode db");
    conn.execute_batch(
        "
        CREATE TABLE message (
          id TEXT,
          session_id TEXT,
          time_updated INTEGER,
          data TEXT
        );
        ",
    )
    .expect("create message schema");
    conn.execute(
        "INSERT INTO message (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
        params![
            "msg_mode",
            "sess_mode",
            1700000005000_i64,
            serde_json::json!({
                "id": "msg_mode",
                "sessionID": "sess_mode",
                "modelID": "gpt-4o",
                "path": { "cwd": "/tmp/project" },
                "role": "assistant",
                "time": { "created": 1700000000000_i64, "completed": 1700000005000_i64 },
                "tokens": { "input": 4, "output": 1, "reasoning": 0, "cache": { "read": 0, "write": 0 } }
            })
            .to_string()
        ],
    )
    .expect("insert message");
    drop(conn);

    let old_xdg_data_home = std::env::var_os("XDG_DATA_HOME");
    std::env::set_var("XDG_DATA_HOME", &data_home);

    let local_usage_path = tmpdir.path().join("local_usage.db");
    let db = LocalUsageDatabase::new_with_path(&local_usage_path).expect("open local usage db");
    db.sync_from_scanner().expect("sync once");
    assert_eq!(
        db.get_local_sync_state("opencode_db_schema_mode")
            .expect("read schema mode"),
        Some("message_only".to_string())
    );

    let conn = Connection::open(&db_path).expect("reopen opencode db");
    conn.execute_batch(
        "
        CREATE TABLE session (
          id TEXT PRIMARY KEY,
          directory TEXT,
          title TEXT,
          model TEXT,
          tokens_input INTEGER,
          tokens_output INTEGER,
          tokens_reasoning INTEGER,
          tokens_cache_read INTEGER,
          tokens_cache_write INTEGER,
          time_created INTEGER,
          time_updated INTEGER,
          time_archived INTEGER
        );
        INSERT INTO session (
          id, directory, title, model, tokens_input, tokens_output, tokens_reasoning,
          tokens_cache_read, tokens_cache_write, time_created, time_updated, time_archived
        ) VALUES (
          'sess_mode', '/tmp/project', 'Recovered Session', '{\"id\":\"gpt-4o\"}', 0, 0, 0, 0, 0, 1700000000000, 1700000005000, NULL
        );
        UPDATE message SET time_updated = 1700000010000;
        ",
    )
    .expect("add session table");
    drop(conn);

    let reopened =
        LocalUsageDatabase::new_with_path(&local_usage_path).expect("reopen local usage db");
    reopened.sync_from_scanner().expect("sync twice");
    assert_eq!(
        reopened
            .get_local_sync_state("opencode_db_schema_mode")
            .expect("read schema mode"),
        Some("full".to_string())
    );

    match old_xdg_data_home {
        Some(value) => std::env::set_var("XDG_DATA_HOME", value),
        None => std::env::remove_var("XDG_DATA_HOME"),
    }
}

#[test]
fn opencode_schema_mode_transition_persists_full_to_message_only() {
    let _guard = opencode_test_guard();
    let tmpdir = tempfile::tempdir().expect("create temp dir");
    let data_home = tmpdir.path().join(".local").join("share");
    let opencode_home = data_home.join("opencode");
    fs::create_dir_all(&opencode_home).expect("create opencode home");

    let db_path = opencode_home.join("opencode.db");
    let conn = Connection::open(&db_path).expect("open opencode db");
    conn.execute_batch(
        "
        CREATE TABLE session (
          id TEXT PRIMARY KEY,
          directory TEXT,
          title TEXT,
          model TEXT,
          tokens_input INTEGER,
          tokens_output INTEGER,
          tokens_reasoning INTEGER,
          tokens_cache_read INTEGER,
          tokens_cache_write INTEGER,
          time_created INTEGER,
          time_updated INTEGER,
          time_archived INTEGER
        );
        CREATE TABLE message (
          id TEXT,
          session_id TEXT,
          time_updated INTEGER,
          data TEXT
        );
        INSERT INTO session (
          id, directory, title, model, tokens_input, tokens_output, tokens_reasoning,
          tokens_cache_read, tokens_cache_write, time_created, time_updated, time_archived
        ) VALUES (
          'sess_mode', '/tmp/project', 'Full Session', '{\"id\":\"gpt-4o\"}', 0, 0, 0, 0, 0, 1700000000000, 1700000005000, NULL
        );
        ",
    )
    .expect("create schema");
    conn.execute(
        "INSERT INTO message (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
        params![
            "msg_mode",
            "sess_mode",
            1700000005000_i64,
            serde_json::json!({
                "id": "msg_mode",
                "sessionID": "sess_mode",
                "modelID": "gpt-4o",
                "path": { "cwd": "/tmp/project" },
                "role": "assistant",
                "time": { "created": 1700000000000_i64, "completed": 1700000005000_i64 },
                "tokens": { "input": 4, "output": 1, "reasoning": 0, "cache": { "read": 0, "write": 0 } }
            })
            .to_string()
        ],
    )
    .expect("insert message");
    drop(conn);

    let old_xdg_data_home = std::env::var_os("XDG_DATA_HOME");
    std::env::set_var("XDG_DATA_HOME", &data_home);

    let local_usage_path = tmpdir.path().join("local_usage.db");
    let db = LocalUsageDatabase::new_with_path(&local_usage_path).expect("open local usage db");
    db.sync_from_scanner().expect("sync once");
    assert_eq!(
        db.get_local_sync_state("opencode_db_schema_mode")
            .expect("read schema mode"),
        Some("full".to_string())
    );

    let conn = Connection::open(&db_path).expect("reopen opencode db");
    conn.execute_batch(
        "
        ALTER TABLE session RENAME TO session_old;
        CREATE TABLE session (
          id TEXT PRIMARY KEY,
          directory TEXT,
          title TEXT,
          tokens_input INTEGER,
          tokens_output INTEGER,
          tokens_cache_read INTEGER,
          tokens_cache_write INTEGER,
          time_created INTEGER,
          time_updated INTEGER
        );
        INSERT INTO session (id, directory, title, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, time_created, time_updated)
        SELECT id, directory, title, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, time_created, time_updated
        FROM session_old;
        DROP TABLE session_old;
        UPDATE message SET time_updated = 1700000010000;
        ",
    )
    .expect("degrade schema");
    drop(conn);

    let reopened =
        LocalUsageDatabase::new_with_path(&local_usage_path).expect("reopen local usage db");
    reopened.sync_from_scanner().expect("sync twice");
    assert_eq!(
        reopened
            .get_local_sync_state("opencode_db_schema_mode")
            .expect("read schema mode"),
        Some("message_only".to_string())
    );

    match old_xdg_data_home {
        Some(value) => std::env::set_var("XDG_DATA_HOME", value),
        None => std::env::remove_var("XDG_DATA_HOME"),
    }
}

#[test]
fn migration_creates_v5_columns() {
    let (_tmp, db) = temp_db();
    let conn = db.conn.lock().unwrap();
    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(local_request_facts)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert!(cols.contains(&"request_key".to_string()), "{:?}", cols);
    assert!(
        cols.contains(&"source_file_present".to_string()),
        "{:?}",
        cols
    );
    assert!(cols.contains(&"source_file_path".to_string()), "{:?}", cols);

    let scols: Vec<String> = conn
        .prepare("PRAGMA table_info(local_source_files)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert!(scols.contains(&"deleted_at".to_string()), "{:?}", scols);
    assert!(
        scols.contains(&"deletion_reason".to_string()),
        "{:?}",
        scols
    );

    let indexes: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='index'")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    for required in [
        "idx_local_request_facts_request_key",
        "idx_local_request_facts_source_file_present",
        "idx_local_source_files_deleted_at",
    ] {
        assert!(
            indexes.iter().any(|n| n == required),
            "missing v5 index {}: {:?}",
            required,
            indexes
        );
    }
}

#[test]
fn open_v4_db_upgrades_to_v5_without_error() {
    let tmpdir = tempfile::tempdir().expect("create temp dir");
    let path = tmpdir.path().join("legacy.db");

    {
        let conn = Connection::open(&path).expect("open legacy db");
        conn.execute_batch(
            r#"
            CREATE TABLE local_sync_state (
                state_key TEXT PRIMARY KEY,
                state_value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE local_source_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tool TEXT NOT NULL,
                session_id TEXT NOT NULL,
                project_key TEXT,
                file_path TEXT NOT NULL UNIQUE,
                file_role TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                mtime_epoch INTEGER NOT NULL,
                fingerprint TEXT NOT NULL,
                last_scanned_at INTEGER NOT NULL,
                last_synced_at INTEGER,
                sync_status TEXT NOT NULL DEFAULT 'ready',
                sync_error TEXT
            );
            CREATE TABLE local_sessions (
                session_id TEXT PRIMARY KEY,
                tool TEXT NOT NULL,
                project_key TEXT,
                cwd TEXT,
                project_name TEXT,
                topic TEXT,
                last_prompt TEXT,
                session_name TEXT,
                primary_file_path TEXT,
                file_size INTEGER NOT NULL DEFAULT 0,
                last_modified INTEGER NOT NULL DEFAULT 0,
                start_time INTEGER NOT NULL DEFAULT 0,
                end_time INTEGER NOT NULL DEFAULT 0,
                request_count INTEGER NOT NULL DEFAULT 0,
                total_input_tokens INTEGER NOT NULL DEFAULT 0,
                total_output_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                model_list_json TEXT NOT NULL DEFAULT '[]',
                source_kind TEXT NOT NULL DEFAULT 'local_transcript',
                sync_version INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE local_request_facts (
                request_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                tool TEXT NOT NULL,
                project_key TEXT,
                timestamp INTEGER NOT NULL,
                message_id TEXT,
                dedupe_key TEXT NOT NULL,
                model TEXT NOT NULL DEFAULT '',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                source_file_id INTEGER,
                source_offset INTEGER,
                event_index INTEGER,
                is_subagent INTEGER NOT NULL DEFAULT 0,
                raw_event_kind TEXT NOT NULL DEFAULT '',
                sync_version INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                UNIQUE(tool, dedupe_key)
            );
            "#,
        )
        .expect("create legacy v4 tables");
        conn.execute(
            "INSERT INTO local_request_facts (
                request_id, session_id, tool, dedupe_key, timestamp, message_id,
                model, input_tokens, output_tokens, total_tokens, created_at
            ) VALUES ('rid', 'sess-x', 'claude_code', 'sess-x:msg-legacy', 1700000000,
                      'msg-legacy', 'm', 10, 20, 30, 1700000000)",
            [],
        )
        .expect("insert legacy fact");
        conn.execute(
            "INSERT INTO local_sync_state (state_key, state_value, updated_at)
             VALUES ('schema_version', '4', 1700000000)",
            [],
        )
        .expect("set schema_version=4");
    }

    let db = LocalUsageDatabase::new_with_path(&path)
        .expect("open legacy db should trigger v5 migration without error");

    let conn = db.conn.lock().unwrap();
    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(local_request_facts)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    for required in ["request_key", "source_file_present", "source_file_path"] {
        assert!(
            cols.contains(&required.to_string()),
            "migration missed column {}: {:?}",
            required,
            cols
        );
    }

    let request_key: String = conn
        .query_row(
            "SELECT request_key FROM local_request_facts WHERE request_id = 'rid'",
            [],
            |row| row.get(0),
        )
        .expect("read backfilled request_key");
    assert_eq!(request_key, "claude_code:msg-legacy");
}

#[test]
fn count_orphan_facts_filters_by_source_present() {
    let (_tmp, db) = temp_db();
    insert_request_fact(&db, "sess-a", "msg-1", "/tmp/a.jsonl", true, 100);
    insert_request_fact(&db, "sess-a", "msg-2", "/tmp/a.jsonl", false, 100);
    insert_request_fact(&db, "sess-b", "msg-3", "/tmp/b.jsonl", false, 200);

    let total = db.count_local_request_facts().unwrap();
    let orphan = db.count_orphan_local_facts().unwrap();
    assert_eq!(total, 3);
    assert_eq!(orphan, 2);
}

#[test]
fn purge_orphan_respects_cutoff_seconds() {
    let (_tmp, db) = temp_db();
    let now = chrono::Utc::now().timestamp();
    insert_request_fact(
        &db,
        "sess-old",
        "msg-old",
        "/tmp/old.jsonl",
        false,
        now - 86400 * 30,
    );
    insert_request_fact(
        &db,
        "sess-new",
        "msg-new",
        "/tmp/new.jsonl",
        false,
        now - 60,
    );
    insert_request_fact(
        &db,
        "sess-alive",
        "msg-alive",
        "/tmp/alive.jsonl",
        true,
        now - 86400 * 30,
    );

    let removed = db.purge_orphan_facts(86400 * 7).unwrap();
    assert_eq!(removed, 1);

    let total = db.count_local_request_facts().unwrap();
    assert_eq!(total, 2, "msg-new 与 msg-alive 应保留");

    let removed_2 = db.purge_orphan_facts(0).unwrap();
    assert_eq!(removed_2, 1);

    let orphan = db.count_orphan_local_facts().unwrap();
    assert_eq!(orphan, 0);
    let total = db.count_local_request_facts().unwrap();
    assert_eq!(total, 1, "仅 msg-alive 应保留");
}

#[test]
fn purge_orphan_cleans_sessions_and_source_files_with_no_references() {
    let (_tmp, db) = temp_db();
    let now = chrono::Utc::now().timestamp();
    insert_source_file(&db, "sess-vanished", "/tmp/v.jsonl", Some(now - 86400));
    insert_request_fact(
        &db,
        "sess-vanished",
        "msg-x",
        "/tmp/v.jsonl",
        false,
        now - 86400 * 100,
    );

    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO local_sessions (
                session_id, tool, project_key, updated_at
             ) VALUES ('sess-vanished', 'claude_code', 'p', ?1)",
            params![now - 86400],
        )
        .unwrap();
    }

    let removed = db.purge_orphan_facts(0).unwrap();
    assert_eq!(removed, 1);

    let conn = db.conn.lock().unwrap();
    let session_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM local_sessions", [], |r| r.get(0))
        .unwrap();
    let source_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM local_source_files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(session_count, 0, "无引用的 session 应被清除");
    assert_eq!(source_count, 0, "无引用的 source 软删行应被清除");
}

#[test]
fn truncate_all_clears_local_tables() {
    let (_tmp, db) = temp_db();
    let now = chrono::Utc::now().timestamp();
    insert_source_file(&db, "sess-a", "/tmp/a.jsonl", None);
    insert_request_fact(&db, "sess-a", "msg-1", "/tmp/a.jsonl", true, now);

    db.truncate_all_local_facts().unwrap();
    assert_eq!(db.count_local_request_facts().unwrap(), 0);
    let conn = db.conn.lock().unwrap();
    let source_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM local_source_files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(source_count, 0);
}

#[test]
fn get_all_request_records_returns_persisted_request_key() {
    let (_tmp, db) = temp_db();
    insert_request_fact(&db, "sess-a", "msg-1", "/tmp/a.jsonl", true, 100);
    let records = db
        .get_request_records_in_range(0, i64::MAX, &ToolFilter::All)
        .unwrap();
    assert_eq!(records.len(), 1);
    let key = records[0]
        .request_key
        .as_deref()
        .expect("request_key 应该被读出来");
    assert_eq!(key, "claude_code:msg-1");
    assert_eq!(records[0].source_file_present, Some(true));
}

#[test]
fn soft_deleted_facts_do_not_disappear_from_query() {
    let (_tmp, db) = temp_db();
    insert_request_fact(&db, "sess-a", "msg-alive", "/tmp/a.jsonl", true, 100);
    insert_request_fact(&db, "sess-a", "msg-vanished", "/tmp/a.jsonl", false, 100);

    let records = db
        .get_request_records_in_range(0, i64::MAX, &ToolFilter::All)
        .unwrap();
    assert_eq!(records.len(), 2);
    let presents: Vec<_> = records
        .iter()
        .map(|r| (r.message_id.clone(), r.source_file_present))
        .collect();
    assert!(presents.contains(&("msg-alive".to_string(), Some(true))));
    assert!(presents.contains(&("msg-vanished".to_string(), Some(false))));
}

#[test]
fn unified_materialized_facts_round_trip() {
    let (_tmp, db) = temp_db();
    let local_date = "2026-05-26".to_string();
    let fact = MergedRequestFact {
        canonical_request_key: "claude_code:msg-1".to_string(),
        session_id: "sess-1".to_string(),
        project_name: Some("Project".to_string()),
        project_path: Some("/tmp/project".to_string()),
        api_key_prefix: Some("sk-ant-1234".to_string()),
        request_base_url: Some("https://api.anthropic.com".to_string()),
        tool: "claude_code".to_string(),
        timestamp_sec: 1_779_811_200,
        timestamp_ms: 1_779_811_200_123,
        model: "claude-sonnet-4".to_string(),
        input_tokens: 10,
        output_tokens: 20,
        cache_create_tokens: 3,
        cache_read_tokens: 4,
        total_tokens: 37,
        estimated_cost: 1.2345,
        coverage_origin: CoverageOrigin::MergedProxyPreferred,
        status_code: Some(200),
        duration_ms: Some(1500),
        output_tokens_per_second: Some(12.5),
        ttft_ms: Some(300),
        source_label: Some("sk-ant-1234".to_string()),
    };
    let state = UnifiedDayMaterializationState {
        local_date: local_date.clone(),
        day_boundary_mode: "standard".to_string(),
        fact_count: 1,
        local_request_count: 1,
        local_max_sync_version: 7,
        local_max_timestamp: fact.timestamp_sec,
        remote_request_count: 0,
        remote_max_export_seq: 0,
        remote_max_timestamp: 0,
        proxy_record_count: 1,
        proxy_all_record_count: 1,
        proxy_max_timestamp_ms: fact.timestamp_ms,
        proxy_max_updated_at: 555,
        max_fact_timestamp_ms: fact.timestamp_ms,
        pricing_fingerprint: 42,
        is_finalized: true,
        finalized_at: Some(123456789),
        materialized_at: 123456790,
    };

    db.replace_unified_day_materialization(
        &local_date,
        &[(String::from("claude_code:msg-1"), fact.clone())],
        &state,
    )
    .expect("store materialized facts");

    let loaded_state = db
        .get_unified_day_materialization_state(&local_date)
        .expect("load state")
        .expect("state exists");
    assert_eq!(loaded_state, state);
    assert_eq!(loaded_state.day_boundary_mode, "standard");

    let loaded = db
        .get_unified_facts_for_dates(std::slice::from_ref(&local_date), &ToolFilter::All)
        .expect("load facts");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].session_id, fact.session_id);
    assert_eq!(loaded[0].project_name, fact.project_name);
    assert_eq!(loaded[0].request_base_url, fact.request_base_url);
    assert_eq!(loaded[0].coverage_origin, fact.coverage_origin);
    assert_eq!(loaded[0].status_code, fact.status_code);
    assert_eq!(
        loaded[0].output_tokens_per_second,
        fact.output_tokens_per_second
    );

    let summaries = db
        .get_unified_daily_summaries_between("2026-05-26", "2026-05-27")
        .expect("load summaries");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].local_date, local_date);
    assert_eq!(summaries[0].request_count, 1);
    assert_eq!(summaries[0].total_tokens, 37);
    assert_eq!(summaries[0].success_request_count, 1);
    assert_eq!(summaries[0].model_count, 1);
    assert_eq!(summaries[0].success_model_count, 1);
}

#[test]
fn unified_materialization_state_persists_day_boundary_mode() {
    let (_tmp, db) = temp_db();
    let local_date = "2026-05-27".to_string();
    let fact = MergedRequestFact {
        canonical_request_key: "claude_code:msg-night".to_string(),
        session_id: "sess-night".to_string(),
        project_name: None,
        project_path: None,
        api_key_prefix: None,
        request_base_url: None,
        tool: "claude_code".to_string(),
        timestamp_sec: 1_779_897_600,
        timestamp_ms: 1_779_897_600_123,
        model: "claude-sonnet-4".to_string(),
        input_tokens: 8,
        output_tokens: 12,
        cache_create_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 20,
        estimated_cost: 0.8,
        coverage_origin: CoverageOrigin::LocalOnly,
        status_code: Some(200),
        duration_ms: None,
        output_tokens_per_second: None,
        ttft_ms: None,
        source_label: None,
    };

    db.replace_unified_day_materialization(
        &local_date,
        &[(String::from("claude_code:msg-night"), fact)],
        &UnifiedDayMaterializationState {
            local_date: local_date.clone(),
            day_boundary_mode: "night_owl".to_string(),
            fact_count: 1,
            local_request_count: 1,
            local_max_sync_version: 1,
            local_max_timestamp: 1_779_897_600,
            remote_request_count: 0,
            remote_max_export_seq: 0,
            remote_max_timestamp: 0,
            proxy_record_count: 0,
            proxy_all_record_count: 0,
            proxy_max_timestamp_ms: 0,
            proxy_max_updated_at: 0,
            max_fact_timestamp_ms: 1_779_897_600_123,
            pricing_fingerprint: 7,
            is_finalized: true,
            finalized_at: Some(300),
            materialized_at: 300,
        },
    )
    .unwrap();

    let state = db
        .get_unified_day_materialization_state(&local_date)
        .unwrap()
        .unwrap();
    assert_eq!(state.day_boundary_mode, "night_owl");
}

#[test]
fn unified_day_local_snapshot_with_settings_uses_passed_day_boundary_mode() {
    let (_tmp, db) = temp_db();
    let tmp_home = tempfile::tempdir().expect("create temp home");
    let old_home = std::env::var_os("HOME");
    std::env::set_var("HOME", tmp_home.path());

    let settings_dir = tmp_home.path().join(".usagemeter");
    fs::create_dir_all(&settings_dir).expect("create settings dir");
    fs::write(
        settings_dir.join("settings.json"),
        serde_json::json!({
            "dayBoundaryMode": "night_owl"
        })
        .to_string(),
    )
    .expect("write settings");

    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO local_request_facts (
                request_id, session_id, tool, project_key, timestamp, message_id, dedupe_key,
                request_key, model, input_tokens, output_tokens, cache_create_tokens,
                cache_read_tokens, total_tokens, source_file_path, source_file_present,
                created_at, raw_event_kind, sync_version, is_subagent
             ) VALUES (
                'rid-1', 'sess-1', 'claude_code', 'p', ?1, 'msg-1', 'sess-1:msg-1',
                'claude_code:msg-1', 'claude-3', 1, 2, 0, 0, 3, '/tmp/a.jsonl', 1,
                ?1, 'request', 1, 0
             )",
            params![1_779_818_400_i64],
        )
        .expect("insert midnight fact");
    }

    let mut standard_settings = AppSettings::default();
    standard_settings.day_boundary_mode = "standard".to_string();

    let standard_snapshot = db
        .get_unified_day_local_snapshot_with_settings("2026-05-27", &standard_settings)
        .expect("load standard snapshot");
    let night_owl_snapshot = db
        .get_unified_day_local_snapshot("2026-05-27")
        .expect("load global night owl snapshot");

    assert_eq!(standard_snapshot.local_request_count, 1);
    assert_eq!(night_owl_snapshot.local_request_count, 0);

    match old_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
fn v13_migration_clears_runtime_merge_cache() {
    let (_tmp, db) = temp_db();
    unified_usage::clear_runtime_caches();
    unified_usage::seed_runtime_merge_cache_for_test();
    assert_eq!(unified_usage::runtime_merge_cache_len_for_test(), 1);

    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE local_sync_state SET state_value = '12' WHERE state_key = 'schema_version'",
            [],
        )
        .expect("degrade schema version");
    }

    let reopened = LocalUsageDatabase::new_with_path(&_tmp.path().join("local_usage.db"))
        .expect("reopen and migrate");
    drop(reopened);
    assert_eq!(unified_usage::runtime_merge_cache_len_for_test(), 0);
}

#[test]
fn today_local_date_with_settings_uses_passed_day_boundary_mode() {
    let tmp_home = tempfile::tempdir().expect("create temp home");
    let old_home = std::env::var_os("HOME");
    std::env::set_var("HOME", tmp_home.path());

    let settings_dir = tmp_home.path().join(".usagemeter");
    fs::create_dir_all(&settings_dir).expect("create settings dir");
    fs::write(
        settings_dir.join("settings.json"),
        serde_json::json!({
            "dayBoundaryMode": "night_owl"
        })
        .to_string(),
    )
    .expect("write settings");

    let mut standard_settings = AppSettings::default();
    standard_settings.day_boundary_mode = "standard".to_string();

    let standard_today = LocalUsageDatabase::today_local_date_with_settings(&standard_settings);
    let global_today = LocalUsageDatabase::today_local_date();

    if global_today != standard_today {
        assert_ne!(global_today, standard_today);
    }

    match old_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
fn collect_history_dates_for_session_uses_sql_business_day_bucketing() {
    let (_tmp, db) = temp_db();
    let mut settings = AppSettings::default();
    settings.day_boundary_mode = "night_owl".to_string();

    let late_night_ts = chrono::Local
        .with_ymd_and_hms(2026, 6, 12, 1, 30, 0)
        .single()
        .expect("build late night ts")
        .timestamp();
    let morning_ts = chrono::Local
        .with_ymd_and_hms(2026, 6, 12, 9, 0, 0)
        .single()
        .expect("build morning ts")
        .timestamp();

    insert_request_fact(
        &db,
        "sess-night",
        "msg-1",
        "/tmp/night.jsonl",
        true,
        late_night_ts,
    );
    insert_request_fact(
        &db,
        "sess-night",
        "msg-2",
        "/tmp/night.jsonl",
        true,
        morning_ts,
    );

    let conn = db.conn.lock().unwrap();
    let tx = conn.unchecked_transaction().expect("open tx");
    let dates = LocalUsageDatabase::collect_history_dates_for_session_tx(
        &tx,
        "sess-night",
        &settings,
        "2026-06-13",
    )
    .expect("collect history dates");
    drop(tx);

    assert_eq!(dates.len(), 2);
    assert!(dates.contains("2026-06-11"));
    assert!(dates.contains("2026-06-12"));
}

#[test]
fn business_date_sql_expr_uses_whitelisted_timestamp_columns() {
    let standard = AppSettings::default();
    assert_eq!(
        LocalUsageDatabase::business_date_sql_expr_for_timestamp(
            &standard,
            TimestampSqlColumn::Timestamp,
        ),
        "strftime('%Y-%m-%d', timestamp, 'unixepoch', 'localtime')"
    );

    let mut night_owl = AppSettings::default();
    night_owl.day_boundary_mode = "night_owl".to_string();
    assert_eq!(
        LocalUsageDatabase::business_date_sql_expr_for_timestamp(
            &night_owl,
            TimestampSqlColumn::Timestamp,
        ),
        "strftime('%Y-%m-%d', timestamp, 'unixepoch', 'localtime', '-4 hours')"
    );
}

#[test]
fn purge_orphan_uses_business_day_bucketing_for_invalidated_dates() {
    let (_tmp, db) = temp_db();
    let mut settings = AppSettings::default();
    settings.day_boundary_mode = "night_owl".to_string();
    let orphan_ts = (chrono::Local::now() - chrono::Duration::hours(30)).timestamp();
    let local_date = crate::utils::business_time::business_date_for_timestamp(orphan_ts, &settings);
    let fact = MergedRequestFact {
        canonical_request_key: "claude_code:orphan-midnight".to_string(),
        session_id: "sess-orphan".to_string(),
        project_name: None,
        project_path: None,
        api_key_prefix: None,
        request_base_url: None,
        tool: "claude_code".to_string(),
        timestamp_sec: orphan_ts,
        timestamp_ms: orphan_ts * 1000 + 123,
        model: "claude-sonnet-4".to_string(),
        input_tokens: 1,
        output_tokens: 2,
        cache_create_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 3,
        estimated_cost: 0.1,
        coverage_origin: CoverageOrigin::LocalOnly,
        status_code: Some(200),
        duration_ms: None,
        output_tokens_per_second: None,
        ttft_ms: None,
        source_label: None,
    };
    db.replace_unified_day_materialization(
        &local_date,
        &[(String::from("claude_code:orphan-midnight"), fact)],
        &UnifiedDayMaterializationState {
            local_date: local_date.clone(),
            day_boundary_mode: "night_owl".to_string(),
            fact_count: 1,
            local_request_count: 1,
            local_max_sync_version: 1,
            local_max_timestamp: orphan_ts,
            remote_request_count: 0,
            remote_max_export_seq: 0,
            remote_max_timestamp: 0,
            proxy_record_count: 0,
            proxy_all_record_count: 0,
            proxy_max_timestamp_ms: 0,
            proxy_max_updated_at: 0,
            max_fact_timestamp_ms: orphan_ts * 1000 + 123,
            pricing_fingerprint: 7,
            is_finalized: true,
            finalized_at: Some(300),
            materialized_at: 300,
        },
    )
    .unwrap();

    let tmp_home = tempfile::tempdir().expect("create temp home");
    let old_home = std::env::var_os("HOME");
    std::env::set_var("HOME", tmp_home.path());
    let settings_dir = tmp_home.path().join(".usagemeter");
    fs::create_dir_all(&settings_dir).expect("create settings dir");
    fs::write(
        settings_dir.join("settings.json"),
        serde_json::json!({
            "dayBoundaryMode": "night_owl"
        })
        .to_string(),
    )
    .expect("write settings");

    insert_request_fact(
        &db,
        "sess-orphan",
        "msg-orphan",
        "/tmp/orphan.jsonl",
        false,
        orphan_ts,
    );

    let removed = db.purge_orphan_facts(0).unwrap();
    assert_eq!(removed, 1);
    assert!(db
        .get_unified_day_materialization_state(&local_date)
        .unwrap()
        .is_none());

    match old_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
fn invalidate_unified_materialization_clears_rows_and_bumps_version() {
    let (_tmp, db) = temp_db();
    let local_date = "2026-05-26".to_string();
    let fact = MergedRequestFact {
        canonical_request_key: "claude_code:msg-1".to_string(),
        session_id: "sess-1".to_string(),
        project_name: None,
        project_path: None,
        api_key_prefix: None,
        request_base_url: None,
        tool: "claude_code".to_string(),
        timestamp_sec: 1_779_811_200,
        timestamp_ms: 1_779_811_200_123,
        model: "claude-sonnet-4".to_string(),
        input_tokens: 10,
        output_tokens: 20,
        cache_create_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 30,
        estimated_cost: 1.0,
        coverage_origin: CoverageOrigin::LocalOnly,
        status_code: Some(200),
        duration_ms: None,
        output_tokens_per_second: None,
        ttft_ms: None,
        source_label: None,
    };
    db.replace_unified_day_materialization(
        &local_date,
        &[(String::from("claude_code:msg-1"), fact)],
        &UnifiedDayMaterializationState {
            local_date: local_date.clone(),
            day_boundary_mode: "standard".to_string(),
            fact_count: 1,
            local_request_count: 1,
            local_max_sync_version: 1,
            local_max_timestamp: 1_779_811_200,
            remote_request_count: 0,
            remote_max_export_seq: 0,
            remote_max_timestamp: 0,
            proxy_record_count: 0,
            proxy_all_record_count: 0,
            proxy_max_timestamp_ms: 0,
            proxy_max_updated_at: 0,
            max_fact_timestamp_ms: 1_779_811_200_123,
            pricing_fingerprint: 99,
            is_finalized: true,
            finalized_at: Some(100),
            materialized_at: 100,
        },
    )
    .unwrap();

    let before = db.get_merge_cache_signature().unwrap();
    db.invalidate_unified_materialization_dates(std::slice::from_ref(&local_date))
        .unwrap();
    let after = db.get_merge_cache_signature().unwrap();
    assert!(
        after.unified_materialization_invalidation_version
            > before.unified_materialization_invalidation_version
    );
    assert!(db
        .get_unified_day_materialization_state(&local_date)
        .unwrap()
        .is_none());
    assert!(db
        .get_unified_daily_summaries_between("2026-05-26", "2026-05-27")
        .unwrap()
        .is_empty());
    assert!(db
        .get_unified_daily_model_summaries_between("2026-05-26", "2026-05-27")
        .unwrap()
        .is_empty());
}

#[test]
fn unified_visible_counts_exclude_3xx_statuses() {
    let (_tmp, db) = temp_db();
    let local_date = "2026-05-26".to_string();
    let ok_fact = MergedRequestFact {
        canonical_request_key: "claude_code:msg-ok".to_string(),
        session_id: "sess-1".to_string(),
        project_name: None,
        project_path: None,
        api_key_prefix: None,
        request_base_url: None,
        tool: "claude_code".to_string(),
        timestamp_sec: 1_779_811_200,
        timestamp_ms: 1_779_811_200_123,
        model: "claude-sonnet-4".to_string(),
        input_tokens: 10,
        output_tokens: 20,
        cache_create_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 30,
        estimated_cost: 1.0,
        coverage_origin: CoverageOrigin::ProxyOnly,
        status_code: Some(200),
        duration_ms: None,
        output_tokens_per_second: None,
        ttft_ms: None,
        source_label: None,
    };
    let redirect_fact = MergedRequestFact {
        status_code: Some(302),
        session_id: "sess-2".to_string(),
        timestamp_ms: 1_779_811_201_123,
        ..ok_fact.clone()
    };

    db.replace_unified_day_materialization(
        &local_date,
        &[
            (String::from("claude_code:msg-ok"), ok_fact),
            (String::from("claude_code:msg-redirect"), redirect_fact),
        ],
        &UnifiedDayMaterializationState {
            local_date: local_date.clone(),
            day_boundary_mode: "standard".to_string(),
            fact_count: 2,
            local_request_count: 0,
            local_max_sync_version: 0,
            local_max_timestamp: 0,
            remote_request_count: 0,
            remote_max_export_seq: 0,
            remote_max_timestamp: 0,
            proxy_record_count: 2,
            proxy_all_record_count: 2,
            proxy_max_timestamp_ms: 1_779_811_201_123,
            proxy_max_updated_at: 200,
            max_fact_timestamp_ms: 1_779_811_201_123,
            pricing_fingerprint: 1,
            is_finalized: true,
            finalized_at: Some(200),
            materialized_at: 200,
        },
    )
    .unwrap();

    let summaries = db
        .get_unified_daily_summaries_between("2026-05-26", "2026-05-27")
        .unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].request_count, 2);
    assert_eq!(summaries[0].visible_request_count, 1);

    let model_rows = db
        .get_unified_daily_model_summaries_between("2026-05-26", "2026-05-27")
        .unwrap();
    assert_eq!(model_rows.len(), 1);
    assert_eq!(model_rows[0].request_count, 2);
    assert_eq!(model_rows[0].visible_request_count, 1);
}

#[test]
fn unified_local_only_day_is_not_marked_partial() {
    let (_tmp, db) = temp_db();
    let local_date = "2026-05-26".to_string();
    let local_only_fact = MergedRequestFact {
        canonical_request_key: "claude_code:msg-local".to_string(),
        session_id: "sess-local".to_string(),
        project_name: None,
        project_path: None,
        api_key_prefix: None,
        request_base_url: None,
        tool: "claude_code".to_string(),
        timestamp_sec: 1_779_811_200,
        timestamp_ms: 1_779_811_200_123,
        model: "claude-sonnet-4".to_string(),
        input_tokens: 10,
        output_tokens: 20,
        cache_create_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 30,
        estimated_cost: 1.0,
        coverage_origin: CoverageOrigin::LocalOnly,
        status_code: None,
        duration_ms: None,
        output_tokens_per_second: None,
        ttft_ms: None,
        source_label: None,
    };

    db.replace_unified_day_materialization(
        &local_date,
        &[(String::from("claude_code:msg-local"), local_only_fact)],
        &UnifiedDayMaterializationState {
            local_date: local_date.clone(),
            day_boundary_mode: "standard".to_string(),
            fact_count: 1,
            local_request_count: 1,
            local_max_sync_version: 1,
            local_max_timestamp: 1_779_811_200,
            remote_request_count: 0,
            remote_max_export_seq: 0,
            remote_max_timestamp: 0,
            proxy_record_count: 0,
            proxy_all_record_count: 0,
            proxy_max_timestamp_ms: 0,
            proxy_max_updated_at: 0,
            max_fact_timestamp_ms: 1_779_811_200_123,
            pricing_fingerprint: 1,
            is_finalized: true,
            finalized_at: Some(200),
            materialized_at: 200,
        },
    )
    .unwrap();

    let summaries = db
        .get_unified_daily_summaries_between("2026-05-26", "2026-05-27")
        .unwrap();
    assert_eq!(summaries.len(), 1);
    assert!(!summaries[0].has_partial_status_coverage);
    assert!(!summaries[0].has_partial_performance_coverage);
}

#[test]
fn unified_mixed_day_is_marked_partial() {
    let (_tmp, db) = temp_db();
    let local_date = "2026-05-26".to_string();
    let local_only_fact = MergedRequestFact {
        canonical_request_key: "claude_code:msg-local".to_string(),
        session_id: "sess-local".to_string(),
        project_name: None,
        project_path: None,
        api_key_prefix: None,
        request_base_url: None,
        tool: "claude_code".to_string(),
        timestamp_sec: 1_779_811_200,
        timestamp_ms: 1_779_811_200_123,
        model: "claude-sonnet-4".to_string(),
        input_tokens: 10,
        output_tokens: 20,
        cache_create_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 30,
        estimated_cost: 1.0,
        coverage_origin: CoverageOrigin::LocalOnly,
        status_code: None,
        duration_ms: None,
        output_tokens_per_second: None,
        ttft_ms: None,
        source_label: None,
    };
    let proxy_fact = MergedRequestFact {
        canonical_request_key: "claude_code:msg-proxy".to_string(),
        session_id: "sess-proxy".to_string(),
        project_name: None,
        project_path: None,
        api_key_prefix: Some("sk-ant-1234".to_string()),
        request_base_url: Some("https://api.anthropic.com".to_string()),
        tool: "claude_code".to_string(),
        timestamp_sec: 1_779_811_260,
        timestamp_ms: 1_779_811_260_123,
        model: "claude-sonnet-4".to_string(),
        input_tokens: 10,
        output_tokens: 20,
        cache_create_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 30,
        estimated_cost: 1.0,
        coverage_origin: CoverageOrigin::ProxyOnly,
        status_code: Some(200),
        duration_ms: Some(1200),
        output_tokens_per_second: Some(18.0),
        ttft_ms: Some(300),
        source_label: Some("sk-ant-1234".to_string()),
    };

    db.replace_unified_day_materialization(
        &local_date,
        &[
            (String::from("claude_code:msg-local"), local_only_fact),
            (String::from("claude_code:msg-proxy"), proxy_fact),
        ],
        &UnifiedDayMaterializationState {
            local_date: local_date.clone(),
            day_boundary_mode: "standard".to_string(),
            fact_count: 2,
            local_request_count: 1,
            local_max_sync_version: 1,
            local_max_timestamp: 1_779_811_260,
            remote_request_count: 0,
            remote_max_export_seq: 0,
            remote_max_timestamp: 0,
            proxy_record_count: 1,
            proxy_all_record_count: 1,
            proxy_max_timestamp_ms: 1_779_811_260_123,
            proxy_max_updated_at: 200,
            max_fact_timestamp_ms: 1_779_811_260_123,
            pricing_fingerprint: 1,
            is_finalized: true,
            finalized_at: Some(200),
            materialized_at: 200,
        },
    )
    .unwrap();

    let summaries = db
        .get_unified_daily_summaries_between("2026-05-26", "2026-05-27")
        .unwrap();
    assert_eq!(summaries.len(), 1);
    assert!(!summaries[0].has_partial_status_coverage);
    assert!(summaries[0].has_partial_performance_coverage);
}
