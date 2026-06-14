use crate::models::ToolFilter;
use crate::session::{LocalRequestRecord, SessionMeta};
use rusqlite::params;
use std::collections::HashSet;

use super::{
    LocalUsageDatabase, RemoteSyncDevice, SyncExportData, SyncExportRequest, SyncExportSession,
};

impl LocalUsageDatabase {
    pub fn get_sync_export_data(&self) -> Result<SyncExportData, String> {
        let conn = self.conn.lock().unwrap();

        let mut session_stmt = conn
            .prepare(
                "SELECT session_id, tool, project_key, project_name, start_time, end_time,
                        request_count, total_input_tokens, total_output_tokens,
                        total_cache_create_tokens, total_cache_read_tokens, total_tokens,
                        model_list_json
                 FROM local_sessions
                 ORDER BY end_time ASC",
            )
            .map_err(|e| format!("Failed to prepare sync session export: {}", e))?;
        let session_rows = session_stmt
            .query_map([], |row| {
                let model_list_json: String = row.get(12)?;
                Ok(SyncExportSession {
                    session_id: row.get(0)?,
                    tool: row.get(1)?,
                    project_key: row.get(2)?,
                    project_name: row.get(3)?,
                    start_time: row.get(4)?,
                    end_time: row.get(5)?,
                    request_count: row.get::<_, i64>(6)? as u64,
                    total_input_tokens: row.get::<_, i64>(7)? as u64,
                    total_output_tokens: row.get::<_, i64>(8)? as u64,
                    total_cache_create_tokens: row.get::<_, i64>(9)? as u64,
                    total_cache_read_tokens: row.get::<_, i64>(10)? as u64,
                    total_tokens: row.get::<_, i64>(11)? as u64,
                    model_list: serde_json::from_str(&model_list_json).unwrap_or_default(),
                })
            })
            .map_err(|e| format!("Failed to query sync session export: {}", e))?;

        let mut sessions = Vec::new();
        for row in session_rows {
            sessions.push(row.map_err(|e| format!("Failed to read sync session row: {}", e))?);
        }

        let mut request_stmt = conn
            .prepare(
                "SELECT session_id, tool, project_key, timestamp, message_id, dedupe_key,
                        model, input_tokens, output_tokens, cache_create_tokens,
                        cache_read_tokens, total_tokens, request_count, explicit_estimated_cost,
                        is_subagent
                 FROM local_request_facts
                 ORDER BY timestamp ASC",
            )
            .map_err(|e| format!("Failed to prepare sync request export: {}", e))?;
        let request_rows = request_stmt
            .query_map([], |row| {
                let session_id: String = row.get(0)?;
                let tool: String = row.get(1)?;
                let timestamp: i64 = row.get(3)?;
                let message_id: Option<String> = row.get(4)?;
                let model: String = row.get(6)?;
                let input_tokens = row.get::<_, i64>(7)? as u64;
                let output_tokens = row.get::<_, i64>(8)? as u64;
                let total_tokens = row.get::<_, i64>(11)? as u64;
                let request_key = match message_id.as_deref() {
                    Some(value) if !value.trim().is_empty() => format!("{}:{}", tool, value),
                    _ => format!(
                        "{}:{}:{}:{}:{}:{}:{}:{}:{}",
                        tool,
                        session_id,
                        timestamp,
                        model,
                        input_tokens,
                        output_tokens,
                        row.get::<_, i64>(9)? as u64,
                        row.get::<_, i64>(10)? as u64,
                        total_tokens
                    ),
                };

                Ok(SyncExportRequest {
                    request_key,
                    session_id,
                    tool,
                    project_key: row.get(2)?,
                    timestamp,
                    message_id,
                    dedupe_key: row.get(5)?,
                    model,
                    input_tokens,
                    output_tokens,
                    cache_create_tokens: row.get::<_, i64>(9)? as u64,
                    cache_read_tokens: row.get::<_, i64>(10)? as u64,
                    total_tokens,
                    request_count: row.get::<_, i64>(12)?.max(1) as u64,
                    explicit_estimated_cost: row.get(13)?,
                    is_subagent: row.get::<_, i64>(14)? != 0,
                    source_kind: "local_usage".to_string(),
                })
            })
            .map_err(|e| format!("Failed to query sync request export: {}", e))?;

        let mut requests = Vec::new();
        for row in request_rows {
            requests.push(row.map_err(|e| format!("Failed to read sync request row: {}", e))?);
        }

        Ok(SyncExportData { sessions, requests })
    }

