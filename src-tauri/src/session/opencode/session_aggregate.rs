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
                let rows = query_session_rows(&conn);
                for session_id in &session_ids {
                    if let Some(row) = rows.get(session_id) {
                        rows_by_session.insert(session_id.clone(), row.clone());
                    }
                    schema_mode_by_session.insert(session_id.clone(), mode);
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
        },
        requests,
        fingerprint,
        source_locator,
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
