use crate::models::ToolFilter;
use crate::session::{LocalRequestRecord, SessionMeta};
use rusqlite::params;

use super::{LocalMergeCacheSignature, LocalUsageDatabase};

impl LocalUsageDatabase {
    pub fn get_request_records_in_range(
        &self,
        start_epoch: i64,
        end_epoch: i64,
        tool_filter: &ToolFilter,
    ) -> Result<Vec<LocalRequestRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let base_select = "SELECT session_id, tool, timestamp, message_id,
                        input_tokens, output_tokens, cache_create_tokens, cache_read_tokens,
                        total_tokens, COALESCE(request_count, 1), model, is_subagent, request_key,
                        source_file_present, COALESCE(reasoning_tokens, 0), explicit_estimated_cost
                 FROM local_request_facts";
        let mapper = |row: &rusqlite::Row<'_>| {
            let request_key: Option<String> = row.get(12)?;
            let source_file_present: Option<i64> = row.get(13)?;
            Ok(LocalRequestRecord {
                session_id: row.get(0)?,
                tool: row.get(1)?,
                timestamp: row.get(2)?,
                message_id: row.get(3)?,
                input_tokens: LocalUsageDatabase::saturating_i64_to_u64(row.get::<_, i64>(4)?),
                output_tokens: LocalUsageDatabase::saturating_i64_to_u64(row.get::<_, i64>(5)?),
                cache_create_tokens: LocalUsageDatabase::saturating_i64_to_u64(
                    row.get::<_, i64>(6)?,
                ),
                cache_read_tokens: LocalUsageDatabase::saturating_i64_to_u64(row.get::<_, i64>(7)?),
                total_tokens: LocalUsageDatabase::saturating_i64_to_u64(row.get::<_, i64>(8)?),
                request_count: LocalUsageDatabase::saturating_i64_to_u64(row.get::<_, i64>(9)?)
                    .max(1),
                model: row.get(10)?,
                is_subagent: row.get::<_, i64>(11)? != 0,
                request_key: request_key.filter(|v| !v.trim().is_empty()),
                explicit_estimated_cost: row.get(15)?,
                source_file_present: source_file_present.map(|v| v != 0),
                reasoning_tokens: LocalUsageDatabase::saturating_i64_to_u64(row.get::<_, i64>(14)?),
            })
        };
        let mut result = Vec::new();
        match tool_filter {
            ToolFilter::All => {
                let sql = format!(
                    "{base_select} WHERE timestamp >= ?1 AND timestamp < ?2 ORDER BY timestamp ASC"
                );
                let mut stmt = conn.prepare(&sql).map_err(|e| {
                    format!("Failed to prepare get_request_records_in_range: {}", e)
                })?;
                for row in stmt
                    .query_map(params![start_epoch, end_epoch], mapper)
                    .map_err(|e| format!("Failed to query local request records in range: {}", e))?
                {
                    result.push(
                        row.map_err(|e| format!("Failed to read local request record row: {}", e))?,
                    );
                }
            }
            ToolFilter::Tool(tool) => {
                let sql = format!("{base_select} WHERE timestamp >= ?1 AND timestamp < ?2 AND tool = ?3 ORDER BY timestamp ASC");
                let mut stmt = conn.prepare(&sql).map_err(|e| {
                    format!(
                        "Failed to prepare get_request_records_in_range by tool: {}",
                        e
                    )
                })?;
                for row in stmt
                    .query_map(params![start_epoch, end_epoch, tool], mapper)
                    .map_err(|e| {
                        format!(
                            "Failed to query local request records in range by tool: {}",
                            e
                        )
                    })?
                {
                    result.push(row.map_err(|e| {
                        format!("Failed to read local request record row by tool: {}", e)
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
                        "Failed to prepare get_request_records_in_range by tools: {}",
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
                    format!(
                        "Failed to query local request records in range by tools: {}",
                        e
                    )
                })? {
                    result.push(row.map_err(|e| {
                        format!("Failed to read local request record row by tools: {}", e)
                    })?);
                }
            }
            ToolFilter::AnyOf(_) => {}
        }
        Ok(result)
    }

    pub fn get_all_sessions(&self, tool_filter: &ToolFilter) -> Result<Vec<SessionMeta>, String> {
        let conn = self.conn.lock().unwrap();
        let base_select =
            "SELECT session_id, tool, cwd, project_name, topic, last_prompt, session_name,
                        scope, primary_file_path, file_size, last_modified, total_input_tokens,
                        total_output_tokens, total_cache_create_tokens, total_cache_read_tokens,
                        request_count, start_time, end_time, source_kind, model_list_json,
                        total_reasoning_tokens, total_elapsed_ms, explicit_cost,
                        explicit_cost_currency, usage_sources_json, estimated,
                        (SELECT p.project_path FROM local_session_projects p WHERE p.session_id = local_sessions.session_id)
                 FROM local_sessions";
        let mapper = |row: &rusqlite::Row<'_>| {
            let model_list_json: String = row.get(19)?;
            let usage_sources_json: String = row.get(24)?;
            Ok(SessionMeta {
                session_id: row.get(0)?,
                tool: row.get(1)?,
                cwd: row.get(2)?,
                project_path: row.get(26)?,
                project_name: row.get(3)?,
                topic: row.get(4)?,
                last_prompt: row.get(5)?,
                session_name: row.get(6)?,
                scope: row.get(7)?,
                file_path: row.get(8)?,
                file_size: LocalUsageDatabase::saturating_i64_to_u64(row.get::<_, i64>(9)?),
                last_modified: row.get(10)?,
                total_input_tokens: LocalUsageDatabase::saturating_i64_to_u64(
                    row.get::<_, i64>(11)?,
                ),
                total_output_tokens: LocalUsageDatabase::saturating_i64_to_u64(
                    row.get::<_, i64>(12)?,
                ),
                total_cache_create_tokens: LocalUsageDatabase::saturating_i64_to_u64(
                    row.get::<_, i64>(13)?,
                ),
                total_cache_read_tokens: LocalUsageDatabase::saturating_i64_to_u64(
                    row.get::<_, i64>(14)?,
                ),
                models: serde_json::from_str(&model_list_json).unwrap_or_default(),
                message_count: LocalUsageDatabase::saturating_i64_to_u64(row.get::<_, i64>(15)?),
                start_time: row.get(16)?,
                end_time: row.get(17)?,
                source: row.get(18)?,
                message_ids: Vec::new(),
                explicit_estimated_cost: match row.get::<_, Option<String>>(23)? {
                    Some(currency) if currency.eq_ignore_ascii_case("USD") => row.get(22)?,
                    _ => None,
                },
                total_reasoning_tokens: LocalUsageDatabase::saturating_i64_to_u64(
                    row.get::<_, i64>(20)?,
                ),
                total_elapsed_ms: LocalUsageDatabase::saturating_i64_to_u64(row.get::<_, i64>(21)?),
                explicit_cost: row.get(22)?,
                explicit_cost_currency: row.get(23)?,
                usage_sources: serde_json::from_str(&usage_sources_json).unwrap_or_default(),
                estimated: row.get::<_, i64>(25)?.max(0) > 0,
            })
        };
        let mut result = Vec::new();
        match tool_filter {
            ToolFilter::All => {
                let sql = format!("{base_select} ORDER BY end_time DESC");
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|e| format!("Failed to prepare get_all_sessions: {}", e))?;
                for row in stmt
                    .query_map([], mapper)
                    .map_err(|e| format!("Failed to query local sessions: {}", e))?
                {
                    result
                        .push(row.map_err(|e| format!("Failed to read local session row: {}", e))?);
                }
            }
            ToolFilter::Tool(tool) => {
                let sql = format!("{base_select} WHERE tool = ?1 ORDER BY end_time DESC");
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|e| format!("Failed to prepare get_all_sessions by tool: {}", e))?;
                for row in stmt
                    .query_map(params![tool], mapper)
                    .map_err(|e| format!("Failed to query local sessions by tool: {}", e))?
                {
                    result.push(
                        row.map_err(|e| {
                            format!("Failed to read local session row by tool: {}", e)
                        })?,
                    );
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
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|e| format!("Failed to prepare get_all_sessions by tools: {}", e))?;
                let all_params: Vec<Box<dyn rusqlite::ToSql>> = tools
                    .iter()
                    .map(|t| Box::new(t.clone()) as Box<dyn rusqlite::ToSql>)
                    .collect();
                let param_refs: Vec<&dyn rusqlite::ToSql> =
                    all_params.iter().map(|p| p.as_ref()).collect();
                for row in stmt
                    .query_map(param_refs.as_slice(), mapper)
                    .map_err(|e| format!("Failed to query local sessions by tools: {}", e))?
                {
                    result.push(row.map_err(|e| {
                        format!("Failed to read local session row by tools: {}", e)
                    })?);
                }
            }
            ToolFilter::AnyOf(_) => {}
        }
        Ok(result)
    }

    pub fn count_local_request_facts(&self) -> Result<u64, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM local_request_facts", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|count| count.max(0) as u64)
        .map_err(|e| format!("Failed to count local request facts: {}", e))
    }

    pub fn get_merge_cache_signature(&self) -> Result<LocalMergeCacheSignature, String> {
        let conn = self.open_readonly_connection()?;
        conn.query_row(
            r#"
            SELECT
                COALESCE(MAX(CASE
                    WHEN state_key = 'merge_cache_generation'
                    THEN CAST(state_value AS INTEGER)
                END), 0),
                COALESCE(MAX(CASE
                    WHEN state_key = 'unified_materialization_invalidation_version'
                    THEN CAST(state_value AS INTEGER)
                END), 0)
            FROM local_sync_state
            WHERE state_key IN (
                'merge_cache_generation',
                'unified_materialization_invalidation_version'
            )
            "#,
            [],
            |row| {
                Ok(LocalMergeCacheSignature {
                    merge_cache_generation: row.get(0)?,
                    unified_materialization_invalidation_version: row.get(1)?,
                })
            },
        )
        .map_err(|e| format!("Failed to load local merge cache signature: {}", e))
    }

    pub fn get_request_time_bounds(&self) -> Result<Option<(i64, i64)>, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            r#"
            SELECT
                MIN(ts),
                MAX(ts)
            FROM (
                SELECT timestamp AS ts FROM local_request_facts
                UNION ALL
                SELECT timestamp AS ts FROM remote_request_facts
            )
            "#,
            [],
            |row| {
                let min_ts: Option<i64> = row.get(0)?;
                let max_ts: Option<i64> = row.get(1)?;
                Ok(match (min_ts, max_ts) {
                    (Some(start), Some(end)) => Some((start, end)),
                    _ => None,
                })
            },
        )
        .map_err(|e| format!("Failed to query local request time bounds: {}", e))
    }
}
