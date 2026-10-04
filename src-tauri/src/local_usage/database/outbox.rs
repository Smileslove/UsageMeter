use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::HashMap;

use super::{LocalUsageDatabase, SyncExportRequest, SyncExportSession, SyncOutboxBatch};

/// A reserved batch is considered abandoned after this interval.  The marker
/// is persisted before the network write, so a process crash cannot leave the
/// rows permanently invisible to the next reservation.
pub(crate) const SYNC_OUTBOX_RESERVATION_TIMEOUT_SECONDS: i64 = 15 * 60;

pub(super) fn enqueue_session_export_tx(
    tx: &Transaction<'_>,
    origin_device_id: &str,
    session: &SyncExportSession,
    queued_at: i64,
) -> Result<(), String> {
    let payload = serde_json::to_string(session)
        .map_err(|e| format!("Failed to serialize sync session outbox payload: {}", e))?;
    tx.execute(
        "INSERT INTO sync_outbox_session_events (
            session_event_id, origin_device_id, session_id, payload_json,
            session_version, queued_at, batched_seq, uploaded_at
         ) VALUES (?1, ?2, ?3, ?4, 1, ?5, NULL, NULL)
         ON CONFLICT(session_event_id) DO UPDATE SET
            payload_json = excluded.payload_json,
            session_version = CASE
                WHEN sync_outbox_session_events.payload_json != excluded.payload_json
                THEN sync_outbox_session_events.session_version + 1
                ELSE sync_outbox_session_events.session_version
            END,
            queued_at = CASE
                WHEN sync_outbox_session_events.payload_json != excluded.payload_json
                THEN excluded.queued_at
                ELSE sync_outbox_session_events.queued_at
            END,
            batched_seq = CASE
                WHEN sync_outbox_session_events.payload_json != excluded.payload_json
                THEN NULL
                ELSE sync_outbox_session_events.batched_seq
            END,
            uploaded_at = CASE
                WHEN sync_outbox_session_events.payload_json != excluded.payload_json
                THEN NULL
                ELSE sync_outbox_session_events.uploaded_at
                END,
            discarded_at = NULL,
            discard_reason = NULL",
        params![
            format!("{}:{}", origin_device_id, session.session_id),
            origin_device_id,
            session.session_id.as_str(),
            payload.as_str(),
            queued_at
        ],
    )
    .map_err(|e| format!("Failed to enqueue sync session outbox payload: {}", e))?;
    Ok(())
}

pub(super) fn enqueue_request_export_tx(
    tx: &Transaction<'_>,
    origin_device_id: &str,
    request: &SyncExportRequest,
    queued_at: i64,
) -> Result<(), String> {
    let payload = serde_json::to_string(request)
        .map_err(|e| format!("Failed to serialize sync request outbox payload: {}", e))?;
    tx.execute(
        "INSERT INTO sync_outbox_request_events (
            event_id, origin_device_id, request_key, payload_json,
            event_version, queued_at, batched_seq, uploaded_at
         ) VALUES (?1, ?2, ?3, ?4, 1, ?5, NULL, NULL)
         ON CONFLICT(event_id) DO UPDATE SET
            payload_json = excluded.payload_json,
            request_key = excluded.request_key,
            event_version = excluded.event_version,
            queued_at = excluded.queued_at,
            batched_seq = NULL,
            uploaded_at = NULL,
            discarded_at = NULL,
            discard_reason = NULL",
        params![
            format!("{}:{}", origin_device_id, request.request_key),
            origin_device_id,
            request.request_key.as_str(),
            payload.as_str(),
            queued_at
        ],
    )
    .map_err(|e| format!("Failed to enqueue sync request outbox payload: {}", e))?;
    Ok(())
}

