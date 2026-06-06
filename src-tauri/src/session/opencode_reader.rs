//! OpenCode 本地数据读取模块
//!
//! OpenCode 旧版将消息落在 `storage/message/**/msg_*.json`，新版（v1.2+）
//! 则存入 `opencode.db`。本模块统一读取两条本地来源，并对外暴露：
//! - 会话级聚合 `SessionMeta`
//! - 请求级事实 `LocalRequestRecord`
//!
//! 设计目标：
//! - 只要 assistant 消息和 token/time 结构还能解析，就尽量产出事实
//! - DB 变化时优先增量读取 message 表，而不是每次整库重扫
//! - `session` 表漂移时退化到 message-only 模式，而不是整条链路返回空

use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::source::{ParsedSessionData, SessionSource, SourceSnapshot, SourceUpdateMode};
use rusqlite::{Connection, OpenFlags};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub(in crate::session) const OPENCODE_DB_FULL_RECONCILE_INTERVAL_SECS: i64 = 24 * 60 * 60;
pub(in crate::session) const REQUIRED_SESSION_COLUMNS: &[&str] = &[
    "id",
    "directory",
    "title",
    "model",
    "tokens_input",
    "tokens_output",
    "tokens_reasoning",
    "tokens_cache_read",
    "tokens_cache_write",
    "time_created",
    "time_updated",
    "time_archived",
];
pub(in crate::session) const REQUIRED_MESSAGE_COLUMNS: &[&str] =
    &["id", "session_id", "data", "time_updated"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum OpenCodeSchemaMode {
    Full,
    MessageOnly,
    #[default]
    Incompatible,
}

impl OpenCodeSchemaMode {
    fn as_str(self) -> &'static str {
        match self {
            OpenCodeSchemaMode::Full => "full",
            OpenCodeSchemaMode::MessageOnly => "message_only",
            OpenCodeSchemaMode::Incompatible => "incompatible",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "full" => OpenCodeSchemaMode::Full,
            "message_only" => OpenCodeSchemaMode::MessageOnly,
            "incompatible" => OpenCodeSchemaMode::Incompatible,
            _ => OpenCodeSchemaMode::Incompatible,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeMessageIdConflictStatus {
    pub has_conflict: bool,
    pub conflict_count: u64,
    #[serde(default)]
    pub sample_ids: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeSchemaStatus {
    pub db_found: bool,
    pub db_path: Option<String>,
    pub schema_compatible: bool,
    pub compatibility_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persisted_compatibility_mode: Option<String>,
    pub incompatibility_reason: Option<String>,
    pub message_id_conflict: OpenCodeMessageIdConflictStatus,
}

#[derive(Debug, Clone)]
pub struct OpenCodeSessionData {
    pub meta: SessionMeta,
    pub requests: Vec<LocalRequestRecord>,
    pub fingerprint: u64,
    pub source_locator: String,
}

#[derive(Debug, Clone)]
pub(in crate::session) struct OpenCodeMessageSnapshot {
    pub source_path: String,
    pub canonical_session_id: String,
    pub raw_message_id: String,
    pub timestamp_sec: i64,
    pub model: String,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    pub source_kind: &'static str,
}

#[derive(Debug, Clone)]
pub(in crate::session) struct SessionRow {
    pub id: String,
    pub directory: String,
    pub title: String,
    pub model_json: Option<String>,
    pub time_created_ms: i64,
    pub time_updated_ms: i64,
    pub tokens_input: i64,
    pub tokens_output: i64,
    pub tokens_reasoning: i64,
    pub tokens_cache_read: i64,
    pub tokens_cache_write: i64,
}

#[derive(Debug, Clone, Default)]
pub(in crate::session) struct OpenCodeDbCacheState {
    pub storage_signature_hash: u64,
    pub file_size: u64,
    pub schema_fingerprint: u64,
    pub assistant_row_count: u64,
    pub last_time_updated_ms: i64,
    pub last_rowid: i64,
    pub last_full_reconcile_at_ms: i64,
    pub schema_mode: OpenCodeSchemaMode,
    pub messages: HashMap<String, OpenCodeMessageSnapshot>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeDbScanState {
    pub storage_signature_hash: u64,
    pub file_size: u64,
    pub schema_fingerprint: u64,
    pub assistant_row_count: u64,
    pub last_time_updated_ms: i64,
    pub last_rowid: i64,
    pub last_full_reconcile_at_ms: i64,
    pub schema_mode: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeDbScanStates {
    #[serde(default)]
    pub stores: HashMap<String, OpenCodeDbScanState>,
}

#[derive(Debug, Clone)]
pub(in crate::session) struct OpenCodeStorageRoot {
    pub id: String,
    pub home: PathBuf,
    pub db_path: PathBuf,
    pub message_root: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub(in crate::session) struct OpenCodePathSignature {
    pub exists: bool,
    pub size: u64,
    pub mtime_ns: u128,
}

#[derive(Debug, Clone, Default)]
pub(in crate::session) struct OpenCodeStorageSignature {
    pub db_path: String,
    pub db: OpenCodePathSignature,
    pub wal: OpenCodePathSignature,
    pub shm: OpenCodePathSignature,
}

#[derive(Debug, Clone, Default)]
pub(in crate::session) struct OpenCodeDbCheckpoint {
    pub schema_fingerprint: u64,
    pub assistant_row_count: u64,
    pub max_time_updated_ms: i64,
    pub max_rowid: i64,
}

#[derive(Debug, Clone)]
pub(in crate::session) struct OpenCodeFileEntryState {
    pub size: u64,
    pub mtime_ms: i64,
    pub message_identity_key: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(in crate::session) struct OpenCodeFileCacheState {
    pub files: HashMap<String, OpenCodeFileEntryState>,
    pub messages: HashMap<String, OpenCodeMessageSnapshot>,
}

#[derive(Debug, Clone, Default)]
struct OpenCodeScanCache {
    stores: HashMap<String, OpenCodeStoreCacheState>,
}

#[derive(Debug, Clone, Default)]
struct OpenCodeStoreCacheState {
    db_state: OpenCodeDbCacheState,
    file_state: OpenCodeFileCacheState,
}

static OPENCODE_SCAN_CACHE: OnceLock<Arc<Mutex<OpenCodeScanCache>>> = OnceLock::new();

pub(super) struct OpenCodeSource;

impl SessionSource for OpenCodeSource {
    fn tool_id(&self) -> &'static str {
        super::constants::TOOL_OPENCODE
    }

    fn scan(&self) -> SourceSnapshot {
        let scanned = scan_opencode_sessions();
        let sessions = scanned
            .iter()
            .map(|session| SessionFile {
                session_id: session.meta.session_id.clone(),
                tool: session.meta.tool.clone(),
                project_path: session.meta.project_name.clone().unwrap_or_default(),
                file_path: session.source_locator.clone(),
                transcript_paths: Vec::new(),
                file_size: session.meta.file_size,
                last_modified: session.meta.last_modified,
                fingerprint: session.fingerprint,
            })
            .collect::<Vec<_>>();

        SourceSnapshot {
            source_id: self.tool_id(),
            update_mode: SourceUpdateMode::ReplaceAll,
            scan_fingerprint: compute_opencode_scan_fingerprint(&scanned),
            sessions,
        }
    }

    fn parse(&self, session: &SessionFile) -> Result<ParsedSessionData, String> {
        let Some(parsed) = scan_opencode_sessions()
            .into_iter()
            .find(|item| item.meta.session_id == session.session_id)
        else {
            return Err(format!(
                "opencode session not found: {}",
                session.session_id
            ));
        };

        Ok(ParsedSessionData {
            meta: parsed.meta,
            requests: parsed.requests,
        })
    }
}

fn get_scan_cache() -> &'static Arc<Mutex<OpenCodeScanCache>> {
    OPENCODE_SCAN_CACHE.get_or_init(|| Arc::new(Mutex::new(OpenCodeScanCache::default())))
}

pub fn find_opencode_db() -> Option<PathBuf> {
    native_opencode_storage_root().and_then(|root| root.db_path.exists().then_some(root.db_path))
}

fn resolve_opencode_home() -> PathBuf {
    if let Ok(v) = std::env::var("OPENCODE_HOME") {
        let path = PathBuf::from(v);
        if path.exists() {
            return path;
        }
    }
    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".local")
                .join("share")
        })
        .join("opencode")
}

pub(in crate::session) fn opencode_message_storage_root() -> PathBuf {
    resolve_opencode_home().join("storage").join("message")
}

fn native_opencode_storage_root() -> Option<OpenCodeStorageRoot> {
    if let Ok(v) = std::env::var("OPENCODE_DB") {
        let db_path = PathBuf::from(v);
        if db_path.exists() {
            let home = db_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(resolve_opencode_home);
            return Some(OpenCodeStorageRoot {
                id: "native".to_string(),
                db_path,
                message_root: home.join("storage").join("message"),
                home,
            });
        }
    }

    let home = resolve_opencode_home();
    Some(OpenCodeStorageRoot {
        id: "native".to_string(),
        db_path: home.join("opencode.db"),
        message_root: home.join("storage").join("message"),
        home,
    })
}

fn discover_opencode_storage_roots() -> Vec<OpenCodeStorageRoot> {
    let mut roots = Vec::new();
    if let Some(native) = native_opencode_storage_root() {
        roots.push(native);
    }

    #[cfg(windows)]
    if let Some(cfg) = super::wsl::scan_config_if_enabled() {
        for home in super::wsl::opencode_home_roots(&cfg) {
            let db_path = home.join("opencode.db");
            let message_root = home.join("storage").join("message");
            if !db_path.exists() && !message_root.exists() {
                continue;
            }
            let distro = super::meta::wsl_distro_from_path(&home.to_string_lossy())
                .unwrap_or_else(|| "unknown".to_string());
            roots.push(OpenCodeStorageRoot {
                id: format!("wsl:{}", sanitize_storage_id_segment(&distro)),
                home,
                db_path,
                message_root,
            });
        }
    }

    dedupe_storage_roots(roots)
}

fn dedupe_storage_roots(roots: Vec<OpenCodeStorageRoot>) -> Vec<OpenCodeStorageRoot> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for root in roots {
        let key = root.home.to_string_lossy().to_ascii_lowercase();
        if seen.insert(key) {
            out.push(root);
        }
    }
    out
}

#[cfg_attr(not(windows), allow(dead_code))]
fn sanitize_storage_id_segment(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

#[allow(dead_code)]
pub fn compute_opencode_db_fingerprint(db_path: &Path) -> u64 {
    super::opencode::db_scan::compute_opencode_db_storage_signature(db_path).hash()
}

pub fn compute_opencode_scan_fingerprint(sessions: &[OpenCodeSessionData]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    sessions.len().hash(&mut hasher);
    for session in sessions {
        session.meta.session_id.hash(&mut hasher);
        session.meta.last_modified.hash(&mut hasher);
        session.meta.source.hash(&mut hasher);
        session.meta.session_name.hash(&mut hasher);
        session.meta.message_count.hash(&mut hasher);
        session.fingerprint.hash(&mut hasher);
    }
    hasher.finish()
}

pub fn check_opencode_schema() -> OpenCodeSchemaStatus {
    let db_path = match find_opencode_db() {
        Some(p) => p,
        None => {
            return OpenCodeSchemaStatus {
                db_found: false,
                db_path: None,
                schema_compatible: true,
                compatibility_mode: "full".to_string(),
                persisted_compatibility_mode: None,
                incompatibility_reason: None,
                message_id_conflict: OpenCodeMessageIdConflictStatus {
                    has_conflict: false,
                    conflict_count: 0,
                    sample_ids: Vec::new(),
                },
            };
        }
    };

    let db_path_str = db_path.to_string_lossy().to_string();
    let conn = match Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(e) => {
            return OpenCodeSchemaStatus {
                db_found: true,
                db_path: Some(db_path_str),
                schema_compatible: false,
                compatibility_mode: "incompatible".to_string(),
                persisted_compatibility_mode: None,
                incompatibility_reason: Some(format!("无法打开数据库：{}", e)),
                message_id_conflict: OpenCodeMessageIdConflictStatus {
                    has_conflict: false,
                    conflict_count: 0,
                    sample_ids: Vec::new(),
                },
            };
        }
    };
    let _ = conn.execute_batch("PRAGMA busy_timeout=2000;");

    let (mode, reason) = super::opencode::schema::detect_schema_mode(
        &conn,
        REQUIRED_SESSION_COLUMNS,
        REQUIRED_MESSAGE_COLUMNS,
    );
    let conflict = super::opencode::conflict::detect_message_id_conflicts(&conn, 5);
    OpenCodeSchemaStatus {
        db_found: true,
        db_path: Some(db_path_str),
        schema_compatible: mode != OpenCodeSchemaMode::Incompatible,
        compatibility_mode: mode.as_str().to_string(),
        persisted_compatibility_mode: None,
        incompatibility_reason: reason,
        message_id_conflict: conflict,
    }
}

pub fn scan_opencode_sessions() -> Vec<OpenCodeSessionData> {
    let mut cache = get_scan_cache().lock().unwrap();
    let roots = discover_opencode_storage_roots();
    let current_ids: HashSet<String> = roots.iter().map(|root| root.id.clone()).collect();
    cache.stores.retain(|id, _| current_ids.contains(id));

    let mut combined: HashMap<String, OpenCodeMessageSnapshot> = HashMap::new();
    let mut session_db_paths: HashMap<String, PathBuf> = HashMap::new();
    let mut session_source_paths: HashMap<String, String> = HashMap::new();

    for root in &roots {
        let store_cache = cache.stores.entry(root.id.clone()).or_default();
        let db_messages =
            super::opencode::db_scan::refresh_db_messages_for_path(&mut store_cache.db_state, root);
        let file_messages = super::opencode::legacy_scan::refresh_legacy_file_messages_for_root(
            &mut store_cache.file_state,
            root,
        );

        if root.db_path.exists() {
            for snapshot in db_messages.values() {
                session_db_paths
                    .entry(snapshot.canonical_session_id.clone())
                    .or_insert_with(|| root.db_path.clone());
            }
        }
        for (key, snapshot) in file_messages.into_iter().chain(db_messages.into_iter()) {
            session_source_paths
                .entry(snapshot.canonical_session_id.clone())
                .or_insert_with(|| snapshot.source_path.clone());
            combined.insert(key, snapshot);
        }
    }
    drop(cache);

    super::opencode::session_aggregate::build_session_data_from_messages(
        combined,
        session_db_paths,
        session_source_paths,
        REQUIRED_SESSION_COLUMNS,
        REQUIRED_MESSAGE_COLUMNS,
        query_session_rows,
    )
}

#[allow(dead_code)]
pub fn get_opencode_db_scan_state() -> OpenCodeDbScanState {
    let cache = get_scan_cache().lock().unwrap();
    cache
        .stores
        .get("native")
        .map(|store| db_cache_state_to_scan_state(&store.db_state))
        .unwrap_or_default()
}

pub fn get_opencode_db_scan_states() -> OpenCodeDbScanStates {
    let cache = get_scan_cache().lock().unwrap();
    OpenCodeDbScanStates {
        stores: cache
            .stores
            .iter()
            .map(|(id, store)| (id.clone(), db_cache_state_to_scan_state(&store.db_state)))
            .collect(),
    }
}

#[allow(dead_code)]
pub fn hydrate_opencode_db_scan_state(state: &OpenCodeDbScanState) {
    let mut cache = get_scan_cache().lock().unwrap();
    hydrate_db_state(
        &mut cache
            .stores
            .entry("native".to_string())
            .or_default()
            .db_state,
        state,
    );
}

pub fn hydrate_opencode_db_scan_states(states: &OpenCodeDbScanStates) {
    let mut cache = get_scan_cache().lock().unwrap();
    for (id, state) in &states.stores {
        hydrate_db_state(
            &mut cache.stores.entry(id.clone()).or_default().db_state,
            state,
        );
    }
}

fn hydrate_db_state(state: &mut OpenCodeDbCacheState, scan_state: &OpenCodeDbScanState) {
    state.storage_signature_hash = scan_state.storage_signature_hash;
    state.file_size = scan_state.file_size;
    state.schema_fingerprint = scan_state.schema_fingerprint;
    state.assistant_row_count = scan_state.assistant_row_count;
    state.last_time_updated_ms = scan_state.last_time_updated_ms;
    state.last_rowid = scan_state.last_rowid;
    state.last_full_reconcile_at_ms = scan_state.last_full_reconcile_at_ms;
    state.schema_mode = OpenCodeSchemaMode::from_str(&scan_state.schema_mode);
}

fn db_cache_state_to_scan_state(state: &OpenCodeDbCacheState) -> OpenCodeDbScanState {
    OpenCodeDbScanState {
        storage_signature_hash: state.storage_signature_hash,
        file_size: state.file_size,
        schema_fingerprint: state.schema_fingerprint,
        assistant_row_count: state.assistant_row_count,
        last_time_updated_ms: state.last_time_updated_ms,
        last_rowid: state.last_rowid,
        last_full_reconcile_at_ms: state.last_full_reconcile_at_ms,
        schema_mode: state.schema_mode(),
    }
}

impl OpenCodeDbCacheState {
    fn schema_mode(&self) -> String {
        if self.storage_signature_hash == 0
            && self.assistant_row_count == 0
            && self.last_time_updated_ms == 0
            && self.last_rowid == 0
        {
            "unknown".to_string()
        } else {
            self.schema_mode.as_str().to_string()
        }
    }
}

impl OpenCodeStorageSignature {
    pub(in crate::session) fn hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.db_path.hash(&mut hasher);
        hash_path_signature(&self.db, &mut hasher);
        hash_path_signature(&self.wal, &mut hasher);
        hash_path_signature(&self.shm, &mut hasher);
        hasher.finish()
    }
}

fn hash_path_signature(
    signature: &OpenCodePathSignature,
    hasher: &mut std::collections::hash_map::DefaultHasher,
) {
    signature.exists.hash(hasher);
    signature.size.hash(hasher);
    signature.mtime_ns.hash(hasher);
}

fn query_session_rows(conn: &Connection) -> HashMap<String, SessionRow> {
    let mut stmt = match conn.prepare(
        "SELECT id, directory, title, model,
                COALESCE(tokens_input, 0), COALESCE(tokens_output, 0), COALESCE(tokens_reasoning, 0),
                COALESCE(tokens_cache_read, 0), COALESCE(tokens_cache_write, 0),
                COALESCE(time_created, 0), COALESCE(time_updated, 0)
         FROM session
         WHERE time_archived IS NULL
         ORDER BY time_updated DESC",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return HashMap::new(),
    };
    let rows = match stmt.query_map([], |row| {
        Ok(SessionRow {
            id: row.get::<_, String>(0)?,
            directory: row.get::<_, String>(1).unwrap_or_default(),
            title: row.get::<_, String>(2).unwrap_or_default(),
            model_json: row.get::<_, Option<String>>(3)?,
            tokens_input: row.get::<_, i64>(4).unwrap_or(0),
            tokens_output: row.get::<_, i64>(5).unwrap_or(0),
            tokens_reasoning: row.get::<_, i64>(6).unwrap_or(0),
            tokens_cache_read: row.get::<_, i64>(7).unwrap_or(0),
            tokens_cache_write: row.get::<_, i64>(8).unwrap_or(0),
            time_created_ms: row.get::<_, i64>(9).unwrap_or(0),
            time_updated_ms: row.get::<_, i64>(10).unwrap_or(0),
        })
    }) {
        Ok(rows) => rows,
        Err(_) => return HashMap::new(),
    };

    let mut out = HashMap::new();
    for row in rows.flatten() {
        out.insert(row.id.clone(), row);
    }
    out
}

impl OpenCodeMessageSnapshot {
    pub(in crate::session) fn message_identity_key(&self) -> String {
        format!("{}|{}", self.canonical_session_id, self.raw_message_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::opencode::message::{
        canonical_opencode_session_id, normalize_model_string, parse_message_snapshot,
    };
    use crate::session::opencode::schema;
    use rusqlite::Connection;
    use std::fs;

    #[test]
    fn canonical_opencode_session_id_adds_tool_namespace_once() {
        assert_eq!(
            canonical_opencode_session_id("sess_abc"),
            "opencode::native::sess_abc"
        );
        assert_eq!(
            canonical_opencode_session_id("opencode::sess_abc"),
            "opencode::native::sess_abc"
        );
        assert_eq!(
            canonical_opencode_session_id("opencode::native::sess_abc"),
            "opencode::native::sess_abc"
        );
    }

    #[test]
    fn missing_query_sessions_columns_are_reported_as_message_only() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("opencode.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "
            CREATE TABLE session (
              id TEXT,
              directory TEXT,
              title TEXT,
              tokens_input INTEGER,
              tokens_output INTEGER,
              tokens_cache_read INTEGER,
              tokens_cache_write INTEGER,
              time_created INTEGER,
              time_updated INTEGER
            );
            CREATE TABLE message (
              id TEXT,
              session_id TEXT,
              time_updated INTEGER,
              data TEXT
            );
            ",
        )
        .unwrap();

        let (mode, reason) =
            schema::detect_schema_mode(&conn, REQUIRED_SESSION_COLUMNS, REQUIRED_MESSAGE_COLUMNS);

        assert_eq!(mode, OpenCodeSchemaMode::MessageOnly);
        assert!(reason.unwrap_or_default().contains("session 表缺少字段"));
    }

    #[test]
    fn parse_message_snapshot_uses_completed_time_and_model_fallbacks() {
        let data = serde_json::json!({
            "id": "msg_1",
            "sessionID": "sess_1",
            "model": "glm-4.7-free",
            "path": { "cwd": "/Users/me/demo" },
            "role": "assistant",
            "time": { "created": 1000, "completed": 2000 },
            "tokens": {
                "input": 10,
                "output": 2,
                "reasoning": 1,
                "cache": { "read": 3, "write": 4 }
            }
        });

        let snapshot = parse_message_snapshot(
            "native",
            "/tmp/opencode.db",
            "sess_1",
            "msg_1",
            &data,
            0,
            "opencode_db",
        )
        .expect("snapshot");

        assert_eq!(snapshot.timestamp_sec, 2);
        assert_eq!(snapshot.canonical_session_id, "opencode::native::sess_1");
        assert_eq!(snapshot.model, "glm-4.7-free");
        assert_eq!(snapshot.cwd.as_deref(), Some("/Users/me/demo"));
        assert_eq!(snapshot.total_tokens, 20);
    }

    #[test]
    fn parse_message_snapshot_namespaces_sessions_by_storage_root() {
        let data = serde_json::json!({
            "id": "msg_same",
            "sessionID": "sess_same",
            "model": "gpt-4o",
            "role": "assistant",
            "time": { "completed": 2000 },
            "tokens": { "input": 1, "output": 1 }
        });

        let native = parse_message_snapshot(
            "native",
            "/tmp/native/opencode.db",
            "sess_same",
            "msg_same",
            &data,
            0,
            "opencode_db",
        )
        .expect("native snapshot");
        let wsl = parse_message_snapshot(
            "wsl:Ubuntu",
            r"\\wsl$\Ubuntu\home\alice\.local\share\opencode\opencode.db",
            "sess_same",
            "msg_same",
            &data,
            0,
            "opencode_db",
        )
        .expect("wsl snapshot");

        assert_eq!(native.canonical_session_id, "opencode::native::sess_same");
        assert_eq!(wsl.canonical_session_id, "opencode::wsl:Ubuntu::sess_same");
        assert_ne!(native.message_identity_key(), wsl.message_identity_key());
        assert!(wsl.source_path.starts_with(r"\\wsl$\Ubuntu\"));
    }

    #[test]
    fn scan_fingerprint_and_meta_preserve_wsl_source_path() {
        let data = serde_json::json!({
            "id": "msg_wsl",
            "sessionID": "sess_wsl",
            "model": "gpt-4o",
            "path": { "cwd": "/home/alice/project" },
            "role": "assistant",
            "time": { "completed": 2000 },
            "tokens": { "input": 1, "output": 1 }
        });
        let source_path = r"\\wsl$\Ubuntu\home\alice\.local\share\opencode\opencode.db";
        let snapshot = parse_message_snapshot(
            "wsl:Ubuntu",
            source_path,
            "sess_wsl",
            "msg_wsl",
            &data,
            0,
            "opencode_db",
        )
        .expect("snapshot");
        let mut combined = HashMap::new();
        combined.insert(snapshot.message_identity_key(), snapshot);
        let mut session_source_paths = HashMap::new();
        session_source_paths.insert(
            "opencode::wsl:Ubuntu::sess_wsl".to_string(),
            source_path.to_string(),
        );

        let sessions =
            crate::session::opencode::session_aggregate::build_session_data_from_messages(
                combined,
                HashMap::new(),
                session_source_paths,
                REQUIRED_SESSION_COLUMNS,
                REQUIRED_MESSAGE_COLUMNS,
                |_| HashMap::new(),
            );

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].meta.file_path, source_path);
        assert_eq!(
            crate::session::meta::wsl_distro_from_path(&sessions[0].meta.file_path),
            Some("Ubuntu".to_string())
        );
    }

    #[test]
    fn normalize_model_string_drops_custom_provider_prefix_when_model_exists() {
        assert_eq!(
            normalize_model_string(Some("xiaomi-mini"), Some("mimo-v2.5")),
            "mimo-v2.5"
        );
        assert_eq!(
            normalize_model_string(Some("anthropic"), Some("claude-sonnet-4-5")),
            "claude-sonnet-4-5"
        );
        assert_eq!(
            normalize_model_string(Some("xiaomi-mini"), None),
            "xiaomi-mini"
        );
    }

    #[test]
    fn legacy_opencode_file_recovers_after_temporary_corruption() {
        let tmpdir = tempfile::tempdir().unwrap();
        let data_home = tmpdir.path().join(".local").join("share");
        let message_dir = data_home
            .join("opencode")
            .join("storage")
            .join("message")
            .join("sess_test");
        fs::create_dir_all(&message_dir).unwrap();
        let message_path = message_dir.join("msg_test.json");

        let old_xdg_data_home = std::env::var_os("XDG_DATA_HOME");
        std::env::set_var("XDG_DATA_HOME", &data_home);

        fs::write(
            &message_path,
            serde_json::json!({
                "id": "msg_test",
                "sessionID": "sess_test",
                "model": "glm-4.7-free",
                "path": { "cwd": "/tmp/project" },
                "role": "assistant",
                "time": { "created": 1700000000000_i64, "completed": 1700000005000_i64 },
                "tokens": { "input": 4, "output": 1, "reasoning": 0, "cache": { "read": 0, "write": 0 } }
            })
            .to_string(),
        )
        .unwrap();

        let first = scan_opencode_sessions();
        let first_session = first
            .iter()
            .find(|session| session.meta.session_id == "opencode::native::sess_test")
            .expect("find target session");
        assert_eq!(first_session.requests.len(), 1);

        fs::write(&message_path, "{").unwrap();
        let corrupted = scan_opencode_sessions();
        let corrupted_session = corrupted
            .iter()
            .find(|session| session.meta.session_id == "opencode::native::sess_test")
            .expect("preserve target session during corruption");
        assert_eq!(corrupted_session.requests[0].input_tokens, 4);
        assert_eq!(corrupted_session.requests[0].output_tokens, 1);

        fs::write(
            &message_path,
            serde_json::json!({
                "id": "msg_test",
                "sessionID": "sess_test",
                "model": "glm-4.7-free",
                "path": { "cwd": "/tmp/project" },
                "role": "assistant",
                "time": { "created": 1700000000000_i64, "completed": 1700000010000_i64 },
                "tokens": { "input": 8, "output": 2, "reasoning": 1, "cache": { "read": 0, "write": 0 } }
            })
            .to_string(),
        )
        .unwrap();
        let recovered = scan_opencode_sessions();
        let recovered_session = recovered
            .iter()
            .find(|session| session.meta.session_id == "opencode::native::sess_test")
            .expect("find recovered target session");
        assert_eq!(recovered_session.requests[0].input_tokens, 8);
        assert_eq!(recovered_session.requests[0].output_tokens, 3);

        match old_xdg_data_home {
            Some(value) => std::env::set_var("XDG_DATA_HOME", value),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
    }
}
