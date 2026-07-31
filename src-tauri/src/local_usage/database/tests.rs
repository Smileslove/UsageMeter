use super::*;
use crate::models::ToolFilter;
use crate::unified_usage;
use crate::unified_usage::{CoverageOrigin, MergedRequestFact};
use chrono::TimeZone;
use rusqlite::{params, Connection};
use std::fs;
use std::sync::{Arc, MutexGuard};

/// OpenCode 相关测试会修改 XDG_DATA_HOME 并共享全局扫描缓存，
/// 统一持有进程级环境变量锁串行执行；锁中毒时继续复用内部值，
/// 避免单个测试 panic 级联毒化后续测试。
fn opencode_test_guard() -> MutexGuard<'static, ()> {
    crate::test_support::env_lock()
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
fn open_v14_db_upgrades_to_v15_with_scope_columns() {
    let tmpdir = tempfile::tempdir().expect("create temp dir");
    let path = tmpdir.path().join("legacy-v14.db");

    {
        let conn = Connection::open(&path).expect("open legacy db");
        conn.execute_batch(
            r#"
            CREATE TABLE local_sync_state (
                state_key TEXT PRIMARY KEY,
                state_value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
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
            CREATE TABLE remote_sessions (
                origin_device_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                tool TEXT NOT NULL,
                project_key TEXT,
                project_name TEXT,
                start_time INTEGER NOT NULL DEFAULT 0,
                end_time INTEGER NOT NULL DEFAULT 0,
                request_count INTEGER NOT NULL DEFAULT 0,
                total_input_tokens INTEGER NOT NULL DEFAULT 0,
                total_output_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                model_list_json TEXT NOT NULL DEFAULT '[]',
                imported_at INTEGER NOT NULL,
                export_seq INTEGER NOT NULL,
                PRIMARY KEY(origin_device_id, session_id)
            );
            "#,
        )
        .expect("create legacy v14 tables");
        conn.execute(
            "INSERT INTO local_sync_state (state_key, state_value, updated_at)
             VALUES ('schema_version', '14', 1700000000)",
            [],
        )
        .expect("set schema_version=14");
    }

    let db = LocalUsageDatabase::new_with_path(&path)
        .expect("open legacy db should trigger v15 migration without error");

    let conn = db.conn.lock().unwrap();
    for table in ["local_sessions", "remote_sessions"] {
        let cols: Vec<String> = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.contains(&"scope".to_string()),
            "migration missed scope column on {table}: {:?}",
            cols
        );
    }
}

#[test]
fn v21_migration_adds_reasonix_fields_without_deleting_sessions() {
    let (tmpdir, db) = temp_db();
    let path = tmpdir.path().join("local_usage.db");
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO local_sessions (
                session_id, tool, project_key, updated_at
             ) VALUES ('reasonix::preserved', 'reasonix', 'p', 1700000000)",
            [],
        )
        .expect("insert preserved session");
        conn.execute(
            "UPDATE local_sync_state SET state_value = '20' WHERE state_key = 'schema_version'",
            [],
        )
        .expect("degrade schema version");
        for table in ["local_sessions", "remote_sessions"] {
            for column in [
                "total_reasoning_tokens",
                "total_elapsed_ms",
                "explicit_cost",
                "explicit_cost_currency",
                "usage_sources_json",
            ] {
                conn.execute_batch(&format!("ALTER TABLE {table} DROP COLUMN {column};"))
                    .unwrap_or_else(|error| panic!("drop {table}.{column}: {error}"));
            }
        }
    }
    drop(db);

    let reopened = LocalUsageDatabase::new_with_path(&path).expect("migrate v20 database to v21");
    let conn = reopened.conn.lock().unwrap();
    let schema_version: String = conn
        .query_row(
            "SELECT state_value FROM local_sync_state WHERE state_key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .expect("read schema version");
    assert_eq!(schema_version, "21");
    for table in ["local_sessions", "remote_sessions"] {
        let columns: Vec<String> = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for required in [
            "total_reasoning_tokens",
            "total_elapsed_ms",
            "explicit_cost",
            "explicit_cost_currency",
            "usage_sources_json",
        ] {
            assert!(
                columns.iter().any(|column| column == required),
                "migration missed {table}.{required}: {columns:?}"
            );
        }
    }
    let preserved: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM local_sessions WHERE session_id = 'reasonix::preserved'",
            [],
            |row| row.get(0),
        )
        .expect("query preserved session");
    assert_eq!(preserved, 1);
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
fn local_session_scope_round_trips_from_database() {
    let (_tmp, db) = temp_db();
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO local_sessions (
                session_id, tool, project_key, cwd, project_name, topic, last_prompt,
                session_name, scope, primary_file_path, file_size, last_modified,
                start_time, end_time, request_count, total_input_tokens,
                total_output_tokens, total_cache_create_tokens, total_cache_read_tokens,
                total_tokens, model_list_json, source_kind, sync_version, updated_at
             ) VALUES (
                'reasonix::session-1', 'reasonix', 'p', '/tmp/project', 'project',
                'Topic', 'Prompt', 'Name', 'project', '/tmp/session.jsonl', 10, 20,
                30, 40, 2, 100, 200, 3, 4, 307, '[]', 'reasonix_session', 1, 50
             )",
            [],
        )
        .expect("insert local session");
    }

    let sessions = db.get_all_sessions(&ToolFilter::All).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].scope.as_deref(), Some("project"));
}

