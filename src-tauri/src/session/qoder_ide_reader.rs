use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::shared::{extract_project_name, parse_u64_from_value, truncate_string};
use super::source::{ParsedSessionData, SessionSource, SourceSnapshot, SourceUpdateMode};
use rusqlite::{params, Connection, OpenFlags};
use serde_json::Value;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

const QODER_DB_SOURCE_KIND: &str = "qoder_ide_sqlite";
const QODER_MODEL_FALLBACK: &str = "custom_model";

pub(super) struct QoderIdeSource {
    tool: &'static str,
    app_dir: &'static str,
    cache: OnceLock<Mutex<HashMap<String, QoderIdeSessionData>>>,
}

#[derive(Debug, Clone)]
pub(crate) struct QoderIdeSessionData {
    pub meta: SessionMeta,
    pub requests: Vec<LocalRequestRecord>,
    pub fingerprint: u64,
    pub source_locator: String,
}

#[derive(Debug, Clone)]
struct QoderDbMeta {
    db_path: PathBuf,
    file_size: u64,
    last_modified: i64,
    fingerprint: u64,
}

#[derive(Debug, Clone)]
struct QoderSessionRow {
    raw_session_id: String,
    session_title: String,
    project_name: Option<String>,
    project_uri: Option<String>,
    gmt_create_ms: i64,
    gmt_modified_ms: i64,
}

impl QoderIdeSource {
    pub(super) const fn new(tool: &'static str, app_dir: &'static str) -> Self {
        Self {
            tool,
            app_dir,
            cache: OnceLock::new(),
        }
    }

    fn cache(&self) -> &Mutex<HashMap<String, QoderIdeSessionData>> {
        self.cache.get_or_init(|| Mutex::new(HashMap::new()))
    }
}

impl SessionSource for QoderIdeSource {
    fn tool_id(&self) -> &'static str {
        self.tool
    }

    fn scan(&self) -> SourceSnapshot {
        let scanned = scan_qoder_ide_sessions_for(self.app_dir, self.tool);
        let scan_fingerprint = compute_qoder_scan_fingerprint(&scanned);
        let sessions = scanned
            .iter()
            .map(|session| SessionFile {
                session_id: session.meta.session_id.clone(),
                tool: session.meta.tool.clone(),
                project_path: session.meta.project_name.clone().unwrap_or_default(),
                file_path: session.source_locator.clone(),
                transcript_paths: vec![session.meta.file_path.clone()],
                file_size: session.meta.file_size,
                last_modified: session.meta.last_modified,
                fingerprint: session.fingerprint,
            })
            .collect::<Vec<_>>();

        let mut cache = self.cache().lock().unwrap();
        cache.clear();
        cache.extend(
            scanned
                .into_iter()
                .map(|session| (session.meta.session_id.clone(), session)),
        );
        drop(cache);

        SourceSnapshot {
            source_id: self.tool_id(),
            update_mode: SourceUpdateMode::ReplaceAll,
            sessions,
            scan_fingerprint,
        }
    }

    fn parse(&self, session: &SessionFile) -> Result<ParsedSessionData, String> {
        let cache = self.cache().lock().unwrap();
        let parsed = cache
            .get(&session.session_id)
            .cloned()
            .ok_or_else(|| format!("qoder ide session not found: {}", session.session_id))?;

        Ok(ParsedSessionData {
            meta: parsed.meta,
            requests: parsed.requests,
        })
    }
}

pub(crate) fn scan_qoder_ide_sessions() -> Vec<QoderIdeSessionData> {
    scan_qoder_ide_sessions_for("Qoder", super::constants::TOOL_QODER_IDE)
}

pub(crate) fn scan_qoder_ide_cn_sessions() -> Vec<QoderIdeSessionData> {
    scan_qoder_ide_sessions_for("QoderCN", super::constants::TOOL_QODER_IDE_CN)
}

