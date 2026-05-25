use crate::models::ToolFilter;
use crate::session::{parse_session_file_for_storage, SessionFile};
use crate::session::{scan_session_files, LocalRequestRecord, SessionMeta};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

static GLOBAL_LOCAL_USAGE_DB: OnceLock<Arc<LocalUsageDatabase>> = OnceLock::new();

#[derive(Debug, Clone)]
struct DirtySessionSync {
    session: SessionFile,
    meta: SessionMeta,
    requests: Vec<LocalRequestRecord>,
    project_key: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncExportSession {
    pub session_id: String,
    pub tool: String,
    pub project_key: Option<String>,
    pub project_name: Option<String>,
    pub start_time: i64,
    pub end_time: i64,
    pub request_count: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_create_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_tokens: u64,
    pub model_list: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncExportRequest {
    pub request_key: String,
    pub session_id: String,
    pub tool: String,
    pub project_key: Option<String>,
    pub timestamp: i64,
    pub message_id: Option<String>,
    pub dedupe_key: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    pub is_subagent: bool,
    pub source_kind: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncExportData {
    pub sessions: Vec<SyncExportSession>,
    pub requests: Vec<SyncExportRequest>,
}

#[derive(Debug, Clone)]
pub struct SyncOutboxBatch {
    pub request_events: Vec<SyncExportRequest>,
    pub session_events: Vec<SyncExportSession>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSyncDevice {
    pub device_id: String,
    pub last_seen_at: Option<i64>,
    pub last_export_seq: i64,
    pub sync_status: String,
    pub updated_at: i64,
}

pub struct LocalUsageDatabase {
    conn: Arc<Mutex<Connection>>,
}

impl LocalUsageDatabase {
    pub fn get_global() -> Result<Arc<Self>, String> {
        if let Some(db) = GLOBAL_LOCAL_USAGE_DB.get() {
            return Ok(db.clone());
        }
        let db = Arc::new(Self::new()?);
        let _ = GLOBAL_LOCAL_USAGE_DB.set(db.clone());
        Ok(db)
    }

    pub fn new() -> Result<Self, String> {
        let db_path = Self::db_path()?;
        Self::new_with_path(&db_path)
    }

    fn new_with_path(path: &PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create local usage DB dir: {}", e))?;
        }

        let conn =
            Connection::open(path).map_err(|e| format!("Failed to open local usage DB: {}", e))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("Failed to enable WAL on local usage DB: {}", e))?;
        conn.busy_timeout(Duration::from_secs(30))
            .map_err(|e| format!("Failed to set local usage DB busy timeout: {}", e))?;

        Self::create_tables(&conn)?;
        Self::migrate_schema(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn db_path() -> Result<PathBuf, String> {
        let home = dirs::home_dir().ok_or_else(|| "Home directory not found".to_string())?;
        Ok(home.join(".usagemeter").join("local_usage.db"))
    }

    fn create_tables(conn: &Connection) -> Result<(), String> {
        Self::create_cache_tables(conn)?;
        Self::create_sync_v2_tables(conn)?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS local_sync_state (
                state_key TEXT PRIMARY KEY,
                state_value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .map_err(|e| format!("Failed to create local usage sync state table: {}", e))?;

        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO local_sync_state (state_key, state_value, updated_at)
             VALUES ('schema_version', '1', ?1)
             ON CONFLICT(state_key) DO NOTHING",
            params![now],
        )
        .map_err(|e| format!("Failed to initialize local usage schema state: {}", e))?;

        Ok(())
    }

    fn create_cache_tables(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS local_source_files (
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
                sync_error TEXT,
                deleted_at INTEGER,
                deletion_reason TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_local_source_files_session_id
                ON local_source_files(session_id);
            CREATE INDEX IF NOT EXISTS idx_local_source_files_tool
                ON local_source_files(tool);
            CREATE INDEX IF NOT EXISTS idx_local_source_files_project_key
                ON local_source_files(project_key);
            -- idx_local_source_files_deleted_at 在 v5 迁移分支创建；
            -- 老库的 local_source_files 在 schema_version<5 时没有 deleted_at 列，
            -- 在这里建索引会立即炸（CREATE TABLE IF NOT EXISTS 不会补列）。

            CREATE TABLE IF NOT EXISTS local_sessions (
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
            CREATE INDEX IF NOT EXISTS idx_local_sessions_tool
                ON local_sessions(tool);
            CREATE INDEX IF NOT EXISTS idx_local_sessions_project_key
                ON local_sessions(project_key);
            CREATE INDEX IF NOT EXISTS idx_local_sessions_end_time
                ON local_sessions(end_time);

            CREATE TABLE IF NOT EXISTS local_request_facts (
                request_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                tool TEXT NOT NULL,
                project_key TEXT,
                timestamp INTEGER NOT NULL,
                message_id TEXT,
                dedupe_key TEXT NOT NULL,
                request_key TEXT,
                model TEXT NOT NULL DEFAULT '',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                source_file_id INTEGER,
                source_file_path TEXT,
                source_file_present INTEGER NOT NULL DEFAULT 1,
                source_offset INTEGER,
                event_index INTEGER,
                is_subagent INTEGER NOT NULL DEFAULT 0,
                raw_event_kind TEXT NOT NULL DEFAULT '',
                sync_version INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                UNIQUE(tool, dedupe_key)
            );
            CREATE INDEX IF NOT EXISTS idx_local_request_facts_timestamp
                ON local_request_facts(timestamp);
            CREATE INDEX IF NOT EXISTS idx_local_request_facts_session_id
                ON local_request_facts(session_id);
            CREATE INDEX IF NOT EXISTS idx_local_request_facts_project_key
                ON local_request_facts(project_key);
            CREATE INDEX IF NOT EXISTS idx_local_request_facts_tool_timestamp
                ON local_request_facts(tool, timestamp);
            CREATE INDEX IF NOT EXISTS idx_local_request_facts_model
                ON local_request_facts(model);
            -- idx_local_request_facts_request_key / _source_file_present 在 v5 迁移分支创建；
            -- 老库的 local_request_facts 缺这两列，在 create_cache_tables 阶段建索引会炸。

            CREATE TABLE IF NOT EXISTS local_sync_cursors (
                cursor_key TEXT PRIMARY KEY,
                tool TEXT NOT NULL,
                file_path TEXT NOT NULL,
                last_offset INTEGER,
                last_event_index INTEGER,
                last_seen_timestamp INTEGER,
                last_seen_dedupe_key TEXT,
                payload_json TEXT,
                updated_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_local_sync_cursors_tool
                ON local_sync_cursors(tool);
            CREATE INDEX IF NOT EXISTS idx_local_sync_cursors_file_path
                ON local_sync_cursors(file_path);

            CREATE TABLE IF NOT EXISTS remote_devices (
                device_id TEXT PRIMARY KEY,
                last_seen_at INTEGER,
                last_export_seq INTEGER NOT NULL DEFAULT 0,
                sync_status TEXT NOT NULL DEFAULT 'ready',
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS remote_request_facts (
                request_key TEXT NOT NULL,
                origin_device_id TEXT NOT NULL,
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
                is_subagent INTEGER NOT NULL DEFAULT 0,
                source_kind TEXT NOT NULL DEFAULT 'remote_sync',
                imported_at INTEGER NOT NULL,
                export_seq INTEGER NOT NULL,
                PRIMARY KEY(origin_device_id, request_key)
            );
            CREATE INDEX IF NOT EXISTS idx_remote_request_facts_timestamp
                ON remote_request_facts(timestamp);
            CREATE INDEX IF NOT EXISTS idx_remote_request_facts_session_id
                ON remote_request_facts(session_id);
            CREATE INDEX IF NOT EXISTS idx_remote_request_facts_tool_timestamp
                ON remote_request_facts(tool, timestamp);

            CREATE TABLE IF NOT EXISTS remote_sessions (
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
            CREATE INDEX IF NOT EXISTS idx_remote_sessions_tool
                ON remote_sessions(tool);
            CREATE INDEX IF NOT EXISTS idx_remote_sessions_end_time
                ON remote_sessions(end_time);

            CREATE TABLE IF NOT EXISTS webdav_sync_state (
                state_key TEXT PRIMARY KEY,
                state_value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .map_err(|e| format!("Failed to create local usage tables: {}", e))?;
        Ok(())
    }

    fn create_sync_v2_tables(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sync_outbox_request_events (
                event_id TEXT PRIMARY KEY,
                origin_device_id TEXT NOT NULL,
                request_key TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                event_version INTEGER NOT NULL,
                queued_at INTEGER NOT NULL,
                batched_seq INTEGER,
                uploaded_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_sync_outbox_request_events_uploaded_at
                ON sync_outbox_request_events(uploaded_at, queued_at);

            CREATE TABLE IF NOT EXISTS sync_outbox_session_events (
                session_event_id TEXT PRIMARY KEY,
                origin_device_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                session_version INTEGER NOT NULL,
                queued_at INTEGER NOT NULL,
                batched_seq INTEGER,
                uploaded_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_sync_outbox_session_events_uploaded_at
                ON sync_outbox_session_events(uploaded_at, queued_at);

            CREATE TABLE IF NOT EXISTS sync_device_cursors (
                device_id TEXT PRIMARY KEY,
                last_imported_batch_seq INTEGER NOT NULL DEFAULT 0,
                last_imported_snapshot_seq INTEGER,
                last_seen_instance_id TEXT,
                last_seen_at INTEGER NOT NULL,
                last_status TEXT NOT NULL DEFAULT 'idle',
                last_error TEXT
            );

            CREATE TABLE IF NOT EXISTS sync_batch_history (
                batch_seq INTEGER PRIMARY KEY,
                request_event_count INTEGER NOT NULL,
                session_event_count INTEGER NOT NULL,
                exported_at INTEGER NOT NULL,
                remote_path TEXT NOT NULL,
                status TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sync_settings_state (
                document_key TEXT PRIMARY KEY,
                local_version INTEGER NOT NULL,
                remote_version INTEGER,
                last_pushed_at INTEGER,
                last_pulled_at INTEGER
            );
            "#,
        )
        .map_err(|e| format!("Failed to create sync V2 tables: {}", e))?;
        Ok(())
    }

    fn load_schema_version(conn: &Connection) -> Result<i64, String> {
        let version = conn
            .query_row(
                "SELECT state_value FROM local_sync_state WHERE state_key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| format!("Failed to query local usage schema version: {}", e))?;

        Ok(version
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(1))
    }

    fn migrate_schema(conn: &Connection) -> Result<(), String> {
        let schema_version = Self::load_schema_version(conn)?;
        if schema_version >= 5 {
            return Ok(());
        }

        if schema_version < 2 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start local usage schema migration: {}", e))?;

            tx.execute_batch(
                r#"
                DROP TABLE IF EXISTS local_request_facts;
                DELETE FROM local_sessions;
                DELETE FROM local_source_files;
                DELETE FROM local_sync_cursors;
                "#,
            )
            .map_err(|e| format!("Failed to reset local usage cache during migration: {}", e))?;

            Self::create_cache_tables(&tx)?;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '2', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| {
                format!(
                    "Failed to update migrated local usage schema version: {}",
                    e
                )
            })?;

            tx.commit()
                .map_err(|e| format!("Failed to commit local usage schema migration: {}", e))?;
        }

        if schema_version < 3 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start remote device schema migration: {}", e))?;

            tx.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS remote_devices_v3 (
                    device_id TEXT PRIMARY KEY,
                    last_seen_at INTEGER,
                    last_export_seq INTEGER NOT NULL DEFAULT 0,
                    sync_status TEXT NOT NULL DEFAULT 'ready',
                    updated_at INTEGER NOT NULL
                );
                INSERT INTO remote_devices_v3 (
                    device_id, last_seen_at, last_export_seq, sync_status, updated_at
                )
                SELECT device_id, last_seen_at, last_export_seq, sync_status, updated_at
                FROM remote_devices;
                DROP TABLE remote_devices;
                ALTER TABLE remote_devices_v3 RENAME TO remote_devices;
                "#,
            )
            .map_err(|e| format!("Failed to migrate remote devices schema: {}", e))?;

            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '3', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update remote device schema version: {}", e))?;

            tx.commit()
                .map_err(|e| format!("Failed to commit remote device schema migration: {}", e))?;
        }

        if schema_version < 4 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start sync V2 schema migration: {}", e))?;

            Self::create_sync_v2_tables(&tx)?;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '4', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update sync V2 schema version: {}", e))?;

            tx.commit()
                .map_err(|e| format!("Failed to commit sync V2 schema migration: {}", e))?;
        }

        if schema_version < 5 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v5 schema migration: {}", e))?;

            // 软删除 + request_key 持久化所需的新列。
            // ALTER TABLE ADD COLUMN 在 SQLite 中是幂等失败（重复加列报错），
            // 所以先按表/列名检测，避免在重启后再次迁移时报错。
            Self::add_column_if_missing(&tx, "local_request_facts", "request_key", "TEXT")?;
            Self::add_column_if_missing(&tx, "local_request_facts", "source_file_path", "TEXT")?;
            Self::add_column_if_missing(
                &tx,
                "local_request_facts",
                "source_file_present",
                "INTEGER NOT NULL DEFAULT 1",
            )?;
            Self::add_column_if_missing(&tx, "local_source_files", "deleted_at", "INTEGER")?;
            Self::add_column_if_missing(&tx, "local_source_files", "deletion_reason", "TEXT")?;

            tx.execute_batch(
                r#"
                CREATE INDEX IF NOT EXISTS idx_local_request_facts_request_key
                    ON local_request_facts(request_key);
                CREATE INDEX IF NOT EXISTS idx_local_request_facts_source_file_present
                    ON local_request_facts(source_file_present);
                CREATE INDEX IF NOT EXISTS idx_local_source_files_deleted_at
                    ON local_source_files(deleted_at);
                "#,
            )
            .map_err(|e| format!("Failed to create v5 indexes: {}", e))?;

            // 回填 request_key：message_id 非空走 tool:message_id，否则走 9 元组。
            // 这一规则必须与 unified_usage::service::request_key_for_local 保持一致。
            tx.execute(
                "UPDATE local_request_facts
                 SET request_key = CASE
                     WHEN message_id IS NOT NULL AND TRIM(message_id) != ''
                       THEN tool || ':' || message_id
                     ELSE tool || ':' || session_id || ':' || timestamp || ':' || model
                          || ':' || input_tokens || ':' || output_tokens
                          || ':' || cache_create_tokens || ':' || cache_read_tokens
                          || ':' || total_tokens
                 END
                 WHERE request_key IS NULL OR request_key = ''",
                [],
            )
            .map_err(|e| format!("Failed to backfill request_key: {}", e))?;

            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '5', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v5 schema version: {}", e))?;

            tx.commit()
                .map_err(|e| format!("Failed to commit v5 schema migration: {}", e))?;
        }
        Ok(())
    }

    fn add_column_if_missing(
        tx: &rusqlite::Transaction<'_>,
        table: &str,
        column: &str,
        column_def: &str,
    ) -> Result<(), String> {
        let exists: bool = tx
            .prepare(&format!("PRAGMA table_info({})", table))
            .map_err(|e| format!("Failed to inspect table {}: {}", table, e))?
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("Failed to read columns of {}: {}", table, e))?
            .filter_map(|name| name.ok())
            .any(|name| name == column);
        if exists {
            return Ok(());
        }
        tx.execute(
            &format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, column_def),
            [],
        )
        .map_err(|e| format!("Failed to add column {}.{}: {}", table, column, e))?;
        Ok(())
    }

    fn upsert_sync_state(
        tx: &rusqlite::Transaction<'_>,
        state_key: &str,
        state_value: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        tx.execute(
            "INSERT INTO local_sync_state (state_key, state_value, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(state_key) DO UPDATE
             SET state_value = excluded.state_value,
                 updated_at = excluded.updated_at",
            params![state_key, state_value, updated_at],
        )
        .map_err(|e| format!("Failed to upsert local sync state `{state_key}`: {}", e))?;
        Ok(())
    }

    fn load_session_fingerprints(&self) -> Result<HashMap<String, String>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT session_id, fingerprint
                 FROM local_source_files
                 WHERE file_role = 'session_group' AND deleted_at IS NULL",
            )
            .map_err(|e| format!("Failed to prepare load_session_fingerprints: {}", e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("Failed to query session fingerprints: {}", e))?;

        let mut result = HashMap::new();
        for row in rows {
            let (session_id, fingerprint) =
                row.map_err(|e| format!("Failed to read session fingerprint row: {}", e))?;
            result.insert(session_id, fingerprint);
        }
        Ok(result)
    }

    pub fn sync_from_scanner(&self) -> Result<(), String> {
        let scanned = scan_session_files();
        let scanned_map: HashMap<String, SessionFile> = scanned
            .into_iter()
            .map(|session| (session.session_id.clone(), session))
            .collect();
        let current_ids: HashSet<String> = scanned_map.keys().cloned().collect();
        let cached_fingerprints = self.load_session_fingerprints()?;
        let cached_ids: HashSet<String> = cached_fingerprints.keys().cloned().collect();

        let removed_ids: Vec<String> = cached_ids.difference(&current_ids).cloned().collect();
        let mut dirty_ids: Vec<String> = scanned_map
            .iter()
            .filter_map(|(session_id, session)| {
                let fingerprint = session.fingerprint.to_string();
                match cached_fingerprints.get(session_id) {
                    Some(existing) if existing == &fingerprint => None,
                    _ => Some(session_id.clone()),
                }
            })
            .collect();
        dirty_ids.sort();

        if dirty_ids.is_empty() && removed_ids.is_empty() {
            return Ok(());
        }

        let dirty_sessions: Vec<DirtySessionSync> = dirty_ids
            .into_iter()
            .filter_map(|session_id| scanned_map.get(&session_id).cloned())
            .map(|session| {
                let (meta, requests) = parse_session_file_for_storage(&session);
                let project_key = meta
                    .project_name
                    .clone()
                    .or(meta.cwd.clone())
                    .unwrap_or_else(|| "unknown_project".to_string());

                DirtySessionSync {
                    session,
                    meta,
                    requests,
                    project_key,
                }
            })
            .collect();
        let dirty_session_count = dirty_sessions.len();
        let removed_session_count = removed_ids.len();

        let now = chrono::Utc::now().timestamp();
        let origin_device_id = self
            .get_webdav_sync_state("device_id")?
            .map(|value| crate::models::normalize_sync_device_id(&value))
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                crate::models::normalize_sync_device_id(&crate::models::default_sync_device_id())
            });
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start local usage transaction: {}", e))?;

        for session_id in &removed_ids {
            // 软删除：不再 DELETE 事实表，仅标记 source 文件已消失。
            // - local_request_facts：保留行，source_file_present 置 0
            // - local_sessions：保留摘要，不动（用户在统计页仍能看到历史）
            // - local_source_files：保留行，记录 deleted_at；不删，便于「revive」时复用同一行
            tx.execute(
                "UPDATE local_request_facts
                 SET source_file_present = 0
                 WHERE session_id = ?1",
                params![session_id],
            )
            .map_err(|e| format!("Failed to soft-delete local request facts: {}", e))?;
            tx.execute(
                "UPDATE local_source_files
                 SET deleted_at = ?2,
                     deletion_reason = 'missing'
                 WHERE session_id = ?1 AND deleted_at IS NULL",
                params![session_id, now],
            )
            .map_err(|e| format!("Failed to mark local source file removed: {}", e))?;
        }

        for dirty_session in dirty_sessions {
            let DirtySessionSync {
                session,
                meta,
                requests,
                project_key,
            } = dirty_session;
            let fingerprint = session.fingerprint.to_string();

            // 抓取本会话历史 dedupe_key 集合，便于：
            // 1. 走 upsert 路径而不 delete-then-insert，保留 created_at（孤立清理的依据）
            // 2. 把"新 JSONL 内容里没出现的旧 message_id"标记为 source_file_present = 0
            let existing_dedupe_keys: HashSet<String> = {
                let mut stmt = tx
                    .prepare("SELECT dedupe_key FROM local_request_facts WHERE session_id = ?1")
                    .map_err(|e| format!("Failed to prepare existing dedupe_key query: {}", e))?;
                let rows = stmt
                    .query_map(params![session.session_id.as_str()], |row| {
                        row.get::<_, String>(0)
                    })
                    .map_err(|e| format!("Failed to query existing dedupe_keys: {}", e))?;
                let mut keys = HashSet::new();
                for row in rows {
                    let key =
                        row.map_err(|e| format!("Failed to read existing dedupe_key row: {}", e))?;
                    keys.insert(key);
                }
                keys
            };
            // local_sessions：摘要可以直接覆盖
            tx.execute(
                "DELETE FROM local_sessions WHERE session_id = ?1",
                params![session.session_id.as_str()],
            )
            .map_err(|e| format!("Failed to clear stale local session row: {}", e))?;

            // local_source_files：用 file_path upsert（file_path 是 UNIQUE），
            // 复用旧行可保留历史指纹链；同时清掉旧的 deleted_at
            tx.execute(
                "INSERT INTO local_source_files (
                    tool, session_id, project_key, file_path, file_role, file_size,
                    mtime_epoch, fingerprint, last_scanned_at, last_synced_at, sync_status,
                    deleted_at, deletion_reason
                ) VALUES (?1, ?2, ?3, ?4, 'session_group', ?5, ?6, ?7, ?8, ?9, 'ready', NULL, NULL)
                ON CONFLICT(file_path) DO UPDATE SET
                    tool = excluded.tool,
                    session_id = excluded.session_id,
                    project_key = excluded.project_key,
                    file_size = excluded.file_size,
                    mtime_epoch = excluded.mtime_epoch,
                    fingerprint = excluded.fingerprint,
                    last_scanned_at = excluded.last_scanned_at,
                    last_synced_at = excluded.last_synced_at,
                    sync_status = 'ready',
                    deleted_at = NULL,
                    deletion_reason = NULL",
                params![
                    session.tool.as_str(),
                    session.session_id.as_str(),
                    project_key.as_str(),
                    session.file_path.as_str(),
                    session.file_size as i64,
                    session.last_modified,
                    fingerprint,
                    now,
                    now
                ],
            )
            .map_err(|e| format!("Failed to upsert local source row: {}", e))?;

            let model_list_json = serde_json::to_string(&meta.models)
                .map_err(|e| format!("Failed to serialize model list: {}", e))?;
            let total_tokens = meta.total_input_tokens
                + meta.total_output_tokens
                + meta.total_cache_create_tokens
                + meta.total_cache_read_tokens;

            tx.execute(
                "INSERT INTO local_sessions (
                    session_id, tool, project_key, cwd, project_name, topic, last_prompt,
                    session_name, primary_file_path, file_size, last_modified, start_time, end_time,
                    request_count, total_input_tokens, total_output_tokens,
                    total_cache_create_tokens, total_cache_read_tokens, total_tokens,
                    model_list_json, source_kind, sync_version, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                          ?16, ?17, ?18, ?19, ?20, ?21, 1, ?22)",
                params![
                    meta.session_id.as_str(),
                    meta.tool.as_str(),
                    project_key.as_str(),
                    meta.cwd.as_deref(),
                    meta.project_name.as_deref(),
                    meta.topic.as_deref(),
                    meta.last_prompt.as_deref(),
                    meta.session_name.as_deref(),
                    meta.file_path.as_str(),
                    meta.file_size as i64,
                    meta.last_modified,
                    meta.start_time,
                    meta.end_time,
                    meta.message_count as i64,
                    meta.total_input_tokens as i64,
                    meta.total_output_tokens as i64,
                    meta.total_cache_create_tokens as i64,
                    meta.total_cache_read_tokens as i64,
                    total_tokens as i64,
                    model_list_json.as_str(),
                    meta.source.as_str(),
                    now
                ],
            )
            .map_err(|e| format!("Failed to insert local session row: {}", e))?;
            let session_export = SyncExportSession {
                session_id: meta.session_id.clone(),
                tool: meta.tool.clone(),
                project_key: Some(project_key.clone()),
                project_name: meta.project_name.clone(),
                start_time: meta.start_time,
                end_time: meta.end_time,
                request_count: meta.message_count,
                total_input_tokens: meta.total_input_tokens,
                total_output_tokens: meta.total_output_tokens,
                total_cache_create_tokens: meta.total_cache_create_tokens,
                total_cache_read_tokens: meta.total_cache_read_tokens,
                total_tokens,
                model_list: meta.models.clone(),
            };
            let session_payload = serde_json::to_string(&session_export)
                .map_err(|e| format!("Failed to serialize sync session outbox payload: {}", e))?;
            tx.execute(
                "INSERT INTO sync_outbox_session_events (
                    session_event_id, origin_device_id, session_id, payload_json,
                    session_version, queued_at, batched_seq, uploaded_at
                 ) VALUES (?1, ?2, ?3, ?4, 1, ?5, NULL, NULL)
                 ON CONFLICT(session_event_id) DO UPDATE SET
                    payload_json = excluded.payload_json,
                    session_version = CASE
                        WHEN sync_outbox_session_events.payload_json != excluded.payload_json
                        THEN sync_outbox_session_events.session_version + 1
                        ELSE sync_outbox_session_events.session_version
                    END,
                    queued_at = CASE
                        WHEN sync_outbox_session_events.payload_json != excluded.payload_json
                        THEN excluded.queued_at
                        ELSE sync_outbox_session_events.queued_at
                    END,
                    batched_seq = CASE
                        WHEN sync_outbox_session_events.payload_json != excluded.payload_json
                        THEN NULL
                        ELSE sync_outbox_session_events.batched_seq
                    END,
                    uploaded_at = CASE
                        WHEN sync_outbox_session_events.payload_json != excluded.payload_json
                        THEN NULL
                        ELSE sync_outbox_session_events.uploaded_at
                    END",
                params![
                    format!("{}:{}", origin_device_id, meta.session_id),
                    origin_device_id.as_str(),
                    meta.session_id.as_str(),
                    session_payload.as_str(),
                    now
                ],
            )
            .map_err(|e| format!("Failed to enqueue sync session outbox payload: {}", e))?;

            let mut seen_dedupe_keys: HashSet<String> = HashSet::new();
            for (idx, request) in requests.iter().enumerate() {
                let request_identity = if request.message_id.trim().is_empty() {
                    format!(
                        "ts:{}:idx:{}:model:{}:tokens:{}",
                        request.timestamp, idx, request.model, request.total_tokens
                    )
                } else {
                    request.message_id.clone()
                };
                let dedupe_key = format!("{}:{}", request.session_id, request_identity);
                let request_id = format!("{}:{}", request.tool, dedupe_key);
                // request_key：与合并层一致的全局键，落库一列；
                // 规则必须与 unified_usage::service::request_key_for_local 保持同步。
                let request_key = if request.message_id.trim().is_empty() {
                    format!(
                        "{}:{}:{}:{}:{}:{}:{}:{}:{}",
                        request.tool,
                        request.session_id,
                        request.timestamp,
                        request.model,
                        request.input_tokens,
                        request.output_tokens,
                        request.cache_create_tokens,
                        request.cache_read_tokens,
                        request.total_tokens
                    )
                } else {
                    format!("{}:{}", request.tool, request.message_id)
                };
                seen_dedupe_keys.insert(dedupe_key.clone());
                tx.execute(
                    "INSERT INTO local_request_facts (
                        request_id, session_id, tool, project_key, timestamp, message_id, dedupe_key,
                        request_key, model, input_tokens, output_tokens, cache_create_tokens,
                        cache_read_tokens, total_tokens, source_offset, event_index, is_subagent,
                        raw_event_kind, sync_version, created_at, source_file_path,
                        source_file_present
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                              NULL, ?15, ?16, 'request', 1, ?17, ?18, 1)
                    ON CONFLICT(tool, dedupe_key) DO UPDATE SET
                        session_id = excluded.session_id,
                        project_key = excluded.project_key,
                        timestamp = excluded.timestamp,
                        message_id = excluded.message_id,
                        request_key = excluded.request_key,
                        model = excluded.model,
                        input_tokens = excluded.input_tokens,
                        output_tokens = excluded.output_tokens,
                        cache_create_tokens = excluded.cache_create_tokens,
                        cache_read_tokens = excluded.cache_read_tokens,
                        total_tokens = excluded.total_tokens,
                        event_index = excluded.event_index,
                        is_subagent = excluded.is_subagent,
                        sync_version = sync_version + 1,
                        source_file_path = excluded.source_file_path,
                        source_file_present = 1",
                    params![
                        request_id.as_str(),
                        request.session_id.as_str(),
                        request.tool.as_str(),
                        project_key.as_str(),
                        request.timestamp,
                        request.message_id.as_str(),
                        dedupe_key.as_str(),
                        request_key.as_str(),
                        request.model.as_str(),
                        request.input_tokens as i64,
                        request.output_tokens as i64,
                        request.cache_create_tokens as i64,
                        request.cache_read_tokens as i64,
                        request.total_tokens as i64,
                        idx as i64,
                        if request.is_subagent { 1 } else { 0 },
                        now,
                        session.file_path.as_str()
                    ],
                )
                .map_err(|e| format!("Failed to upsert local request fact: {}", e))?;

                let request_export = SyncExportRequest {
                    // outbox/远程导入侧也使用与合并层一致的全局键，
                    // 否则远端导入的记录在合并层会因 key 形态不同而无法与本地数据去重，
                    // 导致同一条请求双计。
                    request_key: request_key.clone(),
                    session_id: request.session_id.clone(),
                    tool: request.tool.clone(),
                    project_key: Some(project_key.clone()),
                    timestamp: request.timestamp,
                    message_id: if request.message_id.trim().is_empty() {
                        None
                    } else {
                        Some(request.message_id.clone())
                    },
                    dedupe_key: dedupe_key.clone(),
                    model: request.model.clone(),
                    input_tokens: request.input_tokens,
                    output_tokens: request.output_tokens,
                    cache_create_tokens: request.cache_create_tokens,
                    cache_read_tokens: request.cache_read_tokens,
                    total_tokens: request.total_tokens,
                    is_subagent: request.is_subagent,
                    source_kind: "local_usage".to_string(),
                };
                let request_payload = serde_json::to_string(&request_export).map_err(|e| {
                    format!("Failed to serialize sync request outbox payload: {}", e)
                })?;
                tx.execute(
                    "INSERT INTO sync_outbox_request_events (
                        event_id, origin_device_id, request_key, payload_json,
                        event_version, queued_at, batched_seq, uploaded_at
                     ) VALUES (?1, ?2, ?3, ?4, 1, ?5, NULL, NULL)
                     ON CONFLICT(event_id) DO UPDATE SET
                        payload_json = excluded.payload_json,
                        request_key = excluded.request_key,
                        event_version = excluded.event_version,
                        queued_at = excluded.queued_at,
                        batched_seq = NULL,
                        uploaded_at = NULL",
                    params![
                        // event_id 与 seed_sync_outbox_from_local 保持同一规范化形态
                        // （device_id:request_key），避免两条路径写入不同 event_id 造成 outbox 重复。
                        format!("{}:{}", origin_device_id, request_key),
                        origin_device_id.as_str(),
                        request_key.as_str(),
                        request_payload.as_str(),
                        now
                    ],
                )
                .map_err(|e| format!("Failed to enqueue sync request outbox payload: {}", e))?;
            }

            // 本次新内容里没出现的旧 dedupe_key → 标记为 source_file_present = 0。
            // 不 DELETE：用户可能用 /clear 清掉了上下文，但历史 request 已经发生过、应保留。
            let stale_keys: Vec<String> = existing_dedupe_keys
                .difference(&seen_dedupe_keys)
                .cloned()
                .collect();
            for stale_key in stale_keys {
                tx.execute(
                    "UPDATE local_request_facts
                     SET source_file_present = 0
                     WHERE tool = ?1 AND dedupe_key = ?2",
                    params![session.tool.as_str(), stale_key],
                )
                .map_err(|e| format!("Failed to soft-mark stale local request fact: {}", e))?;
            }
        }

        Self::upsert_sync_state(&tx, "last_sync_completed_at", &now.to_string(), now)?;
        Self::upsert_sync_state(
            &tx,
            "last_dirty_session_count",
            &dirty_session_count.to_string(),
            now,
        )?;
        Self::upsert_sync_state(
            &tx,
            "last_removed_session_count",
            &removed_session_count.to_string(),
            now,
        )?;
        Self::upsert_sync_state(&tx, "last_sync_mode", "session_rebuild_v1", now)?;

        tx.commit()
            .map_err(|e| format!("Failed to commit local usage sync: {}", e))?;
        Ok(())
    }

    pub fn reserve_sync_outbox_batch(
        &self,
        origin_device_id: &str,
        batch_seq: i64,
        max_request_events: usize,
        max_session_events: usize,
    ) -> Result<SyncOutboxBatch, String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start sync outbox reservation: {}", e))?;

        let mut request_ids = Vec::new();
        let mut request_events = Vec::new();
        {
            let mut stmt = tx
                .prepare(
                    "SELECT event_id, payload_json
                     FROM sync_outbox_request_events
                     WHERE origin_device_id = ?1 AND uploaded_at IS NULL AND batched_seq IS NULL
                     ORDER BY queued_at ASC
                     LIMIT ?2",
                )
                .map_err(|e| format!("Failed to prepare sync request outbox query: {}", e))?;
            let rows = stmt
                .query_map(
                    params![origin_device_id, max_request_events as i64],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(|e| format!("Failed to query sync request outbox: {}", e))?;
            for row in rows {
                let (event_id, payload_json) =
                    row.map_err(|e| format!("Failed to read sync request outbox row: {}", e))?;
                let payload: SyncExportRequest = serde_json::from_str(&payload_json)
                    .map_err(|e| format!("Failed to parse sync request outbox payload: {}", e))?;
                request_ids.push(event_id);
                request_events.push(payload);
            }
        }

        let mut session_ids = Vec::new();
        let mut session_events = Vec::new();
        {
            let mut stmt = tx
                .prepare(
                    "SELECT session_event_id, payload_json
                     FROM sync_outbox_session_events
                     WHERE origin_device_id = ?1 AND uploaded_at IS NULL AND batched_seq IS NULL
                     ORDER BY queued_at ASC
                     LIMIT ?2",
                )
                .map_err(|e| format!("Failed to prepare sync session outbox query: {}", e))?;
            let rows = stmt
                .query_map(
                    params![origin_device_id, max_session_events as i64],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(|e| format!("Failed to query sync session outbox: {}", e))?;
            for row in rows {
                let (event_id, payload_json) =
                    row.map_err(|e| format!("Failed to read sync session outbox row: {}", e))?;
                let payload: SyncExportSession = serde_json::from_str(&payload_json)
                    .map_err(|e| format!("Failed to parse sync session outbox payload: {}", e))?;
                session_ids.push(event_id);
                session_events.push(payload);
            }
        }

        for event_id in &request_ids {
            tx.execute(
                "UPDATE sync_outbox_request_events
                 SET batched_seq = ?2
                 WHERE event_id = ?1",
                params![event_id, batch_seq],
            )
            .map_err(|e| format!("Failed to reserve sync request outbox row: {}", e))?;
        }
        for event_id in &session_ids {
            tx.execute(
                "UPDATE sync_outbox_session_events
                 SET batched_seq = ?2
                 WHERE session_event_id = ?1",
                params![event_id, batch_seq],
            )
            .map_err(|e| format!("Failed to reserve sync session outbox row: {}", e))?;
        }

        Self::upsert_sync_state(&tx, "last_sync_outbox_reserved_at", &now.to_string(), now)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit sync outbox reservation: {}", e))?;

        Ok(SyncOutboxBatch {
            request_events,
            session_events,
        })
    }

    pub fn seed_sync_outbox_from_local(&self, origin_device_id: &str) -> Result<(), String> {
        if self.get_last_uploaded_batch_seq()? > 0 {
            return Ok(());
        }

        let export = self.get_sync_export_data()?;
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start sync outbox seed: {}", e))?;

        for session in export.sessions {
            let payload = serde_json::to_string(&session)
                .map_err(|e| format!("Failed to serialize sync session seed payload: {}", e))?;
            tx.execute(
                "INSERT INTO sync_outbox_session_events (
                    session_event_id, origin_device_id, session_id, payload_json,
                    session_version, queued_at, batched_seq, uploaded_at
                 ) VALUES (?1, ?2, ?3, ?4, 1, ?5, NULL, NULL)
                 ON CONFLICT(session_event_id) DO NOTHING",
                params![
                    format!("{}:{}", origin_device_id, session.session_id),
                    origin_device_id,
                    session.session_id.as_str(),
                    payload.as_str(),
                    now
                ],
            )
            .map_err(|e| format!("Failed to seed sync session outbox: {}", e))?;
        }

        for request in export.requests {
            let payload = serde_json::to_string(&request)
                .map_err(|e| format!("Failed to serialize sync request seed payload: {}", e))?;
            tx.execute(
                "INSERT INTO sync_outbox_request_events (
                    event_id, origin_device_id, request_key, payload_json,
                    event_version, queued_at, batched_seq, uploaded_at
                 ) VALUES (?1, ?2, ?3, ?4, 1, ?5, NULL, NULL)
                 ON CONFLICT(event_id) DO NOTHING",
                params![
                    format!("{}:{}", origin_device_id, request.request_key),
                    origin_device_id,
                    request.request_key.as_str(),
                    payload.as_str(),
                    now
                ],
            )
            .map_err(|e| format!("Failed to seed sync request outbox: {}", e))?;
        }

        tx.commit()
            .map_err(|e| format!("Failed to commit sync outbox seed: {}", e))?;
        Ok(())
    }

    pub fn release_sync_outbox_batch(&self, batch_seq: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start sync outbox release: {}", e))?;
        tx.execute(
            "UPDATE sync_outbox_request_events
             SET batched_seq = NULL
             WHERE batched_seq = ?1 AND uploaded_at IS NULL",
            params![batch_seq],
        )
        .map_err(|e| format!("Failed to release sync request outbox rows: {}", e))?;
        tx.execute(
            "UPDATE sync_outbox_session_events
             SET batched_seq = NULL
             WHERE batched_seq = ?1 AND uploaded_at IS NULL",
            params![batch_seq],
        )
        .map_err(|e| format!("Failed to release sync session outbox rows: {}", e))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit sync outbox release: {}", e))?;
        Ok(())
    }

    pub fn mark_sync_outbox_batch_uploaded(
        &self,
        batch_seq: i64,
        remote_path: &str,
        request_event_count: usize,
        session_event_count: usize,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start sync outbox upload mark: {}", e))?;
        tx.execute(
            "UPDATE sync_outbox_request_events
             SET uploaded_at = ?2
             WHERE batched_seq = ?1 AND uploaded_at IS NULL",
            params![batch_seq, now],
        )
        .map_err(|e| format!("Failed to mark sync request outbox rows uploaded: {}", e))?;
        tx.execute(
            "UPDATE sync_outbox_session_events
             SET uploaded_at = ?2
             WHERE batched_seq = ?1 AND uploaded_at IS NULL",
            params![batch_seq, now],
        )
        .map_err(|e| format!("Failed to mark sync session outbox rows uploaded: {}", e))?;
        tx.execute(
            "INSERT INTO sync_batch_history (
                batch_seq, request_event_count, session_event_count, exported_at, remote_path, status
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'uploaded')
             ON CONFLICT(batch_seq) DO UPDATE SET
                request_event_count = excluded.request_event_count,
                session_event_count = excluded.session_event_count,
                exported_at = excluded.exported_at,
                remote_path = excluded.remote_path,
                status = excluded.status",
            params![
                batch_seq,
                request_event_count as i64,
                session_event_count as i64,
                now,
                remote_path
            ],
        )
        .map_err(|e| format!("Failed to record sync batch history: {}", e))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit sync outbox upload mark: {}", e))?;
        Ok(())
    }

    /// 删除所有已成功上传的 outbox 事件行，防止表无限增长。
    /// 同时清理 sync_batch_history 中超出保留窗口的历史记录。
    /// 每次 sync 成功后调用。
    pub fn prune_uploaded_outbox(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start prune outbox transaction: {}", e))?;
        tx.execute(
            "DELETE FROM sync_outbox_request_events WHERE uploaded_at IS NOT NULL",
            [],
        )
        .map_err(|e| format!("Failed to prune uploaded request outbox: {}", e))?;
        tx.execute(
            "DELETE FROM sync_outbox_session_events WHERE uploaded_at IS NOT NULL",
            [],
        )
        .map_err(|e| format!("Failed to prune uploaded session outbox: {}", e))?;
        // 保留最新 200 条 batch 历史记录，其余删除
        tx.execute(
            "DELETE FROM sync_batch_history
             WHERE batch_seq < (
                 SELECT COALESCE(MIN(batch_seq), 0)
                 FROM (
                     SELECT batch_seq FROM sync_batch_history
                     ORDER BY batch_seq DESC
                     LIMIT 200
                 )
             )",
            [],
        )
        .map_err(|e| format!("Failed to prune sync batch history: {}", e))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit prune outbox transaction: {}", e))?;
        Ok(())
    }

    pub fn get_last_uploaded_batch_seq(&self) -> Result<i64, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(MAX(batch_seq), 0) FROM sync_batch_history WHERE status = 'uploaded'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| format!("Failed to read last uploaded batch seq: {}", e))
    }

    pub fn get_import_cursor(&self, device_id: &str) -> Result<i64, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT last_imported_batch_seq FROM sync_device_cursors WHERE device_id = ?1",
            params![device_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|value| value.unwrap_or(0))
        .map_err(|e| format!("Failed to read sync device cursor: {}", e))
    }

    pub fn upsert_import_cursor(
        &self,
        device_id: &str,
        instance_id: Option<&str>,
        batch_seq: i64,
        status: &str,
        last_error: Option<&str>,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_device_cursors (
                device_id, last_imported_batch_seq, last_imported_snapshot_seq,
                last_seen_instance_id, last_seen_at, last_status, last_error
             ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6)
             ON CONFLICT(device_id) DO UPDATE SET
                last_imported_batch_seq = MAX(sync_device_cursors.last_imported_batch_seq, excluded.last_imported_batch_seq),
                last_seen_instance_id = COALESCE(excluded.last_seen_instance_id, sync_device_cursors.last_seen_instance_id),
                last_seen_at = excluded.last_seen_at,
                last_status = excluded.last_status,
                last_error = excluded.last_error",
            params![device_id, batch_seq, instance_id, now, status, last_error],
        )
        .map_err(|e| format!("Failed to upsert sync device cursor: {}", e))?;
        Ok(())
    }

    pub fn get_all_request_records(
        &self,
        tool_filter: &ToolFilter,
    ) -> Result<Vec<LocalRequestRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let (sql, param) = match tool_filter {
            ToolFilter::All => (
                "SELECT session_id, tool, timestamp, message_id,
                        input_tokens, output_tokens, cache_create_tokens, cache_read_tokens,
                        total_tokens, model, is_subagent, request_key, source_file_present
                 FROM local_request_facts
                 ORDER BY timestamp ASC"
                    .to_string(),
                None,
            ),
            ToolFilter::Tool(tool) => (
                "SELECT session_id, tool, timestamp, message_id,
                        input_tokens, output_tokens, cache_create_tokens, cache_read_tokens,
                        total_tokens, model, is_subagent, request_key, source_file_present
                 FROM local_request_facts
                 WHERE tool = ?1
                 ORDER BY timestamp ASC"
                    .to_string(),
                Some(tool.clone()),
            ),
        };
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare get_all_request_records: {}", e))?;
        let mapper = |row: &rusqlite::Row<'_>| {
            let request_key: Option<String> = row.get(11)?;
            let source_file_present: Option<i64> = row.get(12)?;
            Ok(LocalRequestRecord {
                session_id: row.get(0)?,
                tool: row.get(1)?,
                timestamp: row.get(2)?,
                message_id: row.get(3)?,
                input_tokens: row.get::<_, i64>(4)? as u64,
                output_tokens: row.get::<_, i64>(5)? as u64,
                cache_create_tokens: row.get::<_, i64>(6)? as u64,
                cache_read_tokens: row.get::<_, i64>(7)? as u64,
                total_tokens: row.get::<_, i64>(8)? as u64,
                model: row.get(9)?,
                is_subagent: row.get::<_, i64>(10)? != 0,
                request_key: request_key.filter(|v| !v.trim().is_empty()),
                source_file_present: source_file_present.map(|v| v != 0),
            })
        };

        let rows = match param {
            Some(tool) => stmt
                .query_map(params![tool], mapper)
                .map_err(|e| format!("Failed to query local request records by tool: {}", e))?,
            None => stmt
                .query_map([], mapper)
                .map_err(|e| format!("Failed to query local request records: {}", e))?,
        };

        let mut result = Vec::new();
        for row in rows {
            result
                .push(row.map_err(|e| format!("Failed to read local request record row: {}", e))?);
        }
        Ok(result)
    }

    pub fn get_request_records_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<LocalRequestRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT session_id, tool, timestamp, message_id,
                        input_tokens, output_tokens, cache_create_tokens, cache_read_tokens,
                        total_tokens, model, is_subagent, request_key, source_file_present
                 FROM local_request_facts
                 WHERE session_id = ?1
                 ORDER BY timestamp ASC",
            )
            .map_err(|e| format!("Failed to prepare get_request_records_by_session: {}", e))?;
        let rows = stmt
            .query_map(params![session_id], |row| {
                let request_key: Option<String> = row.get(11)?;
                let source_file_present: Option<i64> = row.get(12)?;
                Ok(LocalRequestRecord {
                    session_id: row.get(0)?,
                    tool: row.get(1)?,
                    timestamp: row.get(2)?,
                    message_id: row.get(3)?,
                    input_tokens: row.get::<_, i64>(4)? as u64,
                    output_tokens: row.get::<_, i64>(5)? as u64,
                    cache_create_tokens: row.get::<_, i64>(6)? as u64,
                    cache_read_tokens: row.get::<_, i64>(7)? as u64,
                    total_tokens: row.get::<_, i64>(8)? as u64,
                    model: row.get(9)?,
                    is_subagent: row.get::<_, i64>(10)? != 0,
                    request_key: request_key.filter(|v| !v.trim().is_empty()),
                    source_file_present: source_file_present.map(|v| v != 0),
                })
            })
            .map_err(|e| format!("Failed to query local request records by session: {}", e))?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| {
                format!("Failed to read local request record by session row: {}", e)
            })?);
        }
        Ok(result)
    }

    pub fn get_all_sessions(&self, tool_filter: &ToolFilter) -> Result<Vec<SessionMeta>, String> {
        let conn = self.conn.lock().unwrap();
        let (sql, param) = match tool_filter {
            ToolFilter::All => (
                "SELECT session_id, tool, cwd, project_name, topic, last_prompt, session_name,
                        primary_file_path, file_size, last_modified, total_input_tokens,
                        total_output_tokens, total_cache_create_tokens, total_cache_read_tokens,
                        request_count, start_time, end_time, source_kind, model_list_json
                 FROM local_sessions
                 ORDER BY end_time DESC"
                    .to_string(),
                None,
            ),
            ToolFilter::Tool(tool) => (
                "SELECT session_id, tool, cwd, project_name, topic, last_prompt, session_name,
                        primary_file_path, file_size, last_modified, total_input_tokens,
                        total_output_tokens, total_cache_create_tokens, total_cache_read_tokens,
                        request_count, start_time, end_time, source_kind, model_list_json
                 FROM local_sessions
                 WHERE tool = ?1
                 ORDER BY end_time DESC"
                    .to_string(),
                Some(tool.clone()),
            ),
        };

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare get_all_sessions: {}", e))?;
        let mapper = |row: &rusqlite::Row<'_>| {
            let model_list_json: String = row.get(18)?;
            Ok(SessionMeta {
                session_id: row.get(0)?,
                tool: row.get(1)?,
                cwd: row.get(2)?,
                project_name: row.get(3)?,
                topic: row.get(4)?,
                last_prompt: row.get(5)?,
                session_name: row.get(6)?,
                file_path: row.get(7)?,
                file_size: row.get::<_, i64>(8)? as u64,
                last_modified: row.get(9)?,
                total_input_tokens: row.get::<_, i64>(10)? as u64,
                total_output_tokens: row.get::<_, i64>(11)? as u64,
                total_cache_create_tokens: row.get::<_, i64>(12)? as u64,
                total_cache_read_tokens: row.get::<_, i64>(13)? as u64,
                models: serde_json::from_str(&model_list_json).unwrap_or_default(),
                message_count: row.get::<_, i64>(14)? as u64,
                start_time: row.get(15)?,
                end_time: row.get(16)?,
                source: row.get(17)?,
                message_ids: Vec::new(),
            })
        };

        let rows = match param {
            Some(tool) => stmt
                .query_map(params![tool], mapper)
                .map_err(|e| format!("Failed to query local sessions by tool: {}", e))?,
            None => stmt
                .query_map([], mapper)
                .map_err(|e| format!("Failed to query local sessions: {}", e))?,
        };

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| format!("Failed to read local session row: {}", e))?);
        }
        Ok(result)
    }

    pub fn get_session_by_id(&self, session_id: &str) -> Result<Option<SessionMeta>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT session_id, tool, cwd, project_name, topic, last_prompt, session_name,
                        primary_file_path, file_size, last_modified, total_input_tokens,
                        total_output_tokens, total_cache_create_tokens, total_cache_read_tokens,
                        request_count, start_time, end_time, source_kind, model_list_json
                 FROM local_sessions
                 WHERE session_id = ?1",
            )
            .map_err(|e| format!("Failed to prepare get_session_by_id: {}", e))?;
        let session = stmt
            .query_row(params![session_id], |row| {
                let model_list_json: String = row.get(18)?;
                Ok(SessionMeta {
                    session_id: row.get(0)?,
                    tool: row.get(1)?,
                    cwd: row.get(2)?,
                    project_name: row.get(3)?,
                    topic: row.get(4)?,
                    last_prompt: row.get(5)?,
                    session_name: row.get(6)?,
                    file_path: row.get(7)?,
                    file_size: row.get::<_, i64>(8)? as u64,
                    last_modified: row.get(9)?,
                    total_input_tokens: row.get::<_, i64>(10)? as u64,
                    total_output_tokens: row.get::<_, i64>(11)? as u64,
                    total_cache_create_tokens: row.get::<_, i64>(12)? as u64,
                    total_cache_read_tokens: row.get::<_, i64>(13)? as u64,
                    models: serde_json::from_str(&model_list_json).unwrap_or_default(),
                    message_count: row.get::<_, i64>(14)? as u64,
                    start_time: row.get(15)?,
                    end_time: row.get(16)?,
                    source: row.get(17)?,
                    message_ids: Vec::new(),
                })
            })
            .optional()
            .map_err(|e| format!("Failed to query local session by id: {}", e))?;
        Ok(session)
    }

    pub fn get_sync_export_data(&self) -> Result<SyncExportData, String> {
        let conn = self.conn.lock().unwrap();

        let mut session_stmt = conn
            .prepare(
                "SELECT session_id, tool, project_key, project_name, start_time, end_time,
                        request_count, total_input_tokens, total_output_tokens,
                        total_cache_create_tokens, total_cache_read_tokens, total_tokens,
                        model_list_json
                 FROM local_sessions
                 ORDER BY end_time ASC",
            )
            .map_err(|e| format!("Failed to prepare sync session export: {}", e))?;
        let session_rows = session_stmt
            .query_map([], |row| {
                let model_list_json: String = row.get(12)?;
                Ok(SyncExportSession {
                    session_id: row.get(0)?,
                    tool: row.get(1)?,
                    project_key: row.get(2)?,
                    project_name: row.get(3)?,
                    start_time: row.get(4)?,
                    end_time: row.get(5)?,
                    request_count: row.get::<_, i64>(6)? as u64,
                    total_input_tokens: row.get::<_, i64>(7)? as u64,
                    total_output_tokens: row.get::<_, i64>(8)? as u64,
                    total_cache_create_tokens: row.get::<_, i64>(9)? as u64,
                    total_cache_read_tokens: row.get::<_, i64>(10)? as u64,
                    total_tokens: row.get::<_, i64>(11)? as u64,
                    model_list: serde_json::from_str(&model_list_json).unwrap_or_default(),
                })
            })
            .map_err(|e| format!("Failed to query sync session export: {}", e))?;

        let mut sessions = Vec::new();
        for row in session_rows {
            sessions.push(row.map_err(|e| format!("Failed to read sync session row: {}", e))?);
        }

        let mut request_stmt = conn
            .prepare(
                "SELECT session_id, tool, project_key, timestamp, message_id, dedupe_key,
                        model, input_tokens, output_tokens, cache_create_tokens,
                        cache_read_tokens, total_tokens, is_subagent
                 FROM local_request_facts
                 ORDER BY timestamp ASC",
            )
            .map_err(|e| format!("Failed to prepare sync request export: {}", e))?;
        let request_rows = request_stmt
            .query_map([], |row| {
                let session_id: String = row.get(0)?;
                let tool: String = row.get(1)?;
                let timestamp: i64 = row.get(3)?;
                let message_id: Option<String> = row.get(4)?;
                let model: String = row.get(6)?;
                let input_tokens = row.get::<_, i64>(7)? as u64;
                let output_tokens = row.get::<_, i64>(8)? as u64;
                let total_tokens = row.get::<_, i64>(11)? as u64;
                let request_key = match message_id.as_deref() {
                    Some(value) if !value.trim().is_empty() => format!("{}:{}", tool, value),
                    _ => format!(
                        "{}:{}:{}:{}:{}:{}:{}:{}:{}",
                        tool,
                        session_id,
                        timestamp,
                        model,
                        input_tokens,
                        output_tokens,
                        row.get::<_, i64>(9)? as u64,
                        row.get::<_, i64>(10)? as u64,
                        total_tokens
                    ),
                };

                Ok(SyncExportRequest {
                    request_key,
                    session_id,
                    tool,
                    project_key: row.get(2)?,
                    timestamp,
                    message_id,
                    dedupe_key: row.get(5)?,
                    model,
                    input_tokens,
                    output_tokens,
                    cache_create_tokens: row.get::<_, i64>(9)? as u64,
                    cache_read_tokens: row.get::<_, i64>(10)? as u64,
                    total_tokens,
                    is_subagent: row.get::<_, i64>(12)? != 0,
                    source_kind: "local_usage".to_string(),
                })
            })
            .map_err(|e| format!("Failed to query sync request export: {}", e))?;

        let mut requests = Vec::new();
        for row in request_rows {
            requests.push(row.map_err(|e| format!("Failed to read sync request row: {}", e))?);
        }

        Ok(SyncExportData { sessions, requests })
    }

    pub fn import_remote_sync_data(
        &self,
        device_id: &str,
        export_seq: i64,
        data: &SyncExportData,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start remote sync import: {}", e))?;

        tx.execute(
            "INSERT INTO remote_devices (
                device_id, last_seen_at, last_export_seq, sync_status, updated_at
             ) VALUES (?1, ?2, ?3, 'ready', ?4)
             ON CONFLICT(device_id) DO UPDATE SET
                last_seen_at = excluded.last_seen_at,
                last_export_seq = MAX(remote_devices.last_export_seq, excluded.last_export_seq),
                sync_status = 'ready',
                updated_at = excluded.updated_at",
            params![device_id, now, export_seq, now],
        )
        .map_err(|e| format!("Failed to upsert remote device: {}", e))?;

        for session in &data.sessions {
            let model_list_json = serde_json::to_string(&session.model_list)
                .map_err(|e| format!("Failed to serialize remote session models: {}", e))?;
            tx.execute(
                "INSERT INTO remote_sessions (
                    origin_device_id, session_id, tool, project_key, project_name, start_time,
                    end_time, request_count, total_input_tokens, total_output_tokens,
                    total_cache_create_tokens, total_cache_read_tokens, total_tokens,
                    model_list_json, imported_at, export_seq
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                 ON CONFLICT(origin_device_id, session_id) DO UPDATE SET
                    tool = excluded.tool,
                    project_key = excluded.project_key,
                    project_name = excluded.project_name,
                    start_time = excluded.start_time,
                    end_time = excluded.end_time,
                    request_count = excluded.request_count,
                    total_input_tokens = excluded.total_input_tokens,
                    total_output_tokens = excluded.total_output_tokens,
                    total_cache_create_tokens = excluded.total_cache_create_tokens,
                    total_cache_read_tokens = excluded.total_cache_read_tokens,
                    total_tokens = excluded.total_tokens,
                    model_list_json = excluded.model_list_json,
                    imported_at = excluded.imported_at,
                    export_seq = excluded.export_seq
                 WHERE excluded.export_seq >= remote_sessions.export_seq",
                params![
                    device_id,
                    session.session_id.as_str(),
                    session.tool.as_str(),
                    session.project_key.as_deref(),
                    session.project_name.as_deref(),
                    session.start_time,
                    session.end_time,
                    session.request_count as i64,
                    session.total_input_tokens as i64,
                    session.total_output_tokens as i64,
                    session.total_cache_create_tokens as i64,
                    session.total_cache_read_tokens as i64,
                    session.total_tokens as i64,
                    model_list_json.as_str(),
                    now,
                    export_seq
                ],
            )
            .map_err(|e| format!("Failed to upsert remote session: {}", e))?;
        }

        for request in &data.requests {
            tx.execute(
                "INSERT INTO remote_request_facts (
                    request_key, origin_device_id, session_id, tool, project_key, timestamp,
                    message_id, dedupe_key, model, input_tokens, output_tokens,
                    cache_create_tokens, cache_read_tokens, total_tokens, is_subagent,
                    source_kind, imported_at, export_seq
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
                 ON CONFLICT(origin_device_id, request_key) DO UPDATE SET
                    session_id = excluded.session_id,
                    tool = excluded.tool,
                    project_key = excluded.project_key,
                    timestamp = excluded.timestamp,
                    message_id = excluded.message_id,
                    dedupe_key = excluded.dedupe_key,
                    model = excluded.model,
                    input_tokens = excluded.input_tokens,
                    output_tokens = excluded.output_tokens,
                    cache_create_tokens = excluded.cache_create_tokens,
                    cache_read_tokens = excluded.cache_read_tokens,
                    total_tokens = excluded.total_tokens,
                    is_subagent = excluded.is_subagent,
                    source_kind = excluded.source_kind,
                    imported_at = excluded.imported_at,
                    export_seq = excluded.export_seq
                 WHERE excluded.export_seq >= remote_request_facts.export_seq",
                params![
                    request.request_key.as_str(),
                    device_id,
                    request.session_id.as_str(),
                    request.tool.as_str(),
                    request.project_key.as_deref(),
                    request.timestamp,
                    request.message_id.as_deref(),
                    request.dedupe_key.as_str(),
                    request.model.as_str(),
                    request.input_tokens as i64,
                    request.output_tokens as i64,
                    request.cache_create_tokens as i64,
                    request.cache_read_tokens as i64,
                    request.total_tokens as i64,
                    if request.is_subagent { 1 } else { 0 },
                    request.source_kind.as_str(),
                    now,
                    export_seq
                ],
            )
            .map_err(|e| format!("Failed to upsert remote request fact: {}", e))?;
        }

        tx.execute(
            "INSERT INTO webdav_sync_state (state_key, state_value, updated_at)
             VALUES (?1, '1', ?2)
             ON CONFLICT(state_key) DO UPDATE SET
                state_value = excluded.state_value,
                updated_at = excluded.updated_at",
            params![format!("imported:{}:{}", device_id, export_seq), now],
        )
        .map_err(|e| format!("Failed to mark remote sync package imported: {}", e))?;

        tx.commit()
            .map_err(|e| format!("Failed to commit remote sync import: {}", e))?;
        Ok(())
    }

    pub fn get_remote_request_records(
        &self,
        tool_filter: &ToolFilter,
    ) -> Result<Vec<LocalRequestRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let (sql, param) = match tool_filter {
            ToolFilter::All => (
                "SELECT session_id, tool, timestamp, COALESCE(message_id, ''),
                        input_tokens, output_tokens, cache_create_tokens, cache_read_tokens,
                        total_tokens, model, is_subagent, request_key
                 FROM remote_request_facts
                 ORDER BY timestamp ASC"
                    .to_string(),
                None,
            ),
            ToolFilter::Tool(tool) => (
                "SELECT session_id, tool, timestamp, COALESCE(message_id, ''),
                        input_tokens, output_tokens, cache_create_tokens, cache_read_tokens,
                        total_tokens, model, is_subagent, request_key
                 FROM remote_request_facts
                 WHERE tool = ?1
                 ORDER BY timestamp ASC"
                    .to_string(),
                Some(tool.clone()),
            ),
        };
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare get_remote_request_records: {}", e))?;
        let mapper = |row: &rusqlite::Row<'_>| {
            let request_key: Option<String> = row.get(11)?;
            Ok(LocalRequestRecord {
                session_id: row.get(0)?,
                tool: row.get(1)?,
                timestamp: row.get(2)?,
                message_id: row.get(3)?,
                input_tokens: row.get::<_, i64>(4)? as u64,
                output_tokens: row.get::<_, i64>(5)? as u64,
                cache_create_tokens: row.get::<_, i64>(6)? as u64,
                cache_read_tokens: row.get::<_, i64>(7)? as u64,
                total_tokens: row.get::<_, i64>(8)? as u64,
                model: row.get(9)?,
                is_subagent: row.get::<_, i64>(10)? != 0,
                request_key: request_key.filter(|v| !v.trim().is_empty()),
                source_file_present: None,
            })
        };
        let rows = match param {
            Some(tool) => stmt
                .query_map(params![tool], mapper)
                .map_err(|e| format!("Failed to query remote records by tool: {}", e))?,
            None => stmt
                .query_map([], mapper)
                .map_err(|e| format!("Failed to query remote records: {}", e))?,
        };
        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| format!("Failed to read remote request row: {}", e))?);
        }
        Ok(result)
    }

    pub fn get_remote_sessions(
        &self,
        tool_filter: &ToolFilter,
    ) -> Result<Vec<SessionMeta>, String> {
        let conn = self.conn.lock().unwrap();
        let (sql, param) = match tool_filter {
            ToolFilter::All => (
                "SELECT session_id, tool, project_key, project_name, start_time, end_time,
                        request_count, total_input_tokens, total_output_tokens,
                        total_cache_create_tokens, total_cache_read_tokens, model_list_json
                 FROM remote_sessions
                 ORDER BY end_time DESC"
                    .to_string(),
                None,
            ),
            ToolFilter::Tool(tool) => (
                "SELECT session_id, tool, project_key, project_name, start_time, end_time,
                        request_count, total_input_tokens, total_output_tokens,
                        total_cache_create_tokens, total_cache_read_tokens, model_list_json
                 FROM remote_sessions
                 WHERE tool = ?1
                 ORDER BY end_time DESC"
                    .to_string(),
                Some(tool.clone()),
            ),
        };
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare get_remote_sessions: {}", e))?;
        let mapper = |row: &rusqlite::Row<'_>| {
            let project_key: Option<String> = row.get(2)?;
            let model_list_json: String = row.get(11)?;
            Ok(SessionMeta {
                session_id: row.get(0)?,
                tool: row.get(1)?,
                cwd: project_key.clone(),
                project_name: row.get(3)?,
                topic: None,
                last_prompt: None,
                session_name: None,
                file_path: String::new(),
                file_size: 0,
                last_modified: row.get(5)?,
                total_input_tokens: row.get::<_, i64>(7)? as u64,
                total_output_tokens: row.get::<_, i64>(8)? as u64,
                total_cache_create_tokens: row.get::<_, i64>(9)? as u64,
                total_cache_read_tokens: row.get::<_, i64>(10)? as u64,
                models: serde_json::from_str(&model_list_json).unwrap_or_default(),
                message_count: row.get::<_, i64>(6)? as u64,
                start_time: row.get(4)?,
                end_time: row.get(5)?,
                source: "remote_sync".to_string(),
                message_ids: Vec::new(),
            })
        };
        let rows = match param {
            Some(tool) => stmt
                .query_map(params![tool], mapper)
                .map_err(|e| format!("Failed to query remote sessions by tool: {}", e))?,
            None => stmt
                .query_map([], mapper)
                .map_err(|e| format!("Failed to query remote sessions: {}", e))?,
        };
        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| format!("Failed to read remote session row: {}", e))?);
        }
        Ok(result)
    }

    pub fn upsert_webdav_sync_state(&self, key: &str, value: &str) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO webdav_sync_state (state_key, state_value, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(state_key) DO UPDATE SET
                state_value = excluded.state_value,
                updated_at = excluded.updated_at",
            params![key, value, now],
        )
        .map_err(|e| format!("Failed to upsert WebDAV sync state: {}", e))?;
        Ok(())
    }

    pub fn get_webdav_sync_state(&self, key: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT state_value FROM webdav_sync_state WHERE state_key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Failed to read WebDAV sync state: {}", e))
    }

    pub fn count_local_request_facts(&self) -> Result<u64, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM local_request_facts", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|count| count.max(0) as u64)
        .map_err(|e| format!("Failed to count local request facts: {}", e))
    }

    pub fn count_remote_request_facts(&self) -> Result<u64, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM remote_request_facts", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|count| count.max(0) as u64)
        .map_err(|e| format!("Failed to count remote request facts: {}", e))
    }

    pub fn list_remote_devices(&self) -> Result<Vec<RemoteSyncDevice>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT device_id, last_seen_at, last_export_seq, sync_status, updated_at
                 FROM remote_devices
                 ORDER BY last_seen_at DESC",
            )
            .map_err(|e| format!("Failed to prepare list_remote_devices: {}", e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(RemoteSyncDevice {
                    device_id: row.get(0)?,
                    last_seen_at: row.get(1)?,
                    last_export_seq: row.get(2)?,
                    sync_status: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            })
            .map_err(|e| format!("Failed to query remote devices: {}", e))?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| format!("Failed to read remote device row: {}", e))?);
        }
        Ok(result)
    }

    pub fn remove_remote_device(&self, device_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start remote device removal: {}", e))?;
        tx.execute(
            "DELETE FROM remote_request_facts WHERE origin_device_id = ?1",
            params![device_id],
        )
        .map_err(|e| format!("Failed to delete remote device requests: {}", e))?;
        tx.execute(
            "DELETE FROM remote_sessions WHERE origin_device_id = ?1",
            params![device_id],
        )
        .map_err(|e| format!("Failed to delete remote device sessions: {}", e))?;
        tx.execute(
            "DELETE FROM remote_devices WHERE device_id = ?1",
            params![device_id],
        )
        .map_err(|e| format!("Failed to delete remote device: {}", e))?;
        tx.execute(
            "DELETE FROM sync_device_cursors WHERE device_id = ?1",
            params![device_id],
        )
        .map_err(|e| format!("Failed to delete remote device cursor: {}", e))?;
        tx.execute(
            "DELETE FROM webdav_sync_state WHERE state_key LIKE ?1",
            params![format!("imported:{}:%", device_id)],
        )
        .map_err(|e| format!("Failed to delete remote device import markers: {}", e))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit remote device removal: {}", e))?;
        Ok(())
    }

    pub fn clear_imported_remote_data(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start imported sync clear: {}", e))?;
        tx.execute("DELETE FROM remote_request_facts", [])
            .map_err(|e| format!("Failed to clear remote request facts: {}", e))?;
        tx.execute("DELETE FROM remote_sessions", [])
            .map_err(|e| format!("Failed to clear remote sessions: {}", e))?;
        tx.execute("DELETE FROM remote_devices", [])
            .map_err(|e| format!("Failed to clear remote devices: {}", e))?;
        tx.execute("DELETE FROM sync_device_cursors", [])
            .map_err(|e| format!("Failed to clear sync device cursors: {}", e))?;
        tx.execute(
            "DELETE FROM webdav_sync_state WHERE state_key LIKE 'imported:%'",
            [],
        )
        .map_err(|e| format!("Failed to clear imported sync state: {}", e))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit imported sync clear: {}", e))?;
        // 大批量删除后收缩数据库文件
        conn.execute_batch("VACUUM")
            .map_err(|e| format!("Failed to vacuum after imported sync clear: {}", e))?;
        Ok(())
    }

    /// 统计孤立的本地事实（来源文件已消失）。
    pub fn count_orphan_local_facts(&self) -> Result<u64, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM local_request_facts WHERE source_file_present = 0",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count.max(0) as u64)
        .map_err(|e| format!("Failed to count orphan local request facts: {}", e))
    }

    /// 主动清理孤立的本地事实（来源文件已消失）。
    ///
    /// - `older_than_seconds`: 仅清理 `created_at` 早于 `now - older_than_seconds` 的行；
    ///   传 0 表示不限时间，全清。
    ///
    /// 返回删除的事实行数。同时清理掉随之无任何关联事实的 session 摘要与 source 文件行。
    pub fn purge_orphan_facts(&self, older_than_seconds: i64) -> Result<u64, String> {
        let now = chrono::Utc::now().timestamp();
        let cutoff = if older_than_seconds <= 0 {
            now
        } else {
            now - older_than_seconds
        };
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start orphan purge transaction: {}", e))?;

        let affected = tx
            .execute(
                "DELETE FROM local_request_facts
                 WHERE source_file_present = 0 AND created_at <= ?1",
                params![cutoff],
            )
            .map_err(|e| format!("Failed to purge orphan request facts: {}", e))?;

        // 清掉孤立的 session 摘要：本身已被软删过（即对应 source_files.deleted_at 非空）
        // 且不再有任何 request fact 引用。
        tx.execute(
            "DELETE FROM local_sessions
             WHERE session_id IN (
                 SELECT session_id FROM local_source_files
                 WHERE deleted_at IS NOT NULL
             )
             AND session_id NOT IN (SELECT DISTINCT session_id FROM local_request_facts)",
            [],
        )
        .map_err(|e| format!("Failed to purge orphan local sessions: {}", e))?;

        // 清掉同样无引用的 source files 软删行
        tx.execute(
            "DELETE FROM local_source_files
             WHERE deleted_at IS NOT NULL
               AND session_id NOT IN (SELECT DISTINCT session_id FROM local_request_facts)",
            [],
        )
        .map_err(|e| format!("Failed to purge orphan local source files: {}", e))?;

        Self::upsert_sync_state(&tx, "last_orphan_purge_at", &now.to_string(), now)?;
        Self::upsert_sync_state(
            &tx,
            "last_orphan_purge_count",
            &(affected as i64).to_string(),
            now,
        )?;

        tx.commit()
            .map_err(|e| format!("Failed to commit orphan purge: {}", e))?;
        Ok(affected.max(0) as u64)
    }

    /// 清空本地缓存并强制下一次同步从 JSONL 全量重建。
    ///
    /// 主要给用户「重建本地缓存」按钮使用。会清掉 `local_request_facts` /
    /// `local_sessions` / `local_source_files`；不影响 remote_* 表或 outbox 表。
    pub fn truncate_all_local_facts(&self) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start truncate local facts: {}", e))?;
        tx.execute("DELETE FROM local_request_facts", [])
            .map_err(|e| format!("Failed to delete local request facts: {}", e))?;
        tx.execute("DELETE FROM local_sessions", [])
            .map_err(|e| format!("Failed to delete local sessions: {}", e))?;
        tx.execute("DELETE FROM local_source_files", [])
            .map_err(|e| format!("Failed to delete local source files: {}", e))?;
        tx.execute("DELETE FROM local_sync_cursors", [])
            .map_err(|e| format!("Failed to delete local sync cursors: {}", e))?;
        Self::upsert_sync_state(&tx, "last_truncate_local_at", &now.to_string(), now)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit truncate local facts: {}", e))?;
        Ok(())
    }
}