#[test]
fn reasonix_v2_session_fields_survive_sync_export_and_remote_import() {
    let (_tmp_a, db_a) = temp_db();
    {
        let conn = db_a.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO local_sessions (
                session_id, tool, project_key, cwd, project_name, topic, last_prompt,
                session_name, scope, primary_file_path, file_size, last_modified,
                start_time, end_time, request_count, total_input_tokens,
                total_output_tokens, total_cache_create_tokens, total_cache_read_tokens,
                total_tokens, total_reasoning_tokens, total_elapsed_ms, explicit_cost,
                explicit_cost_currency, usage_sources_json, model_list_json, source_kind,
                sync_version, updated_at
             ) VALUES (
                'reasonix::session-2', 'reasonix', 'p', '/tmp/global', 'global',
                NULL, NULL, NULL, 'global', '/tmp/session-2.jsonl', 0, 60,
                70, 80, 3, 10, 20, 0, 4, 34, 7, 1234, 7.2, 'CNY',
                '{\"executor\":{\"promptTokens\":14,\"completionTokens\":7,\"totalTokens\":21,\"reasoningTokens\":3,\"cacheHitTokens\":4,\"cacheMissTokens\":10,\"requestCount\":2,\"sessionCost\":3.6,\"sessionCurrency\":\"CNY\"}}',
                '[\"deepseek-v4\"]', 'reasonix_session', 1, 90
             )",
            [],
        )
        .expect("insert exportable local session");
    }

    let local_sessions = db_a.get_all_sessions(&ToolFilter::All).unwrap();
    assert_eq!(local_sessions.len(), 1);
    assert_eq!(local_sessions[0].total_reasoning_tokens, 7);
    assert_eq!(local_sessions[0].total_elapsed_ms, 1234);
    assert_eq!(local_sessions[0].explicit_cost, Some(7.2));
    assert_eq!(
        local_sessions[0].explicit_cost_currency.as_deref(),
        Some("CNY")
    );
    assert_eq!(local_sessions[0].usage_sources["executor"].request_count, 2);

    let export = db_a.get_sync_export_data().expect("export sync data");
    assert_eq!(export.sessions.len(), 1);
    assert_eq!(export.sessions[0].scope.as_deref(), Some("global"));
    assert_eq!(export.sessions[0].total_reasoning_tokens, 7);
    assert_eq!(
        export.sessions[0].usage_sources["executor"].cache_hit_tokens,
        4
    );

    let (_tmp_b, db_b) = temp_db();
    db_b.import_remote_sync_data("device-a", 1, &export)
        .expect("import remote sync data");

    let sessions = db_b.get_remote_sessions(&ToolFilter::All).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].scope.as_deref(), Some("global"));
    assert_eq!(sessions[0].total_reasoning_tokens, 7);
    assert_eq!(sessions[0].total_elapsed_ms, 1234);
    assert_eq!(sessions[0].explicit_cost, Some(7.2));
    assert_eq!(sessions[0].explicit_cost_currency.as_deref(), Some("CNY"));
    assert!(sessions[0].explicit_estimated_cost.is_none());
    assert_eq!(sessions[0].usage_sources["executor"].request_count, 2);
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
fn local_request_query_saturates_negative_token_values() {
    let (_tmp, db) = temp_db();
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO local_request_facts (
                request_id, session_id, tool, project_key, timestamp, message_id, dedupe_key,
                request_key, model, input_tokens, output_tokens, reasoning_tokens,
                cache_create_tokens, cache_read_tokens, total_tokens, request_count,
                source_file_path, source_file_present, created_at, raw_event_kind, sync_version, is_subagent
             ) VALUES (
                'rid-negative', 'sess-neg', 'claude_code', 'p', 123, 'msg-neg', 'sess-neg:msg-neg',
                'claude_code:msg-neg', 'claude-3', -5, -7, -11, -13, -17, -19, -1,
                '/tmp/neg.jsonl', 1, 123, 'request', 1, 0
             )",
            [],
        )
        .expect("insert negative local fact");
    }

    let records = db
        .get_request_records_in_range(0, i64::MAX, &ToolFilter::All)
        .expect("load local records");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].input_tokens, 0);
    assert_eq!(records[0].output_tokens, 0);
    assert_eq!(records[0].reasoning_tokens, 0);
    assert_eq!(records[0].cache_create_tokens, 0);
    assert_eq!(records[0].cache_read_tokens, 0);
    assert_eq!(records[0].total_tokens, 0);
    assert_eq!(records[0].request_count, 1);
}

