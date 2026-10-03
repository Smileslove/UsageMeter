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

        let mut cache = self.cache().lock().unwrap_or_else(|err| err.into_inner());
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
        let cache = self.cache().lock().unwrap_or_else(|err| err.into_inner());
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

    let mut sessions_by_id: HashMap<String, HermesSessionData> = HashMap::new();
    let mut seen_model_rows = HashSet::new();
    let mut counted_sessions = HashSet::new();

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

        let rows = query_hermes_usage_rows(&conn);
        for mut row in rows {
            if row.raw_session_id.trim().is_empty() {
                continue;
            }
            let canonical_session_id = canonical_hermes_session_id(&row.raw_session_id);
            if !seen_model_rows.insert((canonical_session_id.clone(), row.model.clone())) {
                continue;
            }
            if counted_sessions.contains(&canonical_session_id) {
                row.message_count = 0;
            } else if row.message_count == 0 {
                row.message_count = 1;
            }
            if let Some(session) = build_hermes_session(&db_meta, &canonical_session_id, row) {
                counted_sessions.insert(canonical_session_id.clone());
                if let Some(existing) = sessions_by_id.get_mut(&canonical_session_id) {
                    merge_hermes_session(existing, session);
                } else {
                    sessions_by_id.insert(canonical_session_id, session);
                }
            }
        }
    }

    let mut sessions = sessions_by_id.into_values().collect::<Vec<_>>();
    sessions.sort_by_key(|session| std::cmp::Reverse(session.meta.last_modified));
    sessions
}

fn merge_hermes_session(existing: &mut HermesSessionData, incoming: HermesSessionData) {
    existing.meta.total_input_tokens = existing
        .meta
        .total_input_tokens
        .saturating_add(incoming.meta.total_input_tokens);
    existing.meta.total_output_tokens = existing
        .meta
        .total_output_tokens
        .saturating_add(incoming.meta.total_output_tokens);
    existing.meta.total_cache_create_tokens = existing
        .meta
        .total_cache_create_tokens
        .saturating_add(incoming.meta.total_cache_create_tokens);
    existing.meta.total_cache_read_tokens = existing
        .meta
        .total_cache_read_tokens
        .saturating_add(incoming.meta.total_cache_read_tokens);
    existing.meta.message_count = existing
        .meta
        .message_count
        .saturating_add(incoming.meta.message_count);
    existing.meta.start_time = match (existing.meta.start_time, incoming.meta.start_time) {
        (0, value) => value,
        (value, 0) => value,
        (left, right) => left.min(right),
    };
    existing.meta.end_time = existing.meta.end_time.max(incoming.meta.end_time);
    existing.meta.last_modified = existing.meta.last_modified.max(incoming.meta.last_modified);
    existing.meta.models.extend(incoming.meta.models);
    existing.meta.models.sort();
    existing.meta.models.dedup();
    existing.meta.message_ids.extend(incoming.meta.message_ids);
    existing.requests.extend(incoming.requests);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    existing.fingerprint.hash(&mut hasher);
    incoming.fingerprint.hash(&mut hasher);
    existing.fingerprint = hasher.finish();
}

fn table_columns(conn: &Connection, table: &str) -> HashSet<String> {
    let mut columns = HashSet::new();
    let query = format!("PRAGMA table_info({table})");
    if let Ok(mut statement) = conn.prepare(&query) {
        if let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(1)) {
            columns.extend(rows.flatten());
        }
    }
    columns
}

fn has_table(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |_| Ok(()),
    )
    .is_ok()
}

fn numeric_column(columns: &HashSet<String>, table_alias: &str, name: &str) -> String {
    if columns.contains(name) {
        format!("COALESCE({table_alias}.{name}, 0)")
    } else {
        "0".to_string()
    }
}

fn optional_cost_column(columns: &HashSet<String>, table_alias: &str, name: &str) -> String {
    if columns.contains(name) {
        format!("{table_alias}.{name}")
    } else {
        "NULL".to_string()
    }
}