pub(crate) fn scan_qoder_ide_sessions_for(
    app_dir: &str,
    tool_id: &str,
) -> Vec<QoderIdeSessionData> {
    let Some(db_path) = find_qoder_ide_db_for(app_dir) else {
        return Vec::new();
    };
    let Some(db_meta) = qoder_db_meta(&db_path) else {
        return Vec::new();
    };

    let conn = match open_qoder_db_read_only(&db_path) {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!(
                "[UsageMeter] Failed to open Qoder IDE DB {}: {}",
                db_path.display(),
                err
            );
            return Vec::new();
        }
    };

    let mut stmt = match conn.prepare(
        "SELECT session_id,
                COALESCE(session_title, ''),
                project_name,
                project_uri,
                COALESCE(gmt_create, 0),
                COALESCE(gmt_modified, 0)
         FROM chat_session
         ORDER BY gmt_modified DESC",
    ) {
        Ok(stmt) => stmt,
        Err(err) => {
            eprintln!(
                "[UsageMeter] Failed to query Qoder IDE sessions from {}: {}",
                db_path.display(),
                err
            );
            return Vec::new();
        }
    };

    let rows = match stmt.query_map([], |row| {
        Ok(QoderSessionRow {
            raw_session_id: row.get(0)?,
            session_title: row.get(1)?,
            project_name: row.get(2)?,
            project_uri: row.get(3)?,
            gmt_create_ms: row.get(4)?,
            gmt_modified_ms: row.get(5)?,
        })
    }) {
        Ok(rows) => rows,
        Err(err) => {
            eprintln!(
                "[UsageMeter] Failed to iterate Qoder IDE sessions from {}: {}",
                db_path.display(),
                err
            );
            return Vec::new();
        }
    };

    let mut sessions = Vec::new();
    for row in rows {
        let Ok(session_row) = row else {
            continue;
        };
        if session_row.raw_session_id.trim().is_empty() {
            continue;
        }
        if let Some(parsed) = parse_qoder_session(&db_meta, &conn, &session_row, tool_id) {
            sessions.push(parsed);
        }
    }
    sessions.sort_by_key(|session| std::cmp::Reverse(session.meta.last_modified));
    sessions
}

#[allow(dead_code)]
pub(crate) fn find_qoder_ide_db() -> Option<PathBuf> {
    find_qoder_ide_db_for("Qoder")
}

pub(crate) fn find_qoder_ide_db_for(app_dir: &str) -> Option<PathBuf> {
    dirs::data_dir()
        .map(|dir| {
            dir.join(app_dir)
                .join("SharedClientCache")
                .join("cache")
                .join("db")
                .join("local.db")
        })
        .filter(|path| path.exists())
}

pub(crate) fn compute_qoder_scan_fingerprint(sessions: &[QoderIdeSessionData]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for session in sessions {
        session.meta.session_id.hash(&mut hasher);
        session.fingerprint.hash(&mut hasher);
    }
    hasher.finish()
}