#[test]
fn remote_request_query_saturates_negative_token_values() {
    let (_tmp, db) = temp_db();
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO remote_request_facts (
                request_key, origin_device_id, session_id, tool, project_key, timestamp,
                message_id, dedupe_key, model, input_tokens, output_tokens, cache_create_tokens,
                cache_read_tokens, total_tokens, request_count, explicit_estimated_cost,
                is_subagent, source_kind, imported_at, export_seq
             ) VALUES (
                'remote:msg-neg', 'device-a', 'sess-remote', 'reasonix', 'p', 456,
                'msg-neg', 'dedupe-neg', 'model-x', -2, -3, -5, -7, -11, -13, NULL,
                0, 'remote_sync', 456, 1
             )",
            [],
        )
        .expect("insert negative remote fact");
    }

    let records = db
        .get_remote_request_records_in_range(0, i64::MAX, &ToolFilter::All)
        .expect("load remote records");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].input_tokens, 0);
    assert_eq!(records[0].output_tokens, 0);
    assert_eq!(records[0].cache_create_tokens, 0);
    assert_eq!(records[0].cache_read_tokens, 0);
    assert_eq!(records[0].total_tokens, 0);
    assert_eq!(records[0].request_count, 1);
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
        request_count: 1,
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
fn unified_materialization_state_preserves_full_u64_pricing_fingerprint() {
    let (_tmp, db) = temp_db();
    let local_date = "2026-05-27".to_string();
    let high_bit_fingerprint = (i64::MAX as u64) + 42;
    let state = UnifiedDayMaterializationState {
        local_date: local_date.clone(),
        day_boundary_mode: "standard".to_string(),
        fact_count: 0,
        local_request_count: 0,
        local_max_sync_version: 0,
        local_max_timestamp: 0,
        remote_request_count: 0,
        remote_max_export_seq: 0,
        remote_max_timestamp: 0,
        proxy_record_count: 0,
        proxy_all_record_count: 0,
        proxy_max_timestamp_ms: 0,
        proxy_max_updated_at: 0,
        max_fact_timestamp_ms: 0,
        pricing_fingerprint: high_bit_fingerprint,
        is_finalized: true,
        finalized_at: Some(123456789),
        materialized_at: 123456790,
    };

    db.replace_unified_day_materialization(&local_date, &[], &state)
        .expect("store materialization state with high-bit fingerprint");

    let loaded = db
        .get_unified_day_materialization_state(&local_date)
        .expect("load high-bit materialization state")
        .expect("high-bit materialization state exists");
    assert_eq!(loaded.pricing_fingerprint, high_bit_fingerprint);

    let batched = db
        .get_unified_days_materialization_states(std::slice::from_ref(&local_date))
        .expect("load batched high-bit materialization state");
    assert_eq!(
        batched
            .get(&local_date)
            .map(|value| value.pricing_fingerprint),
        Some(high_bit_fingerprint)
    );
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
        request_count: 1,
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
fn unified_days_materialization_stamps_track_rows_and_rebuilds() {
    let (_tmp, db) = temp_db();
    let build_state = |local_date: &str, materialized_at: i64| UnifiedDayMaterializationState {
        local_date: local_date.to_string(),
        day_boundary_mode: "standard".to_string(),
        fact_count: 0,
        local_request_count: 0,
        local_max_sync_version: 0,
        local_max_timestamp: 0,
        remote_request_count: 0,
        remote_max_export_seq: 0,
        remote_max_timestamp: 0,
        proxy_record_count: 0,
        proxy_all_record_count: 0,
        proxy_max_timestamp_ms: 0,
        proxy_max_updated_at: 0,
        max_fact_timestamp_ms: 0,
        pricing_fingerprint: 0,
        is_finalized: true,
        finalized_at: Some(materialized_at),
        materialized_at,
    };

    // 空日期列表 → 恒定空结果，不触发 SQL。
    assert!(db
        .get_unified_days_materialization_stamps(&[])
        .unwrap()
        .is_empty());

    let day1 = "2026-06-01".to_string();
    let day2 = "2026-06-02".to_string();
    db.replace_unified_day_materialization(&day1, &[], &build_state(&day1, 100))
        .unwrap();
    db.replace_unified_day_materialization(&day2, &[], &build_state(&day2, 200))
        .unwrap();

    let both = vec![day1.clone(), day2.clone()];
    let mut stamps = db.get_unified_days_materialization_stamps(&both).unwrap();
    stamps.sort();
    assert_eq!(stamps, vec![(day1.clone(), 100), (day2.clone(), 200)]);

    // 缺失日期不返回对应行：调用方以行数 != 请求日期数判定状态行缺失。
    let with_missing = vec![day1.clone(), day2.clone(), "2026-06-03".to_string()];
    assert_eq!(
        db.get_unified_days_materialization_stamps(&with_missing)
            .unwrap()
            .len(),
        2
    );

    // 任一日重建（materialized_at 更新）→ 仅该日 stamp 变化。
    db.replace_unified_day_materialization(&day1, &[], &build_state(&day1, 300))
        .unwrap();
    let mut stamps = db.get_unified_days_materialization_stamps(&both).unwrap();
    stamps.sort();
    assert_eq!(stamps, vec![(day1, 300), (day2, 200)]);
}

#[test]
fn cold_facts_shard_cache_only_refetches_rematerialized_day() {
    let (_tmp, db) = temp_db();
    let build_fact = |key: &str, timestamp_sec: i64| MergedRequestFact {
        canonical_request_key: key.to_string(),
        session_id: "sess-1".to_string(),
        project_name: None,
        project_path: None,
        api_key_prefix: None,
        request_base_url: None,
        tool: "claude_code".to_string(),
        timestamp_sec,
        timestamp_ms: timestamp_sec * 1000,
        model: "claude-sonnet-4".to_string(),
        input_tokens: 10,
        output_tokens: 20,
        cache_create_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 30,
        request_count: 1,
        estimated_cost: 0.5,
        coverage_origin: CoverageOrigin::LocalOnly,
        status_code: Some(200),
        duration_ms: None,
        output_tokens_per_second: None,
        ttft_ms: None,
        source_label: None,
    };
    let build_state =
        |local_date: &str, fact_count: u64, materialized_at: i64| UnifiedDayMaterializationState {
            local_date: local_date.to_string(),
            day_boundary_mode: "standard".to_string(),
            fact_count,
            local_request_count: fact_count,
            local_max_sync_version: 1,
            local_max_timestamp: 0,
            remote_request_count: 0,
            remote_max_export_seq: 0,
            remote_max_timestamp: 0,
            proxy_record_count: 0,
            proxy_all_record_count: 0,
            proxy_max_timestamp_ms: 0,
            proxy_max_updated_at: 0,
            max_fact_timestamp_ms: 0,
            pricing_fingerprint: 0,
            is_finalized: true,
            finalized_at: Some(materialized_at),
            materialized_at,
        };

    let day1 = "2026-06-01".to_string();
    let day2 = "2026-06-02".to_string();
    let day3 = "2026-06-03".to_string();
    db.replace_unified_day_materialization(
        &day1,
        &[("k1".to_string(), build_fact("k1", 1_780_300_000))],
        &build_state(&day1, 1, 100),
    )
    .unwrap();
    db.replace_unified_day_materialization(
        &day2,
        &[("k2".to_string(), build_fact("k2", 1_780_386_400))],
        &build_state(&day2, 1, 200),
    )
    .unwrap();
    db.replace_unified_day_materialization(
        &day3,
        &[("k3".to_string(), build_fact("k3", 1_780_472_800))],
        &build_state(&day3, 1, 300),
    )
    .unwrap();

    // 用测试独立的缓存实例，避免并发测试共享全局单例互相干扰。
    let cache = std::sync::Mutex::new(unified_usage::ColdFactsShardCache::new());
    let dates = vec![day1.clone(), day2.clone(), day3.clone()];

    // 首次读取：全部日期缺失分片 → 一次批量读入并按日写回分片。
    let first = unified_usage::load_cold_facts_via_shards(&cache, &db, &dates, "standard").unwrap();
    assert_eq!(first.days_cached, 0);
    assert_eq!(first.days_fetched, 3);
    assert!(!first.memo_hit);
    assert!(!first.fallback_uncached);
    assert_eq!(first.stale_days_skipped, 0);
    assert_eq!(first.facts.len(), 3);
    // 拼接结果按日期升序（各日内部按 timestamp_ms 升序）。
    assert_eq!(first.facts[0].canonical_request_key, "k1");
    assert_eq!(first.facts[2].canonical_request_key, "k3");

    let day1_shard_before = {
        let guard = cache.lock().unwrap();
        guard.shard_facts_for_test(&day1).expect("day1 shard")
    };
    let day3_shard_before = {
        let guard = cache.lock().unwrap();
        guard.shard_facts_for_test(&day3).expect("day3 shard")
    };

    // 相同日期集合、物化状态未变的重复读取 → memo 命中，零重读。
    let repeat =
        unified_usage::load_cold_facts_via_shards(&cache, &db, &dates, "standard").unwrap();
    assert!(repeat.memo_hit);
    assert_eq!(repeat.days_fetched, 0);
    assert!(Arc::ptr_eq(&first.facts, &repeat.facts));

    // 单日重物化：仅 day2 的 materialized_at 变化，事实内容也更新。
    db.replace_unified_day_materialization(
        &day2,
        &[
            ("k2".to_string(), build_fact("k2", 1_780_386_400)),
            ("k2b".to_string(), build_fact("k2b", 1_780_386_500)),
        ],
        &build_state(&day2, 2, 999),
    )
    .unwrap();

    let second =
        unified_usage::load_cold_facts_via_shards(&cache, &db, &dates, "standard").unwrap();
    // 只有 day2 分片被重建，其余两天直接复用缓存分片。
    assert!(!second.memo_hit);
    assert_eq!(second.days_cached, 2);
    assert_eq!(second.days_fetched, 1);
    assert_eq!(second.facts.len(), 4);
    let guard = cache.lock().unwrap();
    let day1_shard_after = guard.shard_facts_for_test(&day1).expect("day1 shard");
    let day2_shard_after = guard.shard_facts_for_test(&day2).expect("day2 shard");
    let day3_shard_after = guard.shard_facts_for_test(&day3).expect("day3 shard");
    drop(guard);
    // Arc 指针不变 → 未失效分片被原样复用，未发生重读重建。
    assert!(Arc::ptr_eq(&day1_shard_before, &day1_shard_after));
    assert!(Arc::ptr_eq(&day3_shard_before, &day3_shard_after));
    assert_eq!(day2_shard_after.len(), 2);
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
fn v16_migration_clears_stale_unified_materialization_from_codex_fuzzy_match_fix() {
    // Regression guard for the Codex proxy double-counting fix: days materialized under the
    // old (pre-fuzzy-match) merge logic must not silently keep serving stale cached numbers
    // forever just because their raw-input fingerprint hasn't changed.
    let (_tmp, db) = temp_db();
    let local_date = "2026-05-26".to_string();
    let fact = MergedRequestFact {
        canonical_request_key: "codex:sess-1:1".to_string(),
        session_id: "sess-1".to_string(),
        project_name: None,
        project_path: None,
        api_key_prefix: None,
        request_base_url: None,
        tool: "codex".to_string(),
        timestamp_sec: 1_779_811_200,
        timestamp_ms: 1_779_811_200_123,
        model: "gpt-5".to_string(),
        input_tokens: 100,
        output_tokens: 200,
        cache_create_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 300,
        request_count: 1,
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
        &[(String::from("codex:sess-1:1"), fact)],
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

    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE local_sync_state SET state_value = '15' WHERE state_key = 'schema_version'",
            [],
        )
        .expect("degrade schema version");
    }

    let reopened = LocalUsageDatabase::new_with_path(&_tmp.path().join("local_usage.db"))
        .expect("reopen and migrate");

    assert!(reopened
        .get_unified_day_materialization_state(&local_date)
        .unwrap()
        .is_none());
    assert!(reopened
        .get_unified_daily_summaries_between("2026-05-26", "2026-05-27")
        .unwrap()
        .is_empty());
    assert!(reopened
        .get_unified_daily_model_summaries_between("2026-05-26", "2026-05-27")
        .unwrap()
        .is_empty());
}

#[test]
fn v17_migration_clears_stale_unified_materialization_from_codex_session_id_prefix_fix() {
    // Regression guard: the v16 fuzzy-match reconciliation never actually matched anything in
    // practice — the local scanner's session id is namespaced `codex::<uuid>` while the proxy
    // captures the bare uuid, so every Codex request materialized under v16 was still
    // double-counted. Days materialized under v16 must not keep serving those stale numbers.
    let (_tmp, db) = temp_db();
    let local_date = "2026-05-26".to_string();
    let fact = MergedRequestFact {
        canonical_request_key: "codex:sess-1:1".to_string(),
        session_id: "codex::sess-1".to_string(),
        project_name: None,
        project_path: None,
        api_key_prefix: None,
        request_base_url: None,
        tool: "codex".to_string(),
        timestamp_sec: 1_779_811_200,
        timestamp_ms: 1_779_811_200_123,
        model: "gpt-5".to_string(),
        input_tokens: 100,
        output_tokens: 200,
        cache_create_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 300,
        request_count: 1,
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
        &[(String::from("codex:sess-1:1"), fact)],
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

    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE local_sync_state SET state_value = '16' WHERE state_key = 'schema_version'",
            [],
        )
        .expect("degrade schema version");
    }

    let reopened = LocalUsageDatabase::new_with_path(&_tmp.path().join("local_usage.db"))
        .expect("reopen and migrate");

    assert!(reopened
        .get_unified_day_materialization_state(&local_date)
        .unwrap()
        .is_none());
    assert!(reopened
        .get_unified_daily_summaries_between("2026-05-26", "2026-05-27")
        .unwrap()
        .is_empty());
    assert!(reopened
        .get_unified_daily_model_summaries_between("2026-05-26", "2026-05-27")
        .unwrap()
        .is_empty());
}