    pub fn import_remote_sync_data(
        &self,
        device_id: &str,
        export_seq: i64,
        data: &SyncExportData,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start remote sync import: {}", e))?;
        let settings = crate::commands::load_settings().unwrap_or_default();
        let today = Self::today_local_date_with_settings(&settings);
        let mut touched_history_dates = HashSet::new();

        tx.execute(
            "INSERT INTO remote_devices (
                device_id, last_seen_at, last_export_seq, sync_status, updated_at
             ) VALUES (?1, ?2, ?3, 'ready', ?4)
             ON CONFLICT(device_id) DO UPDATE SET
                last_seen_at = excluded.last_seen_at,
                last_export_seq = MAX(remote_devices.last_export_seq, excluded.last_export_seq),
                sync_status = 'ready',
                updated_at = excluded.updated_at",
            params![device_id, now, export_seq, now],
        )
        .map_err(|e| format!("Failed to upsert remote device: {}", e))?;

        for session in &data.sessions {
            let model_list_json = serde_json::to_string(&session.model_list)
                .map_err(|e| format!("Failed to serialize remote session models: {}", e))?;
            tx.execute(
                "INSERT INTO remote_sessions (
                    origin_device_id, session_id, tool, project_key, project_name, start_time,
                    end_time, request_count, total_input_tokens, total_output_tokens,
                    total_cache_create_tokens, total_cache_read_tokens, total_tokens,
                    model_list_json, imported_at, export_seq
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                 ON CONFLICT(origin_device_id, session_id) DO UPDATE SET
                    tool = excluded.tool,
                    project_key = excluded.project_key,
                    project_name = excluded.project_name,
                    start_time = excluded.start_time,
                    end_time = excluded.end_time,
                    request_count = excluded.request_count,
                    total_input_tokens = excluded.total_input_tokens,
                    total_output_tokens = excluded.total_output_tokens,
                    total_cache_create_tokens = excluded.total_cache_create_tokens,
                    total_cache_read_tokens = excluded.total_cache_read_tokens,
                    total_tokens = excluded.total_tokens,
                    model_list_json = excluded.model_list_json,
                    imported_at = excluded.imported_at,
                    export_seq = excluded.export_seq
                 WHERE excluded.export_seq >= remote_sessions.export_seq",
                params![
                    device_id,
                    session.session_id.as_str(),
                    session.tool.as_str(),
                    session.project_key.as_deref(),
                    session.project_name.as_deref(),
                    session.start_time,
                    session.end_time,
                    session.request_count as i64,
                    session.total_input_tokens as i64,
                    session.total_output_tokens as i64,
                    session.total_cache_create_tokens as i64,
                    session.total_cache_read_tokens as i64,
                    session.total_tokens as i64,
                    model_list_json.as_str(),
                    now,
                    export_seq
                ],
            )
            .map_err(|e| format!("Failed to upsert remote session: {}", e))?;
        }

