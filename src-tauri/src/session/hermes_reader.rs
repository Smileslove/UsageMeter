use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::shared::extract_project_name;
use super::source::{ParsedSessionData, SessionSource, SourceSnapshot, SourceUpdateMode};
use rusqlite::{Connection, OpenFlags};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

const HERMES_SOURCE_KIND: &str = "hermes_sqlite";
const HERMES_AGENT_NAME: &str = "Hermes Agent";
const HERMES_FALLBACK_MODEL: &str = "hermes-agent";

pub(super) struct HermesSource {
    cache: OnceLock<Mutex<HashMap<String, HermesSessionData>>>,
}

#[derive(Debug, Clone)]
pub(crate) struct HermesSessionData {
    pub meta: SessionMeta,
    pub requests: Vec<LocalRequestRecord>,
    pub fingerprint: u64,
    pub source_locator: String,
}

#[derive(Debug, Clone)]
struct HermesDbMeta {
    db_path: PathBuf,
    file_size: u64,
    last_modified: i64,
    fingerprint: u64,
}

#[derive(Debug, Clone)]
struct HermesSessionRow {
    raw_session_id: String,
    model: String,
    started_at: i64,
    ended_at: i64,
    message_count: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    reasoning_tokens: u64,
    estimated_cost_usd: Option<f64>,
    actual_cost_usd: Option<f64>,
}

pub(super) static HERMES_SOURCE: HermesSource = HermesSource {
    cache: OnceLock::new(),
};

impl HermesSource {
    fn cache(&self) -> &Mutex<HashMap<String, HermesSessionData>> {
        self.cache.get_or_init(|| Mutex::new(HashMap::new()))
    }
}

impl SessionSource for HermesSource {
    fn tool_id(&self) -> &'static str {
        super::constants::TOOL_HERMES
    }

    fn scan(&self) -> SourceSnapshot {
        let scanned = scan_hermes_sessions();
        let scan_fingerprint = compute_hermes_scan_fingerprint(&scanned);
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
            .ok_or_else(|| format!("hermes session not found: {}", session.session_id))?;

        Ok(ParsedSessionData {
            meta: parsed.meta,
            requests: parsed.requests,
        })
    }
}

pub(crate) fn scan_hermes_sessions() -> Vec<HermesSessionData> {
    let db_paths = discover_hermes_db_paths();
    if db_paths.is_empty() {
        return Vec::new();
    }

    let mut sessions = Vec::new();
    let mut seen_session_ids = HashSet::new();

    for db_path in db_paths {
        let Some(db_meta) = hermes_db_meta(&db_path) else {
            continue;
        };
        let conn = match open_hermes_db_read_only(&db_path) {
            Ok(conn) => conn,
            Err(err) => {
                eprintln!(
                    "[UsageMeter] Failed to open Hermes DB {}: {}",
                    db_path.display(),
                    err
                );
                continue;
            }
        };

        let mut stmt = match conn.prepare(
            "SELECT id, COALESCE(model, ''), COALESCE(started_at, 0), COALESCE(ended_at, 0),
                    COALESCE(message_count, 0), COALESCE(input_tokens, 0), COALESCE(output_tokens, 0),
                    COALESCE(cache_read_tokens, 0), COALESCE(cache_write_tokens, 0),
                    COALESCE(reasoning_tokens, 0), estimated_cost_usd, actual_cost_usd
             FROM sessions
             WHERE TRIM(COALESCE(model, '')) != ''
               AND (
                    COALESCE(input_tokens, 0) > 0 OR
                    COALESCE(output_tokens, 0) > 0 OR
                    COALESCE(cache_read_tokens, 0) > 0 OR
                    COALESCE(cache_write_tokens, 0) > 0 OR
                    COALESCE(reasoning_tokens, 0) > 0 OR
                    COALESCE(actual_cost_usd, estimated_cost_usd, 0) > 0
               )
             ORDER BY started_at DESC",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                eprintln!(
                    "[UsageMeter] Failed to query Hermes sessions from {}: {}",
                    db_path.display(),
                    err
                );
                continue;
            }
        };

        let rows = match stmt.query_map([], |row| {
            Ok(HermesSessionRow {
                raw_session_id: row.get(0)?,
                model: row.get(1)?,
                started_at: normalize_hermes_timestamp(row.get::<_, f64>(2)?),
                ended_at: normalize_hermes_timestamp(row.get::<_, f64>(3)?),
                message_count: row.get::<_, i64>(4)?.max(0) as u64,
                input_tokens: row.get::<_, i64>(5)?.max(0) as u64,
                output_tokens: row.get::<_, i64>(6)?.max(0) as u64,
                cache_read_tokens: row.get::<_, i64>(7)?.max(0) as u64,
                cache_write_tokens: row.get::<_, i64>(8)?.max(0) as u64,
                reasoning_tokens: row.get::<_, i64>(9)?.max(0) as u64,
                estimated_cost_usd: row.get(10)?,
                actual_cost_usd: row.get(11)?,
            })
        }) {
            Ok(rows) => rows,
            Err(err) => {
                eprintln!(
                    "[UsageMeter] Failed to iterate Hermes sessions from {}: {}",
                    db_path.display(),
                    err
                );
                continue;
            }
        };

        for row in rows.flatten() {
            if row.raw_session_id.trim().is_empty() {
                continue;
            }
            let canonical_session_id = canonical_hermes_session_id(&row.raw_session_id);
            if !seen_session_ids.insert(canonical_session_id.clone()) {
                continue;
            }
            if let Some(session) = build_hermes_session(&db_meta, &canonical_session_id, row) {
                sessions.push(session);
            }
        }
    }

    sessions.sort_by_key(|session| std::cmp::Reverse(session.meta.last_modified));
    sessions
}

