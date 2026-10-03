use super::{DirtySessionSync, LocalUsageDatabase};
use crate::{
    cursor::events::{digest, occurrence_keys, CursorEvent},
    models::ToolFilter,
    session::{LocalRequestRecord, SessionMeta, UsageProvenance},
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CursorBatch {
    pub batch_id: String,
    pub account_key: String,
    pub source: String,
    pub range_start_ms: i64,
    pub range_end_ms: i64,
    pub event_count: usize,
    pub imported_at_ms: i64,
}

impl LocalUsageDatabase {
    pub(super) fn create_cursor_tables(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cursor_usage_batches (
            batch_id TEXT PRIMARY KEY, account_key TEXT NOT NULL, source TEXT NOT NULL,
            range_start_ms INTEGER NOT NULL, range_end_ms INTEGER NOT NULL,
            event_count INTEGER NOT NULL, imported_at_ms INTEGER NOT NULL,
            priority INTEGER NOT NULL, CHECK(range_start_ms < range_end_ms)
        );
        CREATE TABLE IF NOT EXISTS cursor_usage_events (
            batch_id TEXT NOT NULL REFERENCES cursor_usage_batches(batch_id),
            event_key TEXT NOT NULL, timestamp_ms INTEGER NOT NULL,
            event_json TEXT NOT NULL, PRIMARY KEY(batch_id, event_key)
        );
        CREATE INDEX IF NOT EXISTS idx_cursor_events_timestamp ON cursor_usage_events(timestamp_ms);
        CREATE TABLE IF NOT EXISTS cursor_usage_sync_state (
            account_key TEXT PRIMARY KEY, last_attempt_ms INTEGER, last_success_ms INTEGER,
            next_retry_ms INTEGER, status TEXT NOT NULL, error_code TEXT,
            range_start_ms INTEGER, range_end_ms INTEGER, event_count INTEGER
        );",
        )
        .map_err(|_| "cursor_database_error".to_string())
    }

    pub(crate) fn list_cursor_batches(&self) -> Result<Vec<CursorBatch>, String> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare("SELECT batch_id, account_key, source, range_start_ms, range_end_ms, event_count, imported_at_ms FROM cursor_usage_batches ORDER BY imported_at_ms DESC, batch_id")
            .map_err(|_| "cursor_database_error")?;
        let rows = statement
            .query_map([], |row| {
                Ok(CursorBatch {
                    batch_id: row.get(0)?,
                    account_key: row.get(1)?,
                    source: row.get(2)?,
                    range_start_ms: row.get(3)?,
                    range_end_ms: row.get(4)?,
                    event_count: row.get::<_, i64>(5)? as usize,
                    imported_at_ms: row.get(6)?,
                })
            })
            .map_err(|_| "cursor_database_error")?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| "cursor_database_error".into())
    }

    /// Network/CSV parsing has completed before this call. Batch and projections publish
    /// together; a transaction failure retains the last good history and materialization.
    pub(crate) fn publish_cursor_batch(
        &self,
        batch: &CursorBatch,
        events: &[CursorEvent],
    ) -> Result<(), String> {
        let metadata = self.current_cursor_metadata()?;
        if batch.source == "api" {
            let active = crate::cursor::sync::current_auth().map_err(|error| error.code)?;
            if active.account_key != batch.account_key {
                return Err("cursor_identity_mismatch".into());
            }
            if !crate::settings::load_settings_blocking()?
                .cursor
                .account_sync_enabled
            {
                return Err("cursor_sync_disabled".into());
            }
        }
        self.publish_cursor_batch_with_metadata(batch, events, metadata)
    }

    fn publish_cursor_batch_with_metadata(
        &self,
        batch: &CursorBatch,
        events: &[CursorEvent],
        metadata: HashMap<String, SessionMeta>,
    ) -> Result<(), String> {
        if batch.range_start_ms < 0
            || batch.range_end_ms <= batch.range_start_ms
            || batch.event_count != events.len()
            || !["csv", "api"].contains(&batch.source.as_str())
            || batch.account_key.len() != 64
            || !batch.account_key.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("cursor_invalid_import_scope".into());
        }
        if events.iter().any(|event| {
            event.timestamp_ms < batch.range_start_ms || event.timestamp_ms >= batch.range_end_ms
        }) {
            return Err("cursor_event_outside_range".into());
        }
        let keys = occurrence_keys(events)?;
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|_| "cursor_database_error")?;
        let previous: Option<(String, String)> = tx
            .query_row(
                "SELECT account_key,source FROM cursor_usage_batches WHERE batch_id=?1",
                [&batch.batch_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|_| "cursor_database_error")?;
        if previous
            .is_some_and(|(account, source)| account != batch.account_key || source != batch.source)
        {
            return Err("cursor_invalid_import_scope".into());
        }
        if batch.source == "csv" {
            let conflict: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM cursor_usage_batches WHERE account_key=?1 AND source='csv' AND batch_id!=?2 AND range_start_ms<?4 AND range_end_ms>?3)", params![batch.account_key, batch.batch_id, batch.range_start_ms, batch.range_end_ms], |row| row.get(0)).map_err(|_| "cursor_database_error")?;
            if conflict {
                return Err("cursor_import_range_conflict".into());
            }
        }
        tx.execute(
            "DELETE FROM cursor_usage_events WHERE batch_id=?1",
            [&batch.batch_id],
        )
        .map_err(|_| "cursor_database_error")?;
        tx.execute("INSERT INTO cursor_usage_batches VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(batch_id) DO UPDATE SET range_start_ms=excluded.range_start_ms, range_end_ms=excluded.range_end_ms, event_count=excluded.event_count, imported_at_ms=excluded.imported_at_ms", params![batch.batch_id, batch.account_key, batch.source, batch.range_start_ms, batch.range_end_ms, events.len() as i64, batch.imported_at_ms, if batch.source == "api" { 1 } else { 0 }]).map_err(|_| "cursor_database_error")?;
        for (event, key) in events.iter().zip(keys) {
            let json = serde_json::to_string(event).map_err(|_| "cursor_invalid_event")?;
            tx.execute(
                "INSERT INTO cursor_usage_events VALUES (?1,?2,?3,?4)",
                params![batch.batch_id, key, event.timestamp_ms, json],
            )
            .map_err(|_| "cursor_database_error")?;
        }
        Self::rebuild_cursor_projection_tx(&tx, metadata)?;
        tx.execute(
            "DELETE FROM local_sync_state WHERE state_key='cursor_metadata_fingerprint'",
            [],
        )
        .map_err(|_| "cursor_database_error")?;
        if batch.source == "api" {
            tx.execute("INSERT INTO cursor_usage_sync_state(account_key,last_attempt_ms,last_success_ms,status,range_start_ms,range_end_ms,event_count) VALUES (?1,?2,?2,'ready',?3,?4,?5) ON CONFLICT(account_key) DO UPDATE SET last_attempt_ms=excluded.last_attempt_ms,last_success_ms=excluded.last_success_ms,next_retry_ms=NULL,status='ready',error_code=NULL,range_start_ms=excluded.range_start_ms,range_end_ms=excluded.range_end_ms,event_count=excluded.event_count", params![batch.account_key,batch.imported_at_ms,batch.range_start_ms,batch.range_end_ms,events.len() as i64]).map_err(|_| "cursor_database_error")?;
        }
        tx.commit().map_err(|_| "cursor_database_error")?;
        crate::unified_usage::clear_runtime_caches();
        Ok(())
    }

    pub(crate) fn revoke_cursor_batch(&self, batch_id: &str) -> Result<(), String> {
        self.revoke_cursor_batch_with_metadata(batch_id, self.current_cursor_metadata()?)
    }

    fn revoke_cursor_batch_with_metadata(
        &self,
        batch_id: &str,
        metadata: HashMap<String, SessionMeta>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|_| "cursor_database_error")?;
        let source: Option<String> = tx
            .query_row(
                "SELECT source FROM cursor_usage_batches WHERE batch_id=?1",
                [batch_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| "cursor_database_error")?;
        if source.as_deref() != Some("csv") {
            return Err("cursor_import_batch_not_found".into());
        }
        tx.execute(
            "DELETE FROM cursor_usage_events WHERE batch_id=?1",
            [batch_id],
        )
        .map_err(|_| "cursor_database_error")?;
        tx.execute(
            "DELETE FROM cursor_usage_batches WHERE batch_id=?1",
            [batch_id],
        )
        .map_err(|_| "cursor_database_error")?;
        Self::rebuild_cursor_projection_tx(&tx, metadata)?;
        tx.execute(
            "DELETE FROM local_sync_state WHERE state_key='cursor_metadata_fingerprint'",
            [],
        )
        .map_err(|_| "cursor_database_error")?;
        tx.commit().map_err(|_| "cursor_database_error")?;
        crate::unified_usage::clear_runtime_caches();
        Ok(())
    }

    pub(crate) fn refresh_cursor_metadata(&self) -> Result<(), String> {
        let path = crate::session::cursor_reader::configured_database_path();
        self.refresh_cursor_metadata_from(path.as_deref())
    }

    fn refresh_cursor_metadata_from(&self, path: Option<&std::path::Path>) -> Result<(), String> {
        let (metadata, status) = if let Some(path) = path.filter(|path| path.exists()) {
            match crate::session::cursor_reader::read_metadata(path) {
                Ok(metadata) => (metadata, "ready"),
                Err(error) => {
                    let conn = self.conn.lock().unwrap();
                    let tx = conn
                        .unchecked_transaction()
                        .map_err(|_| "cursor_database_error")?;
                    Self::upsert_sync_state(
                        &tx,
                        "cursor_metadata_status",
                        &error,
                        chrono::Utc::now().timestamp(),
                    )?;
                    tx.commit().map_err(|_| "cursor_database_error")?;
                    return Err(error);
                }
            }
        } else {
            (HashMap::new(), "cursor_not_found")
        };
        let ordered_metadata: BTreeMap<_, _> = metadata.iter().collect();
        let fingerprint =
            digest(&serde_json::to_vec(&ordered_metadata).map_err(|_| "cursor_invalid_event")?);
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|_| "cursor_database_error")?;
        let previous: Option<String> = tx.query_row("SELECT state_value FROM local_sync_state WHERE state_key='cursor_metadata_fingerprint'",[],|row| row.get(0)).optional().map_err(|_| "cursor_database_error")?;
        if previous.as_deref() != Some(&fingerprint) {
            Self::rebuild_cursor_projection_tx(&tx, metadata)?;
            Self::upsert_sync_state(
                &tx,
                "cursor_metadata_fingerprint",
                &fingerprint,
                chrono::Utc::now().timestamp(),
            )?;
        }
        Self::upsert_sync_state(
            &tx,
            "cursor_metadata_status",
            status,
            chrono::Utc::now().timestamp(),
        )?;
        tx.commit().map_err(|_| "cursor_database_error")?;
        Ok(())
    }

    fn rebuild_cursor_projection_tx(
        tx: &Transaction<'_>,
        metadata: HashMap<String, SessionMeta>,
    ) -> Result<(), String> {
        let mut grouped = BTreeMap::<String, (SessionMeta, Vec<LocalRequestRecord>)>::new();
        let mut matched_ids = HashSet::new();
        let mut provenance = Vec::new();
        let mut projected_tokens = 0_u64;
        {
            let mut statement = tx.prepare("SELECT b.account_key,b.source,e.event_key,e.event_json FROM cursor_usage_events e JOIN cursor_usage_batches b ON b.batch_id=e.batch_id WHERE NOT EXISTS(SELECT 1 FROM cursor_usage_batches newer WHERE newer.account_key=b.account_key AND newer.batch_id!=b.batch_id AND newer.range_start_ms<=e.timestamp_ms AND newer.range_end_ms>e.timestamp_ms AND (newer.priority>b.priority OR (newer.priority=b.priority AND (newer.imported_at_ms>b.imported_at_ms OR (newer.imported_at_ms=b.imported_at_ms AND newer.batch_id>b.batch_id))))) ORDER BY b.account_key,e.timestamp_ms,e.event_key").map_err(|_| "cursor_database_error")?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(|_| "cursor_database_error")?;
            for row in rows {
                let (account, source, key, json) = row.map_err(|_| "cursor_database_error")?;
                let event: CursorEvent =
                    serde_json::from_str(&json).map_err(|_| "cursor_invalid_event")?;
                projected_tokens = projected_tokens
                    .checked_add(event.total_tokens())
                    .filter(|total| *total <= i64::MAX as u64)
                    .ok_or("cursor_invalid_token_count")?;
                let date = chrono::DateTime::from_timestamp_millis(event.timestamp_ms)
                    .ok_or("cursor_invalid_timestamp")?
                    .format("%Y-%m-%d")
                    .to_string();
                let session_kind = if event.conversation_id.is_some() {
                    "conversation"
                } else {
                    "account_bucket"
                };
                let suffix = event
                    .conversation_id
                    .as_ref()
                    .map(|id| format!("conversation-{id}"))
                    .unwrap_or_else(|| format!("day-{date}"));
                let session_id = format!("cursor::{account}::{suffix}");
                let locator = format!("cursor-account://{account}/{suffix}");
                let meta = event.conversation_id.as_ref().and_then(|id| {
                    let meta = metadata.get(id);
                    if meta.is_some() {
                        matched_ids.insert(id.clone());
                    }
                    meta
                });
                let (session, requests) = grouped.entry(session_id.clone()).or_insert_with(|| {
                    (
                        SessionMeta {
                            session_id: session_id.clone(),
                            tool: "cursor".into(),
                            file_path: locator,
                            cwd: meta.and_then(|meta| meta.cwd.clone()),
                            project_name: meta.and_then(|meta| meta.project_name.clone()),
                            scope: Some("account".into()),
                            source: "cursor_account_usage".into(),
                            start_time: event.timestamp_ms / 1000,
                            ..Default::default()
                        },
                        Vec::new(),
                    )
                });
                let evidence = UsageProvenance {
                    source_timestamp_ms: event.timestamp_ms,
                    data_scope: "account".into(),
                    usage_basis: if source == "api" {
                        "account_json"
                    } else {
                        "account_csv"
                    }
                    .into(),
                    cost_basis: if event.usage_cost_usd.is_some() {
                        "service_metered"
                    } else {
                        "unknown"
                    }
                    .into(),
                    usage_complete: event.usage_complete(),
                    session_kind: session_kind.into(),
                    count_basis: "usage_event".into(),
                    charged_amount_usd: event.charged_amount_usd,
                };
                let message_id = format!("{account}:{key}");
                let request_key = format!("cursor:{message_id}");
                provenance.push((
                    request_key.clone(),
                    serde_json::to_string(&evidence).map_err(|_| "cursor_invalid_event")?,
                ));
                session.total_input_tokens += event.input_tokens.unwrap_or(0);
                session.total_output_tokens += event.output_tokens.unwrap_or(0);
                session.total_cache_create_tokens += event.cache_write_tokens.unwrap_or(0);
                session.total_cache_read_tokens += event.cache_read_tokens.unwrap_or(0);
                session.message_count += 1;
                session.end_time = event.timestamp_ms / 1000;
                session.last_modified = session.end_time;
                if !session.models.contains(&event.model) {
                    session.models.push(event.model.clone());
                }
                requests.push(LocalRequestRecord {
                    session_id,
                    tool: "cursor".into(),
                    timestamp: event.timestamp_ms / 1000,
                    message_id,
                    request_key: Some(request_key),
                    model: event.model.clone(),
                    input_tokens: event.input_tokens.unwrap_or(0),
                    output_tokens: event.output_tokens.unwrap_or(0),
                    cache_create_tokens: event.cache_write_tokens.unwrap_or(0),
                    cache_read_tokens: event.cache_read_tokens.unwrap_or(0),
                    total_tokens: event.total_tokens(),
                    request_count: 1,
                    explicit_estimated_cost: event.usage_cost_usd,
                    provenance: Some(evidence),
                    ..Default::default()
                });
            }
        }
        for (id, meta) in metadata {
            if !matched_ids.contains(&id) {
                grouped.insert(meta.session_id.clone(), (meta, Vec::new()));
            }
        }
        let current_ids: HashSet<_> = grouped.keys().cloned().collect();
        let mut statement = tx.prepare("SELECT session_id FROM local_source_files WHERE tool='cursor' AND deleted_at IS NULL").map_err(|_| "cursor_database_error")?;
        let old_ids = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|_| "cursor_database_error")?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(|_| "cursor_database_error")?;
        drop(statement);
        let removed_ids: Vec<String> = old_ids.difference(&current_ids).cloned().collect();
        let fingerprints: HashMap<String, String> = {
            let mut statement = tx.prepare("SELECT session_id,fingerprint FROM local_source_files WHERE tool='cursor' AND deleted_at IS NULL").map_err(|_| "cursor_database_error")?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|_| "cursor_database_error")?;
            rows.collect::<Result<_, _>>()
                .map_err(|_| "cursor_database_error")?
        };
        let dirty_sessions = grouped
            .into_iter()
            .filter_map(|(session_id, (meta, requests))| {
                let fingerprint =
                    digest(&serde_json::to_vec(&(&meta, &requests)).unwrap_or_default());
                if fingerprints.get(&session_id) == Some(&fingerprint) {
                    return None;
                }
                Some(DirtySessionSync {
                    session_id,
                    tool: "cursor".into(),
                    file_path: meta.file_path.clone(),
                    file_role: "cursor_session".into(),
                    file_size: 0,
                    last_modified: meta.last_modified,
                    fingerprint,
                    project_key: meta.cwd.clone().unwrap_or_else(|| "unknown_project".into()),
                    meta,
                    requests,
                })
            })
            .collect();
        Self::sync_dirty_sessions_tx(
            tx,
            dirty_sessions,
            removed_ids.clone(),
            "",
            chrono::Utc::now().timestamp(),
        )?;
        // Other local tools keep deleted transcript history. Cursor withdrawal/correction
        // must remove superseded accounting rows, including the unified query baseline.
        tx.execute(
            "DELETE FROM local_request_facts WHERE tool='cursor' AND source_file_present=0",
            [],
        )
        .map_err(|_| "cursor_database_error")?;
        for id in removed_ids {
            tx.execute(
                "DELETE FROM local_sessions WHERE tool='cursor' AND session_id=?1",
                [id],
            )
            .map_err(|_| "cursor_database_error")?;
        }
        let settings = crate::settings::load_settings_blocking().unwrap_or_default();
        let mut changed_dates = HashSet::new();
        for (key, json) in provenance {
            let changed = tx.execute("UPDATE local_request_facts SET provenance_json=?2,sync_version=sync_version+1 WHERE tool='cursor' AND request_key=?1 AND provenance_json IS NOT ?2", params![key,json]).map_err(|_| "cursor_database_error")?;
            if changed > 0 {
                let timestamp: i64 = tx.query_row("SELECT timestamp FROM local_request_facts WHERE tool='cursor' AND request_key=?1", [&key], |row| row.get(0)).map_err(|_| "cursor_database_error")?;
                changed_dates.insert(crate::utils::business_time::business_date_for_timestamp(
                    timestamp, &settings,
                ));
            }
        }
        Self::invalidate_unified_materialization_dates_tx(
            tx,
            &changed_dates.into_iter().collect::<Vec<_>>(),
            chrono::Utc::now().timestamp(),
        )?;
        Ok(())
    }

    pub(crate) fn get_cursor_sessions(
        &self,
    ) -> Result<Vec<(SessionMeta, Vec<LocalRequestRecord>)>, String> {
        let filter = ToolFilter::Tool("cursor".into());
        let sessions = self.get_all_sessions(&filter)?;
        let mut requests = HashMap::<String, Vec<LocalRequestRecord>>::new();
        for record in self.get_request_records_in_range(0, i64::MAX, &filter)? {
            if record.source_file_present != Some(false) {
                requests
                    .entry(record.session_id.clone())
                    .or_default()
                    .push(record);
            }
        }
        Ok(sessions
            .into_iter()
            .map(|meta| {
                let records = requests.remove(&meta.session_id).unwrap_or_default();
                (meta, records)
            })
            .collect())
    }
}