pub fn ensure_local_usage_synced() -> Result<Arc<LocalUsageDatabase>, String> {
    let db = LocalUsageDatabase::get_global()?;
    db.sync_from_scanner()?;
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn temp_db() -> (tempfile::TempDir, LocalUsageDatabase) {
        let tmpdir = tempfile::tempdir().expect("create temp dir");
        let path = tmpdir.path().join("local_usage.db");
        let db = LocalUsageDatabase::new_with_path(&path).expect("open temp db");
        (tmpdir, db)
    }

    /// 直接往表里插一条事实，绕过 sync_from_scanner（测试不能 mock 文件系统）。
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

        // v5 索引也必须建出来。回归 issue：曾经把这三个索引误放在 create_cache_tables 里，
        // 老库升级时 CREATE TABLE IF NOT EXISTS 不会补列，索引创建在 ALTER TABLE 之前先炸。
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

    /// 回归测试：模拟「老库（v4 schema）」打开时，迁移到 v5 不应该炸。
    /// 之前把 v5 索引误放在 create_cache_tables 里时，这条路径会报
    /// "no such column: deleted_at in CREATE INDEX ..."。
    #[test]
    fn open_v4_db_upgrades_to_v5_without_error() {
        let tmpdir = tempfile::tempdir().expect("create temp dir");
        let path = tmpdir.path().join("legacy.db");

        // 1) 手工建一个「v4」库：表结构故意缺少 v5 才有的列。
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
            // 灌一条老数据，验证 v5 回填 request_key 不会漏
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

        // 2) 用 new_with_path 重新打开 → 应当走 v5 迁移，且不能报错。
        let db = LocalUsageDatabase::new_with_path(&path)
            .expect("open legacy db should trigger v5 migration without error");

        // 3) 验证 v5 列与索引都建出来了
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

        // 4) 验证回填：老数据 request_key 应为 'claude_code:msg-legacy'
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
        // 一条很久以前的孤立事实
        insert_request_fact(
            &db,
            "sess-old",
            "msg-old",
            "/tmp/old.jsonl",
            false,
            now - 86400 * 30,
        );
        // 一条最近的孤立事实
        insert_request_fact(
            &db,
            "sess-new",
            "msg-new",
            "/tmp/new.jsonl",
            false,
            now - 60,
        );
        // 一条仍然 present 的事实，不该被动
        insert_request_fact(
            &db,
            "sess-alive",
            "msg-alive",
            "/tmp/alive.jsonl",
            true,
            now - 86400 * 30,
        );

        // 清理 7 天前的孤立 → 只应该清掉 msg-old
        let removed = db.purge_orphan_facts(86400 * 7).unwrap();
        assert_eq!(removed, 1);

        let total = db.count_local_request_facts().unwrap();
        assert_eq!(total, 2, "msg-new 与 msg-alive 应保留");

        // 再用 0 秒（不限时间）全清剩下的 orphan → msg-new 应被删
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

        // 同时写一条 local_sessions 行
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
            .get_all_request_records(&crate::models::ToolFilter::All)
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
            .get_all_request_records(&crate::models::ToolFilter::All)
            .unwrap();
        // 软删行同样会出现在查询里——这是设计要点：合并层仍要看见它们
        assert_eq!(records.len(), 2);
        let presents: Vec<_> = records
            .iter()
            .map(|r| (r.message_id.clone(), r.source_file_present))
            .collect();
        assert!(presents.contains(&("msg-alive".to_string(), Some(true))));
        assert!(presents.contains(&("msg-vanished".to_string(), Some(false))));
    }
}