fn parse_qoder_session(
    db_meta: &QoderDbMeta,
    conn: &Connection,
    session_row: &QoderSessionRow,
    tool_id: &str,
) -> Option<QoderIdeSessionData> {
    let mut stmt = match conn.prepare(
        "SELECT id, COALESCE(gmt_create, 0), token_info, model_info
         FROM chat_message
         WHERE session_id = ?1
           AND role = 'assistant'
           AND token_info IS NOT NULL
           AND token_info != ''
         ORDER BY gmt_create ASC",
    ) {
        Ok(stmt) => stmt,
        Err(err) => {
            eprintln!(
                "[UsageMeter] Failed to prepare Qoder IDE message query for session {}: {}",
                session_row.raw_session_id, err
            );
            return None;
        }
    };

    let rows = match stmt.query_map(params![session_row.raw_session_id.as_str()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    }) {
        Ok(rows) => rows,
        Err(err) => {
            eprintln!(
                "[UsageMeter] Failed to query Qoder IDE messages for session {}: {}",
                session_row.raw_session_id, err
            );
            return None;
        }
    };

    let canonical_session_id = canonical_qoder_session_id_for(tool_id, &session_row.raw_session_id);
    let mut requests = Vec::new();
    let mut models = std::collections::BTreeSet::new();
    let mut total_input_tokens = 0u64;
    let mut total_output_tokens = 0u64;
    let mut total_cache_read_tokens = 0u64;
    let mut earliest_timestamp: Option<i64> = None;
    let mut latest_timestamp: Option<i64> = None;

    for row in rows {
        let Ok((message_id, timestamp_ms, token_info_raw, model_info_raw)) = row else {
            continue;
        };
        let Some(token_info_str) = token_info_raw else {
            continue;
        };
        let Some(token_info) = parse_qoder_json(&token_info_str) else {
            continue;
        };

        let prompt_tokens = token_info
            .get("prompt_tokens")
            .and_then(parse_u64_from_value)
            .unwrap_or(0);
        let completion_tokens = token_info
            .get("completion_tokens")
            .and_then(parse_u64_from_value)
            .unwrap_or(0);
        let cached_tokens = token_info
            .get("cached_tokens")
            .and_then(parse_u64_from_value)
            .unwrap_or(0);

        if prompt_tokens == 0 && completion_tokens == 0 {
            continue;
        }

        let input_tokens = prompt_tokens.saturating_sub(cached_tokens);
        let output_tokens = completion_tokens;
        let cache_read_tokens = cached_tokens;
        let total_tokens = input_tokens + output_tokens + cache_read_tokens;
        if total_tokens == 0 {
            continue;
        }

        let timestamp = ms_to_sec(timestamp_ms);
        earliest_timestamp = Some(
            earliest_timestamp
                .map(|value| value.min(timestamp))
                .unwrap_or(timestamp),
        );
        latest_timestamp = Some(
            latest_timestamp
                .map(|value| value.max(timestamp))
                .unwrap_or(timestamp),
        );

        let model = extract_qoder_model(model_info_raw.as_deref())
            .unwrap_or_else(|| QODER_MODEL_FALLBACK.to_string());
        models.insert(model.clone());

        total_input_tokens += input_tokens;
        total_output_tokens += output_tokens;
        total_cache_read_tokens += cache_read_tokens;

        requests.push(LocalRequestRecord {
            session_id: canonical_session_id.clone(),
            tool: tool_id.to_string(),
            timestamp,
            message_id,
            input_tokens,
            output_tokens,
            reasoning_tokens: 0,
            cache_create_tokens: 0,
            cache_read_tokens,
            total_tokens,
            request_count: 1,
            model,
            is_subagent: false,
            request_key: None,
            explicit_estimated_cost: None,
            source_file_present: None,
        });
    }

    let topic = (!session_row.session_title.trim().is_empty())
        .then(|| truncate_string(&session_row.session_title, 50));
    let cwd = session_row
        .project_uri
        .clone()
        .filter(|value| !value.trim().is_empty());
    let project_name = session_row
        .project_name
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| cwd.as_deref().and_then(extract_project_name));
    let session_name = (!session_row.session_title.trim().is_empty())
        .then(|| session_row.session_title.trim().to_string());
    let start_time = earliest_timestamp.unwrap_or_else(|| ms_to_sec(session_row.gmt_create_ms));
    let end_time = latest_timestamp.unwrap_or(start_time);
    let meta = SessionMeta {
        session_id: canonical_session_id.clone(),
        tool: tool_id.to_string(),
        cwd,
        project_name,
        topic,
        last_prompt: None,
        session_name,
        file_path: db_meta.db_path.to_string_lossy().to_string(),
        file_size: db_meta.file_size,
        // gmt_modified should always be >= gmt_create; .max() is a safety guard.
        last_modified: ms_to_sec(session_row.gmt_modified_ms.max(session_row.gmt_create_ms))
            .max(db_meta.last_modified),
        total_input_tokens,
        total_output_tokens,
        total_cache_create_tokens: 0,
        total_cache_read_tokens,
        models: models.into_iter().collect(),
        message_count: requests.len() as u64,
        start_time,
        end_time,
        source: QODER_DB_SOURCE_KIND.to_string(),
        message_ids: requests
            .iter()
            .map(|record| record.message_id.clone())
            .collect(),
    };

    let fingerprint = compute_qoder_session_fingerprint(
        db_meta.fingerprint,
        &canonical_session_id,
        meta.message_count,
        meta.total_input_tokens,
        meta.total_output_tokens,
        meta.total_cache_read_tokens,
        meta.last_modified,
    );
    Some(QoderIdeSessionData {
        meta,
        requests,
        fingerprint,
        source_locator: build_qoder_source_locator(&db_meta.db_path, &canonical_session_id),
    })
}

