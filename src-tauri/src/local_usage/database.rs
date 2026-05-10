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
                sync_error TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_local_source_files_session_id
                ON local_source_files(session_id);
            CREATE INDEX IF NOT EXISTS idx_local_source_files_tool
                ON local_source_files(tool);
            CREATE INDEX IF NOT EXISTS idx_local_source_files_project_key
                ON local_source_files(project_key);

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
            "#,
        )
        .map_err(|e| format!("Failed to create local usage tables: {}", e))?;
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
        if schema_version >= 2 {
            return Ok(());
        }

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
                 WHERE file_role = 'session_group'",
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
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start local usage transaction: {}", e))?;

        for session_id in removed_ids {
            tx.execute(
                "DELETE FROM local_request_facts WHERE session_id = ?1",
                params![session_id],
            )
            .map_err(|e| format!("Failed to delete removed local request facts: {}", e))?;
            tx.execute(
                "DELETE FROM local_sessions WHERE session_id = ?1",
                params![session_id],
            )
            .map_err(|e| format!("Failed to delete removed local session: {}", e))?;
            tx.execute(
                "DELETE FROM local_source_files WHERE session_id = ?1",
                params![session_id],
            )
            .map_err(|e| format!("Failed to delete removed local source file rows: {}", e))?;
        }

        for dirty_session in dirty_sessions {
            let DirtySessionSync {
                session,
                meta,
                requests,
                project_key,
            } = dirty_session;
            let fingerprint = session.fingerprint.to_string();

            tx.execute(
                "DELETE FROM local_request_facts WHERE session_id = ?1",
                params![session.session_id.as_str()],
            )
            .map_err(|e| format!("Failed to clear stale local request facts: {}", e))?;
            tx.execute(
                "DELETE FROM local_sessions WHERE session_id = ?1",
                params![session.session_id.as_str()],
            )
            .map_err(|e| format!("Failed to clear stale local session row: {}", e))?;
            tx.execute(
                "DELETE FROM local_source_files WHERE session_id = ?1",
                params![session.session_id.as_str()],
            )
            .map_err(|e| format!("Failed to clear stale local source rows: {}", e))?;

            tx.execute(
                "INSERT INTO local_source_files (
                    tool, session_id, project_key, file_path, file_role, file_size,
                    mtime_epoch, fingerprint, last_scanned_at, last_synced_at, sync_status
                ) VALUES (?1, ?2, ?3, ?4, 'session_group', ?5, ?6, ?7, ?8, ?9, 'ready')",
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
            .map_err(|e| format!("Failed to insert local source row: {}", e))?;

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
                tx.execute(
                    "INSERT INTO local_request_facts (
                        request_id, session_id, tool, project_key, timestamp, message_id, dedupe_key,
                        model, input_tokens, output_tokens, cache_create_tokens, cache_read_tokens,
                        total_tokens, source_offset, event_index, is_subagent, raw_event_kind,
                        sync_version, created_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                              NULL, ?14, ?15, 'request', 1, ?16)",
                    params![
                        request_id.as_str(),
                        request.session_id.as_str(),
                        request.tool.as_str(),
                        project_key.as_str(),
                        request.timestamp,
                        request.message_id.as_str(),
                        dedupe_key.as_str(),
                        request.model.as_str(),
                        request.input_tokens as i64,
                        request.output_tokens as i64,
                        request.cache_create_tokens as i64,
                        request.cache_read_tokens as i64,
                        request.total_tokens as i64,
                        idx as i64,
                        if request.is_subagent { 1 } else { 0 },
                        now
                    ],
                )
                .map_err(|e| format!("Failed to insert local request fact: {}", e))?;
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

    pub fn get_all_request_records(
        &self,
        tool_filter: &ToolFilter,
    ) -> Result<Vec<LocalRequestRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let (sql, param) = match tool_filter {
            ToolFilter::All => (
                "SELECT session_id, tool, timestamp, message_id,
                        input_tokens, output_tokens, cache_create_tokens, cache_read_tokens,
                        total_tokens, model, is_subagent
                 FROM local_request_facts
                 ORDER BY timestamp ASC"
                    .to_string(),
                None,
            ),
            ToolFilter::Tool(tool) => (
                "SELECT session_id, tool, timestamp, message_id,
                        input_tokens, output_tokens, cache_create_tokens, cache_read_tokens,
                        total_tokens, model, is_subagent
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
                        total_tokens, model, is_subagent
                 FROM local_request_facts
                 WHERE session_id = ?1
                 ORDER BY timestamp ASC",
            )
            .map_err(|e| format!("Failed to prepare get_request_records_by_session: {}", e))?;
        let rows = stmt
            .query_map(params![session_id], |row| {
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
}

pub fn ensure_local_usage_synced() -> Result<Arc<LocalUsageDatabase>, String> {
    let db = LocalUsageDatabase::get_global()?;
    db.sync_from_scanner()?;
    Ok(db)
}
