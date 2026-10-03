//! Cursor IDE metadata only. Billing comes from account events, never bubble/text estimates.
use super::meta::{SessionFile, SessionMeta};
use super::source::{
    ParsedSessionData, SessionSource, SourceKind, SourceSnapshot, SourceUpdateMode,
};
use crate::cursor::events::digest;
use rusqlite::{types::ValueRef, Connection, OpenFlags, OptionalExtension};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) struct CursorSource;
static PARSED_SESSIONS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, ParsedSessionData>>> =
    std::sync::OnceLock::new();

pub(crate) fn configured_database_path() -> Option<PathBuf> {
    let settings = crate::settings::load_settings_blocking().ok();
    if let Some(path) = settings
        .and_then(|settings| settings.cursor.database_path)
        .filter(|path| !path.trim().is_empty())
    {
        return Some(PathBuf::from(path));
    }
    default_database_path()
}

pub(crate) fn default_database_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    #[cfg(target_os = "macos")]
    let root = home.join("Library/Application Support/Cursor");
    #[cfg(target_os = "windows")]
    let root = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("AppData/Roaming"))
        .join("Cursor");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
        .join("Cursor");
    Some(root.join("User/globalStorage/state.vscdb"))
}

pub(crate) fn open_readonly(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| "cursor_database_unavailable")?;
    conn.busy_timeout(Duration::from_secs(3))
        .map_err(|_| "cursor_database_unavailable")?;
    Ok(conn)
}

pub(crate) fn read_metadata(path: &Path) -> Result<HashMap<String, SessionMeta>, String> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let conn = open_readonly(path)?;
    let modern: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='composerHeaders')", [], |row| row.get(0))
        .map_err(|_| "cursor_schema_unsupported")?;
    let mut headers = Vec::new();
    let mut legacy_schema_found = false;
    if modern {
        let mut statement = conn
            .prepare("SELECT composerId, value FROM composerHeaders LIMIT 100001")
            .map_err(|_| "cursor_schema_unsupported")?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, json_column(row, 1)?))
            })
            .map_err(|_| "cursor_database_unavailable")?;
        for row in rows {
            let (id, json) = row.map_err(|_| "cursor_database_unavailable")?;
            if json.len() > 1024 * 1024 {
                return Err("cursor_metadata_too_large".into());
            }
            let value: Value =
                serde_json::from_str(&json).map_err(|_| "cursor_schema_unsupported")?;
            headers.push((id, value));
        }
    }
    let legacy_table: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='ItemTable')",
            [],
            |row| row.get(0),
        )
        .map_err(|_| "cursor_database_unavailable")?;
    if legacy_table {
        for key in ["composer.composerHeaders", "composer.composerData"] {
            let value: Option<String> = conn
                .query_row("SELECT value FROM ItemTable WHERE key=?1", [key], |row| {
                    json_column(row, 0)
                })
                .optional()
                .map_err(|_| "cursor_database_unavailable")?;
            if let Some(json) = value {
                legacy_schema_found = true;
                if json.len() > 32 * 1024 * 1024 {
                    return Err("cursor_metadata_too_large".into());
                }
                let value: Value =
                    serde_json::from_str(&json).map_err(|_| "cursor_schema_unsupported")?;
                let list = value
                    .as_array()
                    .or_else(|| value["allComposers"].as_array())
                    .ok_or("cursor_schema_unsupported")?;
                for value in list {
                    if let Some(id) = value["composerId"].as_str() {
                        headers.push((id.to_string(), value.clone()));
                    }
                }
            }
        }
    }
    if !modern && !legacy_schema_found {
        return Err("cursor_schema_unsupported".into());
    }
    if headers.len() > 100_000 {
        return Err("cursor_metadata_too_large".into());
    }
    let namespace = digest(path.to_string_lossy().as_bytes());
    let workspaces = workspace_projects(path)?;
    let mut result = HashMap::new();
    for (id, header) in headers {
        if id.is_empty() || id.len() > 512 || id.chars().any(char::is_control) {
            continue;
        }
        let cwd = project_path(&header).or_else(|| workspaces.get(&id).cloned().flatten());
        let project_name = cwd
            .as_ref()
            .and_then(|path| Path::new(path).file_name()?.to_str().map(String::from));
        let start_time = header["createdAt"].as_i64().unwrap_or(0) / 1000;
        let end_time = header["lastUpdatedAt"]
            .as_i64()
            .map(|time| time / 1000)
            .unwrap_or(start_time);
        let session_id = format!("cursor::local-{namespace}::{id}");
        result.entry(id).or_insert_with(|| SessionMeta {
            session_id: session_id.clone(),
            tool: "cursor".into(),
            cwd,
            project_name,
            file_path: format!("{}#{}", path.display(), session_id),
            start_time,
            end_time,
            last_modified: end_time,
            source: "cursor_metadata".into(),
            scope: Some("local_metadata".into()),
            ..Default::default()
        });
    }
    Ok(result)
}

