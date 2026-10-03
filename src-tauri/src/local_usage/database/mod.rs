use crate::models::AppSettings;
use crate::session::{LocalRequestRecord, SessionMeta};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

static GLOBAL_LOCAL_USAGE_DB: OnceLock<Arc<LocalUsageDatabase>> = OnceLock::new();
const LOCAL_SYNC_THROTTLE_INTERVAL: Duration = Duration::from_secs(3);
const OPENCODE_DB_SYNC_STATE_PREFIX: &str = "opencode_db_";
const OPENCODE_DB_SYNC_STATES_V2_KEY: &str = "opencode_db_scan_states_v2";
const OPENCODE_MESSAGE_ID_CONFLICT_PREFIX: &str = "opencode_message_id_conflict_";

mod attribution;
mod attribution_watcher;
pub(crate) mod cursor;
mod maintenance;
mod materialized;
mod migrations;
mod outbox;
mod queries;
mod remote_sync;
mod scanner_sync;
mod schema;
mod sync_state;
#[cfg(test)]
mod tests;
mod types;

#[cfg(test)]
pub(crate) use attribution::PassiveAttributionSnapshot;
pub use attribution::{ManualAttributionOverride, PassiveAttributionInterval};
pub use attribution_watcher::start_passive_attribution_watcher;

pub use types::*;

#[derive(Debug, Clone, Copy)]
pub(super) enum TimestampSqlColumn {
    Timestamp,
}

impl TimestampSqlColumn {
    fn as_sql_identifier(self) -> &'static str {
        match self {
            Self::Timestamp => "timestamp",
        }
    }
}

#[derive(Debug, Clone)]
struct DirtySessionSync {
    session_id: String,
    tool: String,
    file_path: String,
    file_role: String,
    file_size: u64,
    last_modified: i64,
    fingerprint: String,
    meta: SessionMeta,
    requests: Vec<LocalRequestRecord>,
    project_key: String,
}

pub struct LocalUsageDatabase {
    pub(super) conn: Arc<Mutex<Connection>>,
    db_path: PathBuf,
    sync_gate: Arc<(Mutex<SyncGateState>, Condvar)>,
}

#[derive(Debug, Default)]
struct SyncGateState {
    last_completed_at: Option<Instant>,
    sync_in_progress: bool,
}