#[test]
fn v18_migration_clears_stale_unified_materialization_from_codex_session_id_removal_fix() {
    // Regression guard: the v17 fix still required matching `session_id`, but real Codex CLI
    // requests never carry one that the proxy can observe, so the fuzzy match kept silently
    // failing for every request just like v16. Days materialized under v16/v17 must not keep
    // serving those stale double-counted numbers.
    let (_tmp, db) = temp_db();
    let local_date = "2026-05-26".to_string();
    let fact = MergedRequestFact {
        canonical_request_key: "codex:sess-1:1".to_string(),
        session_id: "codex::sess-1".to_string(),
        project_name: None,
        project_path: None,
        api_key_prefix: None,
        request_base_url: None,
        tool: "codex".to_string(),
        timestamp_sec: 1_779_811_200,
        timestamp_ms: 1_779_811_200_123,
        model: "gpt-5".to_string(),
        input_tokens: 100,
        output_tokens: 200,
        cache_create_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 300,
        request_count: 1,
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
        &[(String::from("codex:sess-1:1"), fact)],
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

    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE local_sync_state SET state_value = '17' WHERE state_key = 'schema_version'",
            [],
        )
        .expect("degrade schema version");
    }

    let reopened = LocalUsageDatabase::new_with_path(&_tmp.path().join("local_usage.db"))
        .expect("reopen and migrate");

    assert!(reopened
        .get_unified_day_materialization_state(&local_date)
        .unwrap()
        .is_none());
    assert!(reopened
        .get_unified_daily_summaries_between("2026-05-26", "2026-05-27")
        .unwrap()
        .is_empty());
    assert!(reopened
        .get_unified_daily_model_summaries_between("2026-05-26", "2026-05-27")
        .unwrap()
        .is_empty());
}

