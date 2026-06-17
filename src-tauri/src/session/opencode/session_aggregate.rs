use crate::session::constants::TOOL_OPENCODE;
use crate::session::meta::{LocalRequestRecord, SessionMeta};
use crate::session::opencode::message::normalize_model_string;
use crate::session::opencode_reader::{
    OpenCodeMessageSnapshot, OpenCodeSchemaMode, OpenCodeSessionData, SessionRow,
};
use crate::session::shared::{extract_project_name, truncate_string};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

pub(in crate::session) fn build_session_data_from_messages(
    combined: HashMap<String, OpenCodeMessageSnapshot>,
    session_db_paths: HashMap<String, PathBuf>,
    session_source_paths: HashMap<String, String>,
    required_session_columns: &[&str],
    required_message_columns: &[&str],
    query_session_rows: fn(&Connection) -> HashMap<String, SessionRow>,
) -> Vec<OpenCodeSessionData> {
    let mut message_by_session: HashMap<String, Vec<OpenCodeMessageSnapshot>> = HashMap::new();
    let mut raw_message_id_sessions: HashMap<String, HashSet<String>> = HashMap::new();
    let mut source_kinds: HashSet<&'static str> = HashSet::new();

    for snapshot in combined.into_values() {
        raw_message_id_sessions
            .entry(snapshot.raw_message_id.clone())
            .or_default()
            .insert(snapshot.canonical_session_id.clone());
        source_kinds.insert(snapshot.source_kind);
        message_by_session
            .entry(snapshot.canonical_session_id.clone())
            .or_default()
            .push(snapshot);
    }

    let mut rows_by_session = HashMap::new();
    let mut schema_mode_by_session = HashMap::new();
    let mut db_paths_by_path: HashMap<String, Vec<String>> = HashMap::new();
    for (session_id, db_path) in &session_db_paths {
        db_paths_by_path
            .entry(db_path.to_string_lossy().to_string())
            .or_default()
            .push(session_id.clone());
    }
    for (db_path_string, session_ids) in db_paths_by_path {
        let db_path = PathBuf::from(db_path_string);
        if let Ok(conn) = Connection::open_with_flags(
            &db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            let _ = conn.execute_batch("PRAGMA busy_timeout=2000;");
            let (mode, _) = super::schema::detect_schema_mode(
                &conn,
                required_session_columns,
                required_message_columns,
            );
            if mode == OpenCodeSchemaMode::Full {
                // query_session_rows returns rows keyed by raw session id (e.g. "ses_xyz"),
                // but session_ids here are canonical (e.g. "opencode::native::ses_xyz").
                // Strip the "opencode::<storage_id>::" prefix to match.
                let rows = query_session_rows(&conn);
                for canonical_id in &session_ids {
                    let raw_id = strip_canonical_prefix(canonical_id);
                    if let Some(row) = rows.get(raw_id) {
                        rows_by_session.insert(canonical_id.clone(), row.clone());
                    }
                    schema_mode_by_session.insert(canonical_id.clone(), mode);
                }
            } else {
                for session_id in &session_ids {
                    schema_mode_by_session.insert(session_id.clone(), mode);
                }
            }
        } else {
            for session_id in &session_ids {
                schema_mode_by_session.insert(session_id.clone(), OpenCodeSchemaMode::Incompatible);
            }
        };
    }

    let mut session_ids: Vec<String> = message_by_session.keys().cloned().collect();
    session_ids.sort();

    session_ids
        .into_iter()
        .filter_map(|canonical_session_id| {
            let mut snapshots = message_by_session.remove(&canonical_session_id)?;
            snapshots.sort_by_key(|snapshot| snapshot.timestamp_sec);
            Some(build_single_session_data(
                &canonical_session_id,
                snapshots,
                rows_by_session.get(&canonical_session_id),
                &raw_message_id_sessions,
                &source_kinds,
                schema_mode_by_session
                    .get(&canonical_session_id)
                    .copied()
                    .unwrap_or(OpenCodeSchemaMode::MessageOnly),
                session_source_paths
                    .get(&canonical_session_id)
                    .cloned()
                    .unwrap_or_else(|| format!("opencode://{}", canonical_session_id)),
            ))
        })
        .collect()
}