impl LocalUsageDatabase {
    fn saturating_i64_to_u64(value: i64) -> u64 {
        value.max(0) as u64
    }

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
        let db = Self::new_with_path(&db_path)?;
        db.run_startup_maintenance();
        Ok(db)
    }

    pub(crate) fn new_with_path(path: &PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create local usage DB dir: {}", e))?;
        }

        let conn =
            Connection::open(path).map_err(|e| format!("Failed to open local usage DB: {}", e))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("Failed to enable WAL on local usage DB: {}", e))?;
        conn.pragma_update(None, "wal_autocheckpoint", 1000_i64)
            .map_err(|e| format!("Failed to configure local usage WAL autocheckpoint: {}", e))?;
        conn.pragma_update(None, "journal_size_limit", 8_i64 * 1024 * 1024)
            .map_err(|e| format!("Failed to configure local usage WAL size limit: {}", e))?;
        conn.busy_timeout(Duration::from_secs(30))
            .map_err(|e| format!("Failed to set local usage DB busy timeout: {}", e))?;

        Self::create_tables(&conn)?;
        Self::migrate_schema(&conn)?;
        Self::create_merge_cache_generation_tracking(&conn)?;

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: path.clone(),
            sync_gate: Arc::new((Mutex::new(SyncGateState::default()), Condvar::new())),
        };
        if let Err(error) = db.prune_uploaded_outbox() {
            eprintln!("[database] Startup outbox cleanup skipped: {error}");
        }
        db.checkpoint_wal_passive();
        Ok(db)
    }

    pub(crate) fn checkpoint_wal_passive(&self) {
        let Ok(conn) = self.conn.lock() else { return };
        if let Err(error) = conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE)") {
            eprintln!("[database] Passive local usage WAL checkpoint skipped: {error}");
        }
    }

    pub(crate) fn compact_after_large_delete(&self, removed_rows: u64) {
        if removed_rows < 1_000 {
            self.checkpoint_wal_passive();
            return;
        }
        let path = self.db_path.clone();
        std::thread::spawn(move || {
            let Ok(conn) = Connection::open(path) else {
                eprintln!("[database] Local usage background compaction could not open database");
                return;
            };
            let _ = conn.busy_timeout(Duration::from_secs(30));
            if let Err(error) = conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE); VACUUM;") {
                eprintln!("[database] Local usage background compaction skipped: {error}");
            }
        });
    }

    fn db_path() -> Result<PathBuf, String> {
        Ok(crate::utils::usagemeter_dir()?.join("local_usage.db"))
    }

    pub(super) fn open_readonly_connection(&self) -> Result<Connection, String> {
        let conn = Connection::open_with_flags(
            &self.db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| format!("Failed to open readonly local usage DB: {}", e))?;
        conn.busy_timeout(Duration::from_secs(30))
            .map_err(|e| format!("Failed to set readonly local usage DB busy timeout: {}", e))?;
        Ok(conn)
    }

    pub fn ensure_synced_throttled(&self, min_interval: Duration) -> Result<(), String> {
        let (lock, cvar) = self.sync_gate.as_ref();

        loop {
            let mut state = lock.lock().unwrap_or_else(|err| err.into_inner());
            if let Some(last_completed_at) = state.last_completed_at {
                if last_completed_at.elapsed() < min_interval {
                    return Ok(());
                }
            }

            if state.sync_in_progress {
                let _guard = cvar.wait(state).unwrap_or_else(|err| err.into_inner());
                continue;
            }

            state.sync_in_progress = true;
            drop(state);

            let result = self.sync_from_scanner();

            let mut state = lock.lock().unwrap_or_else(|err| err.into_inner());
            state.sync_in_progress = false;
            if result.is_ok() {
                state.last_completed_at = Some(Instant::now());
            }
            cvar.notify_all();
            return result;
        }
    }

    pub fn today_local_date() -> String {
        let settings = crate::settings::load_settings_blocking().unwrap_or_default();
        Self::today_local_date_with_settings(&settings)
    }

    #[allow(dead_code)]
    pub fn local_date_epoch_bounds(local_date: &str) -> Result<(i64, i64), String> {
        let settings = crate::settings::load_settings_blocking().unwrap_or_default();
        Self::local_date_epoch_bounds_with_settings(local_date, &settings)
    }

    pub fn today_local_date_with_settings(settings: &AppSettings) -> String {
        crate::utils::business_time::current_business_date(settings)
    }

    pub fn local_date_epoch_bounds_with_settings(
        local_date: &str,
        settings: &AppSettings,
    ) -> Result<(i64, i64), String> {
        crate::utils::business_time::business_date_epoch_bounds(local_date, settings)
    }

    pub(super) fn business_date_sql_expr_for_timestamp(
        settings: &AppSettings,
        timestamp_column: TimestampSqlColumn,
    ) -> String {
        let timestamp_column = timestamp_column.as_sql_identifier();
        match crate::utils::business_time::business_day_start_hour(settings) {
            0 => format!(
                "strftime('%Y-%m-%d', {timestamp_column}, 'unixepoch', 'localtime')"
            ),
            hour => format!(
                "strftime('%Y-%m-%d', {timestamp_column}, 'unixepoch', 'localtime', '-{hour} hours')"
            ),
        }
    }
}

pub fn ensure_local_usage_synced() -> Result<Arc<LocalUsageDatabase>, String> {
    let db = LocalUsageDatabase::get_global()?;
    db.ensure_synced_throttled(LOCAL_SYNC_THROTTLE_INTERVAL)?;
    Ok(db)
}

pub fn get_local_usage_db() -> Result<Arc<LocalUsageDatabase>, String> {
    LocalUsageDatabase::get_global()
}
