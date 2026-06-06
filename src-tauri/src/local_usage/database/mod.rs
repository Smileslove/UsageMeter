use crate::session::{LocalRequestRecord, SessionMeta};
use chrono::{Local, LocalResult, NaiveDate, TimeZone};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

static GLOBAL_LOCAL_USAGE_DB: OnceLock<Arc<LocalUsageDatabase>> = OnceLock::new();
const LOCAL_SYNC_THROTTLE_INTERVAL: Duration = Duration::from_secs(3);
const OPENCODE_DB_SYNC_STATE_PREFIX: &str = "opencode_db_";
const OPENCODE_DB_SYNC_STATES_V2_KEY: &str = "opencode_db_scan_states_v2";
const OPENCODE_MESSAGE_ID_CONFLICT_PREFIX: &str = "opencode_message_id_conflict_";

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

pub use types::*;

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
    sync_gate: Arc<(Mutex<SyncGateState>, Condvar)>,
}

#[derive(Debug, Default)]
struct SyncGateState {
    last_completed_at: Option<Instant>,
    sync_in_progress: bool,
}

impl LocalUsageDatabase {
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
        Self::new_with_path(&db_path)
    }

    fn new_with_path(path: &PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create local usage DB dir: {}", e))?;
        }

        let conn =
            Connection::open(path).map_err(|e| format!("Failed to open local usage DB: {}", e))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("Failed to enable WAL on local usage DB: {}", e))?;
        conn.busy_timeout(Duration::from_secs(30))
            .map_err(|e| format!("Failed to set local usage DB busy timeout: {}", e))?;

        Self::create_tables(&conn)?;
        Self::migrate_schema(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            sync_gate: Arc::new((Mutex::new(SyncGateState::default()), Condvar::new())),
        })
    }

    fn db_path() -> Result<PathBuf, String> {
        Ok(crate::utils::usagemeter_dir()?.join("local_usage.db"))
    }

    pub fn ensure_synced_throttled(&self, min_interval: Duration) -> Result<(), String> {
        let (lock, cvar) = self.sync_gate.as_ref();

        loop {
            let mut state = lock.lock().unwrap();
            if let Some(last_completed_at) = state.last_completed_at {
                if last_completed_at.elapsed() < min_interval {
                    return Ok(());
                }
            }

            if state.sync_in_progress {
                let _guard = cvar.wait(state).unwrap();
                continue;
            }

            state.sync_in_progress = true;
            drop(state);

            let result = self.sync_from_scanner();

            let mut state = lock.lock().unwrap();
            state.sync_in_progress = false;
            if result.is_ok() {
                state.last_completed_at = Some(Instant::now());
            }
            cvar.notify_all();
            return result;
        }
    }

    pub fn today_local_date() -> String {
        Local::now().format("%Y-%m-%d").to_string()
    }

    pub fn local_date_epoch_bounds(local_date: &str) -> Result<(i64, i64), String> {
        let date = NaiveDate::parse_from_str(local_date, "%Y-%m-%d")
            .map_err(|e| format!("Invalid local date `{local_date}`: {e}"))?;
        let next_date = date
            .succ_opt()
            .ok_or_else(|| format!("Invalid local date `{local_date}`"))?;
        let start = Self::resolve_local_day_boundary(date, local_date)?;
        let end = Self::resolve_local_day_boundary(next_date, local_date)?;
        Ok((start, end))
    }

    fn resolve_local_day_boundary(date: NaiveDate, label: &str) -> Result<i64, String> {
        for hour in 0..24 {
            let Some(naive) = date.and_hms_opt(hour, 0, 0) else {
                continue;
            };
            match Local.from_local_datetime(&naive) {
                LocalResult::Single(dt) => return Ok(dt.timestamp()),
                LocalResult::Ambiguous(earliest, _) => return Ok(earliest.timestamp()),
                LocalResult::None => continue,
            }
        }
        Err(format!(
            "Failed to resolve local day boundary for `{label}` in local timezone"
        ))
    }
}

pub fn ensure_local_usage_synced() -> Result<Arc<LocalUsageDatabase>, String> {
    let db = LocalUsageDatabase::get_global()?;
    db.ensure_synced_throttled(LOCAL_SYNC_THROTTLE_INTERVAL)?;
    Ok(db)
}