impl LocalUsageDatabase {
    /// Releases rows reserved by a previous process that crashed during the
    /// upload window.  The timestamp is intentionally a coarse global marker:
    /// startup runs before a new sync job can reserve another batch, therefore
    /// releasing every still-unuploaded batch is safe and idempotent.
    pub fn recover_stale_sync_outbox_reservations(&self) -> Result<u64, String> {
        let now = chrono::Utc::now().timestamp();
        let reserved_at = self
            .get_local_sync_state("last_sync_outbox_reserved_at")?
            .and_then(|value| value.parse::<i64>().ok());
        let Some(reserved_at) = reserved_at else {
            return Ok(0);
        };
        if now.saturating_sub(reserved_at) < SYNC_OUTBOX_RESERVATION_TIMEOUT_SECONDS {
            return Ok(0);
        }

        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start stale sync outbox recovery: {e}"))?;
        let request_count = tx
            .execute(
                "UPDATE sync_outbox_request_events
                 SET batched_seq = NULL
                WHERE batched_seq IS NOT NULL AND uploaded_at IS NULL",
                [],
            )
            .map_err(|e| format!("Failed to release stale request outbox rows: {e}"))?;
        let session_count = tx
            .execute(
                "UPDATE sync_outbox_session_events
                 SET batched_seq = NULL
                WHERE batched_seq IS NOT NULL AND uploaded_at IS NULL",
                [],
            )
            .map_err(|e| format!("Failed to release stale session outbox rows: {e}"))?;
        let recovered = (request_count + session_count) as u64;
        Self::upsert_sync_state(
            &tx,
            "last_sync_outbox_recovery_count",
            &recovered.to_string(),
            now,
        )?;
        // Clear the marker once all abandoned reservations have been released;
        // a later reservation will write a fresh timestamp.
        Self::upsert_sync_state(&tx, "last_sync_outbox_reserved_at", "0", now)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit stale sync outbox recovery: {e}"))?;
        Ok(recovered)
    }