#[test]
fn v19_migration_clears_stale_unified_materialization_from_per_field_match_fix() {
    // Regression guard: v18 dropped session_id but still matched local vs proxy on a single
    // `total_tokens` equality, which never holds for Codex (cache_creation is absent locally
    // and the two sides derive tokens differently). Days materialized under v16–v18 are still
    // double-counted and must be cleared so they recompute under the per-field fingerprint.
    let (_tmp, db) = temp_db();
    let local_date = "2026-05-26".to_string();
    let fact = MergedRequestFact {
        canonical_request_key: "codex:sess-1:1".to_string(),
        session_id: "codex::sess-1".to_string(),
        project_name: None,
        project_path: None,
        api_key_prefix: None,
        request_base_url: None,
        tool: "codex".to_string(),
        timestamp_sec: 1_779_811_200,
        timestamp_ms: 1_779_811_200_123,
        model: "gpt-5".to_string(),
        input_tokens: 100,
        output_tokens: 200,
        cache_create_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 300,
        request_count: 1,
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
        &[(String::from("codex:sess-1:1"), fact)],
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

    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE local_sync_state SET state_value = '18' WHERE state_key = 'schema_version'",
            [],
        )
        .expect("degrade schema version");
    }

    let reopened = LocalUsageDatabase::new_with_path(&_tmp.path().join("local_usage.db"))
        .expect("reopen and migrate");

    assert!(reopened
        .get_unified_day_materialization_state(&local_date)
        .unwrap()
        .is_none());
    assert!(reopened
        .get_unified_daily_summaries_between("2026-05-26", "2026-05-27")
        .unwrap()
        .is_empty());
    assert!(reopened
        .get_unified_daily_model_summaries_between("2026-05-26", "2026-05-27")
        .unwrap()
        .is_empty());
}

#[test]
fn v20_migration_clears_pre_authoritative_materialization_and_runtime_caches() {
    let (_tmp, db) = temp_db();
    let local_date = "2026-05-26".to_string();
    let fact = MergedRequestFact {
        canonical_request_key: "opencode:req-v20".to_string(),
        session_id: "opencode::native::sess-v20".to_string(),
        project_name: None,
        project_path: None,
        api_key_prefix: None,
        request_base_url: None,
        tool: "opencode".to_string(),
        timestamp_sec: 1_779_811_200,
        timestamp_ms: 1_779_811_200_123,
        model: "gpt-5".to_string(),
        input_tokens: 100,
        output_tokens: 200,
        cache_create_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 300,
        request_count: 1,
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
        &[("opencode:req-v20".to_string(), fact)],
        &UnifiedDayMaterializationState {
            local_date: local_date.clone(),
            day_boundary_mode: "standard".to_string(),
            fact_count: 1,
            local_request_count: 0,
            local_max_sync_version: 0,
            local_max_timestamp: 0,
            remote_request_count: 0,
            remote_max_export_seq: 0,
            remote_max_timestamp: 0,
            proxy_record_count: 1,
            proxy_all_record_count: 1,
            proxy_max_timestamp_ms: 1_779_811_200_123,
            proxy_max_updated_at: 100,
            max_fact_timestamp_ms: 1_779_811_200_123,
            pricing_fingerprint: 99,
            is_finalized: true,
            finalized_at: Some(100),
            materialized_at: 100,
        },
    )
    .expect("seed pre-v20 materialization");
    let invalidation_before = db
        .get_merge_cache_signature()
        .expect("read signature before v20")
        .unified_materialization_invalidation_version;
    unified_usage::clear_runtime_caches();
    unified_usage::seed_runtime_merge_cache_for_test();
    assert_eq!(unified_usage::runtime_merge_cache_len_for_test(), 1);

    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE local_sync_state SET state_value = '19' WHERE state_key = 'schema_version'",
            [],
        )
        .expect("degrade schema version to 19");
    }

    let reopened = LocalUsageDatabase::new_with_path(&_tmp.path().join("local_usage.db"))
        .expect("reopen and run v20 migration");

    assert!(reopened
        .get_unified_day_materialization_state(&local_date)
        .unwrap()
        .is_none());
    assert!(reopened
        .get_unified_facts_for_dates(std::slice::from_ref(&local_date), &ToolFilter::All)
        .unwrap()
        .is_empty());
    assert!(reopened
        .get_unified_daily_summaries_between("2026-05-26", "2026-05-27")
        .unwrap()
        .is_empty());
    assert!(reopened
        .get_unified_daily_model_summaries_between("2026-05-26", "2026-05-27")
        .unwrap()
        .is_empty());
    assert_eq!(
        reopened
            .get_local_sync_state("schema_version")
            .unwrap()
            .as_deref(),
        Some("21")
    );
    assert!(
        reopened
            .get_merge_cache_signature()
            .unwrap()
            .unified_materialization_invalidation_version
            > invalidation_before
    );
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
        request_count: 1,
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
        request_count: 1,
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
        request_count: 1,
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
    assert_eq!(summaries[0].success_request_count, 1);

    let model_rows = db
        .get_unified_daily_model_summaries_between("2026-05-26", "2026-05-27")
        .unwrap();
    assert_eq!(model_rows.len(), 1);
    assert_eq!(model_rows[0].request_count, 2);
    assert_eq!(model_rows[0].visible_request_count, 1);
    assert_eq!(model_rows[0].success_request_count, 1);
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
        request_count: 1,
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
        request_count: 1,
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
        request_count: 1,
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