impl LocalUsageDatabase {
    fn current_cursor_metadata(&self) -> Result<HashMap<String, SessionMeta>, String> {
        let path = crate::session::cursor_reader::configured_database_path();
        if let Some(path) = path {
            if path.exists() {
                if let Ok(metadata) = crate::session::cursor_reader::read_metadata(&path) {
                    return Ok(metadata);
                }
                // Retain previously observed associations on a transient/schema read failure.
                return Ok(self
                    .get_all_sessions(&ToolFilter::Tool("cursor".into()))?
                    .into_iter()
                    .filter_map(|meta| {
                        let suffix = meta.session_id.split("::").last()?.to_string();
                        let id = if meta.source == "cursor_metadata" {
                            suffix
                        } else {
                            suffix.strip_prefix("conversation-")?.to_string()
                        };
                        Some((id, meta))
                    })
                    .collect());
            }
        }
        Ok(HashMap::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cursor::events::parse_json_event;
    use serde_json::json;

    fn batch(id: &str, source: &str, count: usize) -> CursorBatch {
        CursorBatch {
            batch_id: id.into(),
            account_key: digest(b"test-account"),
            source: source.into(),
            range_start_ms: 1750000000000,
            range_end_ms: 1750000010000,
            event_count: count,
            imported_at_ms: 1750000020000,
        }
    }
    fn event() -> CursorEvent {
        parse_json_event(&json!({"timestamp":"1750000000123","conversationId":"c1","model":"auto","prompt":"NEVER STORE", "chargedCents":5,
            "tokenUsage":{"inputTokens":10,"outputTokens":20,"cacheReadTokens":0,"cacheWriteTokens":3,"totalCents":25}})).unwrap()
    }
    fn db() -> (tempfile::TempDir, LocalUsageDatabase) {
        let temp = tempfile::tempdir().unwrap();
        let db = LocalUsageDatabase::new_with_path(&temp.path().join("usage.db")).unwrap();
        (temp, db)
    }
    fn records(db: &LocalUsageDatabase) -> Vec<LocalRequestRecord> {
        db.get_request_records_in_range(0, i64::MAX, &ToolFilter::Tool("cursor".into()))
            .unwrap()
            .into_iter()
            .filter(|record| record.source_file_present != Some(false))
            .collect()
    }

    #[test]
    fn cursor_batch_is_atomic_idempotent_content_free_and_revocable() {
        let (_temp, db) = db();
        let events = vec![event(), event()];
        let csv = batch("csv1", "csv", 2);
        db.publish_cursor_batch_with_metadata(&csv, &events, HashMap::new())
            .unwrap();
        db.publish_cursor_batch_with_metadata(&csv, &events, HashMap::new())
            .unwrap();
        let records = records(&db);
        assert_eq!(records.len(), 2);
        assert_eq!(
            records
                .iter()
                .map(|record| record.total_tokens)
                .sum::<u64>(),
            66
        );
        assert_eq!(
            records[0].provenance.as_ref().unwrap().source_timestamp_ms,
            1750000000123
        );
        assert_eq!(records[0].explicit_estimated_cost, Some(0.25));
        assert_eq!(
            db.set_manual_attribution_overrides(
                &[records[0].request_key.clone().unwrap()],
                None,
                1
            )
            .unwrap_err(),
            "cursor_fixed_account_source"
        );
        assert_eq!(
            records[0].provenance.as_ref().unwrap().charged_amount_usd,
            Some(0.05)
        );
        let json: String = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT event_json FROM cursor_usage_events LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!json.contains("NEVER STORE"));
        assert!(db.get_sync_export_data().unwrap().requests.is_empty());
        assert!(db.get_sync_export_data().unwrap().sessions.is_empty());
        assert!(db
            .publish_cursor_batch_with_metadata(
                &batch("overlap", "csv", 2),
                &events,
                HashMap::new()
            )
            .is_err());
        db.revoke_cursor_batch_with_metadata("csv1", HashMap::new())
            .unwrap();
        assert!(self::records(&db).is_empty());
    }

    #[test]
    fn cursor_failed_projection_rolls_back_batch_and_retains_last_success() {
        let (_temp, db) = db();
        db.publish_cursor_batch_with_metadata(&batch("old", "api", 1), &[event()], HashMap::new())
            .unwrap();
        db.conn.lock().unwrap().execute_batch("CREATE TRIGGER fail_cursor_projection BEFORE INSERT ON local_sessions WHEN NEW.tool='cursor' BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;").unwrap();
        let mut updated = event();
        updated.input_tokens = Some(100);
        let mut new_batch = batch("new", "api", 1);
        new_batch.imported_at_ms += 1;
        assert!(db
            .publish_cursor_batch_with_metadata(&new_batch, &[updated], HashMap::new())
            .is_err());
        assert_eq!(db.list_cursor_batches().unwrap().len(), 1);
        assert_eq!(records(&db)[0].input_tokens, 10);
    }

    #[test]
    fn cursor_complete_api_window_replaces_csv_without_double_counting() {
        let (_temp, db) = db();
        let mut metadata = HashMap::new();
        metadata.insert(
            "c1".into(),
            SessionMeta {
                session_id: "cursor::local-test::c1".into(),
                tool: "cursor".into(),
                cwd: Some("/tmp/test-project".into()),
                project_name: Some("test-project".into()),
                file_path: "test-db#c1".into(),
                source: "cursor_metadata".into(),
                ..Default::default()
            },
        );
        db.publish_cursor_batch_with_metadata(
            &batch("csv", "csv", 1),
            &[event()],
            metadata.clone(),
        )
        .unwrap();
        let mut corrected = event();
        corrected.input_tokens = Some(15);
        db.publish_cursor_batch_with_metadata(
            &batch("api", "api", 1),
            &[corrected],
            metadata.clone(),
        )
        .unwrap();
        assert_eq!(records(&db).len(), 1);
        assert_eq!(records(&db)[0].input_tokens, 15);
        assert_eq!(
            records(&db)[0].provenance.as_ref().unwrap().usage_basis,
            "account_json"
        );
        let sessions = db.get_cursor_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].0.cwd.as_deref(), Some("/tmp/test-project"));
        assert_eq!(sessions[0].0.topic, None);
        let mut empty = batch("api", "api", 0);
        empty.imported_at_ms += 1;
        db.publish_cursor_batch_with_metadata(&empty, &[], metadata)
            .unwrap();
        assert!(records(&db).is_empty());
        assert_eq!(db.get_cursor_sessions().unwrap().len(), 1);
        assert_eq!(db.get_cursor_sessions().unwrap()[0].0.message_count, 0);
    }

    #[test]
    fn cursor_cold_materialization_preserves_unknown_cost_time_and_source() {
        use super::super::UnifiedDayMaterializationState;
        let (temp, db) = db();
        let unknown = parse_json_event(
            &json!({"timestamp":1750000000123_i64,"model":"claude-sonnet","chargedCents":0}),
        )
        .unwrap();
        db.publish_cursor_batch_with_metadata(
            &batch("unknown", "csv", 1),
            &[unknown],
            HashMap::new(),
        )
        .unwrap();
        let records = records(&db);
        let mut fact = crate::unified_usage::MergedRequestFact::from_local(&records[0], None, 99.0);
        // A missing metered value must not become an estimate from model pricing.
        assert_eq!(fact.estimated_cost, 0.0);
        assert_eq!(fact.status_code, None);
        crate::unified_usage::apply_passive_attribution(
            &db,
            std::slice::from_mut(&mut fact),
            &crate::models::AppSettings::default(),
        )
        .unwrap();
        let date = "2025-06-15".to_string();
        let state = UnifiedDayMaterializationState {
            local_date: date.clone(),
            day_boundary_mode: "standard".into(),
            fact_count: 1,
            local_request_count: 1,
            local_max_sync_version: 1,
            local_max_timestamp: fact.timestamp_sec,
            remote_request_count: 0,
            remote_max_export_seq: 0,
            remote_max_timestamp: 0,
            proxy_record_count: 0,
            proxy_all_record_count: 0,
            proxy_max_timestamp_ms: 0,
            proxy_max_updated_at: 0,
            max_fact_timestamp_ms: fact.timestamp_ms,
            pricing_fingerprint: 42,
            is_finalized: true,
            finalized_at: Some(1),
            materialized_at: 2,
        };
        db.replace_unified_day_materialization(
            &date,
            &[(fact.canonical_request_key.clone(), fact.clone())],
            &state,
        )
        .unwrap();
        drop(db);
        let db = LocalUsageDatabase::new_with_path(&temp.path().join("usage.db")).unwrap();
        let mut cold = db
            .get_unified_facts_for_dates(
                std::slice::from_ref(&date),
                &ToolFilter::Tool("cursor".into()),
            )
            .unwrap();
        assert_eq!(cold.len(), 1);
        assert_eq!(cold[0].provenance, fact.provenance);
        assert_eq!(cold[0].timestamp_ms, 1750000000123);
        assert_eq!(cold[0].estimated_cost, 0.0);
        assert_eq!(cold[0].status_code, None);
        crate::unified_usage::apply_passive_attribution(
            &db,
            &mut cold,
            &crate::models::AppSettings::default(),
        )
        .unwrap();
        let filter = crate::models::SourceAwareSettings {
            active_source_filter: Some(crate::models::OFFICIAL_CURSOR_ACCOUNT_SOURCE_ID.into()),
            ..Default::default()
        }
        .build_filter();
        assert!(crate::unified_usage::matches_source_filter(
            &cold[0], &filter
        ));
        assert!(!crate::unified_usage::matches_source_filter(
            &cold[0],
            &crate::models::SourceFilter::Unknown {
                known_pairs: vec![]
            }
        ));
        let summary = db
            .get_unified_daily_summaries_between(&date, "2025-06-16")
            .unwrap();
        assert!(summary[0].has_partial_status_coverage);
        assert_eq!(summary[0].success_request_count, 0);
        assert_eq!(
            db.cursor_quality_counts_in_range(Some(fact.timestamp_sec), Some(fact.timestamp_sec))
                .unwrap(),
            (1, 1, 1)
        );
        assert_eq!(
            db.cursor_quality_counts_in_range(Some(fact.timestamp_sec + 1), None)
                .unwrap(),
            (0, 0, 0)
        );
    }

    #[test]
    fn cursor_projection_overflow_rolls_back_instead_of_wrapping_sqlite_counts() {
        let (_temp, db) = db();
        let mut huge = event();
        huge.input_tokens = Some(i64::MAX as u64 / 4);
        huge.output_tokens = huge.input_tokens;
        huge.cache_write_tokens = huge.input_tokens;
        huge.cache_read_tokens = huge.input_tokens;
        assert_eq!(
            db.publish_cursor_batch_with_metadata(
                &batch("huge", "csv", 2),
                &[huge.clone(), huge],
                HashMap::new()
            )
            .unwrap_err(),
            "cursor_invalid_token_count"
        );
        assert!(db.list_cursor_batches().unwrap().is_empty());
        assert!(records(&db).is_empty());
    }

    #[test]
    fn cursor_unknown_metadata_schema_retains_sessions_and_account_history() {
        let (temp, db) = db();
        let metadata = HashMap::from([(
            "local".into(),
            SessionMeta {
                tool: "cursor".into(),
                session_id: "cursor::local-fixture::local".into(),
                file_path: "fixture#local".into(),
                source: "cursor_metadata".into(),
                ..Default::default()
            },
        )]);
        db.publish_cursor_batch_with_metadata(&batch("history", "csv", 1), &[event()], metadata)
            .unwrap();
        let before = db
            .get_all_sessions(&ToolFilter::Tool("cursor".into()))
            .unwrap()
            .len();
        let cursor_path = temp.path().join("unknown.vscdb");
        Connection::open(&cursor_path)
            .unwrap()
            .execute_batch("CREATE TABLE ItemTable(key TEXT,value TEXT)")
            .unwrap();
        assert_eq!(
            db.refresh_cursor_metadata_from(Some(&cursor_path))
                .unwrap_err(),
            "cursor_schema_unsupported"
        );
        assert_eq!(
            db.get_all_sessions(&ToolFilter::Tool("cursor".into()))
                .unwrap()
                .len(),
            before
        );
        assert_eq!(records(&db).len(), 1);
        assert_eq!(
            db.get_local_sync_state("cursor_metadata_status")
                .unwrap()
                .as_deref(),
            Some("cursor_schema_unsupported")
        );
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CursorSyncState {
    pub account_key: String,
    pub status: String,
    pub error_code: Option<String>,
    pub last_attempt_ms: Option<i64>,
    pub last_success_ms: Option<i64>,
    pub next_retry_ms: Option<i64>,
    pub range_start_ms: Option<i64>,
    pub range_end_ms: Option<i64>,
    pub event_count: Option<usize>,
}

impl LocalUsageDatabase {
    pub(crate) fn get_cursor_sync_state(
        &self,
        account_key: &str,
    ) -> Result<Option<CursorSyncState>, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT account_key,status,error_code,last_attempt_ms,last_success_ms,next_retry_ms,range_start_ms,range_end_ms,event_count FROM cursor_usage_sync_state WHERE account_key=?1",[account_key],|row| Ok(CursorSyncState {
            account_key:row.get(0)?,status:row.get(1)?,error_code:row.get(2)?,last_attempt_ms:row.get(3)?,last_success_ms:row.get(4)?,next_retry_ms:row.get(5)?,range_start_ms:row.get(6)?,range_end_ms:row.get(7)?,event_count:row.get::<_,Option<i64>>(8)?.map(|count|count.max(0) as usize)
        })).optional().map_err(|_| "cursor_database_error".into())
    }

    pub(crate) fn record_cursor_sync_attempt(
        &self,
        account_key: &str,
        status: &str,
        failure: Option<&crate::cursor::client::CursorFailure>,
    ) -> Result<(), String> {
        self.conn.lock().unwrap().execute("INSERT INTO cursor_usage_sync_state(account_key,last_attempt_ms,status,error_code,next_retry_ms) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(account_key) DO UPDATE SET last_attempt_ms=excluded.last_attempt_ms,status=excluded.status,error_code=excluded.error_code,next_retry_ms=excluded.next_retry_ms",params![account_key,chrono::Utc::now().timestamp_millis(),status,failure.map(|failure| &failure.code),failure.and_then(|failure| failure.next_retry_ms)])
            .map_err(|_| "cursor_database_error")?;
        Ok(())
    }
}

impl LocalUsageDatabase {
    pub(crate) fn cursor_quality_counts(&self) -> Result<(u64, u64, u64), String> {
        self.cursor_quality_counts_in_range(None, None)
    }
    pub(crate) fn cursor_quality_counts_in_range(
        &self,
        start: Option<i64>,
        end: Option<i64>,
    ) -> Result<(u64, u64, u64), String> {
        self.conn.lock().unwrap().query_row("SELECT COUNT(*), COALESCE(SUM(CASE WHEN json_extract(provenance_json,'$.costBasis')='unknown' THEN 1 ELSE 0 END),0),COALESCE(SUM(CASE WHEN json_extract(provenance_json,'$.usageComplete')=0 THEN 1 ELSE 0 END),0) FROM local_request_facts WHERE tool='cursor' AND source_file_present=1 AND (?1 IS NULL OR timestamp>=?1) AND (?2 IS NULL OR timestamp<=?2)", params![start,end], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(|_| "cursor_database_error".into())
    }
}
