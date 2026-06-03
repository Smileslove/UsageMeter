use rusqlite::{params, OptionalExtension, Transaction};

use super::{LocalUsageDatabase, SyncExportRequest, SyncExportSession, SyncOutboxBatch};

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
            END",
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
            uploaded_at = NULL",
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
                     WHERE origin_device_id = ?1 AND uploaded_at IS NULL AND batched_seq IS NULL
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
            for row in rows {
                let (event_id, payload_json) =
                    row.map_err(|e| format!("Failed to read sync request outbox row: {}", e))?;
                let payload: SyncExportRequest = serde_json::from_str(&payload_json)
                    .map_err(|e| format!("Failed to parse sync request outbox payload: {}", e))?;
                request_ids.push(event_id);
                request_events.push(payload);
            }
        }

        let mut session_ids = Vec::new();
        let mut session_events = Vec::new();
        {
            let mut stmt = tx
                .prepare(
                    "SELECT session_event_id, payload_json
                     FROM sync_outbox_session_events
                     WHERE origin_device_id = ?1 AND uploaded_at IS NULL AND batched_seq IS NULL
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
            for row in rows {
                let (event_id, payload_json) =
                    row.map_err(|e| format!("Failed to read sync session outbox row: {}", e))?;
                let payload: SyncExportSession = serde_json::from_str(&payload_json)
                    .map_err(|e| format!("Failed to parse sync session outbox payload: {}", e))?;
                session_ids.push(event_id);
                session_events.push(payload);
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
        if self.get_last_uploaded_batch_seq()? > 0 {
            return Ok(());
        }

        let export = self.get_sync_export_data()?;
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start sync outbox seed: {}", e))?;

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
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT last_imported_batch_seq FROM sync_device_cursors WHERE device_id = ?1",
            params![device_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|value| value.unwrap_or(0))
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
}