fn qoder_db_meta(db_path: &Path) -> Option<QoderDbMeta> {
    let db_metadata = std::fs::metadata(db_path).ok()?;
    let wal_path = db_path.with_extension("db-wal");
    let wal_metadata = std::fs::metadata(&wal_path).ok();

    let db_size = db_metadata.len();
    let wal_size = wal_metadata.as_ref().map(|meta| meta.len()).unwrap_or(0);
    let file_size = db_size + wal_size;

    let db_mtime = modified_epoch_seconds(&db_metadata);
    let wal_mtime = wal_metadata
        .as_ref()
        .map(modified_epoch_seconds)
        .unwrap_or(0);
    let last_modified = db_mtime.max(wal_mtime);

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    db_path.to_string_lossy().hash(&mut hasher);
    file_size.hash(&mut hasher);
    last_modified.hash(&mut hasher);
    let fingerprint = hasher.finish();

    Some(QoderDbMeta {
        db_path: db_path.to_path_buf(),
        file_size,
        last_modified,
        fingerprint,
    })
}

fn open_qoder_db_read_only(db_path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_millis(500))?;
    Ok(conn)
}

fn modified_epoch_seconds(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
fn canonical_qoder_session_id(raw_session_id: &str) -> String {
    canonical_qoder_session_id_for(super::constants::TOOL_QODER_IDE, raw_session_id)
}

fn canonical_qoder_session_id_for(tool_id: &str, raw_session_id: &str) -> String {
    format!("{}::{}", tool_id, raw_session_id)
}

fn build_qoder_source_locator(db_path: &Path, session_id: &str) -> String {
    format!("{}#{}", db_path.to_string_lossy(), session_id)
}

fn ms_to_sec(value: i64) -> i64 {
    if value > 10_000_000_000 {
        value / 1000
    } else {
        value
    }
}

fn parse_qoder_json(raw: &str) -> Option<Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

fn extract_qoder_model(raw: Option<&str>) -> Option<String> {
    let parsed = parse_qoder_json(raw?)?;
    parsed
        .get("model_key")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
}

fn compute_qoder_session_fingerprint(
    db_fingerprint: u64,
    session_id: &str,
    message_count: u64,
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_cache_read_tokens: u64,
    last_modified: i64,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    db_fingerprint.hash(&mut hasher);
    session_id.hash(&mut hasher);
    message_count.hash(&mut hasher);
    total_input_tokens.hash(&mut hasher);
    total_output_tokens.hash(&mut hasher);
    total_cache_read_tokens.hash(&mut hasher);
    last_modified.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::tempdir;

    #[test]
    fn qoder_session_id_is_namespaced() {
        assert_eq!(
            canonical_qoder_session_id("7a6c2d4b-8429-47c8-92b6-2fa9032323ae"),
            "qoder_ide::7a6c2d4b-8429-47c8-92b6-2fa9032323ae"
        );
    }

    #[test]
    fn qoder_model_defaults_to_none_for_empty_json() {
        assert_eq!(extract_qoder_model(Some("{}")), None);
        assert_eq!(
            extract_qoder_model(Some("{\"model_key\":\"custom_model\"}")),
            Some("custom_model".to_string())
        );
    }

    #[test]
    fn qoder_json_skips_empty_shapes() {
        assert!(parse_qoder_json("").is_none());
        assert!(parse_qoder_json("{}").is_none());
        assert!(parse_qoder_json("{\"prompt_tokens\":1}").is_some());
    }

    #[test]
    fn parse_qoder_session_normalizes_tokens_and_meta() {
        let dir = tempdir().expect("create temp dir");
        let db_path = dir.path().join("local.db");
        let conn = Connection::open(&db_path).expect("open qoder db");
        conn.execute_batch(
            "
            CREATE TABLE chat_session (
                session_id TEXT PRIMARY KEY,
                session_title TEXT NOT NULL,
                project_id TEXT,
                project_uri TEXT,
                project_name TEXT,
                gmt_create INTEGER,
                gmt_modified INTEGER
            );
            CREATE TABLE chat_message (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                role TEXT,
                token_info TEXT,
                model_info TEXT,
                gmt_create INTEGER
            );
            ",
        )
        .expect("create schema");
        conn.execute(
            "INSERT INTO chat_session (
                session_id, session_title, project_id, project_uri, project_name, gmt_create, gmt_modified
            ) VALUES (?1, ?2, 'p1', ?3, ?4, ?5, ?6)",
            params![
                "sess_1",
                "检查Qoder用量统计",
                "/Users/test/work/reference-project",
                "参考项目",
                1_781_147_471_063i64,
                1_781_147_982_238i64
            ],
        )
        .expect("insert session");
        conn.execute(
            "INSERT INTO chat_message (id, session_id, role, token_info, model_info, gmt_create)
             VALUES (?1, ?2, 'assistant', ?3, ?4, ?5)",
            params![
                "msg_1",
                "sess_1",
                "{\"prompt_tokens\":25054,\"completion_tokens\":273,\"cached_tokens\":3072}",
                "{\"model_key\":\"custom_model\"}",
                1_781_147_500_000i64
            ],
        )
        .expect("insert first message");
        conn.execute(
            "INSERT INTO chat_message (id, session_id, role, token_info, model_info, gmt_create)
             VALUES (?1, ?2, 'assistant', ?3, ?4, ?5)",
            params![
                "msg_2",
                "sess_1",
                "{\"prompt_tokens\":26887,\"completion_tokens\":174,\"cached_tokens\":25024}",
                "{}",
                1_781_147_560_000i64
            ],
        )
        .expect("insert second message");

        let db_meta = qoder_db_meta(&db_path).expect("db meta");
        let session_row = QoderSessionRow {
            raw_session_id: "sess_1".to_string(),
            session_title: "检查Qoder用量统计".to_string(),
            project_name: Some("参考项目".to_string()),
            project_uri: Some("/Users/test/work/reference-project".to_string()),
            gmt_create_ms: 1_781_147_471_063i64,
            gmt_modified_ms: 1_781_147_982_238i64,
        };

        let conn = open_qoder_db_read_only(&db_path).expect("reopen qoder db");
        let parsed = parse_qoder_session(&db_meta, &conn, &session_row, "qoder_ide")
            .expect("parse qoder session");
        assert_eq!(parsed.meta.session_id, "qoder_ide::sess_1");
        assert_eq!(parsed.meta.tool, "qoder_ide");
        assert_eq!(parsed.meta.project_name.as_deref(), Some("参考项目"));
        assert_eq!(
            parsed.meta.cwd.as_deref(),
            Some("/Users/test/work/reference-project")
        );
        assert_eq!(parsed.meta.message_count, 2);
        assert_eq!(parsed.meta.total_input_tokens, 23_845);
        assert_eq!(parsed.meta.total_output_tokens, 447);
        assert_eq!(parsed.meta.total_cache_read_tokens, 28_096);
        assert_eq!(parsed.meta.total_cache_create_tokens, 0);
        assert_eq!(parsed.meta.models, vec!["custom_model".to_string()]);
        assert_eq!(parsed.requests.len(), 2);

        let first = &parsed.requests[0];
        assert_eq!(first.message_id, "msg_1");
        assert_eq!(first.input_tokens, 21_982);
        assert_eq!(first.cache_read_tokens, 3_072);
        assert_eq!(first.output_tokens, 273);
        assert_eq!(first.total_tokens, 25_327);
        assert_eq!(first.model, "custom_model");

        let second = &parsed.requests[1];
        assert_eq!(second.input_tokens, 1_863);
        assert_eq!(second.cache_read_tokens, 25_024);
        assert_eq!(second.output_tokens, 174);
        assert_eq!(second.total_tokens, 27_061);
        assert_eq!(second.model, "custom_model");
    }
}
