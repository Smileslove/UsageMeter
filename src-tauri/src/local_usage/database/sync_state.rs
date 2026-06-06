use rusqlite::{params, OptionalExtension};

use super::{
    LocalUsageDatabase, OPENCODE_DB_SYNC_STATES_V2_KEY, OPENCODE_DB_SYNC_STATE_PREFIX,
    OPENCODE_MESSAGE_ID_CONFLICT_PREFIX,
};

impl LocalUsageDatabase {
    pub(super) fn upsert_sync_state(
        tx: &rusqlite::Transaction<'_>,
        state_key: &str,
        state_value: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        tx.execute(
            "INSERT INTO local_sync_state (state_key, state_value, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(state_key) DO UPDATE
             SET state_value = excluded.state_value,
                 updated_at = excluded.updated_at",
            params![state_key, state_value, updated_at],
        )
        .map_err(|e| format!("Failed to upsert local sync state `{state_key}`: {}", e))?;
        Ok(())
    }

    pub(crate) fn get_local_sync_state(&self, key: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT state_value FROM local_sync_state WHERE state_key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to read local sync state `{}`: {}", key, e))
    }

    pub(super) fn load_opencode_db_scan_state(
        &self,
    ) -> Result<crate::session::opencode_reader::OpenCodeDbScanState, String> {
        Ok(crate::session::opencode_reader::OpenCodeDbScanState {
            storage_signature_hash: self
                .get_local_sync_state(&format!(
                    "{}storage_signature_hash",
                    OPENCODE_DB_SYNC_STATE_PREFIX
                ))?
                .and_then(|value| value.parse::<u64>().ok())
                .or_else(|| {
                    self.get_local_sync_state(&format!(
                        "{}fingerprint",
                        OPENCODE_DB_SYNC_STATE_PREFIX
                    ))
                    .ok()
                    .flatten()
                    .and_then(|value| value.parse::<u64>().ok())
                })
                .unwrap_or(0),
            file_size: self
                .get_local_sync_state(&format!("{}file_size", OPENCODE_DB_SYNC_STATE_PREFIX))?
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0),
            schema_fingerprint: self
                .get_local_sync_state(&format!(
                    "{}schema_fingerprint",
                    OPENCODE_DB_SYNC_STATE_PREFIX
                ))?
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0),
            assistant_row_count: self
                .get_local_sync_state(&format!(
                    "{}assistant_row_count",
                    OPENCODE_DB_SYNC_STATE_PREFIX
                ))?
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0),
            last_time_updated_ms: self
                .get_local_sync_state(&format!(
                    "{}last_time_updated_ms",
                    OPENCODE_DB_SYNC_STATE_PREFIX
                ))?
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(0),
            last_rowid: self
                .get_local_sync_state(&format!("{}last_rowid", OPENCODE_DB_SYNC_STATE_PREFIX))?
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(0),
            last_full_reconcile_at_ms: self
                .get_local_sync_state(&format!(
                    "{}last_full_reconcile_at_ms",
                    OPENCODE_DB_SYNC_STATE_PREFIX
                ))?
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(0),
            schema_mode: self
                .get_local_sync_state(&format!("{}schema_mode", OPENCODE_DB_SYNC_STATE_PREFIX))?
                .unwrap_or_else(|| "unknown".to_string()),
        })
    }

    pub(super) fn load_opencode_db_scan_states(
        &self,
    ) -> Result<crate::session::opencode_reader::OpenCodeDbScanStates, String> {
        if let Some(raw) = self.get_local_sync_state(OPENCODE_DB_SYNC_STATES_V2_KEY)? {
            if let Ok(states) =
                serde_json::from_str::<crate::session::opencode_reader::OpenCodeDbScanStates>(&raw)
            {
                return Ok(states);
            }
        }

        let legacy = self.load_opencode_db_scan_state()?;
        let mut states = crate::session::opencode_reader::OpenCodeDbScanStates::default();
        if legacy.storage_signature_hash != 0
            || legacy.file_size != 0
            || legacy.schema_fingerprint != 0
            || legacy.assistant_row_count != 0
            || legacy.last_time_updated_ms != 0
            || legacy.last_rowid != 0
            || legacy.last_full_reconcile_at_ms != 0
        {
            states.stores.insert("native".to_string(), legacy);
        }
        Ok(states)
    }

    pub(super) fn persist_opencode_db_scan_state_tx(
        tx: &rusqlite::Transaction<'_>,
        state: &crate::session::opencode_reader::OpenCodeDbScanState,
        now: i64,
    ) -> Result<(), String> {
        Self::upsert_sync_state(
            tx,
            &format!("{}storage_signature_hash", OPENCODE_DB_SYNC_STATE_PREFIX),
            &state.storage_signature_hash.to_string(),
            now,
        )?;
        Self::upsert_sync_state(
            tx,
            &format!("{}file_size", OPENCODE_DB_SYNC_STATE_PREFIX),
            &state.file_size.to_string(),
            now,
        )?;
        Self::upsert_sync_state(
            tx,
            &format!("{}schema_fingerprint", OPENCODE_DB_SYNC_STATE_PREFIX),
            &state.schema_fingerprint.to_string(),
            now,
        )?;
        Self::upsert_sync_state(
            tx,
            &format!("{}assistant_row_count", OPENCODE_DB_SYNC_STATE_PREFIX),
            &state.assistant_row_count.to_string(),
            now,
        )?;
        Self::upsert_sync_state(
            tx,
            &format!("{}last_time_updated_ms", OPENCODE_DB_SYNC_STATE_PREFIX),
            &state.last_time_updated_ms.to_string(),
            now,
        )?;
        Self::upsert_sync_state(
            tx,
            &format!("{}last_rowid", OPENCODE_DB_SYNC_STATE_PREFIX),
            &state.last_rowid.to_string(),
            now,
        )?;
        Self::upsert_sync_state(
            tx,
            &format!("{}last_full_reconcile_at_ms", OPENCODE_DB_SYNC_STATE_PREFIX),
            &state.last_full_reconcile_at_ms.to_string(),
            now,
        )?;
        Self::upsert_sync_state(
            tx,
            &format!("{}schema_mode", OPENCODE_DB_SYNC_STATE_PREFIX),
            &state.schema_mode,
            now,
        )?;
        Ok(())
    }

    pub(super) fn persist_opencode_db_scan_states_tx(
        tx: &rusqlite::Transaction<'_>,
        states: &crate::session::opencode_reader::OpenCodeDbScanStates,
        now: i64,
    ) -> Result<(), String> {
        let raw = serde_json::to_string(states)
            .map_err(|e| format!("Failed to serialize OpenCode DB scan states: {}", e))?;
        Self::upsert_sync_state(tx, OPENCODE_DB_SYNC_STATES_V2_KEY, &raw, now)?;
        if let Some(native) = states.stores.get("native") {
            Self::persist_opencode_db_scan_state_tx(tx, native, now)?;
        }
        Ok(())
    }

    pub(super) fn persist_opencode_message_id_conflict_tx(
        tx: &rusqlite::Transaction<'_>,
        status: &crate::session::opencode_reader::OpenCodeMessageIdConflictStatus,
        now: i64,
    ) -> Result<(), String> {
        Self::upsert_sync_state(
            tx,
            &format!("{}has_conflict", OPENCODE_MESSAGE_ID_CONFLICT_PREFIX),
            if status.has_conflict { "1" } else { "0" },
            now,
        )?;
        Self::upsert_sync_state(
            tx,
            &format!("{}count", OPENCODE_MESSAGE_ID_CONFLICT_PREFIX),
            &status.conflict_count.to_string(),
            now,
        )?;
        let sample_json = serde_json::to_string(&status.sample_ids)
            .map_err(|e| format!("Failed to serialize OpenCode conflict sample ids: {}", e))?;
        Self::upsert_sync_state(
            tx,
            &format!("{}sample_ids", OPENCODE_MESSAGE_ID_CONFLICT_PREFIX),
            &sample_json,
            now,
        )?;
        Self::upsert_sync_state(
            tx,
            &format!("{}seen_at", OPENCODE_MESSAGE_ID_CONFLICT_PREFIX),
            &now.to_string(),
            now,
        )?;
        Ok(())
    }

    pub fn upsert_webdav_sync_state(&self, key: &str, value: &str) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO webdav_sync_state (state_key, state_value, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(state_key) DO UPDATE SET
                state_value = excluded.state_value,
                updated_at = excluded.updated_at",
            params![key, value, now],
        )
        .map_err(|e| format!("Failed to upsert WebDAV sync state: {}", e))?;
        Ok(())
    }

    pub fn get_webdav_sync_state(&self, key: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT state_value FROM webdav_sync_state WHERE state_key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Failed to read WebDAV sync state: {}", e))
    }
}
