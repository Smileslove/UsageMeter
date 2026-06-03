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

use super::meta::{LocalRequestRecord, SessionMeta};
use rusqlite::{params, Connection, OpenFlags};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

const TOOL_OPENCODE: &str = "opencode";
const OPENCODE_DB_FULL_RECONCILE_INTERVAL_SECS: i64 = 24 * 60 * 60;
const REQUIRED_SESSION_COLUMNS: &[&str] = &[
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
const REQUIRED_MESSAGE_COLUMNS: &[&str] = &["id", "session_id", "data", "time_updated"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum OpenCodeSchemaMode {
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
struct OpenCodeMessageSnapshot {
    canonical_session_id: String,
    raw_session_id: String,
    raw_message_id: String,
    timestamp_sec: i64,
    model: String,
    cwd: Option<String>,
    title: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
    cache_create_tokens: u64,
    cache_read_tokens: u64,
    total_tokens: u64,
    source_kind: &'static str,
}

#[derive(Debug, Clone)]
struct SessionRow {
    id: String,
    directory: String,
    title: String,
    model_json: Option<String>,
    time_created_ms: i64,
    time_updated_ms: i64,
    tokens_input: i64,
    tokens_output: i64,
    tokens_reasoning: i64,
    tokens_cache_read: i64,
    tokens_cache_write: i64,
}

#[derive(Debug, Clone, Default)]
struct OpenCodeDbCacheState {
    storage_signature_hash: u64,
    file_size: u64,
    schema_fingerprint: u64,
    assistant_row_count: u64,
    last_time_updated_ms: i64,
    last_rowid: i64,
    last_full_reconcile_at_ms: i64,
    schema_mode: OpenCodeSchemaMode,
    messages: HashMap<String, OpenCodeMessageSnapshot>,
}

#[derive(Debug, Clone, Default)]
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

#[derive(Debug, Clone, Default)]
struct OpenCodePathSignature {
    exists: bool,
    size: u64,
    mtime_ns: u128,
}

#[derive(Debug, Clone, Default)]
struct OpenCodeStorageSignature {
    db_path: String,
    db: OpenCodePathSignature,
    wal: OpenCodePathSignature,
    shm: OpenCodePathSignature,
}

#[derive(Debug, Clone, Default)]
struct OpenCodeDbCheckpoint {
    schema_fingerprint: u64,
    assistant_row_count: u64,
    max_time_updated_ms: i64,
    max_rowid: i64,
}

#[derive(Debug, Clone)]
struct OpenCodeFileEntryState {
    size: u64,
    mtime_ms: i64,
    message_identity_key: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct OpenCodeFileCacheState {
    files: HashMap<String, OpenCodeFileEntryState>,
    messages: HashMap<String, OpenCodeMessageSnapshot>,
}

#[derive(Debug, Clone, Default)]
struct OpenCodeScanCache {
    db_state: OpenCodeDbCacheState,
    file_state: OpenCodeFileCacheState,
}

static OPENCODE_SCAN_CACHE: OnceLock<Arc<Mutex<OpenCodeScanCache>>> = OnceLock::new();

fn get_scan_cache() -> &'static Arc<Mutex<OpenCodeScanCache>> {
    OPENCODE_SCAN_CACHE.get_or_init(|| Arc::new(Mutex::new(OpenCodeScanCache::default())))
}

pub fn find_opencode_db() -> Option<PathBuf> {
    if let Ok(v) = std::env::var("OPENCODE_DB") {
        let path = PathBuf::from(v);
        if path.exists() {
            return Some(path);
        }
    }

    let path = resolve_opencode_home().join("opencode.db");
    path.exists().then_some(path)
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

fn opencode_message_storage_root() -> PathBuf {
    resolve_opencode_home().join("storage").join("message")
}

#[allow(dead_code)]
pub fn compute_opencode_db_fingerprint(db_path: &Path) -> u64 {
    compute_opencode_db_storage_signature(db_path).hash()
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

    let (mode, reason) = detect_schema_mode(&conn);
    let conflict = detect_message_id_conflicts(&conn, 5);
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
    let db_messages = refresh_db_messages(&mut cache.db_state);
    let file_messages = refresh_legacy_file_messages(&mut cache.file_state);

    let mut combined: HashMap<String, OpenCodeMessageSnapshot> = HashMap::new();
    for (key, snapshot) in file_messages {
        combined.insert(key, snapshot);
    }
    for (key, snapshot) in db_messages {
        combined.insert(key, snapshot);
    }
    drop(cache);

    build_session_data_from_messages(combined)
}

pub fn get_opencode_db_scan_state() -> OpenCodeDbScanState {
    let cache = get_scan_cache().lock().unwrap();
    OpenCodeDbScanState {
        storage_signature_hash: cache.db_state.storage_signature_hash,
        file_size: cache.db_state.file_size,
        schema_fingerprint: cache.db_state.schema_fingerprint,
        assistant_row_count: cache.db_state.assistant_row_count,
        last_time_updated_ms: cache.db_state.last_time_updated_ms,
        last_rowid: cache.db_state.last_rowid,
        last_full_reconcile_at_ms: cache.db_state.last_full_reconcile_at_ms,
        schema_mode: cache.db_state.schema_mode(),
    }
}

pub fn hydrate_opencode_db_scan_state(state: &OpenCodeDbScanState) {
    let mut cache = get_scan_cache().lock().unwrap();
    cache.db_state.storage_signature_hash = state.storage_signature_hash;
    cache.db_state.file_size = state.file_size;
    cache.db_state.schema_fingerprint = state.schema_fingerprint;
    cache.db_state.assistant_row_count = state.assistant_row_count;
    cache.db_state.last_time_updated_ms = state.last_time_updated_ms;
    cache.db_state.last_rowid = state.last_rowid;
    cache.db_state.last_full_reconcile_at_ms = state.last_full_reconcile_at_ms;
    cache.db_state.schema_mode = OpenCodeSchemaMode::from_str(&state.schema_mode);
}

fn refresh_db_messages(
    state: &mut OpenCodeDbCacheState,
) -> HashMap<String, OpenCodeMessageSnapshot> {
    let Some(db_path) = find_opencode_db() else {
        state.messages.clear();
        state.storage_signature_hash = 0;
        state.file_size = 0;
        state.schema_fingerprint = 0;
        state.assistant_row_count = 0;
        state.last_time_updated_ms = 0;
        state.last_rowid = 0;
        state.last_full_reconcile_at_ms = 0;
        state.schema_mode = OpenCodeSchemaMode::Incompatible;
        return state.messages.clone();
    };

    let storage_signature = compute_opencode_db_storage_signature(&db_path);
    let storage_signature_hash = storage_signature.hash();
    let storage_signature_changed = storage_signature_hash != state.storage_signature_hash;

    let file_size = storage_signature.db.size;
    let conn = match open_opencode_db_read_only(&db_path) {
        Ok(c) => c,
        Err(_) => return state.messages.clone(),
    };

    let previous_schema_mode = state.schema_mode;
    let (schema_mode, _) = detect_schema_mode(&conn);
    if schema_mode == OpenCodeSchemaMode::Incompatible {
        state.messages.clear();
        state.storage_signature_hash = storage_signature_hash;
        state.file_size = file_size;
        state.schema_fingerprint = compute_schema_fingerprint(&conn);
        state.assistant_row_count = 0;
        state.last_time_updated_ms = 0;
        state.last_rowid = 0;
        state.schema_mode = schema_mode;
        return state.messages.clone();
    }

    let checkpoint = read_db_checkpoint(&conn);
    let checkpoint_unchanged = checkpoint.schema_fingerprint == state.schema_fingerprint
        && checkpoint.assistant_row_count == state.assistant_row_count
        && checkpoint.max_rowid == state.last_rowid
        && checkpoint.max_time_updated_ms == state.last_time_updated_ms;
    if !storage_signature_changed
        && checkpoint_unchanged
        && previous_schema_mode == schema_mode
        && !state.messages.is_empty()
    {
        return state.messages.clone();
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    let checkpoint_rewound = checkpoint.assistant_row_count < state.assistant_row_count
        || checkpoint.max_rowid < state.last_rowid
        || checkpoint.max_time_updated_ms < state.last_time_updated_ms;
    let checkpoint_advanced = checkpoint.assistant_row_count > state.assistant_row_count
        || checkpoint.max_rowid > state.last_rowid
        || checkpoint.max_time_updated_ms > state.last_time_updated_ms;
    let should_full_reconcile = state.messages.is_empty()
        || checkpoint_rewound
        || state.last_full_reconcile_at_ms == 0
        || previous_schema_mode != schema_mode
        || checkpoint.schema_fingerprint != state.schema_fingerprint
        || (storage_signature_changed && !checkpoint_advanced)
        || now_ms.saturating_sub(state.last_full_reconcile_at_ms)
            >= OPENCODE_DB_FULL_RECONCILE_INTERVAL_SECS * 1000;

    let rows = if should_full_reconcile {
        query_db_message_rows(&conn, None, None)
    } else {
        query_db_message_rows(
            &conn,
            Some(state.last_time_updated_ms),
            Some(state.last_rowid),
        )
    };

    if should_full_reconcile {
        state.messages.clear();
    }

    let mut max_time_updated_ms = if should_full_reconcile {
        0
    } else {
        state.last_time_updated_ms
    };
    let mut max_rowid = if should_full_reconcile {
        0
    } else {
        state.last_rowid
    };
    for row in rows {
        max_time_updated_ms = max_time_updated_ms.max(row.time_updated_ms);
        max_rowid = max_rowid.max(row.rowid);
        if let Some(snapshot) = parse_message_snapshot(
            &row.session_id,
            &row.id,
            &row.data,
            row.time_updated_ms,
            "opencode_db",
            &format!("opencode.db::{}::{}", row.session_id, row.id),
        ) {
            state
                .messages
                .insert(snapshot.message_identity_key(), snapshot);
        }
    }

    state.storage_signature_hash = storage_signature_hash;
    state.file_size = file_size;
    state.schema_fingerprint = checkpoint.schema_fingerprint;
    state.assistant_row_count = checkpoint.assistant_row_count;
    state.last_time_updated_ms = checkpoint.max_time_updated_ms.max(max_time_updated_ms);
    state.last_rowid = checkpoint.max_rowid.max(max_rowid);
    state.schema_mode = schema_mode;
    if should_full_reconcile {
        state.last_full_reconcile_at_ms = now_ms;
    }

    state.messages.clone()
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
    fn hash(&self) -> u64 {
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

fn detect_message_id_conflicts(conn: &Connection, limit: usize) -> OpenCodeMessageIdConflictStatus {
    let sql = format!(
        "SELECT id, COUNT(DISTINCT session_id) AS session_count
         FROM message
         WHERE id IS NOT NULL AND TRIM(id) != ''
         GROUP BY id
         HAVING COUNT(DISTINCT session_id) > 1
         LIMIT {}",
        limit.max(1)
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(stmt) => stmt,
        Err(_) => {
            return OpenCodeMessageIdConflictStatus {
                has_conflict: false,
                conflict_count: 0,
                sample_ids: Vec::new(),
            }
        }
    };
    let rows = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1).unwrap_or(0).max(0) as u64,
        ))
    }) {
        Ok(rows) => rows,
        Err(_) => {
            return OpenCodeMessageIdConflictStatus {
                has_conflict: false,
                conflict_count: 0,
                sample_ids: Vec::new(),
            }
        }
    };
    let mut sample_ids = Vec::new();
    let mut count = 0_u64;
    for row in rows.flatten() {
        count += 1;
        sample_ids.push(row.0);
    }
    OpenCodeMessageIdConflictStatus {
        has_conflict: count > 0,
        conflict_count: count,
        sample_ids,
    }
}

fn refresh_legacy_file_messages(
    state: &mut OpenCodeFileCacheState,
) -> HashMap<String, OpenCodeMessageSnapshot> {
    let root = opencode_message_storage_root();
    if !root.exists() {
        state.files.clear();
        state.messages.clear();
        return state.messages.clone();
    }

    let files = collect_legacy_message_files(&root);
    let current_paths: HashSet<String> = files
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();

    let stale_paths: Vec<String> = state
        .files
        .keys()
        .filter(|path| !current_paths.contains(*path))
        .cloned()
        .collect();
    for stale_path in stale_paths {
        if let Some(entry) = state.files.remove(&stale_path) {
            if let Some(message_key) = entry.message_identity_key {
                state.messages.remove(&message_key);
            }
        }
    }

    for path in files {
        let path_string = path.to_string_lossy().to_string();
        let metadata = match fs::metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        let size = metadata.len();
        let mtime_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);

        let unchanged = state
            .files
            .get(&path_string)
            .map(|entry| entry.size == size && entry.mtime_ms == mtime_ms)
            .unwrap_or(false);
        if unchanged {
            continue;
        }

        let previous_identity = state
            .files
            .get(&path_string)
            .and_then(|entry| entry.message_identity_key.clone());
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        let json = match serde_json::from_str::<Value>(&content) {
            Ok(json) => json,
            Err(_) => {
                state.files.insert(
                    path_string,
                    OpenCodeFileEntryState {
                        size,
                        mtime_ms,
                        message_identity_key: previous_identity,
                    },
                );
                continue;
            }
        };

        if let Some(ref identity) = previous_identity {
            state.messages.remove(identity);
        }

        let raw_session_id = json
            .get("sessionID")
            .or_else(|| json.get("sessionId"))
            .or_else(|| json.get("session_id"))
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let raw_message_id = json
            .get("id")
            .or_else(|| json.get("messageID"))
            .or_else(|| json.get("messageId"))
            .and_then(|value| value.as_str())
            .unwrap_or_default();

        let snapshot = parse_message_snapshot(
            raw_session_id,
            raw_message_id,
            &json,
            0,
            "opencode_file",
            &path.to_string_lossy(),
        );

        state.files.insert(
            path_string,
            OpenCodeFileEntryState {
                size,
                mtime_ms,
                message_identity_key: snapshot.as_ref().map(|entry| entry.message_identity_key()),
            },
        );

        if let Some(snapshot) = snapshot {
            state
                .messages
                .insert(snapshot.message_identity_key(), snapshot);
        }
    }

    state.messages.clone()
}

#[derive(Debug, Clone)]
struct DbMessageRow {
    rowid: i64,
    id: String,
    session_id: String,
    time_updated_ms: i64,
    data: Value,
}

fn query_db_message_rows(
    conn: &Connection,
    last_time_updated_ms: Option<i64>,
    last_rowid: Option<i64>,
) -> Vec<DbMessageRow> {
    let (sql, params_vec): (&str, Vec<i64>) = if let (Some(last_time), Some(last_rowid)) =
        (last_time_updated_ms, last_rowid)
    {
        (
            "SELECT rowid, id, session_id, COALESCE(time_updated, 0), data
             FROM message
             WHERE json_extract(data, '$.role') = 'assistant'
               AND (COALESCE(time_updated, 0) > ?1 OR (COALESCE(time_updated, 0) = ?1 AND rowid > ?2))
             ORDER BY COALESCE(time_updated, 0) ASC, rowid ASC",
            vec![last_time, last_rowid],
        )
    } else {
        (
            "SELECT rowid, id, session_id, COALESCE(time_updated, 0), data
             FROM message
             WHERE json_extract(data, '$.role') = 'assistant'
             ORDER BY COALESCE(time_updated, 0) ASC, rowid ASC",
            Vec::new(),
        )
    };

    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };
    let mut rows = if params_vec.is_empty() {
        match stmt.query([]) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        }
    } else {
        match stmt.query(params![params_vec[0], params_vec[1]]) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        }
    };

    let mut out = Vec::new();
    while let Ok(Some(row)) = rows.next() {
        let data_str: String = match row.get(4) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let data = match serde_json::from_str::<Value>(&data_str) {
            Ok(v) => v,
            Err(_) => continue,
        };
        out.push(DbMessageRow {
            rowid: row.get(0).unwrap_or(0),
            id: row.get::<_, String>(1).unwrap_or_default(),
            session_id: row.get::<_, String>(2).unwrap_or_default(),
            time_updated_ms: row.get::<_, i64>(3).unwrap_or(0),
            data,
        });
    }
    out
}