pub(crate) fn compute_hermes_scan_fingerprint(sessions: &[HermesSessionData]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for session in sessions {
        session.meta.session_id.hash(&mut hasher);
        session.fingerprint.hash(&mut hasher);
    }
    hasher.finish()
}

fn build_hermes_session(
    db_meta: &HermesDbMeta,
    canonical_session_id: &str,
    row: HermesSessionRow,
) -> Option<HermesSessionData> {
    let total_tokens =
        row.input_tokens + row.output_tokens + row.cache_read_tokens + row.cache_write_tokens;
    let total_tokens = total_tokens.max(row.input_tokens + row.output_tokens);
    let request_count = row.message_count.max(1);
    if total_tokens == 0
        && row.reasoning_tokens == 0
        && effective_hermes_cost(row.actual_cost_usd, row.estimated_cost_usd) <= 0.0
    {
        return None;
    }

    let model = row.model.trim().to_string();
    let model = if model.is_empty() {
        HERMES_FALLBACK_MODEL.to_string()
    } else {
        model
    };
    let project_name = infer_project_name_from_profile_path(&db_meta.db_path);
    // Hermes stores session totals at the session row level. For a finished session we prefer
    // `ended_at`; for an active session (`ended_at` missing/zero) we use the latest DB/WAL mtime
    // as the best available "last activity" approximation instead of backfilling a fake end time.
    let activity_time =
        resolve_hermes_activity_time(row.started_at, row.ended_at, db_meta.last_modified);
    let last_modified = activity_time.max(db_meta.last_modified);
    let explicit_cost = effective_hermes_cost(row.actual_cost_usd, row.estimated_cost_usd);
    let request_key = Some(format!(
        "{}:{}:{}:{}",
        super::constants::TOOL_HERMES,
        canonical_session_id,
        activity_time,
        total_tokens
    ));

    let requests = vec![LocalRequestRecord {
        session_id: canonical_session_id.to_string(),
        tool: super::constants::TOOL_HERMES.to_string(),
        timestamp: activity_time.max(0),
        message_id: format!("session:{}", row.raw_session_id),
        input_tokens: row.input_tokens,
        output_tokens: row.output_tokens + row.reasoning_tokens,
        reasoning_tokens: row.reasoning_tokens,
        cache_create_tokens: row.cache_write_tokens,
        cache_read_tokens: row.cache_read_tokens,
        total_tokens: total_tokens + row.reasoning_tokens,
        request_count,
        model: model.clone(),
        is_subagent: false,
        request_key,
        explicit_estimated_cost: (explicit_cost > 0.0).then_some(explicit_cost),
        source_file_present: None,
    }];

    let meta = SessionMeta {
        session_id: canonical_session_id.to_string(),
        tool: super::constants::TOOL_HERMES.to_string(),
        cwd: None,
        project_name,
        topic: Some(HERMES_AGENT_NAME.to_string()),
        last_prompt: None,
        session_name: Some(row.raw_session_id.clone()),
        file_path: db_meta.db_path.to_string_lossy().to_string(),
        file_size: db_meta.file_size,
        last_modified,
        total_input_tokens: row.input_tokens,
        total_output_tokens: row.output_tokens + row.reasoning_tokens,
        total_cache_create_tokens: row.cache_write_tokens,
        total_cache_read_tokens: row.cache_read_tokens,
        models: vec![model.clone()],
        message_count: row.message_count,
        start_time: row.started_at.max(0),
        end_time: activity_time,
        source: HERMES_SOURCE_KIND.to_string(),
        message_ids: requests
            .iter()
            .map(|record| record.message_id.clone())
            .collect(),
        explicit_estimated_cost: None,
        scope: None,
    };

    let fingerprint = compute_hermes_session_fingerprint(
        db_meta.fingerprint,
        request_count,
        &meta,
        explicit_cost,
    );

    Some(HermesSessionData {
        meta,
        requests,
        fingerprint,
        source_locator: build_hermes_source_locator(&db_meta.db_path, canonical_session_id),
    })
}

