//! Authoritative application configuration storage.
//!
//! Settings are split into top-level JSON documents so unrelated namespaces can
//! evolve independently while every full settings update still commits in one
//! SQLite transaction.

use crate::models::AppSettings;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
#[cfg(not(test))]
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

const CONFIG_SCHEMA_VERSION: i64 = 1;
const ENTITY_DOCUMENT_KEYS: [&str; 3] = ["gateway", "sourceAware", "clientTools"];

#[cfg(not(test))]
static GLOBAL_CONFIG_DB: LazyLock<Mutex<Option<AppConfigDatabase>>> =
    LazyLock::new(|| Mutex::new(None));

pub struct AppConfigDatabase {
    conn: Connection,
    path: PathBuf,
}

#[cfg(not(test))]
pub fn with_config_database<T>(
    operation: impl FnOnce(&mut AppConfigDatabase) -> Result<T, String>,
) -> Result<T, String> {
    let mut guard = GLOBAL_CONFIG_DB
        .lock()
        .map_err(|_| "ERR_CONFIG_DB_LOCK_POISONED".to_string())?;
    if guard.is_none() {
        *guard = Some(AppConfigDatabase::new()?);
    }
    operation(guard.as_mut().expect("config database initialized"))
}

#[cfg(test)]
pub fn with_config_database<T>(
    operation: impl FnOnce(&mut AppConfigDatabase) -> Result<T, String>,
) -> Result<T, String> {
    let mut database = AppConfigDatabase::new()?;
    operation(&mut database)
}

impl AppConfigDatabase {
    pub fn new() -> Result<Self, String> {
        let path = crate::utils::usagemeter_dir()?.join("app_config.db");
        Self::new_with_path(&path)
    }