fn compute_opencode_db_storage_signature(db_path: &Path) -> OpenCodeStorageSignature {
    OpenCodeStorageSignature {
        db_path: db_path.to_string_lossy().to_string(),
        db: read_path_signature(db_path),
        wal: read_path_signature(&db_path.with_extension("db-wal")),
        shm: read_path_signature(&db_path.with_extension("db-shm")),
    }
}

fn read_path_signature(path: &Path) -> OpenCodePathSignature {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return OpenCodePathSignature::default(),
    };
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    OpenCodePathSignature {
        exists: true,
        size: meta.len(),
        mtime_ns,
    }
}

fn open_opencode_db_read_only(db_path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let _ = conn.execute_batch("PRAGMA busy_timeout=3000;");
    Ok(conn)
}

fn read_db_checkpoint(conn: &Connection) -> OpenCodeDbCheckpoint {
    let mut checkpoint = OpenCodeDbCheckpoint {
        schema_fingerprint: compute_schema_fingerprint(conn),
        ..Default::default()
    };
    let values: rusqlite::Result<(i64, i64, i64)> = conn.query_row(
        "SELECT
            COUNT(*),
            COALESCE(MAX(rowid), 0),
            COALESCE(MAX(COALESCE(time_updated, 0)), 0)
         FROM message
         WHERE json_extract(data, '$.role') = 'assistant'",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0).unwrap_or(0),
                row.get::<_, i64>(1).unwrap_or(0),
                row.get::<_, i64>(2).unwrap_or(0),
            ))
        },
    );
    if let Ok((count, max_rowid, max_time)) = values {
        checkpoint.assistant_row_count = count.max(0) as u64;
        checkpoint.max_rowid = max_rowid.max(0);
        checkpoint.max_time_updated_ms = max_time.max(0);
    }
    checkpoint
}