        for request in &data.requests {
            let date = crate::utils::business_time::business_date_for_timestamp(
                request.timestamp,
                &settings,
            );
            if date < today {
                touched_history_dates.insert(date);
            }
            tx.execute(
                "INSERT INTO remote_request_facts (
                    request_key, origin_device_id, session_id, tool, project_key, timestamp,
                    message_id, dedupe_key, model, input_tokens, output_tokens,
                    cache_create_tokens, cache_read_tokens, total_tokens, request_count, explicit_estimated_cost,
                    is_subagent, source_kind, imported_at, export_seq
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
                 ON CONFLICT(origin_device_id, request_key) DO UPDATE SET
                    session_id = excluded.session_id,
                    tool = excluded.tool,
                    project_key = excluded.project_key,
                    timestamp = excluded.timestamp,
                    message_id = excluded.message_id,
                    dedupe_key = excluded.dedupe_key,
                    model = excluded.model,
                    input_tokens = excluded.input_tokens,
                    output_tokens = excluded.output_tokens,
                    cache_create_tokens = excluded.cache_create_tokens,
                    cache_read_tokens = excluded.cache_read_tokens,
                    total_tokens = excluded.total_tokens,
                    request_count = excluded.request_count,
                    explicit_estimated_cost = excluded.explicit_estimated_cost,
                    is_subagent = excluded.is_subagent,
                    source_kind = excluded.source_kind,
                    imported_at = excluded.imported_at,
                    export_seq = excluded.export_seq
                 WHERE excluded.export_seq >= remote_request_facts.export_seq",
                params![
                    request.request_key.as_str(),
                    device_id,
                    request.session_id.as_str(),
                    request.tool.as_str(),
                    request.project_key.as_deref(),
                    request.timestamp,
                    request.message_id.as_deref(),
                    request.dedupe_key.as_str(),
                    request.model.as_str(),
                    request.input_tokens as i64,
                    request.output_tokens as i64,
                    request.cache_create_tokens as i64,
                    request.cache_read_tokens as i64,
                    request.total_tokens as i64,
                    request.request_count as i64,
                    request.explicit_estimated_cost,
                    if request.is_subagent { 1 } else { 0 },
                    request.source_kind.as_str(),
                    now,
                    export_seq
                ],
            )
            .map_err(|e| format!("Failed to upsert remote request fact: {}", e))?;
        }

        tx.execute(
            "INSERT INTO webdav_sync_state (state_key, state_value, updated_at)
             VALUES (?1, '1', ?2)
             ON CONFLICT(state_key) DO UPDATE SET
                state_value = excluded.state_value,
                updated_at = excluded.updated_at",
            params![format!("imported:{}:{}", device_id, export_seq), now],
        )
        .map_err(|e| format!("Failed to mark remote sync package imported: {}", e))?;
        Self::invalidate_unified_materialization_dates_tx(
            &tx,
            &touched_history_dates.into_iter().collect::<Vec<_>>(),
            now,
        )?;