#[test]
fn unified_summary_respects_request_count_weight() {
    let (_tmp, db) = temp_db();
    let local_date = "2026-05-28".to_string();
    let fact = MergedRequestFact {
        canonical_request_key: "hermes:session-1".to_string(),
        session_id: "hermes::session-1".to_string(),
        project_name: Some("work".to_string()),
        project_path: None,
        api_key_prefix: None,
        request_base_url: None,
        tool: "hermes".to_string(),
        timestamp_sec: 1_780_000_000,
        timestamp_ms: 1_780_000_000_000,
        model: "claude-sonnet-4".to_string(),
        input_tokens: 100,
        output_tokens: 40,
        cache_create_tokens: 20,
        cache_read_tokens: 10,
        total_tokens: 170,
        request_count: 7,
        estimated_cost: 0.42,
        coverage_origin: CoverageOrigin::LocalOnly,
        status_code: Some(200),
        duration_ms: None,
        output_tokens_per_second: None,
        ttft_ms: None,
        source_label: None,
    };

    db.replace_unified_day_materialization(
        &local_date,
        &[(String::from("hermes:session-1"), fact)],
        &UnifiedDayMaterializationState {
            local_date: local_date.clone(),
            day_boundary_mode: "standard".to_string(),
            fact_count: 1,
            local_request_count: 1,
            local_max_sync_version: 1,
            local_max_timestamp: 1_780_000_000,
            remote_request_count: 0,
            remote_max_export_seq: 0,
            remote_max_timestamp: 0,
            proxy_record_count: 0,
            proxy_all_record_count: 0,
            proxy_max_timestamp_ms: 0,
            proxy_max_updated_at: 0,
            max_fact_timestamp_ms: 1_780_000_000_000,
            pricing_fingerprint: 1,
            is_finalized: true,
            finalized_at: Some(200),
            materialized_at: 200,
        },
    )
    .unwrap();

    let summaries = db
        .get_unified_daily_summaries_between("2026-05-28", "2026-05-29")
        .unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].request_count, 7);
    assert_eq!(summaries[0].visible_request_count, 7);
    assert_eq!(summaries[0].success_request_count, 7);
}

// ============================================================
// 跨天会话增量同步：内容未变的历史行不应产生任何写副作用
// ============================================================

/// 构造 scanner 解析出的单条请求事实
fn make_scan_request(
    session_id: &str,
    message_id: &str,
    timestamp: i64,
    output_tokens: u64,
) -> crate::session::LocalRequestRecord {
    crate::session::LocalRequestRecord {
        session_id: session_id.to_string(),
        tool: "claude_code".to_string(),
        timestamp,
        message_id: message_id.to_string(),
        input_tokens: 10,
        output_tokens,
        total_tokens: 10 + output_tokens,
        model: "claude-3".to_string(),
        ..Default::default()
    }
}

/// 构造一个待同步的脏会话（模拟 JSONL fingerprint 变化后的重新解析结果）
fn make_reasonix_dirty_session(
    session_id: &str,
    fingerprint: &str,
    end_time: i64,
    input_tokens: u64,
) -> DirtySessionSync {
    let file_path = format!("/tmp/{session_id}.jsonl");
    let meta = crate::session::SessionMeta {
        session_id: session_id.to_string(),
        tool: "reasonix".to_string(),
        file_path: file_path.clone(),
        file_size: 100,
        last_modified: end_time,
        start_time: end_time.saturating_sub(60),
        end_time,
        message_count: 1,
        total_input_tokens: input_tokens,
        total_output_tokens: 10,
        source: "reasonix_session".to_string(),
        ..Default::default()
    };
    DirtySessionSync {
        session_id: session_id.to_string(),
        tool: "reasonix".to_string(),
        file_path,
        file_role: "session_group".to_string(),
        file_size: 100,
        last_modified: end_time,
        fingerprint: fingerprint.to_string(),
        meta,
        requests: Vec::new(),
        project_key: "reasonix-project".to_string(),
    }
}

fn make_dirty_session(
    session_id: &str,
    fingerprint: &str,
    requests: Vec<crate::session::LocalRequestRecord>,
) -> DirtySessionSync {
    let file_path = format!("/tmp/{session_id}.jsonl");
    let meta = crate::session::SessionMeta {
        session_id: session_id.to_string(),
        tool: "claude_code".to_string(),
        file_path: file_path.clone(),
        file_size: 100,
        message_count: requests.len() as u64,
        source: "local".to_string(),
        ..Default::default()
    };
    DirtySessionSync {
        session_id: session_id.to_string(),
        tool: "claude_code".to_string(),
        file_path,
        file_role: "session_group".to_string(),
        file_size: 100,
        last_modified: 0,
        fingerprint: fingerprint.to_string(),
        meta,
        requests,
        project_key: "p".to_string(),
    }
}

/// 读取指定事实行的 (sync_version, created_at, source_file_present)
fn get_fact_state(db: &LocalUsageDatabase, session_id: &str, message_id: &str) -> (i64, i64, i64) {
    let conn = db.conn.lock().unwrap();
    let dedupe_key = format!("{}:{}", session_id, message_id);
    conn.query_row(
        "SELECT sync_version, created_at, source_file_present
         FROM local_request_facts
         WHERE tool = 'claude_code' AND dedupe_key = ?1",
        params![dedupe_key],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        },
    )
    .expect("read fact state")
}

/// 直接写入一行历史日物化状态，模拟该日已完成物化
fn seed_materialization_state(db: &LocalUsageDatabase, local_date: &str) {
    let conn = db.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO unified_daily_materialization_state (local_date, materialized_at)
         VALUES (?1, 0)",
        params![local_date],
    )
    .expect("seed materialization state");
}

fn invalidation_version(db: &LocalUsageDatabase) -> String {
    db.get_local_sync_state("unified_materialization_invalidation_version")
        .expect("read invalidation version")
        .unwrap_or_else(|| "0".to_string())
}

#[test]
fn reasonix_telemetry_only_session_contributes_to_time_bounds() {
    let _guard = opencode_test_guard();
    let (_tmp, db) = temp_db();
    let history_ts = chrono::Utc::now().timestamp() - 3 * 86_400;
    db.sync_dirty_sessions(
        vec![make_reasonix_dirty_session(
            "reasonix::bounds",
            "fp-1",
            history_ts,
            100,
        )],
        vec![],
    )
    .expect("sync telemetry-only session");

    assert_eq!(
        db.get_request_time_bounds().unwrap(),
        Some((history_ts, history_ts))
    );
}