fn compute_schema_fingerprint(conn: &Connection) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut stmt = match conn.prepare(
        "SELECT name, COALESCE(sql, '')
         FROM sqlite_schema
         WHERE type = 'table' AND name IN ('message', 'session')
         ORDER BY name ASC",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return 0,
    };
    let rows = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0).unwrap_or_default(),
            row.get::<_, String>(1).unwrap_or_default(),
        ))
    }) {
        Ok(rows) => rows,
        Err(_) => return 0,
    };
    for row in rows.flatten() {
        row.0.hash(&mut hasher);
        row.1.hash(&mut hasher);
    }
    hasher.finish()
}

fn detect_schema_mode(conn: &Connection) -> (OpenCodeSchemaMode, Option<String>) {
    if let Some(col) = missing_required_columns(conn, "message", REQUIRED_MESSAGE_COLUMNS)
        .into_iter()
        .next()
    {
        return (
            OpenCodeSchemaMode::Incompatible,
            Some(format!(
                "message 表缺少字段 `{}`，可能是较旧或较新版本的 OpenCode",
                col
            )),
        );
    }

    if !verify_json_structure(conn) {
        return (
            OpenCodeSchemaMode::Incompatible,
            Some(
                "message.data JSON 结构与预期不匹配（tokens.input / tokens.output 字段不存在），可能是 OpenCode 版本升级后更改了内部格式"
                    .to_string(),
            ),
        );
    }

    if let Some(col) = missing_required_columns(conn, "session", REQUIRED_SESSION_COLUMNS)
        .into_iter()
        .next()
    {
        return (
            OpenCodeSchemaMode::MessageOnly,
            Some(format!("session 表缺少字段 `{}`，将退化为仅消息模式", col)),
        );
    }

    (OpenCodeSchemaMode::Full, None)
}

