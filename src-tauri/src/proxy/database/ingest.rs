use super::session;
use super::ProxyDatabase;
use crate::models::ModelPricingConfig;
use crate::proxy::types::UsageRecord;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

impl ProxyDatabase {
    fn current_pricing_context(&self) -> (Vec<ModelPricingConfig>, String, String) {
        let settings = crate::settings::load_settings_blocking().unwrap_or_default();
        let pricings = self.get_all_model_pricings().unwrap_or_default();
        let match_mode = settings.model_pricing.match_mode;
        let snapshot_id = Self::pricing_snapshot_id(&pricings, &match_mode);
        (pricings, match_mode, snapshot_id)
    }

    fn estimate_record_cost(
        record: &UsageRecord,
        pricings: &[ModelPricingConfig],
        match_mode: &str,
    ) -> f64 {
        crate::models::estimate_session_cost(
            record.input_tokens,
            record.output_tokens,
            record.cache_create_tokens,
            record.cache_read_tokens,
            &record.model,
            pricings,
            match_mode,
        )
    }

    fn computed_storage_dedupe_key(record: &UsageRecord) -> String {
        if let Some(key) = record.storage_dedupe_key.as_ref() {
            let trimmed = key.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        if record.ingress_kind == "gateway" {
            if let Some(request_id) = record.gateway_request_id.as_ref() {
                let request_id = request_id.trim();
                if !request_id.is_empty() {
                    return format!(
                        "gateway:{}:{}",
                        record.gateway_profile_id.as_deref().unwrap_or_default(),
                        request_id
                    );
                }
            }
        }
        if record.client_tool == "opencode" && !record.message_id.trim().is_empty() {
            let stable_time = if record.request_start_time > 0 {
                record.request_start_time
            } else {
                record.timestamp
            };
            format!(
                "{}:{}:{}",
                record.client_tool, record.message_id, stable_time
            )
        } else {
            session::computed_canonical_request_key(record)
        }
    }

    pub async fn insert_record(&self, record: &UsageRecord) -> Result<i64, String> {
        let (pricings, match_mode, snapshot_id) = self.current_pricing_context();
        let estimated_cost = if record.cost_locked {
            record.estimated_cost
        } else {
            Self::estimate_record_cost(record, &pricings, &match_mode)
        };
        let pricing_matched = record.cost_locked
            || crate::models::find_pricing(&record.model, &pricings, &match_mode).is_some();
        let pricing_snapshot_id = record
            .pricing_snapshot_id
            .clone()
            .unwrap_or_else(|| snapshot_id.clone());
        let storage_dedupe_key = Self::computed_storage_dedupe_key(record);
        let canonical_request_key = session::computed_canonical_request_key(record);
        let session_resolution_state = session::computed_session_resolution_state(record);
        let now = chrono::Utc::now().timestamp();
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        conn.execute(
            r#"
            INSERT OR REPLACE INTO usage_records
            (timestamp, message_id, storage_dedupe_key, canonical_request_key, input_tokens, output_tokens, cache_create_tokens,
             cache_read_tokens, model, session_id, session_resolution_state, message_id_conflicted, request_start_time,
             request_end_time, duration_ms, output_tokens_per_second, ttft_ms, status_code,
             migration_attempted_at, estimated_cost, pricing_snapshot_id, cost_locked, api_key_prefix, request_base_url,
             client_tool, proxy_profile_id, client_detection_method, ingress_kind, gateway_profile_id,
             gateway_caller_label, usage_source, gateway_request_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, NULL, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32)
            "#,
            rusqlite::params![
                record.timestamp,
                &record.message_id,
                &storage_dedupe_key,
                &canonical_request_key,
                record.input_tokens as i64,
                record.output_tokens as i64,
                record.cache_create_tokens as i64,
                record.cache_read_tokens as i64,
                &record.model,
                &record.session_id,
                &session_resolution_state,
                if record.message_id_conflicted { 1 } else { 0 },
                record.request_start_time,
                record.request_end_time,
                record.duration_ms as i64,
                record.output_tokens_per_second,
                record.ttft_ms.map(|v| v as i64),
                record.status_code as i64,
                estimated_cost,
                pricing_snapshot_id,
                if pricing_matched { 1 } else { 0 },
                &record.api_key_prefix,
                &record.request_base_url,
                &record.client_tool,
                &record.proxy_profile_id,
                &record.client_detection_method,
                &record.ingress_kind,
                &record.gateway_profile_id,
                &record.gateway_caller_label,
                &record.usage_source,
                &record.gateway_request_id,
                now,
            ],
        )
        .map_err(|e| format!("Failed to insert record: {}", e))?;

        let id = conn.last_insert_rowid();
        let date = Self::record_local_date(record.timestamp);
        if date < Self::today_local_date() {
            Self::refresh_daily_summary_for_date_conn(&conn, &date)?;
            if let Ok(local_db) = crate::local_usage::LocalUsageDatabase::get_global() {
                let _ = local_db.invalidate_unified_materialization_dates(&[date]);
            }
        }
        Ok(id)
    }

    pub(crate) fn ccswitch_import_state(&self) -> Result<super::CcSwitchImportState, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {e}"))?;
        let value = conn
            .query_row(
                "SELECT state_value FROM ccswitch_import_state WHERE state_key = 'state'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read CC Switch import state: {e}"))?;
        value
            .map(|json| serde_json::from_str(&json).map_err(|e| e.to_string()))
            .transpose()
            .map(|state| state.unwrap_or_default())
            .map_err(|e| format!("Failed to decode CC Switch import state: {e}"))
    }

    pub(crate) fn save_ccswitch_import_state(
        &self,
        state: &super::CcSwitchImportState,
    ) -> Result<(), String> {
        let json = serde_json::to_string(state)
            .map_err(|e| format!("Failed to encode CC Switch import state: {e}"))?;
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {e}"))?;
        conn.execute(
            "INSERT INTO ccswitch_import_state (state_key, state_value, updated_at)
             VALUES ('state', ?1, ?2)
             ON CONFLICT(state_key) DO UPDATE SET
                state_value = excluded.state_value,
                updated_at = excluded.updated_at",
            rusqlite::params![json, chrono::Utc::now().timestamp()],
        )
        .map_err(|e| format!("Failed to save CC Switch import state: {e}"))?;
        Ok(())
    }

    pub(crate) fn insert_ccswitch_batch(
        &self,
        records: &[UsageRecord],
        state: &super::CcSwitchImportState,
    ) -> Result<usize, String> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {e}"))?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| format!("Failed to start CC Switch import transaction: {e}"))?;
        let mut touched_dates = std::collections::HashSet::new();
        let mut imported = 0usize;
        {
            let mut statement = tx
                .prepare(
                    "INSERT INTO usage_records
                     (timestamp, message_id, storage_dedupe_key, canonical_request_key,
                      input_tokens, output_tokens, cache_create_tokens, cache_read_tokens,
                      model, session_id, session_resolution_state, message_id_conflicted,
                      request_start_time, request_end_time, duration_ms,
                      output_tokens_per_second, ttft_ms, status_code, estimated_cost,
                      pricing_snapshot_id, cost_locked, api_key_prefix, request_base_url,
                      client_tool, proxy_profile_id, client_detection_method, ingress_kind,
                      gateway_profile_id, gateway_caller_label, usage_source,
                      gateway_request_id, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                             ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, 1, NULL, NULL,
                             ?21, ?22, 'ccswitch_proxy_log', 'ccswitch_proxy', NULL, NULL, 'provider',
                             NULL, ?23)
                     ON CONFLICT(storage_dedupe_key) DO NOTHING",
                )
                .map_err(|e| format!("Failed to prepare CC Switch import: {e}"))?;
            for record in records {
                let storage_key = Self::computed_storage_dedupe_key(record);
                let canonical_key = session::computed_canonical_request_key(record);
                let session_state = session::computed_session_resolution_state(record);
                let changed = statement
                    .execute(rusqlite::params![
                        record.timestamp,
                        &record.message_id,
                        storage_key,
                        canonical_key,
                        record.input_tokens as i64,
                        record.output_tokens as i64,
                        record.cache_create_tokens as i64,
                        record.cache_read_tokens as i64,
                        &record.model,
                        &record.session_id,
                        session_state,
                        if record.message_id_conflicted { 1 } else { 0 },
                        record.request_start_time,
                        record.request_end_time,
                        record.duration_ms as i64,
                        record.output_tokens_per_second,
                        record.ttft_ms.map(|value| value as i64),
                        record.status_code as i64,
                        record.estimated_cost,
                        &record.pricing_snapshot_id,
                        &record.client_tool,
                        &record.proxy_profile_id,
                        chrono::Utc::now().timestamp(),
                    ])
                    .map_err(|e| format!("Failed to import CC Switch request: {e}"))?;
                if changed > 0 {
                    imported += 1;
                    let date = Self::record_local_date(record.timestamp);
                    if date < Self::today_local_date() {
                        touched_dates.insert(date);
                    }
                }
            }
        }

        for date in &touched_dates {
            Self::refresh_daily_summary_for_date_conn(&tx, date)?;
        }
        let persisted_state = tx
            .query_row(
                "SELECT state_value FROM ccswitch_import_state WHERE state_key = 'state'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read CC Switch import state: {e}"))?
            .and_then(|json| serde_json::from_str::<super::CcSwitchImportState>(&json).ok());
        let mut updated_state = state.clone();
        updated_state.total_imported = persisted_state
            .map(|persisted| persisted.total_imported)
            .unwrap_or(state.total_imported)
            .saturating_add(imported as u64);
        let json = serde_json::to_string(&updated_state)
            .map_err(|e| format!("Failed to encode CC Switch import state: {e}"))?;
        tx.execute(
            "INSERT INTO ccswitch_import_state (state_key, state_value, updated_at)
             VALUES ('state', ?1, ?2)
             ON CONFLICT(state_key) DO UPDATE SET
                state_value = excluded.state_value,
                updated_at = excluded.updated_at",
            rusqlite::params![json, chrono::Utc::now().timestamp()],
        )
        .map_err(|e| format!("Failed to persist CC Switch import state: {e}"))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit CC Switch import: {e}"))?;

        if !touched_dates.is_empty() {
            if let Ok(local_db) = crate::local_usage::LocalUsageDatabase::get_global() {
                let dates: Vec<_> = touched_dates.into_iter().collect();
                let _ = local_db.invalidate_unified_materialization_dates(&dates);
            }
        }
        Ok(imported)
    }

    pub(super) fn refresh_daily_summary_for_date_conn(
        conn: &Connection,
        date: &str,
    ) -> Result<(), String> {
        let settings = crate::settings::load_settings_blocking().unwrap_or_default();
        let current_mode =
            crate::utils::business_time::normalize_day_boundary_mode(&settings.day_boundary_mode);
        let stored_mode = Self::stored_day_boundary_mode_conn(conn)?;
        if stored_mode.as_deref() != Some(current_mode.as_str()) {
            conn.execute("DELETE FROM daily_summary", [])
                .map_err(|e| format!("Failed to clear daily summary: {}", e))?;
            conn.execute("DELETE FROM model_usage", [])
                .map_err(|e| format!("Failed to clear model usage: {}", e))?;
            Self::set_day_boundary_mode_conn(conn, &current_mode)?;
        }
        let (start_epoch, end_epoch) =
            crate::utils::business_time::business_date_epoch_bounds(date, &settings)?;
        let start_ms = start_epoch.saturating_mul(1000);
        let end_ms = end_epoch.saturating_mul(1000);

        conn.execute("DELETE FROM daily_summary WHERE date = ?1", [date])
            .map_err(|e| format!("Failed to clear daily summary: {}", e))?;
        conn.execute("DELETE FROM model_usage WHERE date = ?1", [date])
            .map_err(|e| format!("Failed to clear model usage: {}", e))?;

        conn.execute(
            r#"
            INSERT INTO daily_summary (
                date, total_tokens, input_tokens, output_tokens, cache_create_tokens,
                cache_read_tokens, request_count, cost, success_total_tokens,
                success_input_tokens, success_output_tokens, success_cache_create_tokens,
                success_cache_read_tokens, success_cost, model_count, success_requests,
                client_error_requests, server_error_requests, finalized_at
            )
            SELECT
                ?1,
                COALESCE(SUM(input_tokens + cache_create_tokens + cache_read_tokens + output_tokens), 0),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(cache_create_tokens), 0),
                COALESCE(SUM(cache_read_tokens), 0),
                COUNT(*),
                COALESCE(SUM(estimated_cost), 0),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN input_tokens + cache_create_tokens + cache_read_tokens + output_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN input_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN output_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN cache_create_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN cache_read_tokens ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN estimated_cost ELSE 0 END), 0),
                COUNT(DISTINCT CASE WHEN model != '' THEN model END),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 400 AND status_code < 500 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 500 THEN 1 ELSE 0 END), 0),
                ?2
            FROM usage_records
            WHERE timestamp >= ?3 AND timestamp < ?4
            HAVING COUNT(*) > 0
            "#,
            rusqlite::params![date, chrono::Utc::now().timestamp_millis(), start_ms, end_ms],
        )
        .map_err(|e| format!("Failed to refresh daily summary: {}", e))?;

        conn.execute(
            r#"
            INSERT INTO model_usage (
                date, model, total_tokens, input_tokens, output_tokens, cache_create_tokens,
                cache_read_tokens, request_count, cost, success_requests,
                client_error_requests, server_error_requests
            )
            SELECT
                ?1,
                model,
                COALESCE(SUM(input_tokens + cache_create_tokens + cache_read_tokens + output_tokens), 0),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(cache_create_tokens), 0),
                COALESCE(SUM(cache_read_tokens), 0),
                COUNT(*),
                COALESCE(SUM(estimated_cost), 0),
                COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 400 AND status_code < 500 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 500 THEN 1 ELSE 0 END), 0)
            FROM usage_records
            WHERE timestamp >= ?2 AND timestamp < ?3
            GROUP BY model
            "#,
            rusqlite::params![date, start_ms, end_ms],
        )
        .map_err(|e| format!("Failed to refresh model usage: {}", e))?;

        Ok(())
    }

    pub async fn backfill_unlocked_costs(&self) -> Result<usize, String> {
        let (pricings, match_mode, snapshot_id) = self.current_pricing_context();
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        // 修复旧版本把未知模型按 $0 锁定的记录；真正匹配到零价模型的记录保持冻结。
        let stale_locked: Vec<(i64, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id, model FROM usage_records
                     WHERE cost_locked = 1 AND estimated_cost = 0",
                )
                .map_err(|e| format!("Failed to prepare stale cost query: {}", e))?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|e| format!("Failed to query stale costs: {}", e))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect stale costs: {}", e))?;
            rows
        };
        if !stale_locked.is_empty() {
            let mut stmt = conn
                .prepare(
                    "UPDATE usage_records
                     SET cost_locked = 0, pricing_snapshot_id = NULL, updated_at = ?1
                     WHERE id = ?2",
                )
                .map_err(|e| format!("Failed to prepare stale cost repair: {}", e))?;
            let now = chrono::Utc::now().timestamp();
            for (id, model) in stale_locked {
                if crate::models::find_pricing(&model, &pricings, &match_mode).is_none() {
                    stmt.execute(rusqlite::params![now, id])
                        .map_err(|e| format!("Failed to repair stale cost: {}", e))?;
                }
            }
        }

        let records = {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, timestamp, input_tokens, output_tokens, cache_create_tokens,
                           cache_read_tokens, model
                    FROM usage_records
                    WHERE cost_locked = 0 OR cost_locked IS NULL
                    "#,
                )
                .map_err(|e| format!("Failed to prepare cost backfill query: {}", e))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        Self::safe_i64_to_u64(row.get::<_, i64>(2)?),
                        Self::safe_i64_to_u64(row.get::<_, i64>(3)?),
                        Self::safe_i64_to_u64(row.get::<_, i64>(4)?),
                        Self::safe_i64_to_u64(row.get::<_, i64>(5)?),
                        row.get::<_, String>(6)?,
                    ))
                })
                .map_err(|e| format!("Failed to query cost backfill records: {}", e))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect cost backfill records: {}", e))?
        };

        if records.is_empty() {
            return Ok(0);
        }

        let mut touched_dates = std::collections::HashSet::new();
        let now = chrono::Utc::now().timestamp();
        let mut stmt = conn
            .prepare(
                r#"
                UPDATE usage_records
                SET estimated_cost = ?1, pricing_snapshot_id = ?2, cost_locked = 1, updated_at = ?3
                WHERE id = ?4
                "#,
            )
            .map_err(|e| format!("Failed to prepare cost backfill update: {}", e))?;

        let mut updated_count = 0;
        for (id, timestamp, input, output, cache_create, cache_read, model) in &records {
            if crate::models::find_pricing(model, &pricings, &match_mode).is_none() {
                continue;
            }
            let cost = crate::models::estimate_session_cost(
                *input,
                *output,
                *cache_create,
                *cache_read,
                model,
                &pricings,
                &match_mode,
            );
            stmt.execute(rusqlite::params![cost, snapshot_id, now, id])
                .map_err(|e| format!("Failed to update cost backfill record: {}", e))?;
            updated_count += 1;
            let date = Self::record_local_date(*timestamp);
            if date < Self::today_local_date() {
                touched_dates.insert(date);
            }
        }
        drop(stmt);

        for date in &touched_dates {
            Self::refresh_daily_summary_for_date_conn(&conn, date)?;
        }

        eprintln!(
            "[database] Backfilled frozen cost for {} usage records",
            updated_count
        );
        if !touched_dates.is_empty() {
            if let Ok(local_db) = crate::local_usage::LocalUsageDatabase::get_global() {
                let _ = local_db.invalidate_unified_materialization_dates(
                    &touched_dates.into_iter().collect::<Vec<_>>(),
                );
            }
        }
        Ok(updated_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_storage_dedupe_key_uses_profile_and_local_request_id() {
        let record = UsageRecord {
            ingress_kind: "gateway".to_string(),
            gateway_profile_id: Some("gateway-deepseek".to_string()),
            gateway_request_id: Some("gw-request-1".to_string()),
            message_id: "same-upstream-message".to_string(),
            ..Default::default()
        };

        assert_eq!(
            ProxyDatabase::computed_storage_dedupe_key(&record),
            "gateway:gateway-deepseek:gw-request-1"
        );
    }

    #[tokio::test]
    async fn gateway_provenance_round_trips_through_proxy_database() {
        let temp = tempfile::tempdir().expect("temp database directory");
        let database = ProxyDatabase::new_with_path(&temp.path().join("proxy_data.db"))
            .expect("open proxy database");
        let timestamp = chrono::Utc::now().timestamp_millis();
        let record = UsageRecord {
            timestamp,
            message_id: "gateway-message".to_string(),
            model: "deepseek-v4-flash".to_string(),
            ingress_kind: "gateway".to_string(),
            gateway_profile_id: Some("gateway-deepseek".to_string()),
            gateway_caller_label: Some("Cursor".to_string()),
            usage_source: "provider".to_string(),
            gateway_request_id: Some("gw-request-roundtrip".to_string()),
            ..Default::default()
        };

        database
            .insert_record(&record)
            .await
            .expect("insert record");
        let records = database
            .get_records_since(timestamp.saturating_sub(1))
            .await
            .expect("query record");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].ingress_kind, "gateway");
        assert_eq!(
            records[0].gateway_profile_id.as_deref(),
            Some("gateway-deepseek")
        );
        assert_eq!(records[0].gateway_caller_label.as_deref(), Some("Cursor"));
        assert_eq!(records[0].usage_source, "provider");
        assert_eq!(
            records[0].gateway_request_id.as_deref(),
            Some("gw-request-roundtrip")
        );
        assert!(!records[0].cost_locked);
    }

    #[tokio::test]
    async fn ccswitch_batch_is_idempotent_and_persists_cursor_atomically() {
        let temp = tempfile::tempdir().expect("temp database directory");
        let database = ProxyDatabase::new_with_path(&temp.path().join("proxy_data.db"))
            .expect("open proxy database");
        let now = chrono::Utc::now().timestamp_millis();
        let record = UsageRecord {
            timestamp: now,
            message_id: "cc-request-1".to_string(),
            storage_dedupe_key: Some("ccswitch:cc-request-1".to_string()),
            canonical_request_key: Some("codex:cc-request-1".to_string()),
            input_tokens: 70,
            cache_read_tokens: 20,
            cache_create_tokens: 10,
            output_tokens: 5,
            total_tokens: 105,
            model: "gpt-5".to_string(),
            client_tool: "codex".to_string(),
            ingress_kind: "ccswitch_proxy".to_string(),
            usage_source: "provider".to_string(),
            estimated_cost: 0.012,
            cost_locked: true,
            ..Default::default()
        };
        let state = super::super::CcSwitchImportState {
            cursor_created_at: now / 1000,
            cursor_request_id: "cc-request-1".to_string(),
            last_import_at_ms: Some(now),
            ..Default::default()
        };

        let first_insert = database
            .insert_ccswitch_batch(&[record.clone()], &state)
            .unwrap();
        let stored = database
            .get_records_since(now.saturating_sub(1))
            .await
            .unwrap();
        assert_eq!(first_insert, 1, "stored records: {stored:?}");
        assert_eq!(
            database.insert_ccswitch_batch(&[record], &state).unwrap(),
            0
        );
        let stored_state = database.ccswitch_import_state().unwrap();
        assert_eq!(stored_state.cursor_request_id, "cc-request-1");
        assert_eq!(stored_state.total_imported, 1);
        assert_eq!(
            database
                .get_records_since(now.saturating_sub(1))
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn insert_record_locks_zero_cost_only_when_pricing_matches() {
        let temp = tempfile::tempdir().expect("temp database directory");
        let database = ProxyDatabase::new_with_path(&temp.path().join("proxy_data.db"))
            .expect("open proxy database");
        database
            .upsert_model_pricings(&[ModelPricingConfig {
                model_id: "free-model".to_string(),
                display_name: None,
                input_price: 0.0,
                output_price: 0.0,
                cache_write_price: Some(0.0),
                cache_read_price: Some(0.0),
                source: "custom".to_string(),
                last_updated: 0,
            }])
            .expect("insert pricing");

        let base = UsageRecord {
            timestamp: chrono::Utc::now().timestamp_millis(),
            input_tokens: 100,
            model: "free-model".to_string(),
            message_id: "known-zero".to_string(),
            ..Default::default()
        };
        database.insert_record(&base).await.expect("insert known");

        let mut unknown = base.clone();
        unknown.model = "unknown-model".to_string();
        unknown.message_id = "unknown-zero".to_string();
        database
            .insert_record(&unknown)
            .await
            .expect("insert unknown");

        let records = database
            .get_records_since(base.timestamp.saturating_sub(1))
            .await
            .expect("query records");
        let known = records
            .iter()
            .find(|record| record.message_id == "known-zero")
            .unwrap();
        let unknown = records
            .iter()
            .find(|record| record.message_id == "unknown-zero")
            .unwrap();
        assert!(known.cost_locked);
        assert!(!unknown.cost_locked);
    }

    #[tokio::test]
    async fn backfill_unlocks_legacy_unknown_zero_cost_records() {
        let temp = tempfile::tempdir().expect("temp database directory");
        let database = ProxyDatabase::new_with_path(&temp.path().join("proxy_data.db"))
            .expect("open proxy database");
        let record = UsageRecord {
            timestamp: chrono::Utc::now().timestamp_millis(),
            input_tokens: 100,
            model: "legacy-unknown".to_string(),
            message_id: "legacy-unknown".to_string(),
            cost_locked: true,
            estimated_cost: 0.0,
            ..Default::default()
        };
        database
            .insert_record(&record)
            .await
            .expect("insert legacy");
        let updated = database
            .backfill_unlocked_costs()
            .await
            .expect("repair legacy");
        assert_eq!(updated, 0);
        let stored = database
            .get_records_since(record.timestamp.saturating_sub(1))
            .await
            .expect("query legacy")
            .into_iter()
            .find(|item| item.message_id == "legacy-unknown")
            .expect("legacy record");
        assert!(!stored.cost_locked);
    }
}