#[test]
fn reasonix_telemetry_update_invalidates_old_and_new_history_dates() {
    let _guard = opencode_test_guard();
    let (_tmp, db) = temp_db();
    let settings = crate::commands::load_settings_blocking().unwrap_or_default();
    let old_ts = chrono::Utc::now().timestamp() - 5 * 86_400;
    let new_ts = old_ts + 2 * 86_400;
    let old_date = crate::utils::business_time::business_date_for_timestamp(old_ts, &settings);
    let new_date = crate::utils::business_time::business_date_for_timestamp(new_ts, &settings);

    db.sync_dirty_sessions(
        vec![make_reasonix_dirty_session(
            "reasonix::moved",
            "fp-1",
            old_ts,
            100,
        )],
        vec![],
    )
    .expect("sync old telemetry");
    seed_materialization_state(&db, &old_date);
    seed_materialization_state(&db, &new_date);

    db.sync_dirty_sessions(
        vec![make_reasonix_dirty_session(
            "reasonix::moved",
            "fp-2",
            new_ts,
            200,
        )],
        vec![],
    )
    .expect("sync moved telemetry");

    assert!(db
        .get_unified_day_materialization_state(&old_date)
        .unwrap()
        .is_none());
    assert!(db
        .get_unified_day_materialization_state(&new_date)
        .unwrap()
        .is_none());
}

#[test]
fn removing_reasonix_session_deletes_telemetry_summary_and_invalidates_history() {
    let _guard = opencode_test_guard();
    let (_tmp, db) = temp_db();
    let settings = crate::commands::load_settings_blocking().unwrap_or_default();
    let history_ts = chrono::Utc::now().timestamp() - 3 * 86_400;
    let history_date =
        crate::utils::business_time::business_date_for_timestamp(history_ts, &settings);

    db.sync_dirty_sessions(
        vec![make_reasonix_dirty_session(
            "reasonix::removed",
            "fp-1",
            history_ts,
            100,
        )],
        vec![],
    )
    .expect("sync telemetry session");
    seed_materialization_state(&db, &history_date);

    db.sync_dirty_sessions(vec![], vec!["reasonix::removed".to_string()])
        .expect("remove telemetry session");

    assert!(db
        .get_all_sessions(&ToolFilter::Tool("reasonix".to_string()))
        .unwrap()
        .is_empty());
    assert!(db
        .get_unified_day_materialization_state(&history_date)
        .unwrap()
        .is_none());
}

#[test]
fn remote_reasonix_telemetry_only_import_invalidates_history_date() {
    let _guard = opencode_test_guard();
    let (_tmp, db) = temp_db();
    let settings = crate::commands::load_settings_blocking().unwrap_or_default();
    let history_ts = chrono::Utc::now().timestamp() - 3 * 86_400;
    let history_date =
        crate::utils::business_time::business_date_for_timestamp(history_ts, &settings);
    seed_materialization_state(&db, &history_date);
    let data = SyncExportData {
        sessions: vec![SyncExportSession {
            session_id: "reasonix::remote-only".to_string(),
            tool: "reasonix".to_string(),
            project_key: Some("reasonix-project".to_string()),
            project_name: Some("Reasonix".to_string()),
            scope: Some("global".to_string()),
            start_time: history_ts - 60,
            end_time: history_ts,
            request_count: 1,
            total_input_tokens: 100,
            total_output_tokens: 10,
            total_cache_create_tokens: 0,
            total_cache_read_tokens: 0,
            total_tokens: 110,
            total_reasoning_tokens: 5,
            total_elapsed_ms: 1_000,
            explicit_cost: None,
            explicit_cost_currency: None,
            usage_sources: Default::default(),
            model_list: vec!["deepseek-reasoner".to_string()],
        }],
        requests: Vec::new(),
    };

    db.import_remote_sync_data("device-remote", 1, &data)
        .expect("import telemetry-only remote session");

    assert!(db
        .get_unified_day_materialization_state(&history_date)
        .unwrap()
        .is_none());
    assert_eq!(
        db.get_request_time_bounds().unwrap(),
        Some((history_ts, history_ts))
    );
}

#[test]
fn append_only_resync_leaves_history_facts_and_materialization_untouched() {
    // 持有环境锁：sync 内部会读取 settings（依赖 HOME），避免与修改 HOME 的测试并发
    let _guard = opencode_test_guard();
    let (_tmp, db) = temp_db();
    let settings = crate::commands::load_settings_blocking().unwrap_or_default();
    let now = chrono::Utc::now().timestamp();
    // 取 3 天前，任何业务日边界模式下都严格早于 today
    let history_ts = now - 3 * 86_400;
    let history_date =
        crate::utils::business_time::business_date_for_timestamp(history_ts, &settings);
    let session = "sess-cross-day";

    // 首次同步：一条历史日请求 + 一条今天请求
    db.sync_dirty_sessions(
        vec![make_dirty_session(
            session,
            "fp-1",
            vec![
                make_scan_request(session, "msg-old", history_ts, 100),
                make_scan_request(session, "msg-today-1", now, 200),
            ],
        )],
        vec![],
    )
    .expect("first sync");

    let (version_before, created_before, _) = get_fact_state(&db, session, "msg-old");
    assert_eq!(version_before, 1);

    // 模拟历史日已完成物化 + 首轮 outbox 已全部上传
    seed_materialization_state(&db, &history_date);
    {
        let conn = db.conn.lock().unwrap();
        conn.execute("UPDATE sync_outbox_request_events SET uploaded_at = 1", [])
            .expect("mark outbox uploaded");
    }
    let invalidation_before = invalidation_version(&db);

    // 模拟文件追加：同一会话多出一条今天的请求，fingerprint 变化触发全量重解析
    db.sync_dirty_sessions(
        vec![make_dirty_session(
            session,
            "fp-2",
            vec![
                make_scan_request(session, "msg-old", history_ts, 100),
                make_scan_request(session, "msg-today-1", now, 200),
                make_scan_request(session, "msg-today-2", now, 300),
            ],
        )],
        vec![],
    )
    .expect("second sync");

    // 历史行内容未变：sync_version / created_at 均不应变化，仍在场
    let (version_after, created_after, present_after) = get_fact_state(&db, session, "msg-old");
    assert_eq!(
        version_after, 1,
        "unchanged history fact must not bump sync_version"
    );
    assert_eq!(created_after, created_before);
    assert_eq!(present_after, 1);

    // 历史日物化状态不应被失效删除，失效版本不应 bump
    assert!(db
        .get_unified_day_materialization_state(&history_date)
        .expect("read materialization state")
        .is_some());
    assert_eq!(invalidation_version(&db), invalidation_before);

    // 未变更的行不应重新入队 outbox（uploaded_at 不被重置）；仅新行处于待上传状态
    {
        let conn = db.conn.lock().unwrap();
        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_outbox_request_events WHERE uploaded_at IS NULL",
                [],
                |row| row.get(0),
            )
            .expect("count pending outbox events");
        assert_eq!(
            pending, 1,
            "only the newly appended request should be re-queued"
        );
    }

    // 新追加的今天行被正确插入
    let (new_version, _, new_present) = get_fact_state(&db, session, "msg-today-2");
    assert_eq!(new_version, 1);
    assert_eq!(new_present, 1);
}