fn get_table_columns(conn: &Connection, table: &str) -> Vec<String> {
    let sql = format!("PRAGMA table_info({})", table);
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([], |row| row.get::<_, String>(1))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

fn missing_required_columns(conn: &Connection, table: &str, required: &[&str]) -> Vec<String> {
    let columns = get_table_columns(conn, table);
    required
        .iter()
        .filter(|col| !columns.iter().any(|existing| existing == **col))
        .map(|col| (*col).to_string())
        .collect()
}

fn verify_json_structure(conn: &Connection) -> bool {
    let result: rusqlite::Result<Option<i64>> = conn.query_row(
        "SELECT COUNT(*) FROM message
         WHERE json_extract(data, '$.role') = 'assistant'
           AND (json_extract(data, '$.tokens.input') IS NOT NULL
             OR json_extract(data, '$.tokens.output') IS NOT NULL
             OR json_extract(data, '$.tokens.reasoning') IS NOT NULL)
         LIMIT 1",
        [],
        |row| row.get(0),
    );
    result.is_ok()
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

fn build_session_data_from_messages(
    combined: HashMap<String, OpenCodeMessageSnapshot>,
) -> Vec<OpenCodeSessionData> {
    let mut message_by_session: HashMap<String, Vec<OpenCodeMessageSnapshot>> = HashMap::new();
    let mut raw_message_id_sessions: HashMap<String, HashSet<String>> = HashMap::new();
    let mut raw_sessions: HashSet<String> = HashSet::new();
    let mut source_kinds: HashSet<&'static str> = HashSet::new();

    for snapshot in combined.into_values() {
        raw_message_id_sessions
            .entry(snapshot.raw_message_id.clone())
            .or_default()
            .insert(snapshot.canonical_session_id.clone());
        raw_sessions.insert(snapshot.raw_session_id.clone());
        source_kinds.insert(snapshot.source_kind);
        message_by_session
            .entry(snapshot.canonical_session_id.clone())
            .or_default()
            .push(snapshot);
    }

    let mut session_rows = HashMap::new();
    let schema_mode = if let Some(db_path) = find_opencode_db() {
        if let Ok(conn) = Connection::open_with_flags(
            &db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            let _ = conn.execute_batch("PRAGMA busy_timeout=2000;");
            let (mode, _) = detect_schema_mode(&conn);
            if mode == OpenCodeSchemaMode::Full {
                session_rows = query_session_rows(&conn);
            }
            mode
        } else {
            OpenCodeSchemaMode::Incompatible
        }
    } else {
        OpenCodeSchemaMode::MessageOnly
    };

    let mut result = Vec::new();
    for (session_id, mut snapshots) in message_by_session {
        snapshots.sort_by_key(|snapshot| snapshot.timestamp_sec);
        let raw_session_id = snapshots
            .first()
            .map(|snapshot| snapshot.raw_session_id.clone())
            .unwrap_or_default();
        let session_row = session_rows.get(&raw_session_id);
        let data = build_session_data(
            &session_id,
            session_row,
            &snapshots,
            &raw_message_id_sessions,
            schema_mode,
            &source_kinds,
        );
        result.push(data);
    }
    result.sort_by_key(|entry| std::cmp::Reverse(entry.meta.last_modified));
    result
}

fn build_session_data(
    canonical_session_id: &str,
    session_row: Option<&SessionRow>,
    snapshots: &[OpenCodeMessageSnapshot],
    raw_message_id_sessions: &HashMap<String, HashSet<String>>,
    schema_mode: OpenCodeSchemaMode,
    source_kinds: &HashSet<&'static str>,
) -> OpenCodeSessionData {
    let fallback_cwd = snapshots
        .iter()
        .rev()
        .find_map(|snapshot| snapshot.cwd.clone());
    let cwd = session_row
        .map(|row| row.directory.clone())
        .filter(|value| !value.is_empty())
        .or(fallback_cwd.clone());
    let project_name = cwd.as_deref().and_then(extract_project_name);

    let session_model = session_row.and_then(|row| {
        row.model_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Value>(json).ok())
            .map(|value| {
                let model_id = value
                    .get("id")
                    .or_else(|| value.get("modelID"))
                    .or_else(|| value.get("modelId"))
                    .and_then(|v| v.as_str());
                let provider_id = value
                    .get("providerID")
                    .or_else(|| value.get("providerId"))
                    .and_then(|v| v.as_str());
                normalize_model_string(provider_id, model_id)
            })
    });

    let mut models = BTreeSet::new();
    let mut total_input = 0_u64;
    let mut total_output = 0_u64;
    let mut total_cache_create = 0_u64;
    let mut total_cache_read = 0_u64;
    let mut message_ids = Vec::new();
    let mut requests = Vec::new();

    for snapshot in snapshots {
        if !snapshot.model.is_empty() && snapshot.model != "unknown" {
            models.insert(snapshot.model.clone());
        }
        total_input += snapshot.input_tokens;
        total_output += snapshot.output_tokens + snapshot.reasoning_tokens;
        total_cache_create += snapshot.cache_create_tokens;
        total_cache_read += snapshot.cache_read_tokens;
        message_ids.push(snapshot.raw_message_id.clone());

        let duplicate_raw_message_id = raw_message_id_sessions
            .get(&snapshot.raw_message_id)
            .map(|sessions| sessions.len() > 1)
            .unwrap_or(false);
        let request_key = if duplicate_raw_message_id {
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
            model: snapshot.model.clone(),
            is_subagent: false,
            request_key,
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
    let source_locator = if source.starts_with("opencode_file") {
        format!("opencode-file://{}", canonical_session_id)
    } else {
        format!("opencode-db://{}", canonical_session_id)
    };

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

fn collect_legacy_message_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut queue = VecDeque::from([root.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                queue.push_back(path);
                continue;
            }
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("msg_") && name.ends_with(".json"))
                .unwrap_or(false)
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn parse_message_snapshot(
    raw_session_id: &str,
    raw_message_id: &str,
    data: &Value,
    fallback_time_updated_ms: i64,
    source_kind: &'static str,
    _source_locator: &str,
) -> Option<OpenCodeMessageSnapshot> {
    let role = data
        .get("role")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if role != "assistant" {
        return None;
    }

    let canonical_session_id = canonical_opencode_session_id(raw_session_id);
    let message_id = if raw_message_id.is_empty() {
        data.get("id")
            .or_else(|| data.get("messageID"))
            .or_else(|| data.get("messageId"))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        raw_message_id.trim().to_string()
    };
    if canonical_session_id.trim().is_empty() || message_id.trim().is_empty() {
        return None;
    }

    let tokens = data.get("tokens")?.as_object()?;
    let input_tokens = tokens.get("input").map(to_non_negative_u64).unwrap_or(0);
    let output_tokens = tokens.get("output").map(to_non_negative_u64).unwrap_or(0);
    let reasoning_tokens = tokens
        .get("reasoning")
        .map(to_non_negative_u64)
        .unwrap_or(0);
    let cache_read_tokens = tokens
        .get("cache")
        .and_then(|cache| cache.get("read"))
        .map(to_non_negative_u64)
        .unwrap_or(0);
    let cache_create_tokens = tokens
        .get("cache")
        .and_then(|cache| cache.get("write"))
        .map(to_non_negative_u64)
        .unwrap_or(0);
    let total_tokens =
        input_tokens + output_tokens + reasoning_tokens + cache_read_tokens + cache_create_tokens;
    if total_tokens == 0 {
        return None;
    }

    let timestamp_ms = extract_opencode_timestamp_ms(data).unwrap_or(fallback_time_updated_ms);
    if timestamp_ms <= 0 {
        return None;
    }
    let timestamp_sec = timestamp_ms / 1000;

    let provider_id = data
        .get("providerID")
        .or_else(|| data.get("providerId"))
        .or_else(|| data.get("provider"))
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let model_id = data
        .get("modelID")
        .or_else(|| data.get("modelId"))
        .or_else(|| data.get("model"))
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let model = normalize_model_string(provider_id.as_deref(), model_id.as_deref());
    let cwd = data
        .pointer("/path/cwd")
        .or_else(|| data.pointer("/path/cwdPath"))
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let title = data
        .get("title")
        .or_else(|| data.get("sessionTitle"))
        .and_then(|value| value.as_str())
        .map(str::to_string);

    Some(OpenCodeMessageSnapshot {
        canonical_session_id,
        raw_session_id: raw_session_id.to_string(),
        raw_message_id: message_id,
        timestamp_sec,
        model,
        cwd,
        title,
        input_tokens,
        output_tokens,
        reasoning_tokens,
        cache_create_tokens,
        cache_read_tokens,
        total_tokens,
        source_kind,
    })
}

impl OpenCodeMessageSnapshot {
    fn message_identity_key(&self) -> String {
        format!("{}|{}", self.canonical_session_id, self.raw_message_id)
    }
}

fn extract_opencode_timestamp_ms(data: &Value) -> Option<i64> {
    let completed = data
        .pointer("/time/completed")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    if completed > 0 {
        return Some(completed);
    }
    let created = data
        .pointer("/time/created")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    (created > 0).then_some(created)
}

fn normalize_model_string(provider_id: Option<&str>, model_id: Option<&str>) -> String {
    match (provider_id, model_id) {
        (_, Some(model)) if !model.is_empty() => model.to_string(),
        (Some(provider), None) if !provider.is_empty() => provider.to_string(),
        _ => "unknown".to_string(),
    }
}

fn extract_project_name(cwd: &str) -> Option<String> {
    Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn canonical_opencode_session_id(raw_session_id: &str) -> String {
    if raw_session_id.starts_with("opencode::") {
        raw_session_id.to_string()
    } else {
        format!("opencode::{}", raw_session_id)
    }
}

fn truncate_string(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        chars[..max_chars].iter().collect::<String>() + "…"
    }
}

fn to_non_negative_u64(value: &Value) -> u64 {
    value
        .as_i64()
        .map(|v| v.max(0) as u64)
        .or_else(|| value.as_u64())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::fs;

    #[test]
    fn canonical_opencode_session_id_adds_tool_namespace_once() {
        assert_eq!(
            canonical_opencode_session_id("sess_abc"),
            "opencode::sess_abc"
        );
        assert_eq!(
            canonical_opencode_session_id("opencode::sess_abc"),
            "opencode::sess_abc"
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

        let (mode, reason) = detect_schema_mode(&conn);

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

        let snapshot =
            parse_message_snapshot("sess_1", "msg_1", &data, 0, "opencode_db", "locator")
                .expect("snapshot");

        assert_eq!(snapshot.timestamp_sec, 2);
        assert_eq!(snapshot.model, "glm-4.7-free");
        assert_eq!(snapshot.cwd.as_deref(), Some("/Users/me/demo"));
        assert_eq!(snapshot.total_tokens, 20);
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
            .find(|session| session.meta.session_id == "opencode::sess_test")
            .expect("find target session");
        assert_eq!(first_session.requests.len(), 1);

        fs::write(&message_path, "{").unwrap();
        let corrupted = scan_opencode_sessions();
        let corrupted_session = corrupted
            .iter()
            .find(|session| session.meta.session_id == "opencode::sess_test")
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
            .find(|session| session.meta.session_id == "opencode::sess_test")
            .expect("find recovered target session");
        assert_eq!(recovered_session.requests[0].input_tokens, 8);
        assert_eq!(recovered_session.requests[0].output_tokens, 3);

        match old_xdg_data_home {
            Some(value) => std::env::set_var("XDG_DATA_HOME", value),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
    }
}