fn discover_hermes_db_paths() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let base_dir = resolve_hermes_base_dir();
    let default_db = base_dir.join("state.db");
    if default_db.exists() {
        candidates.push(default_db);
    }

    let profiles_dir = base_dir.join("profiles");
    if let Ok(entries) = std::fs::read_dir(&profiles_dir) {
        for entry in entries.flatten() {
            let db_path = entry.path().join("state.db");
            if db_path.exists() {
                candidates.push(db_path);
            }
        }
    }

    dedupe_paths(candidates)
}

fn resolve_hermes_base_dir() -> PathBuf {
    if let Some(value) = std::env::var_os("HERMES_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(value);
    }
    #[cfg(windows)]
    {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            let windows_path = PathBuf::from(local_app_data).join("hermes");
            if windows_path.exists() {
                return windows_path;
            }
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".hermes")
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for path in paths {
        let normalized = path.canonicalize().unwrap_or(path.clone());
        if seen.insert(normalized.clone()) {
            deduped.push(normalized);
        }
    }
    deduped
}

fn infer_project_name_from_profile_path(db_path: &Path) -> Option<String> {
    let profile_dir = db_path.parent()?;
    let profiles_dir = profile_dir.parent()?;
    if profiles_dir.file_name().and_then(|name| name.to_str()) == Some("profiles") {
        return profile_dir
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_string())
            .or_else(|| extract_project_name(profile_dir.to_string_lossy().as_ref()));
    }
    None
}

fn hermes_db_meta(db_path: &Path) -> Option<HermesDbMeta> {
    let db_metadata = std::fs::metadata(db_path).ok()?;
    let wal_path =
        db_path.with_file_name(format!("{}-wal", db_path.file_name()?.to_string_lossy()));
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

    Some(HermesDbMeta {
        db_path: db_path.to_path_buf(),
        file_size,
        last_modified,
        fingerprint,
    })
}

fn open_hermes_db_read_only(db_path: &Path) -> rusqlite::Result<Connection> {
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

fn normalize_hermes_timestamp(value: f64) -> i64 {
    if value <= 0.0 {
        0
    } else if value > 1_000_000_000_000.0 {
        (value / 1000.0).floor() as i64
    } else {
        value.floor() as i64
    }
}

fn effective_hermes_cost(actual: Option<f64>, estimated: Option<f64>) -> f64 {
    actual.or(estimated).unwrap_or(0.0).max(0.0)
}

fn resolve_hermes_activity_time(started_at: i64, ended_at: i64, db_last_modified: i64) -> i64 {
    if ended_at > 0 {
        ended_at.max(started_at).max(0)
    } else {
        db_last_modified.max(started_at).max(0)
    }
}

fn canonical_hermes_session_id(raw_session_id: &str) -> String {
    format!("{}::{}", super::constants::TOOL_HERMES, raw_session_id)
}

fn build_hermes_source_locator(db_path: &Path, session_id: &str) -> String {
    format!("{}#{}", db_path.to_string_lossy(), session_id)
}

fn compute_hermes_session_fingerprint(
    db_fingerprint: u64,
    request_count: u64,
    meta: &SessionMeta,
    explicit_cost: f64,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    db_fingerprint.hash(&mut hasher);
    meta.session_id.hash(&mut hasher);
    request_count.hash(&mut hasher);
    meta.total_input_tokens.hash(&mut hasher);
    meta.total_output_tokens.hash(&mut hasher);
    meta.total_cache_create_tokens.hash(&mut hasher);
    meta.total_cache_read_tokens.hash(&mut hasher);
    meta.end_time.hash(&mut hasher);
    explicit_cost.to_bits().hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_hermes_seconds_and_millis() {
        assert_eq!(normalize_hermes_timestamp(1_717_000_000.0), 1_717_000_000);
        assert_eq!(
            normalize_hermes_timestamp(1_717_000_000_123.0),
            1_717_000_000
        );
        assert_eq!(normalize_hermes_timestamp(0.0), 0);
    }

    #[test]
    fn canonical_hermes_session_id_is_namespaced() {
        assert_eq!(
            canonical_hermes_session_id("abc-123"),
            "hermes::abc-123".to_string()
        );
    }

    #[test]
    fn active_session_uses_db_mtime_as_last_activity() {
        assert_eq!(
            resolve_hermes_activity_time(1_717_000_000, 0, 1_717_000_123),
            1_717_000_123
        );
    }

    #[test]
    fn finished_session_prefers_ended_at() {
        assert_eq!(
            resolve_hermes_activity_time(1_717_000_000, 1_717_000_456, 1_717_000_123),
            1_717_000_456
        );
    }
}