#[test]
fn history_row_content_change_bumps_version_and_invalidates_history_date() {
    let _guard = opencode_test_guard();
    let (_tmp, db) = temp_db();
    let settings = crate::commands::load_settings_blocking().unwrap_or_default();
    let now = chrono::Utc::now().timestamp();
    let history_ts = now - 3 * 86_400;
    let history_date =
        crate::utils::business_time::business_date_for_timestamp(history_ts, &settings);
    let session = "sess-history-edit";

    db.sync_dirty_sessions(
        vec![make_dirty_session(
            session,
            "fp-1",
            vec![
                make_scan_request(session, "msg-old", history_ts, 100),
                make_scan_request(session, "msg-today", now, 200),
            ],
        )],
        vec![],
    )
    .expect("first sync");
    seed_materialization_state(&db, &history_date);
    let invalidation_before = invalidation_version(&db);

    // 修改历史日请求的 token 数后再同步
    db.sync_dirty_sessions(
        vec![make_dirty_session(
            session,
            "fp-2",
            vec![
                make_scan_request(session, "msg-old", history_ts, 999),
                make_scan_request(session, "msg-today", now, 200),
            ],
        )],
        vec![],
    )
    .expect("second sync");

    // 内容变更的历史行必须 bump sync_version
    let (version_after, _, present_after) = get_fact_state(&db, session, "msg-old");
    assert_eq!(version_after, 2);
    assert_eq!(present_after, 1);

    // 历史日被判定为 touched：物化状态被失效删除、失效版本 bump
    assert!(db
        .get_unified_day_materialization_state(&history_date)
        .expect("read materialization state")
        .is_none());
    assert_ne!(invalidation_version(&db), invalidation_before);
}

#[test]
fn history_row_removal_soft_deletes_and_invalidates_history_date() {
    let _guard = opencode_test_guard();
    let (_tmp, db) = temp_db();
    let settings = crate::commands::load_settings_blocking().unwrap_or_default();
    let now = chrono::Utc::now().timestamp();
    let history_ts = now - 3 * 86_400;
    let history_date =
        crate::utils::business_time::business_date_for_timestamp(history_ts, &settings);
    let session = "sess-history-remove";

    db.sync_dirty_sessions(
        vec![make_dirty_session(
            session,
            "fp-1",
            vec![
                make_scan_request(session, "msg-old", history_ts, 100),
                make_scan_request(session, "msg-today", now, 200),
            ],
        )],
        vec![],
    )
    .expect("first sync");
    seed_materialization_state(&db, &history_date);
    let invalidation_before = invalidation_version(&db);

    // 文件重写后不再包含历史日那条请求 → 软删生效
    db.sync_dirty_sessions(
        vec![make_dirty_session(
            session,
            "fp-2",
            vec![make_scan_request(session, "msg-today", now, 200)],
        )],
        vec![],
    )
    .expect("second sync");

    let (version_after, _, present_after) = get_fact_state(&db, session, "msg-old");
    assert_eq!(present_after, 0, "removed history row must be soft-deleted");
    assert_eq!(version_after, 2, "soft delete must bump sync_version");

    assert!(db
        .get_unified_day_materialization_state(&history_date)
        .expect("read materialization state")
        .is_none());
    assert_ne!(invalidation_version(&db), invalidation_before);
}

#[test]
fn local_merge_cache_generation_tracks_only_source_tables_transactionally() {
    let (_tmp, db) = temp_db();
    let initial = db
        .get_merge_cache_signature()
        .expect("load initial signature")
        .merge_cache_generation;

    insert_request_fact(
        &db,
        "sess-generation",
        "msg-1",
        "/tmp/generation.jsonl",
        true,
        100,
    );
    let after_insert = db
        .get_merge_cache_signature()
        .expect("load signature after insert")
        .merge_cache_generation;
    assert!(after_insert > initial);

    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE local_request_facts SET output_tokens = output_tokens + 1
             WHERE session_id = 'sess-generation'",
            [],
        )
        .expect("update source fact");
    }
    let after_update = db
        .get_merge_cache_signature()
        .expect("load signature after update")
        .merge_cache_generation;
    assert!(after_update > after_insert);

    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO unified_daily_summary (local_date, request_count)
             VALUES ('2026-06-01', 1)",
            [],
        )
        .expect("insert derived summary");
    }
    let after_derived_write = db
        .get_merge_cache_signature()
        .expect("load signature after derived write")
        .merge_cache_generation;
    assert_eq!(after_derived_write, after_update);

    {
        let conn = db.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .expect("open rollback transaction");
        tx.execute(
            "DELETE FROM local_request_facts WHERE session_id = 'sess-generation'",
            [],
        )
        .expect("delete source fact in rollback transaction");
        tx.rollback().expect("rollback source delete");
    }
    let after_rollback = db
        .get_merge_cache_signature()
        .expect("load signature after rollback")
        .merge_cache_generation;
    assert_eq!(after_rollback, after_update);

    let conn = db.conn.lock().unwrap();
    let trigger_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'trigger'
               AND name LIKE 'trg_%_merge_generation_%'",
            [],
            |row| row.get(0),
        )
        .expect("count generation triggers");
    assert_eq!(trigger_count, 12);
}