fn build_single_session_data(
    canonical_session_id: &str,
    snapshots: Vec<OpenCodeMessageSnapshot>,
    session_row: Option<&SessionRow>,
    raw_message_id_sessions: &HashMap<String, HashSet<String>>,
    source_kinds: &HashSet<&'static str>,
    schema_mode: OpenCodeSchemaMode,
    source_locator: String,
) -> OpenCodeSessionData {
    let cwd = session_row
        .map(|row| row.directory.clone())
        .filter(|value| !value.is_empty())
        .or_else(|| snapshots.iter().find_map(|snapshot| snapshot.cwd.clone()));
    let project_name = cwd.as_deref().and_then(extract_project_name);

    let mut models = BTreeSet::new();
    let mut requests = Vec::new();
    let mut message_ids = Vec::new();
    let mut total_input = 0_u64;
    let mut total_output = 0_u64;
    let mut total_cache_create = 0_u64;
    let mut total_cache_read = 0_u64;

    let session_model = session_row.and_then(|row| {
        row.model_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .map(|json| {
                let provider = json
                    .get("providerID")
                    .or_else(|| json.get("providerId"))
                    .or_else(|| json.get("provider"))
                    .and_then(|value| value.as_str());
                let model = json
                    .get("modelID")
                    .or_else(|| json.get("modelId"))
                    .or_else(|| json.get("model"))
                    .and_then(|value| value.as_str());
                normalize_model_string(provider, model)
            })
    });

    for snapshot in &snapshots {
        if !snapshot.model.is_empty() && snapshot.model != "unknown" {
            models.insert(snapshot.model.clone());
        }

        total_input += snapshot.input_tokens;
        total_output += snapshot.output_tokens + snapshot.reasoning_tokens;
        total_cache_create += snapshot.cache_create_tokens;
        total_cache_read += snapshot.cache_read_tokens;
        message_ids.push(snapshot.raw_message_id.clone());

        let request_key = if raw_message_id_sessions
            .get(&snapshot.raw_message_id)
            .map(|sessions| sessions.len() > 1)
            .unwrap_or(false)
        {
            Some(format!(
                "{}:{}|{}",
                TOOL_OPENCODE, canonical_session_id, snapshot.raw_message_id
            ))
        } else {
            Some(format!("{}:{}", TOOL_OPENCODE, snapshot.raw_message_id))
        };

        requests.push(LocalRequestRecord {
            session_id: canonical_session_id.to_string(),
            tool: TOOL_OPENCODE.to_string(),
            timestamp: snapshot.timestamp_sec,
            message_id: snapshot.raw_message_id.clone(),
            input_tokens: snapshot.input_tokens,
            output_tokens: snapshot.output_tokens + snapshot.reasoning_tokens,
            reasoning_tokens: snapshot.reasoning_tokens,
            cache_create_tokens: snapshot.cache_create_tokens,
            cache_read_tokens: snapshot.cache_read_tokens,
            total_tokens: snapshot.total_tokens,
            request_count: 1,
            model: snapshot.model.clone(),
            is_subagent: false,
            request_key,
            explicit_estimated_cost: None,
            source_file_present: Some(true),
        });
    }

    if let Some(model) = session_model {
        if !model.is_empty() && model != "unknown" {
            models.insert(model);
        }
    }

    let start_time = session_row
        .map(|row| row.time_created_ms / 1000)
        .filter(|t| *t > 0)
        .unwrap_or_else(|| {
            snapshots
                .iter()
                .map(|snapshot| snapshot.timestamp_sec)
                .filter(|&t| t > 0)
                .min()
                .unwrap_or(0)
        });
    let end_time = session_row
        .map(|row| row.time_updated_ms / 1000)
        .filter(|t| *t > 0)
        .unwrap_or_else(|| {
            snapshots
                .iter()
                .map(|snapshot| snapshot.timestamp_sec)
                .filter(|&t| t > 0)
                .max()
                .unwrap_or(0)
        });
    let last_modified = end_time.max(start_time);

    let title = session_row
        .map(|row| row.title.clone())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            snapshots
                .iter()
                .rev()
                .find_map(|snapshot| snapshot.title.clone())
        });

    let source = match schema_mode {
        OpenCodeSchemaMode::Full => {
            if source_kinds.contains("opencode_db") && source_kinds.contains("opencode_file") {
                "opencode_mixed"
            } else if source_kinds.contains("opencode_db") {
                "opencode_sqlite"
            } else {
                "opencode_file"
            }
        }
        OpenCodeSchemaMode::MessageOnly => "opencode_sqlite_message_only",
        OpenCodeSchemaMode::Incompatible => "opencode_incompatible",
    }
    .to_string();

    let session_row_totals = session_row.map(|row| {
        (
            row.tokens_input.max(0) as u64,
            (row.tokens_output + row.tokens_reasoning).max(0) as u64,
            row.tokens_cache_write.max(0) as u64,
            row.tokens_cache_read.max(0) as u64,
        )
    });

    let (meta_input, meta_output, meta_cache_create, meta_cache_read) = if requests.is_empty() {
        session_row_totals.unwrap_or((0, 0, 0, 0))
    } else {
        (
            total_input,
            total_output,
            total_cache_create,
            total_cache_read,
        )
    };

    let fingerprint = compute_session_fingerprint(canonical_session_id, &requests);
    OpenCodeSessionData {
        meta: SessionMeta {
            session_id: canonical_session_id.to_string(),
            tool: TOOL_OPENCODE.to_string(),
            cwd,
            project_name,
            topic: title.as_deref().map(|value| truncate_string(value, 50)),
            last_prompt: None,
            session_name: title.clone(),
            file_path: source_locator.clone(),
            file_size: 0,
            last_modified,
            total_input_tokens: meta_input,
            total_output_tokens: meta_output,
            total_cache_create_tokens: meta_cache_create,
            total_cache_read_tokens: meta_cache_read,
            models: models.into_iter().collect(),
            message_count: requests.len() as u64,
            start_time,
            end_time,
            source,
            message_ids,
            scope: None,
            explicit_estimated_cost: None,
        },
        requests,
        fingerprint,
        source_locator,
    }
}