        tx.commit()
            .map_err(|e| format!("Failed to commit remote sync import: {}", e))?;
        Ok(())
    }

    pub fn get_remote_request_records_in_range(
        &self,
        start_epoch: i64,
        end_epoch: i64,
        tool_filter: &ToolFilter,
    ) -> Result<Vec<LocalRequestRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let base_select = "SELECT session_id, tool, timestamp, COALESCE(message_id, ''),
                        input_tokens, output_tokens, cache_create_tokens, cache_read_tokens,
                        total_tokens, COALESCE(request_count, 1), model, explicit_estimated_cost,
                        is_subagent, request_key
                 FROM remote_request_facts";
        let mapper = |row: &rusqlite::Row<'_>| {
            let request_key: Option<String> = row.get(13)?;
            Ok(LocalRequestRecord {
                session_id: row.get(0)?,
                tool: row.get(1)?,
                timestamp: row.get(2)?,
                message_id: row.get(3)?,
                input_tokens: row.get::<_, i64>(4)? as u64,
                output_tokens: row.get::<_, i64>(5)? as u64,
                cache_create_tokens: row.get::<_, i64>(6)? as u64,
                cache_read_tokens: row.get::<_, i64>(7)? as u64,
                total_tokens: row.get::<_, i64>(8)? as u64,
                request_count: row.get::<_, i64>(9)?.max(1) as u64,
                model: row.get(10)?,
                explicit_estimated_cost: row.get(11)?,
                is_subagent: row.get::<_, i64>(12)? != 0,
                request_key: request_key.filter(|v| !v.trim().is_empty()),
                source_file_present: None,
                reasoning_tokens: 0,
            })
        };
        let mut result = Vec::new();
        match tool_filter {
            ToolFilter::All => {
                let sql = format!(
                    "{base_select} WHERE timestamp >= ?1 AND timestamp < ?2 ORDER BY timestamp ASC"
                );
                let mut stmt = conn.prepare(&sql).map_err(|e| {
                    format!(
                        "Failed to prepare get_remote_request_records_in_range: {}",
                        e
                    )
                })?;
                for row in stmt
                    .query_map(params![start_epoch, end_epoch], mapper)
                    .map_err(|e| format!("Failed to query remote records in range: {}", e))?
                {
                    result.push(
                        row.map_err(|e| format!("Failed to read remote request row: {}", e))?,
                    );
                }
            }
            ToolFilter::Tool(tool) => {
                let sql = format!("{base_select} WHERE timestamp >= ?1 AND timestamp < ?2 AND tool = ?3 ORDER BY timestamp ASC");
                let mut stmt = conn.prepare(&sql).map_err(|e| {
                    format!(
                        "Failed to prepare get_remote_request_records_in_range by tool: {}",
                        e
                    )
                })?;
                for row in stmt
                    .query_map(params![start_epoch, end_epoch, tool], mapper)
                    .map_err(|e| {
                        format!("Failed to query remote records in range by tool: {}", e)
                    })?
                {
                    result.push(row.map_err(|e| {
                        format!("Failed to read remote request row by tool: {}", e)
                    })?);
                }
            }
            ToolFilter::AnyOf(tools) if !tools.is_empty() => {
                let placeholders = tools
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("?{}", i + 3))
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!("{base_select} WHERE timestamp >= ?1 AND timestamp < ?2 AND tool IN ({placeholders}) ORDER BY timestamp ASC");
                let mut stmt = conn.prepare(&sql).map_err(|e| {
                    format!(
                        "Failed to prepare get_remote_request_records_in_range by tools: {}",
                        e
                    )
                })?;
                let mut all_params: Vec<Box<dyn rusqlite::ToSql>> =
                    vec![Box::new(start_epoch), Box::new(end_epoch)];
                for t in tools {
                    all_params.push(Box::new(t.clone()));
                }
                let param_refs: Vec<&dyn rusqlite::ToSql> =
                    all_params.iter().map(|p| p.as_ref()).collect();
                for row in stmt.query_map(param_refs.as_slice(), mapper).map_err(|e| {
                    format!("Failed to query remote records in range by tools: {}", e)
                })? {
                    result.push(row.map_err(|e| {
                        format!("Failed to read remote request row by tools: {}", e)
                    })?);
                }
            }
            ToolFilter::AnyOf(_) => {}
        }
        Ok(result)
    }

    pub fn get_remote_sessions(
        &self,
        tool_filter: &ToolFilter,
    ) -> Result<Vec<SessionMeta>, String> {
        let conn = self.conn.lock().unwrap();
        let base_select =
            "SELECT session_id, tool, project_key, project_name, start_time, end_time,
                        request_count, total_input_tokens, total_output_tokens,
                        total_cache_create_tokens, total_cache_read_tokens, model_list_json
                 FROM remote_sessions";
        let mapper = |row: &rusqlite::Row<'_>| {
            let project_key: Option<String> = row.get(2)?;
            let model_list_json: String = row.get(11)?;
            Ok(SessionMeta {
                session_id: row.get(0)?,
                tool: row.get(1)?,
                cwd: project_key.clone(),
                project_name: row.get(3)?,
                topic: None,
                last_prompt: None,
                session_name: None,
                file_path: String::new(),
                file_size: 0,
                last_modified: row.get(5)?,
                total_input_tokens: row.get::<_, i64>(7)? as u64,
                total_output_tokens: row.get::<_, i64>(8)? as u64,
                total_cache_create_tokens: row.get::<_, i64>(9)? as u64,
                total_cache_read_tokens: row.get::<_, i64>(10)? as u64,
                models: serde_json::from_str(&model_list_json).unwrap_or_default(),
                message_count: row.get::<_, i64>(6)? as u64,
                start_time: row.get(4)?,
                end_time: row.get(5)?,
                source: "remote_sync".to_string(),
                message_ids: Vec::new(),
            })
        };
        let mut result = Vec::new();
        match tool_filter {
            ToolFilter::All => {
                let sql = format!("{base_select} ORDER BY end_time DESC");
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|e| format!("Failed to prepare get_remote_sessions: {}", e))?;
                for row in stmt
                    .query_map([], mapper)
                    .map_err(|e| format!("Failed to query remote sessions: {}", e))?
                {
                    result.push(
                        row.map_err(|e| format!("Failed to read remote session row: {}", e))?,
                    );
                }
            }
            ToolFilter::Tool(tool) => {
                let sql = format!("{base_select} WHERE tool = ?1 ORDER BY end_time DESC");
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|e| format!("Failed to prepare get_remote_sessions by tool: {}", e))?;
                for row in stmt
                    .query_map(params![tool], mapper)
                    .map_err(|e| format!("Failed to query remote sessions by tool: {}", e))?
                {
                    result.push(row.map_err(|e| {
                        format!("Failed to read remote session row by tool: {}", e)
                    })?);
                }
            }
            ToolFilter::AnyOf(tools) if !tools.is_empty() => {
                let placeholders = tools
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("?{}", i + 1))
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql =
                    format!("{base_select} WHERE tool IN ({placeholders}) ORDER BY end_time DESC");
                let mut stmt = conn.prepare(&sql).map_err(|e| {
                    format!("Failed to prepare get_remote_sessions by tools: {}", e)
                })?;
                let all_params: Vec<Box<dyn rusqlite::ToSql>> = tools
                    .iter()
                    .map(|t| Box::new(t.clone()) as Box<dyn rusqlite::ToSql>)
                    .collect();
                let param_refs: Vec<&dyn rusqlite::ToSql> =
                    all_params.iter().map(|p| p.as_ref()).collect();
                for row in stmt
                    .query_map(param_refs.as_slice(), mapper)
                    .map_err(|e| format!("Failed to query remote sessions by tools: {}", e))?
                {
                    result.push(row.map_err(|e| {
                        format!("Failed to read remote session row by tools: {}", e)
                    })?);
                }
            }
            ToolFilter::AnyOf(_) => {}
        }
        Ok(result)
    }

    pub fn count_remote_request_facts(&self) -> Result<u64, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM remote_request_facts", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|count| count.max(0) as u64)
        .map_err(|e| format!("Failed to count remote request facts: {}", e))
    }

    pub fn list_remote_devices(&self) -> Result<Vec<RemoteSyncDevice>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT device_id, last_seen_at, last_export_seq, sync_status, updated_at
                 FROM remote_devices
                 ORDER BY last_seen_at DESC",
            )
            .map_err(|e| format!("Failed to prepare list_remote_devices: {}", e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(RemoteSyncDevice {
                    device_id: row.get(0)?,
                    last_seen_at: row.get(1)?,
                    last_export_seq: row.get(2)?,
                    sync_status: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            })
            .map_err(|e| format!("Failed to query remote devices: {}", e))?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| format!("Failed to read remote device row: {}", e))?);
        }
        Ok(result)
    }

    pub fn remove_remote_device(&self, device_id: &str) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start remote device removal: {}", e))?;
        tx.execute(
            "DELETE FROM remote_request_facts WHERE origin_device_id = ?1",
            params![device_id],
        )
        .map_err(|e| format!("Failed to delete remote device requests: {}", e))?;
        tx.execute(
            "DELETE FROM remote_sessions WHERE origin_device_id = ?1",
            params![device_id],
        )
        .map_err(|e| format!("Failed to delete remote device sessions: {}", e))?;
        tx.execute(
            "DELETE FROM remote_devices WHERE device_id = ?1",
            params![device_id],
        )
        .map_err(|e| format!("Failed to delete remote device: {}", e))?;
        tx.execute(
            "DELETE FROM sync_device_cursors WHERE device_id = ?1",
            params![device_id],
        )
        .map_err(|e| format!("Failed to delete remote device cursor: {}", e))?;
        tx.execute(
            "DELETE FROM webdav_sync_state WHERE state_key LIKE ?1",
            params![format!("imported:{}:%", device_id)],
        )
        .map_err(|e| format!("Failed to delete remote device import markers: {}", e))?;
        Self::clear_unified_materialization_tx(&tx, now)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit remote device removal: {}", e))?;
        Ok(())
    }
}