    pub(crate) fn new_with_path(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("ERR_CREATE_CONFIG_DB_DIR: {e}"))?;
        }

        let conn = Connection::open(path).map_err(|e| format!("ERR_OPEN_CONFIG_DB: {e}"))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("ERR_CONFIG_DB_WAL: {e}"))?;
        conn.pragma_update(None, "wal_autocheckpoint", 1000_i64)
            .map_err(|e| format!("ERR_CONFIG_DB_WAL_AUTOCHECKPOINT: {e}"))?;
        conn.pragma_update(None, "journal_size_limit", 1_048_576_i64)
            .map_err(|e| format!("ERR_CONFIG_DB_JOURNAL_SIZE_LIMIT: {e}"))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| format!("ERR_CONFIG_DB_FOREIGN_KEYS: {e}"))?;
        conn.busy_timeout(Duration::from_secs(30))
            .map_err(|e| format!("ERR_CONFIG_DB_BUSY_TIMEOUT: {e}"))?;

        Self::create_schema(&conn)?;
        if let Err(error) = conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE)") {
            eprintln!("[database] Passive config WAL checkpoint skipped: {error}");
        }
        set_sqlite_private_permissions(path);
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    fn create_schema(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS config_meta (
                meta_key TEXT PRIMARY KEY,
                meta_value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS config_documents (
                document_key TEXT PRIMARY KEY,
                payload_json TEXT NOT NULL,
                revision INTEGER NOT NULL DEFAULT 1,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS runtime_documents (
                document_key TEXT PRIMARY KEY,
                payload_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .map_err(|e| format!("ERR_CREATE_CONFIG_SCHEMA: {e}"))?;

        let now = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO config_meta (meta_key, meta_value, updated_at)
             VALUES ('schema_version', ?1, ?2)
             ON CONFLICT(meta_key) DO NOTHING",
            params![CONFIG_SCHEMA_VERSION.to_string(), now],
        )
        .map_err(|e| format!("ERR_INIT_CONFIG_SCHEMA_VERSION: {e}"))?;

        let raw_version = conn
            .query_row(
                "SELECT meta_value FROM config_meta WHERE meta_key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| format!("ERR_READ_CONFIG_SCHEMA_VERSION: {e}"))?
            .ok_or_else(|| "ERR_CONFIG_SCHEMA_VERSION_MISSING".to_string())?;
        let version = raw_version
            .parse::<i64>()
            .map_err(|e| format!("ERR_PARSE_CONFIG_SCHEMA_VERSION: {e}"))?;
        if version > CONFIG_SCHEMA_VERSION {
            return Err(format!("ERR_CONFIG_SCHEMA_TOO_NEW:{version}"));
        }
        Ok(())
    }

    pub fn load_settings(&self) -> Result<Option<AppSettings>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_key, payload_json FROM config_documents ORDER BY document_key",
            )
            .map_err(|e| format!("ERR_PREPARE_CONFIG_LOAD: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("ERR_QUERY_CONFIG_DOCUMENTS: {e}"))?;

        let mut documents = Map::new();
        for row in rows {
            let (key, payload) = row.map_err(|e| format!("ERR_READ_CONFIG_DOCUMENT: {e}"))?;
            let value = serde_json::from_str(&payload)
                .map_err(|e| format!("ERR_PARSE_CONFIG_DOCUMENT:{key}:{e}"))?;
            documents.insert(key, value);
        }
        if documents.is_empty() {
            return Ok(None);
        }

        serde_json::from_value(Value::Object(documents))
            .map(Some)
            .map_err(|e| format!("ERR_ASSEMBLE_CONFIG_SETTINGS: {e}"))
    }

    pub fn save_settings(&mut self, settings: &AppSettings) -> Result<(), String> {
        let documents = settings_documents(settings)?;
        let tx = self
            .conn
            .transaction()
            .map_err(|e| format!("ERR_BEGIN_CONFIG_TRANSACTION: {e}"))?;
        Self::save_documents(&tx, documents)?;
        tx.execute(
            "DELETE FROM config_documents
             WHERE document_key NOT IN ('gateway', 'sourceAware', 'clientTools')",
            [],
        )
        .map_err(|e| format!("ERR_REMOVE_LEGACY_CONFIG_DOCUMENTS: {e}"))?;
        tx.commit()
            .map_err(|e| format!("ERR_COMMIT_CONFIG_TRANSACTION: {e}"))?;
        set_sqlite_private_permissions(&self.path);
        Ok(())
    }

    fn save_documents(tx: &Transaction<'_>, documents: Map<String, Value>) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp_millis();
        let mut statement = tx
            .prepare(
                "INSERT INTO config_documents (document_key, payload_json, revision, updated_at)
                 VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(document_key) DO UPDATE SET
                    payload_json = excluded.payload_json,
                    revision = config_documents.revision + 1,
                    updated_at = excluded.updated_at
                 WHERE config_documents.payload_json != excluded.payload_json",
            )
            .map_err(|e| format!("ERR_PREPARE_CONFIG_SAVE: {e}"))?;

        for (key, value) in documents {
            let payload = serde_json::to_string(&value)
                .map_err(|e| format!("ERR_SERIALIZE_CONFIG_DOCUMENT:{key}:{e}"))?;
            statement
                .execute(params![key, payload, now])
                .map_err(|e| format!("ERR_SAVE_CONFIG_DOCUMENT: {e}"))?;
        }
        Ok(())
    }

    pub fn load_runtime_document(&self, key: &str) -> Result<Option<Value>, String> {
        let payload = self
            .conn
            .query_row(
                "SELECT payload_json FROM runtime_documents WHERE document_key = ?1",
                [key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| format!("ERR_READ_RUNTIME_DOCUMENT:{key}:{e}"))?;
        payload
            .map(|json| {
                serde_json::from_str(&json)
                    .map_err(|e| format!("ERR_PARSE_RUNTIME_DOCUMENT:{key}:{e}"))
            })
            .transpose()
    }

    pub fn save_runtime_document(&mut self, key: &str, value: &Value) -> Result<(), String> {
        let payload = serde_json::to_string(value)
            .map_err(|e| format!("ERR_SERIALIZE_RUNTIME_DOCUMENT:{key}:{e}"))?;
        self.conn
            .execute(
                "INSERT INTO runtime_documents (document_key, payload_json, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(document_key) DO UPDATE SET
                    payload_json = excluded.payload_json,
                    updated_at = excluded.updated_at",
                params![key, payload, chrono::Utc::now().timestamp_millis()],
            )
            .map_err(|e| format!("ERR_SAVE_RUNTIME_DOCUMENT:{key}:{e}"))?;
        set_sqlite_private_permissions(&self.path);
        Ok(())
    }

    #[cfg(test)]
    fn document_revision(&self, key: &str) -> Result<Option<i64>, String> {
        self.conn
            .query_row(
                "SELECT revision FROM config_documents WHERE document_key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("ERR_READ_CONFIG_REVISION: {e}"))
    }
}

pub fn load_runtime_document(key: &str) -> Result<Option<Value>, String> {
    with_config_database(|database| database.load_runtime_document(key))
}

pub fn save_runtime_document(key: &str, value: &Value) -> Result<(), String> {
    with_config_database(|database| database.save_runtime_document(key, value))
}

fn settings_documents(settings: &AppSettings) -> Result<Map<String, Value>, String> {
    match serde_json::to_value(settings).map_err(|e| format!("ERR_SERIALIZE_SETTINGS: {e}"))? {
        Value::Object(mut documents) => {
            documents.retain(|key, _| ENTITY_DOCUMENT_KEYS.contains(&key.as_str()));
            Ok(documents)
        }
        _ => Err("ERR_SETTINGS_NOT_OBJECT".to_string()),
    }
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) {}

fn set_sqlite_private_permissions(path: &Path) {
    set_private_permissions(path);
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        set_private_permissions(Path::new(&sidecar));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_preserves_entity_documents_only() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = AppConfigDatabase::new_with_path(&temp.path().join("app_config.db")).unwrap();
        let mut settings = AppSettings {
            locale: "en-US".to_string(),
            ..AppSettings::default()
        };
        settings
            .gateway
            .profiles
            .push(crate::models::GatewayProfile {
                id: "gateway-1".to_string(),
                name: "Test gateway".to_string(),
                protocol: crate::models::GatewayProtocol::OpenAiChatCompletions,
                base_url: "https://example.com".to_string(),
                enabled: true,
                client_label: String::new(),
                auth_mode: crate::models::GatewayAuthMode::ClientPassthrough,
                dispatch_strategy: crate::models::GatewayDispatchStrategy::RoundRobin,
                upstream_keys: vec![],
                local_keys: vec![],
                upstream_models: vec![],
            });

        db.save_settings(&settings).unwrap();
        let loaded = db.load_settings().unwrap().unwrap();

        assert_eq!(loaded.locale, crate::models::default_locale());
        assert_eq!(loaded.gateway.profiles[0].id, "gateway-1");
    }

    #[test]
    fn repeated_save_only_tracks_entity_document_revisions() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = AppConfigDatabase::new_with_path(&temp.path().join("app_config.db")).unwrap();
        let mut settings = AppSettings::default();

        db.save_settings(&settings).unwrap();
        settings.gateway.storage_version = 7;
        db.save_settings(&settings).unwrap();

        assert_eq!(db.document_revision("locale").unwrap(), None);
        assert_eq!(db.document_revision("gateway").unwrap(), Some(2));
    }

    #[test]
    fn empty_database_has_no_authoritative_settings() {
        let temp = tempfile::tempdir().unwrap();
        let db = AppConfigDatabase::new_with_path(&temp.path().join("app_config.db")).unwrap();
        assert!(db.load_settings().unwrap().is_none());
    }

    #[test]
    fn newer_schema_is_rejected_before_settings_are_loaded() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("app_config.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE config_meta (
                meta_key TEXT PRIMARY KEY,
                meta_value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
             );
             INSERT INTO config_meta VALUES ('schema_version', '99', 0);",
        )
        .unwrap();
        drop(conn);

        let error = AppConfigDatabase::new_with_path(&path).err().unwrap();
        assert_eq!(error, "ERR_CONFIG_SCHEMA_TOO_NEW:99");
    }

    #[test]
    fn corrupt_namespace_reports_its_document_key() {
        let temp = tempfile::tempdir().unwrap();
        let mut db = AppConfigDatabase::new_with_path(&temp.path().join("app_config.db")).unwrap();
        db.save_settings(&AppSettings::default()).unwrap();
        db.conn
            .execute(
                "UPDATE config_documents SET payload_json = '{' WHERE document_key = 'gateway'",
                [],
            )
            .unwrap();

        let error = db.load_settings().unwrap_err();
        assert!(error.starts_with("ERR_PARSE_CONFIG_DOCUMENT:gateway:"));
    }
}
