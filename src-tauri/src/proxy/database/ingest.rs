use super::session;
use super::ProxyDatabase;
use crate::models::ModelPricingConfig;
use crate::proxy::types::UsageRecord;
use rusqlite::Connection;

impl ProxyDatabase {
    fn current_pricing_context(&self) -> (Vec<ModelPricingConfig>, String, String) {
        let settings = crate::commands::load_settings().unwrap_or_default();
        let mut pricings = settings.model_pricing.pricings;
        if let Ok(db_pricings) = self.get_all_model_pricings() {
            pricings.extend(db_pricings);
        }
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
             client_tool, proxy_profile_id, client_detection_method, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, NULL, ?19, ?20, 1, ?21, ?22, ?23, ?24, ?25, ?26)
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
                &record.api_key_prefix,
                &record.request_base_url,
                &record.client_tool,
                &record.proxy_profile_id,
                &record.client_detection_method,
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

    pub(super) fn refresh_daily_summary_for_date_conn(
        conn: &Connection,
        date: &str,
    ) -> Result<(), String> {
        let settings = crate::commands::load_settings().unwrap_or_default();
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

        for (id, timestamp, input, output, cache_create, cache_read, model) in &records {
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
            records.len()
        );
        if !touched_dates.is_empty() {
            if let Ok(local_db) = crate::local_usage::LocalUsageDatabase::get_global() {
                let _ = local_db.invalidate_unified_materialization_dates(
                    &touched_dates.into_iter().collect::<Vec<_>>(),
                );
            }
        }
        Ok(records.len())
    }
}