/// Strip the "opencode::<storage_id>::" prefix from a canonical session id to get the raw DB id.
/// "opencode::native::ses_xyz"  → "ses_xyz"
/// "opencode::wsl:Ubuntu::ses_xyz" → "ses_xyz"
/// bare "ses_xyz" → "ses_xyz" (unchanged)
fn strip_canonical_prefix(canonical_id: &str) -> &str {
    let remainder = canonical_id
        .strip_prefix("opencode::")
        .unwrap_or(canonical_id);
    // remainder is now "<storage_id>::<raw_id>" or "<raw_id>"
    if let Some(pos) = remainder.find("::") {
        &remainder[pos + 2..]
    } else {
        remainder
    }
}

fn compute_session_fingerprint(session_id: &str, requests: &[LocalRequestRecord]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    session_id.hash(&mut hasher);
    for request in requests {
        request.session_id.hash(&mut hasher);
        request.message_id.hash(&mut hasher);
        request.timestamp.hash(&mut hasher);
        request.model.hash(&mut hasher);
        request.input_tokens.hash(&mut hasher);
        request.output_tokens.hash(&mut hasher);
        request.cache_create_tokens.hash(&mut hasher);
        request.cache_read_tokens.hash(&mut hasher);
        request.total_tokens.hash(&mut hasher);
        request.request_key.hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::opencode::message::parse_message_snapshot;

    #[test]
    fn strip_canonical_prefix_extracts_raw_id() {
        assert_eq!(
            strip_canonical_prefix("opencode::native::ses_abc"),
            "ses_abc"
        );
        assert_eq!(
            strip_canonical_prefix("opencode::wsl:Ubuntu::ses_abc"),
            "ses_abc"
        );
        assert_eq!(strip_canonical_prefix("ses_abc"), "ses_abc");
    }

    #[test]
    fn forked_session_excludes_replayed_messages_via_db() {
        // Build two sessions in a temp SQLite DB:
        //   - original  (time_created=1000): 1 message at t=1050
        //   - fork      (time_created=2000): 1 replayed message at t=1050 + 1 new at t=2050
        // After scanning, the fork session should have exactly 1 request (only the new message).
        use rusqlite::Connection;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("opencode.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "
            CREATE TABLE session (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL DEFAULT '',
              slug TEXT NOT NULL DEFAULT '',
              directory TEXT NOT NULL DEFAULT '',
              title TEXT NOT NULL DEFAULT '',
              version TEXT NOT NULL DEFAULT '',
              model TEXT,
              cost REAL DEFAULT 0 NOT NULL,
              tokens_input INTEGER DEFAULT 0 NOT NULL,
              tokens_output INTEGER DEFAULT 0 NOT NULL,
              tokens_reasoning INTEGER DEFAULT 0 NOT NULL,
              tokens_cache_read INTEGER DEFAULT 0 NOT NULL,
              tokens_cache_write INTEGER DEFAULT 0 NOT NULL,
              time_created INTEGER NOT NULL,
              time_updated INTEGER NOT NULL,
              time_archived INTEGER
            );
            CREATE TABLE message (
              id TEXT PRIMARY KEY,
              session_id TEXT NOT NULL,
              time_created INTEGER NOT NULL,
              time_updated INTEGER NOT NULL,
              data TEXT NOT NULL
            );
        ",
        )
        .unwrap();

        // original session
        conn.execute(
            "INSERT INTO session (id, time_created, time_updated) VALUES ('ses_orig', 1000, 9999)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (
               'msg_orig', 'ses_orig', 1050, 1050,
               '{\"role\":\"assistant\",\"tokens\":{\"input\":100,\"output\":10},\"time\":{\"created\":1050000,\"completed\":1051000}}'
             )",
            [],
        ).unwrap();

        // fork session (created at t=2000)
        conn.execute(
            "INSERT INTO session (id, time_created, time_updated) VALUES ('ses_fork', 2000, 9999)",
            [],
        )
        .unwrap();
        // replayed copy (time_created=1050 < 2000)
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (
               'msg_fork_replay', 'ses_fork', 1050, 2001,
               '{\"role\":\"assistant\",\"tokens\":{\"input\":100,\"output\":10},\"time\":{\"created\":1050000,\"completed\":1051000}}'
             )",
            [],
        ).unwrap();
        // new message (time_created=2050 >= 2000)
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (
               'msg_fork_new', 'ses_fork', 2050, 2050,
               '{\"role\":\"assistant\",\"tokens\":{\"input\":50,\"output\":5},\"time\":{\"created\":2050000,\"completed\":2051000}}'
             )",
            [],
        ).unwrap();

        // Use db_scan to load messages (applies the fork filter)
        let mut db_state = crate::session::opencode_reader::OpenCodeDbCacheState::default();
        let root = crate::session::opencode_reader::OpenCodeStorageRoot {
            id: "native".to_string(),
            home: dir.path().to_path_buf(),
            db_path: db_path.clone(),
            message_root: dir.path().join("storage").join("message"),
        };
        let messages = super::super::db_scan::refresh_db_messages_for_path(&mut db_state, &root);

        // Collect per session
        let orig_msgs: Vec<_> = messages
            .values()
            .filter(|m| m.canonical_session_id == "opencode::native::ses_orig")
            .collect();
        let fork_msgs: Vec<_> = messages
            .values()
            .filter(|m| m.canonical_session_id == "opencode::native::ses_fork")
            .collect();

        assert_eq!(orig_msgs.len(), 1, "original session should have 1 message");
        assert_eq!(
            fork_msgs.len(),
            1,
            "fork session should have 1 message (replayed copy filtered out)"
        );
        assert_eq!(
            fork_msgs[0].raw_message_id, "msg_fork_new",
            "remaining message should be the new one, not the replayed copy"
        );
        assert_eq!(fork_msgs[0].input_tokens, 50);
    }

    #[test]
    fn session_row_lookup_uses_raw_id() {
        use rusqlite::Connection;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("opencode.db");

        // Create a real DB with the full schema so detect_schema_mode returns Full.
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "
            CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL DEFAULT '',
                slug TEXT NOT NULL DEFAULT '',
                directory TEXT NOT NULL DEFAULT '',
                title TEXT NOT NULL DEFAULT '',
                version TEXT NOT NULL DEFAULT '',
                model TEXT,
                tokens_input INTEGER DEFAULT 0,
                tokens_output INTEGER DEFAULT 0,
                tokens_reasoning INTEGER DEFAULT 0,
                tokens_cache_read INTEGER DEFAULT 0,
                tokens_cache_write INTEGER DEFAULT 0,
                time_created INTEGER NOT NULL DEFAULT 0,
                time_updated INTEGER NOT NULL DEFAULT 0,
                time_archived INTEGER,
                cost REAL DEFAULT 0
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL DEFAULT 0,
                time_updated INTEGER NOT NULL DEFAULT 0,
                data TEXT NOT NULL
            );
            INSERT INTO session (id, directory, title, tokens_input, tokens_output,
                                 time_created, time_updated)
            VALUES ('ses_x', '/my/project', 'Test Session', 999, 99, 4000, 7000);
            INSERT INTO message (id, session_id, time_created, time_updated, data)
            VALUES ('msg_x', 'ses_x', 5000, 6000,
                    '{\"role\":\"assistant\",\"tokens\":{\"input\":20,\"output\":2}}');
        ",
        )
        .unwrap();
        drop(conn);

        let data = serde_json::json!({
            "id": "msg_x",
            "role": "assistant",
            "tokens": { "input": 20, "output": 2 },
            "time": { "created": 5000, "completed": 6000 }
        });
        let snapshot = parse_message_snapshot(
            "native",
            "opencode://test",
            "ses_x",
            "msg_x",
            &data,
            5000,
            "opencode_db",
        )
        .unwrap();

        let canonical_id = snapshot.canonical_session_id.clone();
        let mut combined = HashMap::new();
        combined.insert(snapshot.message_identity_key(), snapshot);

        let mut session_db_paths = HashMap::new();
        session_db_paths.insert(canonical_id.clone(), db_path);

        let mut session_source_paths = HashMap::new();
        session_source_paths.insert(canonical_id.clone(), "opencode://test".to_string());

        let sessions = build_session_data_from_messages(
            combined,
            session_db_paths,
            session_source_paths,
            crate::session::opencode_reader::REQUIRED_SESSION_COLUMNS,
            crate::session::opencode_reader::REQUIRED_MESSAGE_COLUMNS,
            crate::session::opencode_reader::query_session_rows,
        );

        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].meta.cwd.as_deref(),
            Some("/my/project"),
            "cwd should come from session row (proves raw-id lookup worked)"
        );
        assert_eq!(sessions[0].meta.topic.as_deref(), Some("Test Session"));
    }
}