    /// Applies the user-facing sync policy to the durable outbox.
    /// Disabled sync means no pending upload queue; local facts remain untouched.
    pub fn reconcile_sync_policy(&self, enabled: bool) -> Result<u64, String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start sync policy reconciliation: {e}"))?;
        let previous = tx
            .query_row(
                "SELECT state_value FROM local_sync_state WHERE state_key = 'sync_policy_enabled'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read sync policy state: {e}"))?;
        let mut removed = 0_u64;

        if !enabled {
            removed += tx
                .execute(
                    "DELETE FROM sync_outbox_request_events
                     WHERE uploaded_at IS NULL AND batched_seq IS NULL",
                    [],
                )
                .map_err(|e| format!("Failed to clear pending sync request outbox: {e}"))?
                as u64;
            removed += tx
                .execute(
                    "DELETE FROM sync_outbox_session_events
                     WHERE uploaded_at IS NULL AND batched_seq IS NULL",
                    [],
                )
                .map_err(|e| format!("Failed to clear pending sync session outbox: {e}"))?
                as u64;
            Self::upsert_sync_state(&tx, "sync_policy_enabled", "0", now)?;
            Self::upsert_sync_state(&tx, "sync_snapshot_required", "0", now)?;
        } else {
            let became_enabled = previous.as_deref() == Some("0");
            let current_generation = tx
                .query_row(
                    "SELECT state_value FROM local_sync_state WHERE state_key = 'sync_generation'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .ok()
                .and_then(|value| value.parse::<i64>().ok());
            if became_enabled || current_generation.is_none() {
                let next_generation = current_generation.unwrap_or(0).saturating_add(1);
                Self::upsert_sync_state(&tx, "sync_generation", &next_generation.to_string(), now)?;
                let needs_snapshot = became_enabled
                    || tx
                        .query_row(
                            "SELECT COALESCE(MAX(batch_seq), 0) FROM sync_batch_history WHERE status = 'uploaded'",
                            [],
                            |row| row.get::<_, i64>(0),
                        )
                        .unwrap_or(0)
                        > 0;
                Self::upsert_sync_state(
                    &tx,
                    "sync_snapshot_required",
                    if needs_snapshot { "1" } else { "0" },
                    now,
                )?;
            }
            Self::upsert_sync_state(&tx, "sync_policy_enabled", "1", now)?;
        }

        Self::upsert_sync_state(
            &tx,
            "last_sync_policy_reconcile_count",
            &removed.to_string(),
            now,
        )?;
        Self::upsert_sync_state(&tx, "sync_policy_reconcile_pending", "0", now)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit sync policy reconciliation: {e}"))?;
        Ok(removed)
    }

    pub fn mark_sync_policy_reconcile_pending(&self, pending: bool) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start sync policy marker update: {e}"))?;
        Self::upsert_sync_state(
            &tx,
            "sync_policy_reconcile_pending",
            if pending { "1" } else { "0" },
            now,
        )?;
        tx.commit()
            .map_err(|e| format!("Failed to commit sync policy marker update: {e}"))
    }

    /// Completes the enabled-policy transition for legacy databases. A disabled
    /// generation explicitly requests a new snapshot before incremental batches resume.
    pub fn prepare_sync_generation_for_enabled(
        &self,
        origin_device_id: &str,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start sync generation preparation: {e}"))?;
        let previous = tx
            .query_row(
                "SELECT state_value FROM local_sync_state WHERE state_key = 'sync_policy_enabled'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read sync generation policy: {e}"))?;
        let generation = tx
            .query_row(
                "SELECT state_value FROM local_sync_state WHERE state_key = 'sync_generation'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .and_then(|value| value.parse::<i64>().ok());
        let previous_origin = tx
            .query_row(
                "SELECT state_value FROM local_sync_state WHERE state_key = 'sync_origin_device_id'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok();
        let origin_changed = previous_origin.as_deref() != Some(origin_device_id);
        if origin_changed {
            let old_origin = previous_origin
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("");
            let mut removed = 0_u64;
            if !old_origin.is_empty() {
                removed += tx
                    .execute(
                        "DELETE FROM sync_outbox_request_events
                         WHERE origin_device_id = ?1 AND uploaded_at IS NULL",
                        [old_origin],
                    )
                    .map_err(|e| format!("Failed to clear old-origin request outbox: {e}"))?
                    as u64;
                removed += tx
                    .execute(
                        "DELETE FROM sync_outbox_session_events
                         WHERE origin_device_id = ?1 AND uploaded_at IS NULL",
                        [old_origin],
                    )
                    .map_err(|e| format!("Failed to clear old-origin session outbox: {e}"))?
                    as u64;
            }
            Self::upsert_sync_state(
                &tx,
                "last_sync_origin_cleanup_count",
                &removed.to_string(),
                now,
            )?;
            if !old_origin.is_empty() {
                Self::upsert_sync_state(&tx, "last_sync_origin_cleanup_origin", old_origin, now)?;
            }
        }
        if previous.as_deref() == Some("0") || origin_changed {
            let current_generation = tx
                .query_row(
                    "SELECT COALESCE(state_value, '0') FROM local_sync_state WHERE state_key = 'sync_generation'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .ok()
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(0);
            Self::upsert_sync_state(
                &tx,
                "sync_generation",
                &current_generation.saturating_add(1).to_string(),
                now,
            )?;
            Self::upsert_sync_state(&tx, "sync_snapshot_required", "1", now)?;
        } else if generation.is_none() {
            Self::upsert_sync_state(&tx, "sync_generation", "1", now)?;
            let last_uploaded = tx
                .query_row(
                    "SELECT COALESCE(MAX(batch_seq), 0) FROM sync_batch_history WHERE status = 'uploaded'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap_or(0);
            Self::upsert_sync_state(
                &tx,
                "sync_snapshot_required",
                if last_uploaded > 0 { "1" } else { "0" },
                now,
            )?;
        }
        Self::upsert_sync_state(&tx, "sync_policy_enabled", "1", now)?;
        Self::upsert_sync_state(&tx, "sync_origin_device_id", origin_device_id, now)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit sync generation preparation: {e}"))?;
        Ok(())
    }

    /// Repairs pending local outbox rows whose legacy request key was reconstructed
    /// differently from the persisted fact key. In-flight batches are left untouched.
    pub fn repair_pending_request_keys(&self, origin_device_id: &str) -> Result<u64, String> {
        let (pending_count, merge_generation, last_repair_generation, repair_algorithm) = {
            let conn = self.conn.lock().unwrap();
            let pending_count = conn
                .query_row(
                    "SELECT COUNT(*) FROM sync_outbox_request_events
                     WHERE origin_device_id = ?1 AND uploaded_at IS NULL
                       AND batched_seq IS NULL AND discarded_at IS NULL",
                    [origin_device_id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|e| format!("Failed to inspect pending outbox requests: {e}"))?;
            let merge_generation = conn
                .query_row(
                    "SELECT COALESCE(state_value, '0') FROM local_sync_state WHERE state_key = 'merge_cache_generation'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .ok();
            let last_repair_generation = conn
                .query_row(
                    "SELECT state_value FROM local_sync_state WHERE state_key = 'last_outbox_request_key_repair_generation'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .ok();
            let repair_algorithm = conn
                .query_row(
                    "SELECT state_value FROM local_sync_state WHERE state_key = 'outbox_repair_algorithm'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .ok();
            (
                pending_count,
                merge_generation,
                last_repair_generation,
                repair_algorithm,
            )
        };
        if pending_count == 0
            || repair_algorithm.as_deref() == Some("tombstone-v1")
                && merge_generation.is_some()
                && merge_generation == last_repair_generation
        {
            return Ok(0);
        }
        let export = self.get_sync_export_data()?;
        let by_dedupe: HashMap<(String, String), SyncExportRequest> = export
            .requests
            .into_iter()
            .map(|request| ((request.tool.clone(), request.dedupe_key.clone()), request))
            .collect();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start outbox request-key repair: {e}"))?;
        let mut stmt = tx
            .prepare(
                "SELECT event_id, origin_device_id, request_key, payload_json,
                        event_version, queued_at
                 FROM sync_outbox_request_events
                 WHERE origin_device_id = ?1 AND uploaded_at IS NULL
                   AND batched_seq IS NULL AND discarded_at IS NULL",
            )
            .map_err(|e| format!("Failed to prepare outbox request-key repair query: {e}"))?;
        let rows: Vec<(String, String, String, String, i64, i64)> = stmt
            .query_map([origin_device_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .map_err(|e| format!("Failed to query outbox request-key repair rows: {e}"))?
            .collect::<Result<_, _>>()
            .map_err(|e| format!("Failed to read outbox request-key repair row: {e}"))?;
        drop(stmt);

        let mut repaired = 0_u64;
        for (old_event_id, row_origin, old_key, payload_json, event_version, queued_at) in rows {
            let Ok(mut payload) = serde_json::from_str::<SyncExportRequest>(&payload_json) else {
                tx.execute(
                    "UPDATE sync_outbox_request_events
                     SET discarded_at = ?2, discard_reason = 'invalid_payload'
                     WHERE event_id = ?1 AND uploaded_at IS NULL",
                    params![old_event_id, chrono::Utc::now().timestamp()],
                )
                .map_err(|e| format!("Failed to quarantine invalid repair payload: {e}"))?;
                continue;
            };
            let Some(current) = by_dedupe.get(&(payload.tool.clone(), payload.dedupe_key.clone()))
            else {
                // The local fact was purged or soft-deleted. Preserve a compact,
                // ordered tombstone so the remote copy cannot survive forever.
                payload.deleted = true;
                payload.project_key = None;
                payload.timestamp = 0;
                payload.message_id = None;
                payload.model.clear();
                payload.input_tokens = 0;
                payload.output_tokens = 0;
                payload.cache_create_tokens = 0;
                payload.cache_read_tokens = 0;
                payload.total_tokens = 0;
                payload.request_count = 0;
                payload.explicit_estimated_cost = None;
                payload.is_subagent = false;
                payload.source_kind = "local_usage_tombstone".to_string();
                let tombstone = serde_json::to_string(&payload)
                    .map_err(|e| format!("Failed to serialize outbox tombstone: {e}"))?;
                tx.execute(
                    "UPDATE sync_outbox_request_events
                     SET payload_json = ?2, discarded_at = NULL, discard_reason = NULL
                     WHERE event_id = ?1 AND uploaded_at IS NULL",
                    params![old_event_id, tombstone],
                )
                .map_err(|e| format!("Failed to preserve outbox tombstone: {e}"))?;
                repaired += 1;
                continue;
            };
            if current.request_key == old_key && current.deleted == payload.deleted {
                continue;
            }
            if current.request_key == old_key {
                let new_payload = serde_json::to_string(current)
                    .map_err(|e| format!("Failed to serialize repaired tombstone: {e}"))?;
                tx.execute(
                    "UPDATE sync_outbox_request_events
                     SET payload_json = ?2, discarded_at = NULL, discard_reason = NULL
                     WHERE event_id = ?1 AND uploaded_at IS NULL",
                    params![old_event_id, new_payload],
                )
                .map_err(|e| format!("Failed to update repaired tombstone: {e}"))?;
                repaired += 1;
                continue;
            }
            let new_payload = serde_json::to_string(current)
                .map_err(|e| format!("Failed to serialize repaired outbox payload: {e}"))?;
            let new_event_id = format!("{}:{}", row_origin, current.request_key);
            tx.execute(
                "INSERT INTO sync_outbox_request_events (
                    event_id, origin_device_id, request_key, payload_json,
                    event_version, queued_at, batched_seq, uploaded_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL)
                 ON CONFLICT(event_id) DO UPDATE SET
                    request_key = excluded.request_key,
                    payload_json = excluded.payload_json,
                    event_version = MAX(sync_outbox_request_events.event_version, excluded.event_version),
                    queued_at = MIN(sync_outbox_request_events.queued_at, excluded.queued_at),
                    batched_seq = NULL,
                    uploaded_at = NULL",
                params![
                    new_event_id,
                    row_origin,
                    current.request_key,
                    new_payload,
                    event_version,
                    queued_at
                ],
            )
            .map_err(|e| format!("Failed to insert repaired outbox request row: {e}"))?;
            tx.execute(
                "DELETE FROM sync_outbox_request_events WHERE event_id = ?1",
                [old_event_id],
            )
            .map_err(|e| format!("Failed to remove legacy outbox request row: {e}"))?;
            repaired += 1;
        }
        if let Some(generation) = merge_generation {
            let now = chrono::Utc::now().timestamp();
            Self::upsert_sync_state(
                &tx,
                "last_outbox_request_key_repair_generation",
                &generation,
                now,
            )?;
            Self::upsert_sync_state(&tx, "outbox_repair_algorithm", "tombstone-v1", now)?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit outbox request-key repair: {e}"))?;
        Ok(repaired)
    }

    pub fn reserve_sync_outbox_batch(
        &self,
        origin_device_id: &str,
        batch_seq: i64,
        max_request_events: usize,
        max_session_events: usize,
    ) -> Result<SyncOutboxBatch, String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start sync outbox reservation: {}", e))?;

        let mut request_ids = Vec::new();
        let mut request_events = Vec::new();
        {
            let mut stmt = tx
                .prepare(
                    "SELECT event_id, payload_json
                     FROM sync_outbox_request_events
                     WHERE origin_device_id = ?1 AND uploaded_at IS NULL
                       AND batched_seq IS NULL AND discarded_at IS NULL
                     ORDER BY queued_at ASC
                     LIMIT ?2",
                )
                .map_err(|e| format!("Failed to prepare sync request outbox query: {}", e))?;
            let rows = stmt
                .query_map(
                    params![origin_device_id, max_request_events as i64],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(|e| format!("Failed to query sync request outbox: {}", e))?;
            let rows: Vec<(String, String)> = rows
                .collect::<Result<_, _>>()
                .map_err(|e| format!("Failed to read sync request outbox row: {}", e))?;
            drop(stmt);
            for (event_id, payload_json) in rows {
                match serde_json::from_str::<SyncExportRequest>(&payload_json) {
                    Ok(payload) => {
                        request_ids.push(event_id);
                        request_events.push(payload);
                    }
                    Err(error) => {
                        tx.execute(
                            "UPDATE sync_outbox_request_events
                             SET discarded_at = ?2, discard_reason = ?3
                             WHERE event_id = ?1 AND uploaded_at IS NULL",
                            params![event_id, now, format!("invalid_payload:{error}")],
                        )
                        .map_err(|e| {
                            format!("Failed to quarantine invalid request outbox row: {e}")
                        })?;
                    }
                }
            }
        }

        let mut session_ids = Vec::new();
        let mut session_events = Vec::new();
        {
            let mut stmt = tx
                .prepare(
                    "SELECT session_event_id, payload_json
                     FROM sync_outbox_session_events
                     WHERE origin_device_id = ?1 AND uploaded_at IS NULL
                       AND batched_seq IS NULL AND discarded_at IS NULL
                     ORDER BY queued_at ASC
                     LIMIT ?2",
                )
                .map_err(|e| format!("Failed to prepare sync session outbox query: {}", e))?;
            let rows = stmt
                .query_map(
                    params![origin_device_id, max_session_events as i64],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(|e| format!("Failed to query sync session outbox: {}", e))?;
            let rows: Vec<(String, String)> = rows
                .collect::<Result<_, _>>()
                .map_err(|e| format!("Failed to read sync session outbox row: {}", e))?;
            drop(stmt);
            for (event_id, payload_json) in rows {
                match serde_json::from_str::<SyncExportSession>(&payload_json) {
                    Ok(payload) => {
                        session_ids.push(event_id);
                        session_events.push(payload);
                    }
                    Err(error) => {
                        tx.execute(
                            "UPDATE sync_outbox_session_events
                             SET discarded_at = ?2, discard_reason = ?3
                             WHERE session_event_id = ?1 AND uploaded_at IS NULL",
                            params![event_id, now, format!("invalid_payload:{error}")],
                        )
                        .map_err(|e| {
                            format!("Failed to quarantine invalid session outbox row: {e}")
                        })?;
                    }
                }
            }
        }

        for event_id in &request_ids {
            tx.execute(
                "UPDATE sync_outbox_request_events
                 SET batched_seq = ?2
                 WHERE event_id = ?1",
                params![event_id, batch_seq],
            )
            .map_err(|e| format!("Failed to reserve sync request outbox row: {}", e))?;
        }
        for event_id in &session_ids {
            tx.execute(
                "UPDATE sync_outbox_session_events
                 SET batched_seq = ?2
                 WHERE session_event_id = ?1",
                params![event_id, batch_seq],
            )
            .map_err(|e| format!("Failed to reserve sync session outbox row: {}", e))?;
        }

        Self::upsert_sync_state(&tx, "last_sync_outbox_reserved_at", &now.to_string(), now)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit sync outbox reservation: {}", e))?;

        Ok(SyncOutboxBatch {
            request_events,
            session_events,
        })
    }

    pub fn seed_sync_outbox_from_local(&self, origin_device_id: &str) -> Result<(), String> {
        let snapshot_required = self
            .get_local_sync_state("sync_snapshot_required")?
            .as_deref()
            == Some("1");
        if self.get_last_uploaded_batch_seq()? > 0 && !snapshot_required {
            return Ok(());
        }

        let export = self.get_sync_export_data()?;
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start sync outbox seed: {}", e))?;

        // A disabled period intentionally discards pending upload rows. On the
        // next enable, rebuild the current-device queue from local facts rather
        // than relying on event ids that may already be marked uploaded.
        if snapshot_required {
            tx.execute(
                "DELETE FROM sync_outbox_request_events WHERE origin_device_id = ?1",
                [origin_device_id],
            )
            .map_err(|e| format!("Failed to reset sync request snapshot outbox: {e}"))?;
            tx.execute(
                "DELETE FROM sync_outbox_session_events WHERE origin_device_id = ?1",
                [origin_device_id],
            )
            .map_err(|e| format!("Failed to reset sync session snapshot outbox: {e}"))?;
        }

        for session in export.sessions {
            let payload = serde_json::to_string(&session)
                .map_err(|e| format!("Failed to serialize sync session seed payload: {}", e))?;
            tx.execute(
                "INSERT INTO sync_outbox_session_events (
                    session_event_id, origin_device_id, session_id, payload_json,
                    session_version, queued_at, batched_seq, uploaded_at
                 ) VALUES (?1, ?2, ?3, ?4, 1, ?5, NULL, NULL)
                 ON CONFLICT(session_event_id) DO NOTHING",
                params![
                    format!("{}:{}", origin_device_id, session.session_id),
                    origin_device_id,
                    session.session_id.as_str(),
                    payload.as_str(),
                    now
                ],
            )
            .map_err(|e| format!("Failed to seed sync session outbox: {}", e))?;
        }

        for request in export.requests {
            let payload = serde_json::to_string(&request)
                .map_err(|e| format!("Failed to serialize sync request seed payload: {}", e))?;
            tx.execute(
                "INSERT INTO sync_outbox_request_events (
                    event_id, origin_device_id, request_key, payload_json,
                    event_version, queued_at, batched_seq, uploaded_at
                 ) VALUES (?1, ?2, ?3, ?4, 1, ?5, NULL, NULL)
                 ON CONFLICT(event_id) DO NOTHING",
                params![
                    format!("{}:{}", origin_device_id, request.request_key),
                    origin_device_id,
                    request.request_key.as_str(),
                    payload.as_str(),
                    now
                ],
            )
            .map_err(|e| format!("Failed to seed sync request outbox: {}", e))?;
        }

        if snapshot_required {
            Self::upsert_sync_state(&tx, "sync_snapshot_required", "0", now)?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit sync outbox seed: {}", e))?;
        Ok(())
    }

    pub fn release_sync_outbox_batch(&self, batch_seq: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start sync outbox release: {}", e))?;
        tx.execute(
            "UPDATE sync_outbox_request_events
             SET batched_seq = NULL
             WHERE batched_seq = ?1 AND uploaded_at IS NULL",
            params![batch_seq],
        )
        .map_err(|e| format!("Failed to release sync request outbox rows: {}", e))?;
        tx.execute(
            "UPDATE sync_outbox_session_events
             SET batched_seq = NULL
             WHERE batched_seq = ?1 AND uploaded_at IS NULL",
            params![batch_seq],
        )
        .map_err(|e| format!("Failed to release sync session outbox rows: {}", e))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit sync outbox release: {}", e))?;
        Ok(())
    }

    pub fn mark_sync_outbox_batch_uploaded(
        &self,
        batch_seq: i64,
        remote_path: &str,
        request_event_count: usize,
        session_event_count: usize,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start sync outbox upload mark: {}", e))?;
        tx.execute(
            "UPDATE sync_outbox_request_events
             SET uploaded_at = ?2
             WHERE batched_seq = ?1 AND uploaded_at IS NULL",
            params![batch_seq, now],
        )
        .map_err(|e| format!("Failed to mark sync request outbox rows uploaded: {}", e))?;
        tx.execute(
            "UPDATE sync_outbox_session_events
             SET uploaded_at = ?2
             WHERE batched_seq = ?1 AND uploaded_at IS NULL",
            params![batch_seq, now],
        )
        .map_err(|e| format!("Failed to mark sync session outbox rows uploaded: {}", e))?;
        tx.execute(
            "INSERT INTO sync_batch_history (
                batch_seq, request_event_count, session_event_count, exported_at, remote_path, status
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'uploaded')
             ON CONFLICT(batch_seq) DO UPDATE SET
                request_event_count = excluded.request_event_count,
                session_event_count = excluded.session_event_count,
                exported_at = excluded.exported_at,
                remote_path = excluded.remote_path,
                status = excluded.status",
            params![
                batch_seq,
                request_event_count as i64,
                session_event_count as i64,
                now,
                remote_path
            ],
        )
        .map_err(|e| format!("Failed to record sync batch history: {}", e))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit sync outbox upload mark: {}", e))?;
        Ok(())
    }

    /// 删除所有已成功上传的 outbox 事件行，防止表无限增长。
    /// 同时清理 sync_batch_history 中超出保留窗口的历史记录。
    /// 每次 sync 成功后调用。
    pub fn prune_uploaded_outbox(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start prune outbox transaction: {}", e))?;
        tx.execute(
            "DELETE FROM sync_outbox_request_events WHERE uploaded_at IS NOT NULL",
            [],
        )
        .map_err(|e| format!("Failed to prune uploaded request outbox: {}", e))?;
        tx.execute(
            "DELETE FROM sync_outbox_session_events WHERE uploaded_at IS NOT NULL",
            [],
        )
        .map_err(|e| format!("Failed to prune uploaded session outbox: {}", e))?;
        tx.execute(
            "DELETE FROM sync_outbox_request_events WHERE discarded_at IS NOT NULL",
            [],
        )
        .map_err(|e| format!("Failed to prune discarded request outbox: {}", e))?;
        tx.execute(
            "DELETE FROM sync_outbox_session_events WHERE discarded_at IS NOT NULL",
            [],
        )
        .map_err(|e| format!("Failed to prune discarded session outbox: {}", e))?;
        // 保留最新 200 条 batch 历史记录，其余删除
        tx.execute(
            "DELETE FROM sync_batch_history
             WHERE batch_seq < (
                 SELECT COALESCE(MIN(batch_seq), 0)
                 FROM (
                     SELECT batch_seq FROM sync_batch_history
                     ORDER BY batch_seq DESC
                     LIMIT 200
                 )
             )",
            [],
        )
        .map_err(|e| format!("Failed to prune sync batch history: {}", e))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit prune outbox transaction: {}", e))?;
        Ok(())
    }

    pub fn get_last_uploaded_batch_seq(&self) -> Result<i64, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(MAX(batch_seq), 0) FROM sync_batch_history WHERE status = 'uploaded'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| format!("Failed to read last uploaded batch seq: {}", e))
    }

    pub fn get_import_cursor(&self, device_id: &str) -> Result<i64, String> {
        Ok(self.get_import_cursor_state(device_id)?.0)
    }

    pub fn get_import_cursor_state(
        &self,
        device_id: &str,
    ) -> Result<(i64, Option<String>), String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT last_imported_batch_seq, last_seen_instance_id
             FROM sync_device_cursors WHERE device_id = ?1",
            params![device_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map(|value| value.unwrap_or((0, None)))
        .map_err(|e| format!("Failed to read sync device cursor: {}", e))
    }

    pub fn upsert_import_cursor(
        &self,
        device_id: &str,
        instance_id: Option<&str>,
        batch_seq: i64,
        status: &str,
        last_error: Option<&str>,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_device_cursors (
                device_id, last_imported_batch_seq, last_imported_snapshot_seq,
                last_seen_instance_id, last_seen_at, last_status, last_error
             ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6)
             ON CONFLICT(device_id) DO UPDATE SET
                last_imported_batch_seq = MAX(sync_device_cursors.last_imported_batch_seq, excluded.last_imported_batch_seq),
                last_seen_instance_id = COALESCE(excluded.last_seen_instance_id, sync_device_cursors.last_seen_instance_id),
                last_seen_at = excluded.last_seen_at,
                last_status = excluded.last_status,
                last_error = excluded.last_error",
            params![device_id, batch_seq, instance_id, now, status, last_error],
        )
        .map_err(|e| format!("Failed to upsert sync device cursor: {}", e))?;
        Ok(())
    }

    /// Reset a remote device cursor when its incremental batches were pruned
    /// before this consumer could read the corresponding snapshot.
    pub fn reset_import_cursor(
        &self,
        device_id: &str,
        instance_id: Option<&str>,
        status: &str,
        last_error: Option<&str>,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_device_cursors (
                device_id, last_imported_batch_seq, last_imported_snapshot_seq,
                last_seen_instance_id, last_seen_at, last_status, last_error
             ) VALUES (?1, 0, NULL, ?2, ?3, ?4, ?5)
             ON CONFLICT(device_id) DO UPDATE SET
                last_imported_batch_seq = 0,
                last_imported_snapshot_seq = NULL,
                last_seen_instance_id = COALESCE(excluded.last_seen_instance_id, sync_device_cursors.last_seen_instance_id),
                last_seen_at = excluded.last_seen_at,
                last_status = excluded.last_status,
                last_error = excluded.last_error",
            params![device_id, instance_id, now, status, last_error],
        )
        .map_err(|e| format!("Failed to reset sync device cursor: {}", e))?;
        Ok(())
    }
}