fn project_path(header: &Value) -> Option<String> {
    let uri = &header["workspaceIdentifier"]["uri"];
    for candidate in [
        uri["fsPath"].as_str(),
        uri["path"].as_str(),
        header["workspacePath"].as_str(),
    ] {
        if let Some(path) = candidate.filter(|path| Path::new(path).is_absolute()) {
            return Some(path.into());
        }
    }
    if let Some(uri) = uri.as_str().filter(|uri| uri.starts_with("file://")) {
        return reqwest::Url::parse(uri)
            .ok()?
            .to_file_path()
            .ok()
            .map(|path| path.to_string_lossy().into_owned());
    }
    // Multiple repositories do not prove one project; keep the association unknown.
    if let Some(repositories) = header["trackedGitRepos"]
        .as_array()
        .filter(|repos| repos.len() == 1)
    {
        return repositories[0]["path"]
            .as_str()
            .filter(|path| Path::new(path).is_absolute())
            .map(String::from);
    }
    None
}

fn json_column(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<String> {
    let bytes = match row.get_ref(index)? {
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    std::str::from_utf8(bytes)
        .map(String::from)
        .map_err(|_| rusqlite::Error::InvalidQuery)
}

fn workspace_projects(database: &Path) -> Result<HashMap<String, Option<String>>, String> {
    use std::io::Read;
    let Some(user_dir) = database.parent().and_then(Path::parent) else {
        return Ok(HashMap::new());
    };
    let root = user_dir.join("workspaceStorage");
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(_) => return Err("cursor_database_unavailable".into()),
    };
    let mut result = HashMap::<String, Option<String>>::new();
    let started = std::time::Instant::now();
    for (index, entry) in entries.enumerate() {
        if index >= 2048 || started.elapsed() > Duration::from_secs(5) {
            return Err("cursor_metadata_too_large".into());
        }
        let entry = entry.map_err(|_| "cursor_database_unavailable")?;
        if !entry
            .file_type()
            .map_err(|_| "cursor_database_unavailable")?
            .is_dir()
        {
            continue;
        }
        let mut bytes = Vec::new();
        let Ok(file) = std::fs::File::open(entry.path().join("workspace.json")) else {
            continue;
        };
        if file.take(65537).read_to_end(&mut bytes).is_err() || bytes.len() > 65536 {
            continue;
        }
        let Ok(workspace) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let Some(folder) = workspace["folder"].as_str() else {
            continue;
        };
        let header = serde_json::json!({"workspaceIdentifier":{"uri":folder}});
        let Some(project) = project_path(&header) else {
            continue;
        };
        let db_path = entry.path().join("state.vscdb");
        if !db_path.exists() {
            continue;
        }
        let conn = open_readonly(&db_path)?;
        for key in ["composer.composerHeaders", "composer.composerData"] {
            let json: Option<String> = conn
                .query_row("SELECT value FROM ItemTable WHERE key=?1", [key], |row| {
                    json_column(row, 0)
                })
                .optional()
                .map_err(|_| "cursor_schema_unsupported")?;
            let Some(json) = json else {
                continue;
            };
            let value: Value =
                serde_json::from_str(&json).map_err(|_| "cursor_schema_unsupported")?;
            let Some(composers) = value["allComposers"]
                .as_array()
                .or_else(|| value.as_array())
            else {
                return Err("cursor_schema_unsupported".into());
            };
            if composers.len() > 100_000 {
                return Err("cursor_metadata_too_large".into());
            }
            for composer in composers {
                let Some(id) = composer["composerId"]
                    .as_str()
                    .filter(|id| !id.is_empty() && id.len() <= 512)
                else {
                    continue;
                };
                result
                    .entry(id.into())
                    .and_modify(|previous| {
                        if previous.as_deref() != Some(&project) {
                            *previous = None;
                        }
                    })
                    .or_insert_with(|| Some(project.clone()));
            }
        }
    }
    Ok(result)
}

impl SessionSource for CursorSource {
    fn tool_id(&self) -> &'static str {
        "cursor"
    }
    fn source_kind(&self) -> SourceKind {
        SourceKind::ReadonlyDatabase
    }
    fn scan(&self) -> SourceSnapshot {
        self.try_scan().unwrap_or(SourceSnapshot {
            source_id: "cursor",
            update_mode: SourceUpdateMode::ReplaceAll,
            sessions: vec![],
            scan_fingerprint: 0,
        })
    }
    fn try_scan(&self) -> Result<SourceSnapshot, String> {
        let mut sessions = crate::local_usage::get_local_usage_db()?.get_cursor_sessions()?;
        sessions.sort_by(|left, right| left.0.session_id.cmp(&right.0.session_id));
        let mut parsed = HashMap::new();
        let mut fingerprint = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash, Hasher};
        let mut sessions: Vec<_> = sessions
            .into_iter()
            .map(|(meta, requests)| {
                let contents = serde_json::to_vec(&(&meta, &requests)).unwrap_or_default();
                let mut session_hash = std::collections::hash_map::DefaultHasher::new();
                contents.hash(&mut session_hash);
                let hash = session_hash.finish();
                hash.hash(&mut fingerprint);
                parsed.insert(
                    meta.session_id.clone(),
                    ParsedSessionData {
                        meta: meta.clone(),
                        requests,
                    },
                );
                SessionFile {
                    session_id: meta.session_id,
                    tool: "cursor".into(),
                    project_path: meta.cwd.unwrap_or_default(),
                    file_path: meta.file_path,
                    transcript_paths: vec![],
                    file_size: 0,
                    last_modified: meta.end_time,
                    fingerprint: hash,
                }
            })
            .collect();
        sessions.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        *PARSED_SESSIONS
            .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
            .lock()
            .map_err(|_| "cursor_database_error")? = parsed;
        Ok(SourceSnapshot {
            source_id: "cursor",
            update_mode: SourceUpdateMode::ReplaceAll,
            sessions,
            scan_fingerprint: fingerprint.finish(),
        })
    }
    fn parse(&self, session: &SessionFile) -> Result<ParsedSessionData, String> {
        PARSED_SESSIONS
            .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
            .lock()
            .map_err(|_| "cursor_database_error")?
            .get(&session.session_id)
            .cloned()
            .ok_or_else(|| "cursor_session_unavailable".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_workspace_join_is_exact_ambiguous_safe_and_reads_live_wal() {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join("User/globalStorage");
        std::fs::create_dir_all(&global).unwrap();
        let path = global.join("state.vscdb");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; CREATE TABLE composerHeaders(composerId TEXT, value BLOB);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO composerHeaders VALUES ('matched', ?1)",
            [br#"{"createdAt":1750000000000}"#.as_slice()],
        )
        .unwrap();
        for (name, folder) in [
            ("one", "file:///tmp/project-one"),
            ("two", "file:///tmp/project-two"),
        ] {
            let directory = temp.path().join("User/workspaceStorage").join(name);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join("workspace.json"),
                serde_json::to_vec(&serde_json::json!({"folder":folder})).unwrap(),
            )
            .unwrap();
            let workspace = Connection::open(directory.join("state.vscdb")).unwrap();
            workspace
                .execute_batch("CREATE TABLE ItemTable(key TEXT, value BLOB)")
                .unwrap();
            workspace.execute("INSERT INTO ItemTable VALUES ('composer.composerHeaders', ?1)", [br#"{"allComposers":[{"composerId":"ambiguous"},{"composerId":"matched"}]}"#.as_slice()]).unwrap();
            if name == "one" {
                assert_eq!(
                    read_metadata(&path).unwrap()["matched"].cwd.as_deref(),
                    Some("/tmp/project-one")
                );
            }
        }
        assert_eq!(read_metadata(&path).unwrap()["matched"].cwd, None);
        conn.execute(
            "UPDATE composerHeaders SET value=?1",
            [br#"{"workspacePath":"/tmp/explicit","lastUpdatedAt":1750000000123}"#.as_slice()],
        )
        .unwrap();
        let meta = read_metadata(&path).unwrap();
        assert_eq!(meta["matched"].cwd.as_deref(), Some("/tmp/explicit"));
        assert_eq!(meta["matched"].end_time, 1750000000);
    }
    #[test]
    fn cursor_metadata_is_content_free_readonly_and_schema_checked() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.vscdb");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE composerHeaders(composerId TEXT, value TEXT); CREATE TABLE ItemTable(key TEXT, value TEXT);").unwrap();
        conn.execute("INSERT INTO composerHeaders VALUES (?1, ?2)", ["c1", r#"{"createdAt":1750000000000,"name":"SECRET prompt","workspaceIdentifier":{"uri":"file:///tmp/hello%20world"}}"#]).unwrap();
        let before = std::fs::read(&path).unwrap();
        let meta = read_metadata(&path).unwrap();
        assert_eq!(meta["c1"].cwd.as_deref(), Some("/tmp/hello world"));
        assert_eq!(meta["c1"].topic, None);
        assert_eq!(meta["c1"].session_name, None);
        assert_eq!(meta["c1"].message_count, 0);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        conn.execute_batch("DROP TABLE composerHeaders; INSERT INTO ItemTable VALUES ('composer.composerData', '{\"allComposers\":[{\"composerId\":\"old\",\"createdAt\":1750000000000}]}');").unwrap();
        assert!(read_metadata(&path).unwrap().contains_key("old"));
        conn.execute_batch("DROP TABLE ItemTable").unwrap();
        assert!(read_metadata(&path).is_err());
        conn.execute_batch("CREATE TABLE ItemTable(key TEXT,value TEXT); INSERT INTO ItemTable VALUES ('cursorAuth/accessToken','fixture');").unwrap();
        // An unknown format must not look like an authoritative empty conversation list.
        assert_eq!(
            read_metadata(&path).unwrap_err(),
            "cursor_schema_unsupported"
        );
    }
}