fn query_hermes_usage_rows(conn: &Connection) -> Vec<HermesSessionRow> {
    let sessions = table_columns(conn, "sessions");
    if !sessions.contains("id") {
        return Vec::new();
    }
    let session_expr = |name: &str| numeric_column(&sessions, "s", name);
    let session_cost_est = optional_cost_column(&sessions, "s", "estimated_cost_usd");
    let session_cost_actual = optional_cost_column(&sessions, "s", "actual_cost_usd");
    let model_expr = if sessions.contains("model") {
        "COALESCE(s.model, '')"
    } else {
        "''"
    };
    let mut rows = Vec::new();
    let mut covered = HashSet::new();

    if has_table(conn, "session_model_usage") {
        let usage = table_columns(conn, "session_model_usage");
        if usage.contains("session_id") && usage.contains("model") {
            let field = |name: &str| numeric_column(&usage, "u", name);
            let actual = optional_cost_column(&usage, "u", "actual_cost_usd");
            let estimated = optional_cost_column(&usage, "u", "estimated_cost_usd");
            let join = if sessions.contains("started_at")
                || sessions.contains("ended_at")
                || sessions.contains("message_count")
                || sessions.contains("model")
            {
                "LEFT JOIN sessions s ON s.id = u.session_id"
            } else {
                ""
            };
            let session_id = "u.session_id";
            let started = if sessions.contains("started_at") {
                "COALESCE(s.started_at, 0)"
            } else {
                "0"
            };
            let ended = if sessions.contains("ended_at") {
                "COALESCE(s.ended_at, 0)"
            } else {
                "0"
            };
            let message_count = if sessions.contains("message_count") {
                "COALESCE(s.message_count, 0)"
            } else {
                "0"
            };
            let actual_cost = if usage.contains("actual_cost_usd") {
                format!("SUM(COALESCE(NULLIF({actual}, 0), {estimated}, 0))")
            } else if usage.contains("estimated_cost_usd") {
                format!("SUM(COALESCE({estimated}, 0))")
            } else {
                "0".to_string()
            };
            let provider_join = join;
            let order = if sessions.contains("model") {
                "CASE WHEN u.model = s.model THEN 0 ELSE 1 END, u.model"
            } else {
                "u.model"
            };
            let query = format!(
                "SELECT {session_id}, u.model, {started}, {ended}, {message_count}, SUM({input}), SUM({output}), SUM({cache_read}), SUM({cache_write}), SUM({reasoning}), {actual_cost}, 0 FROM session_model_usage u {provider_join} WHERE TRIM(COALESCE(u.model, '')) != '' GROUP BY u.session_id, u.model HAVING SUM({input}) > 0 OR SUM({output}) > 0 OR SUM({cache_read}) > 0 OR SUM({cache_write}) > 0 OR SUM({reasoning}) > 0 OR {actual_cost} > 0 ORDER BY {started} DESC, {order}",
                input = field("input_tokens"), output = field("output_tokens"),
                cache_read = field("cache_read_tokens"), cache_write = field("cache_write_tokens"),
                reasoning = field("reasoning_tokens")
            );
            if let Ok(mut statement) = conn.prepare(&query) {
                if let Ok(mapped) = statement.query_map([], |row| {
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
                    for row in mapped.flatten() {
                        covered.insert(row.raw_session_id.clone());
                        rows.push(row);
                    }
                }
            }
        }
    }

    let started = session_expr("started_at");
    let ended = session_expr("ended_at");
    let count = session_expr("message_count");
    let input = session_expr("input_tokens");
    let output = session_expr("output_tokens");
    let cache_read = session_expr("cache_read_tokens");
    let cache_write = session_expr("cache_write_tokens");
    let reasoning = session_expr("reasoning_tokens");
    let model_filter = if sessions.contains("model") {
        "TRIM(COALESCE(s.model, '')) != ''"
    } else {
        "1 = 1"
    };
    let query = format!(
        "SELECT s.id, {model_expr}, {started}, {ended}, {count}, {input}, {output}, {cache_read}, {cache_write}, {reasoning}, {session_cost_est}, {session_cost_actual} FROM sessions s WHERE {model_filter} ORDER BY {started} DESC"
    );
    if let Ok(mut statement) = conn.prepare(&query) {
        if let Ok(mapped) = statement.query_map([], |row| {
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
            for row in mapped.flatten() {
                if !covered.contains(&row.raw_session_id) {
                    rows.push(row);
                }
            }
        }
    }
    rows
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
    // Hermes state.db 的 output_tokens 列不含 reasoning（reasoning 单列在 reasoning_tokens），
    // 与 meta.rs 口径保持一致：output_tokens 含 reasoning，
    // total = input + cache_read + cache_write + output（不把 reasoning 单列成 total 的加项）。
    // 注意：若上游将来在 output_tokens 列并入 reasoning，此处的相加必须移除，否则会双计。
    let output_tokens = row.output_tokens + row.reasoning_tokens;
    let total_tokens =
        row.input_tokens + row.cache_read_tokens + row.cache_write_tokens + output_tokens;
    let request_count = row.message_count;
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
    // request_key 与 record.total_tokens 使用同一口径（total_tokens 已含 reasoning），
    // 保证持久化的 request_key 与事实表的 total_tokens 一致。
    let request_key = Some(format!(
        "{}:{}:{}:{}:{}",
        super::constants::TOOL_HERMES,
        canonical_session_id,
        model,
        activity_time,
        total_tokens
    ));

    let requests = vec![LocalRequestRecord {
        session_id: canonical_session_id.to_string(),
        tool: super::constants::TOOL_HERMES.to_string(),
        timestamp: activity_time.max(0),
        message_id: format!("session:{}:{}", row.raw_session_id, model),
        input_tokens: row.input_tokens,
        // raw output_tokens 不含 reasoning，已合并（见 build_hermes_session 上方注释）。
        output_tokens,
        reasoning_tokens: row.reasoning_tokens,
        cache_create_tokens: row.cache_write_tokens,
        cache_read_tokens: row.cache_read_tokens,
        total_tokens,
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
        // output_tokens 已含 reasoning（见上方注释），与 record.output_tokens 同口径。
        total_output_tokens: output_tokens,
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
        ..Default::default()
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
    actual
        .filter(|value| *value > 0.0)
        .or(estimated)
        .unwrap_or(0.0)
        .max(0.0)
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

    #[test]
    fn hermes_session_folds_reasoning_into_output_and_total_once() {
        // 口径回归：output_tokens 含 reasoning（40+10=50），total 只含一次
        // reasoning（input 10 + cache_read 5 + cache_write 3 + output 50 = 68），
        // 且 request_key 与 total_tokens 使用同一口径，不允许双计或口径分裂。
        let db_meta = HermesDbMeta {
            db_path: PathBuf::from("/tmp/hermes/state.db"),
            file_size: 100,
            last_modified: 1_717_000_000,
            fingerprint: 1,
        };
        let row = HermesSessionRow {
            raw_session_id: "sess-1".to_string(),
            model: "hermes-3".to_string(),
            started_at: 1_717_000_000,
            ended_at: 1_717_000_100,
            message_count: 1,
            input_tokens: 10,
            output_tokens: 40,
            cache_read_tokens: 5,
            cache_write_tokens: 3,
            reasoning_tokens: 10,
            estimated_cost_usd: None,
            actual_cost_usd: None,
        };

        let session = build_hermes_session(&db_meta, "hermes::sess-1", row)
            .expect("session with tokens must be built");
        let record = &session.requests[0];

        assert_eq!(record.output_tokens, 50); // raw output 40 + reasoning 10
        assert_eq!(record.total_tokens, 68); // 10 + 5 + 3 + 50
        let request_key = record.request_key.as_deref().expect("request_key present");
        assert!(
            request_key.ends_with(":68"),
            "request_key must use the same reasoning-inclusive total: {request_key}"
        );
    }

    #[test]
    fn hermes_query_splits_models_and_falls_back_for_uncovered_legacy_sessions() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, model TEXT, started_at REAL, message_count INTEGER, input_tokens INTEGER, output_tokens INTEGER, cache_read_tokens INTEGER, cache_write_tokens INTEGER, reasoning_tokens INTEGER, estimated_cost_usd REAL, actual_cost_usd REAL);
             CREATE TABLE session_model_usage (session_id TEXT, model TEXT, input_tokens INTEGER, output_tokens INTEGER, cache_read_tokens INTEGER, cache_write_tokens INTEGER, estimated_cost_usd REAL, actual_cost_usd REAL);",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO sessions VALUES ('multi', 'model-a', 1000, 7, 300, 60, 0, 0, 0, NULL, NULL);
             INSERT INTO session_model_usage VALUES ('multi', 'model-a', 100, 20, 0, 0, 0.1, 0.1);
             INSERT INTO session_model_usage VALUES ('multi', 'model-b', 200, 40, 0, 0, 0.2, 0.2);
             INSERT INTO sessions VALUES ('legacy', 'model-c', 2000, 3, 50, 10, 4, 0, 0, NULL, NULL);",
        )
        .unwrap();

        let rows = query_hermes_usage_rows(&conn);
        assert_eq!(rows.len(), 3);
        let model_a = rows
            .iter()
            .find(|row| row.raw_session_id == "multi" && row.model == "model-a")
            .unwrap();
        let model_b = rows
            .iter()
            .find(|row| row.raw_session_id == "multi" && row.model == "model-b")
            .unwrap();
        let legacy = rows
            .iter()
            .find(|row| row.raw_session_id == "legacy")
            .unwrap();
        assert_eq!(model_a.input_tokens, 100);
        assert_eq!(model_b.input_tokens, 200);
        assert_eq!(model_a.message_count, 7);
        assert_eq!(model_b.message_count, 7);
        assert_eq!(legacy.input_tokens, 50);
        assert_eq!(legacy.cache_read_tokens, 4);
    }

    #[test]
    fn hermes_query_tolerates_old_session_model_usage_schema() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, model TEXT, started_at REAL, input_tokens INTEGER, output_tokens INTEGER);
             CREATE TABLE session_model_usage (session_id TEXT, model TEXT, input_tokens INTEGER, output_tokens INTEGER);
             INSERT INTO sessions VALUES ('old', 'fallback', 1000, 9, 2);
             INSERT INTO session_model_usage VALUES ('old', 'actual-model', 6, 1);",
        )
        .unwrap();

        let rows = query_hermes_usage_rows(&conn);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model, "actual-model");
        assert_eq!(rows[0].input_tokens, 6);
        assert_eq!(rows[0].output_tokens, 1);
    }

    #[test]
    fn hermes_multimodel_rows_merge_into_one_session_and_count_requests_once() {
        let db_meta = HermesDbMeta {
            db_path: PathBuf::from("/tmp/hermes/state.db"),
            file_size: 100,
            last_modified: 1_717_000_000,
            fingerprint: 1,
        };
        let first = HermesSessionRow {
            raw_session_id: "sess-multi".to_string(),
            model: "model-a".to_string(),
            started_at: 1_717_000_000,
            ended_at: 1_717_000_100,
            message_count: 7,
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            estimated_cost_usd: None,
            actual_cost_usd: None,
        };
        let second = HermesSessionRow {
            model: "model-b".to_string(),
            message_count: 0,
            input_tokens: 200,
            output_tokens: 40,
            ..first.clone()
        };

        let mut combined = build_hermes_session(&db_meta, "hermes::sess-multi", first).unwrap();
        let another = build_hermes_session(&db_meta, "hermes::sess-multi", second).unwrap();
        merge_hermes_session(&mut combined, another);

        assert_eq!(combined.requests.len(), 2);
        assert_eq!(combined.meta.models, vec!["model-a", "model-b"]);
        assert_eq!(combined.meta.total_input_tokens, 300);
        assert_eq!(combined.meta.total_output_tokens, 60);
        assert_eq!(combined.meta.message_count, 7);
        assert_eq!(combined.requests[0].request_count, 7);
        assert_eq!(combined.requests[1].request_count, 0);
    }
}
