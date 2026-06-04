use crate::models::ToolFilter;
use crate::unified_usage::{has_partial_coverage, CoverageOrigin, MergedRequestFact};
use rusqlite::{params, OptionalExtension};
use std::collections::{HashMap, HashSet};

use super::{
    LocalUsageDatabase, UnifiedDailyModelSummaryRow, UnifiedDailySummaryRow,
    UnifiedDayLocalSnapshot, UnifiedDayMaterializationState,
};

impl LocalUsageDatabase {
    fn build_unified_daily_summary(
        local_date: &str,
        facts: &[MergedRequestFact],
        materialized_at: i64,
    ) -> UnifiedDailySummaryRow {
        let mut summary = UnifiedDailySummaryRow {
            local_date: local_date.to_string(),
            materialized_at,
            ..Default::default()
        };
        let mut models = HashSet::new();
        let mut success_models = HashSet::new();

        for fact in facts {
            summary.request_count += 1;
            let visible = fact.status_code.map(|code| code < 300).unwrap_or(true);
            if visible {
                summary.visible_request_count += 1;
                summary.visible_total_tokens += fact.total_tokens;
                summary.visible_input_tokens += fact.input_tokens;
                summary.visible_output_tokens += fact.output_tokens;
                summary.visible_cache_create_tokens += fact.cache_create_tokens;
                summary.visible_cache_read_tokens += fact.cache_read_tokens;
                summary.visible_cost += fact.estimated_cost;
            }
            summary.total_tokens += fact.total_tokens;
            summary.input_tokens += fact.input_tokens;
            summary.output_tokens += fact.output_tokens;
            summary.cache_create_tokens += fact.cache_create_tokens;
            summary.cache_read_tokens += fact.cache_read_tokens;
            summary.total_cost += fact.estimated_cost;

            match fact.coverage_origin {
                CoverageOrigin::ProxyOnly => summary.proxy_backed_requests += 1,
                CoverageOrigin::LocalOnly => summary.local_only_requests += 1,
                CoverageOrigin::MergedProxyPreferred => {
                    summary.proxy_backed_requests += 1;
                    summary.merged_overlap_requests += 1;
                }
            }

            if !fact.model.trim().is_empty() {
                models.insert(fact.model.clone());
            }

            if let Some(status_code) = fact.status_code {
                if status_code < 400 {
                    summary.success_request_count += 1;
                    summary.success_total_tokens += fact.total_tokens;
                    summary.success_input_tokens += fact.input_tokens;
                    summary.success_output_tokens += fact.output_tokens;
                    summary.success_cache_create_tokens += fact.cache_create_tokens;
                    summary.success_cache_read_tokens += fact.cache_read_tokens;
                    summary.success_cost += fact.estimated_cost;
                    if !fact.model.trim().is_empty() {
                        success_models.insert(fact.model.clone());
                    }
                } else if status_code < 500 {
                    summary.client_error_requests += 1;
                } else {
                    summary.server_error_requests += 1;
                }
            }
        }

        summary.model_count = models.len() as u64;
        summary.success_model_count = success_models.len() as u64;
        let has_partial =
            has_partial_coverage(summary.proxy_backed_requests, summary.local_only_requests);
        summary.has_partial_status_coverage = false;
        summary.has_partial_performance_coverage = has_partial;
        summary
    }

    fn build_unified_daily_model_summaries(
        local_date: &str,
        facts: &[MergedRequestFact],
        materialized_at: i64,
    ) -> Vec<UnifiedDailyModelSummaryRow> {
        let mut by_model: HashMap<String, UnifiedDailyModelSummaryRow> = HashMap::new();
        for fact in facts {
            let model_name = if fact.model.trim().is_empty() {
                "unknown".to_string()
            } else {
                fact.model.clone()
            };
            let entry =
                by_model
                    .entry(model_name.clone())
                    .or_insert_with(|| UnifiedDailyModelSummaryRow {
                        local_date: local_date.to_string(),
                        model_name: model_name.clone(),
                        materialized_at,
                        ..Default::default()
                    });
            entry.request_count += 1;
            let visible = fact.status_code.map(|code| code < 300).unwrap_or(true);
            if visible {
                entry.visible_request_count += 1;
                entry.visible_total_tokens += fact.total_tokens;
                entry.visible_input_tokens += fact.input_tokens;
                entry.visible_output_tokens += fact.output_tokens;
                entry.visible_cache_create_tokens += fact.cache_create_tokens;
                entry.visible_cache_read_tokens += fact.cache_read_tokens;
                entry.visible_cost += fact.estimated_cost;
            }
            entry.total_tokens += fact.total_tokens;
            entry.input_tokens += fact.input_tokens;
            entry.output_tokens += fact.output_tokens;
            entry.cache_create_tokens += fact.cache_create_tokens;
            entry.cache_read_tokens += fact.cache_read_tokens;
            entry.total_cost += fact.estimated_cost;
            if let Some(rate) = fact.output_tokens_per_second {
                if rate > 0.0 {
                    entry.rate_sum += rate;
                    entry.rate_count += 1;
                }
            }
            if let Some(ttft_ms) = fact.ttft_ms {
                if ttft_ms > 0 {
                    entry.ttft_sum += ttft_ms as f64;
                    entry.ttft_count += 1;
                }
            }
            if let Some(status_code) = fact.status_code {
                *entry.status_code_counts.entry(status_code).or_insert(0) += 1;
                if status_code < 400 {
                    entry.success_request_count += 1;
                    entry.success_total_tokens += fact.total_tokens;
                    entry.success_input_tokens += fact.input_tokens;
                    entry.success_output_tokens += fact.output_tokens;
                    entry.success_cache_create_tokens += fact.cache_create_tokens;
                    entry.success_cache_read_tokens += fact.cache_read_tokens;
                    entry.success_cost += fact.estimated_cost;
                } else if status_code < 500 {
                    entry.client_error_requests += 1;
                } else {
                    entry.server_error_requests += 1;
                }
            }
        }
        let mut rows: Vec<_> = by_model.into_values().collect();
        rows.sort_by(|a, b| a.model_name.cmp(&b.model_name));
        rows
    }

    pub(super) fn bump_unified_materialization_invalidation_version_tx(
        tx: &rusqlite::Transaction<'_>,
        updated_at: i64,
    ) -> Result<i64, String> {
        let current = tx
            .query_row(
                "SELECT COALESCE(state_value, '0') FROM local_sync_state WHERE state_key = 'unified_materialization_invalidation_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read invalidation version: {}", e))?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0);
        let next = current.saturating_add(1);
        Self::upsert_sync_state(
            tx,
            "unified_materialization_invalidation_version",
            &next.to_string(),
            updated_at,
        )?;
        Ok(next)
    }

    pub(super) fn invalidate_unified_materialization_dates_tx(
        tx: &rusqlite::Transaction<'_>,
        local_dates: &[String],
        updated_at: i64,
    ) -> Result<(), String> {
        let mut unique_dates = HashSet::new();
        for date in local_dates {
            let trimmed = date.trim();
            if trimmed.is_empty() || trimmed >= Self::today_local_date().as_str() {
                continue;
            }
            unique_dates.insert(trimmed.to_string());
        }
        if unique_dates.is_empty() {
            return Ok(());
        }

        {
            let mut delete_facts = tx
                .prepare("DELETE FROM unified_daily_materialized_facts WHERE local_date = ?1")
                .map_err(|e| format!("Failed to prepare materialized fact invalidation: {}", e))?;
            let mut delete_summary = tx
                .prepare("DELETE FROM unified_daily_summary WHERE local_date = ?1")
                .map_err(|e| format!("Failed to prepare daily summary invalidation: {}", e))?;
            let mut delete_model_summary = tx
                .prepare("DELETE FROM unified_daily_model_summary WHERE local_date = ?1")
                .map_err(|e| format!("Failed to prepare model summary invalidation: {}", e))?;
            let mut delete_state = tx
                .prepare("DELETE FROM unified_daily_materialization_state WHERE local_date = ?1")
                .map_err(|e| {
                    format!(
                        "Failed to prepare materialization state invalidation: {}",
                        e
                    )
                })?;

            for date in &unique_dates {
                delete_facts.execute([date]).map_err(|e| {
                    format!("Failed to invalidate materialized facts for {date}: {e}")
                })?;
                delete_summary
                    .execute([date])
                    .map_err(|e| format!("Failed to invalidate daily summary for {date}: {e}"))?;
                delete_model_summary
                    .execute([date])
                    .map_err(|e| format!("Failed to invalidate model summary for {date}: {e}"))?;
                delete_state
                    .execute([date])
                    .map_err(|e| format!("Failed to invalidate state for {date}: {e}"))?;
            }
        }

        Self::bump_unified_materialization_invalidation_version_tx(tx, updated_at)?;
        Ok(())
    }

    pub(super) fn clear_unified_materialization_tx(
        tx: &rusqlite::Transaction<'_>,
        updated_at: i64,
    ) -> Result<(), String> {
        tx.execute("DELETE FROM unified_daily_materialized_facts", [])
            .map_err(|e| format!("Failed to clear unified materialized facts: {}", e))?;
        tx.execute("DELETE FROM unified_daily_summary", [])
            .map_err(|e| format!("Failed to clear unified daily summary: {}", e))?;
        tx.execute("DELETE FROM unified_daily_model_summary", [])
            .map_err(|e| format!("Failed to clear unified daily model summary: {}", e))?;
        tx.execute("DELETE FROM unified_daily_materialization_state", [])
            .map_err(|e| format!("Failed to clear unified materialization state: {}", e))?;
        Self::bump_unified_materialization_invalidation_version_tx(tx, updated_at)?;
        Ok(())
    }

    pub fn get_unified_day_local_snapshot(
        &self,
        local_date: &str,
    ) -> Result<UnifiedDayLocalSnapshot, String> {
        let (start_epoch, end_epoch) = Self::local_date_epoch_bounds(local_date)?;
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            r#"
            SELECT
                (SELECT COUNT(*) FROM local_request_facts
                  WHERE timestamp >= ?1 AND timestamp < ?2),
                (SELECT COALESCE(MAX(sync_version), 0) FROM local_request_facts
                  WHERE timestamp >= ?1 AND timestamp < ?2),
                (SELECT COALESCE(MAX(timestamp), 0) FROM local_request_facts
                  WHERE timestamp >= ?1 AND timestamp < ?2),
                (SELECT COUNT(*) FROM remote_request_facts
                  WHERE timestamp >= ?1 AND timestamp < ?2),
                (SELECT COALESCE(MAX(export_seq), 0) FROM remote_request_facts
                  WHERE timestamp >= ?1 AND timestamp < ?2),
                (SELECT COALESCE(MAX(timestamp), 0) FROM remote_request_facts
                  WHERE timestamp >= ?1 AND timestamp < ?2)
            "#,
            params![start_epoch, end_epoch],
            |row| {
                Ok(UnifiedDayLocalSnapshot {
                    local_request_count: row.get::<_, i64>(0)?.max(0) as u64,
                    local_max_sync_version: row.get::<_, i64>(1)?,
                    local_max_timestamp: row.get::<_, i64>(2)?,
                    remote_request_count: row.get::<_, i64>(3)?.max(0) as u64,
                    remote_max_export_seq: row.get::<_, i64>(4)?,
                    remote_max_timestamp: row.get::<_, i64>(5)?,
                })
            },
        )
        .map_err(|e| {
            format!(
                "Failed to compute unified day local snapshot for {local_date}: {}",
                e
            )
        })
    }

    pub fn get_unified_day_materialization_state(
        &self,
        local_date: &str,
    ) -> Result<Option<UnifiedDayMaterializationState>, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            r#"
            SELECT
                local_date,
                fact_count,
                local_request_count,
                local_max_sync_version,
                local_max_timestamp,
                remote_request_count,
                remote_max_export_seq,
                remote_max_timestamp,
                proxy_record_count,
                proxy_all_record_count,
                proxy_max_timestamp_ms,
                proxy_max_updated_at,
                max_fact_timestamp_ms,
                pricing_fingerprint,
                is_finalized,
                finalized_at,
                materialized_at
            FROM unified_daily_materialization_state
            WHERE local_date = ?1
            "#,
            [local_date],
            |row| {
                Ok(UnifiedDayMaterializationState {
                    local_date: row.get(0)?,
                    fact_count: row.get::<_, i64>(1)?.max(0) as u64,
                    local_request_count: row.get::<_, i64>(2)?.max(0) as u64,
                    local_max_sync_version: row.get::<_, i64>(3)?,
                    local_max_timestamp: row.get::<_, i64>(4)?,
                    remote_request_count: row.get::<_, i64>(5)?.max(0) as u64,
                    remote_max_export_seq: row.get::<_, i64>(6)?,
                    remote_max_timestamp: row.get::<_, i64>(7)?,
                    proxy_record_count: row.get::<_, i64>(8)?.max(0) as u64,
                    proxy_all_record_count: row.get::<_, i64>(9)?.max(0) as u64,
                    proxy_max_timestamp_ms: row.get::<_, i64>(10)?,
                    proxy_max_updated_at: row.get::<_, i64>(11)?,
                    max_fact_timestamp_ms: row.get(12)?,
                    pricing_fingerprint: row.get::<_, i64>(13)?.max(0) as u64,
                    is_finalized: row.get::<_, i64>(14)? != 0,
                    finalized_at: row.get(15)?,
                    materialized_at: row.get(16)?,
                })
            },
        )
        .optional()
        .map_err(|e| format!("Failed to load unified materialization state: {}", e))
    }

    pub fn invalidate_unified_materialization_dates(
        &self,
        local_dates: &[String],
    ) -> Result<(), String> {
        if local_dates.is_empty() {
            return Ok(());
        }
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start unified invalidation transaction: {}", e))?;
        Self::invalidate_unified_materialization_dates_tx(&tx, local_dates, now)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit unified invalidation transaction: {}", e))?;
        Ok(())
    }

    pub fn clear_unified_materialization(&self) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start unified clear transaction: {}", e))?;
        Self::clear_unified_materialization_tx(&tx, now)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit unified clear transaction: {}", e))?;
        Ok(())
    }

    pub fn replace_unified_day_materialization(
        &self,
        local_date: &str,
        facts: &[(String, MergedRequestFact)],
        state: &UnifiedDayMaterializationState,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start unified materialization transaction: {}", e))?;

        tx.execute(
            "DELETE FROM unified_daily_materialized_facts WHERE local_date = ?1",
            [local_date],
        )
        .map_err(|e| format!("Failed to clear unified materialized facts: {}", e))?;

        {
            let mut stmt = tx
                .prepare(
                    r#"
                    INSERT INTO unified_daily_materialized_facts (
                        local_date, request_key, session_id, project_name, project_path,
                        api_key_prefix, request_base_url, tool, timestamp_sec, timestamp_ms,
                        model, input_tokens, output_tokens, cache_create_tokens,
                        cache_read_tokens, total_tokens, estimated_cost, coverage_origin,
                        status_code, duration_ms, output_tokens_per_second, ttft_ms, source_label
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5,
                        ?6, ?7, ?8, ?9, ?10,
                        ?11, ?12, ?13, ?14,
                        ?15, ?16, ?17, ?18,
                        ?19, ?20, ?21, ?22, ?23
                    )
                    "#,
                )
                .map_err(|e| format!("Failed to prepare unified fact insert: {}", e))?;

            for (request_key, fact) in facts {
                stmt.execute(params![
                    local_date,
                    request_key,
                    fact.session_id,
                    fact.project_name,
                    fact.project_path,
                    fact.api_key_prefix,
                    fact.request_base_url,
                    fact.tool,
                    fact.timestamp_sec,
                    fact.timestamp_ms,
                    fact.model,
                    fact.input_tokens as i64,
                    fact.output_tokens as i64,
                    fact.cache_create_tokens as i64,
                    fact.cache_read_tokens as i64,
                    fact.total_tokens as i64,
                    fact.estimated_cost,
                    fact.coverage_origin.as_storage_str(),
                    fact.status_code.map(i64::from),
                    fact.duration_ms.map(|v| v as i64),
                    fact.output_tokens_per_second,
                    fact.ttft_ms.map(|v| v as i64),
                    fact.source_label,
                ])
                .map_err(|e| format!("Failed to insert unified materialized fact: {}", e))?;
            }
        }

        let fact_values: Vec<MergedRequestFact> =
            facts.iter().map(|(_, fact)| fact.clone()).collect();
        let summary =
            Self::build_unified_daily_summary(local_date, &fact_values, state.materialized_at);
        let model_summaries = Self::build_unified_daily_model_summaries(
            local_date,
            &fact_values,
            state.materialized_at,
        );

        tx.execute(
            r#"
            INSERT INTO unified_daily_materialization_state (
                local_date, fact_count, local_request_count, local_max_sync_version, local_max_timestamp,
                remote_request_count, remote_max_export_seq, remote_max_timestamp,
                proxy_record_count, proxy_all_record_count, proxy_max_timestamp_ms, proxy_max_updated_at,
                max_fact_timestamp_ms,
                pricing_fingerprint, is_finalized, finalized_at, materialized_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5,
                ?6, ?7, ?8,
                ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17
            )
            ON CONFLICT(local_date) DO UPDATE SET
                fact_count = excluded.fact_count,
                local_request_count = excluded.local_request_count,
                local_max_sync_version = excluded.local_max_sync_version,
                local_max_timestamp = excluded.local_max_timestamp,
                remote_request_count = excluded.remote_request_count,
                remote_max_export_seq = excluded.remote_max_export_seq,
                remote_max_timestamp = excluded.remote_max_timestamp,
                proxy_record_count = excluded.proxy_record_count,
                proxy_all_record_count = excluded.proxy_all_record_count,
                proxy_max_timestamp_ms = excluded.proxy_max_timestamp_ms,
                proxy_max_updated_at = excluded.proxy_max_updated_at,
                max_fact_timestamp_ms = excluded.max_fact_timestamp_ms,
                pricing_fingerprint = excluded.pricing_fingerprint,
                is_finalized = excluded.is_finalized,
                finalized_at = excluded.finalized_at,
                materialized_at = excluded.materialized_at
            "#,
            params![
                state.local_date,
                state.fact_count as i64,
                state.local_request_count as i64,
                state.local_max_sync_version,
                state.local_max_timestamp,
                state.remote_request_count as i64,
                state.remote_max_export_seq,
                state.remote_max_timestamp,
                state.proxy_record_count as i64,
                state.proxy_all_record_count as i64,
                state.proxy_max_timestamp_ms,
                state.proxy_max_updated_at,
                state.max_fact_timestamp_ms,
                state.pricing_fingerprint as i64,
                if state.is_finalized { 1 } else { 0 },
                state.finalized_at,
                state.materialized_at,
            ],
        )
        .map_err(|e| format!("Failed to upsert unified materialization state: {}", e))?;

        tx.execute(
            r#"
            INSERT INTO unified_daily_summary (
                local_date, request_count, visible_request_count, total_tokens, visible_total_tokens,
                input_tokens, visible_input_tokens, output_tokens, visible_output_tokens,
                cache_create_tokens, visible_cache_create_tokens, cache_read_tokens, visible_cache_read_tokens,
                total_cost, visible_cost, success_request_count, success_total_tokens, success_input_tokens, success_output_tokens,
                success_cache_create_tokens, success_cache_read_tokens, success_cost,
                client_error_requests, server_error_requests, model_count, success_model_count,
                proxy_backed_requests, local_only_requests, merged_overlap_requests,
                has_partial_status_coverage, has_partial_performance_coverage, materialized_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5,
                ?6, ?7, ?8, ?9,
                ?10, ?11, ?12, ?13,
                ?14, ?15, ?16, ?17, ?18, ?19,
                ?20, ?21, ?22,
                ?23, ?24, ?25, ?26,
                ?27, ?28, ?29,
                ?30, ?31, ?32
            )
            ON CONFLICT(local_date) DO UPDATE SET
                request_count = excluded.request_count,
                visible_request_count = excluded.visible_request_count,
                total_tokens = excluded.total_tokens,
                visible_total_tokens = excluded.visible_total_tokens,
                input_tokens = excluded.input_tokens,
                visible_input_tokens = excluded.visible_input_tokens,
                output_tokens = excluded.output_tokens,
                visible_output_tokens = excluded.visible_output_tokens,
                cache_create_tokens = excluded.cache_create_tokens,
                visible_cache_create_tokens = excluded.visible_cache_create_tokens,
                cache_read_tokens = excluded.cache_read_tokens,
                visible_cache_read_tokens = excluded.visible_cache_read_tokens,
                total_cost = excluded.total_cost,
                visible_cost = excluded.visible_cost,
                success_request_count = excluded.success_request_count,
                success_total_tokens = excluded.success_total_tokens,
                success_input_tokens = excluded.success_input_tokens,
                success_output_tokens = excluded.success_output_tokens,
                success_cache_create_tokens = excluded.success_cache_create_tokens,
                success_cache_read_tokens = excluded.success_cache_read_tokens,
                success_cost = excluded.success_cost,
                client_error_requests = excluded.client_error_requests,
                server_error_requests = excluded.server_error_requests,
                model_count = excluded.model_count,
                success_model_count = excluded.success_model_count,
                proxy_backed_requests = excluded.proxy_backed_requests,
                local_only_requests = excluded.local_only_requests,
                merged_overlap_requests = excluded.merged_overlap_requests,
                has_partial_status_coverage = excluded.has_partial_status_coverage,
                has_partial_performance_coverage = excluded.has_partial_performance_coverage,
                materialized_at = excluded.materialized_at
            "#,
            params![
                summary.local_date,
                summary.request_count as i64,
                summary.visible_request_count as i64,
                summary.total_tokens as i64,
                summary.visible_total_tokens as i64,
                summary.input_tokens as i64,
                summary.visible_input_tokens as i64,
                summary.output_tokens as i64,
                summary.visible_output_tokens as i64,
                summary.cache_create_tokens as i64,
                summary.visible_cache_create_tokens as i64,
                summary.cache_read_tokens as i64,
                summary.visible_cache_read_tokens as i64,
                summary.total_cost,
                summary.visible_cost,
                summary.success_request_count as i64,
                summary.success_total_tokens as i64,
                summary.success_input_tokens as i64,
                summary.success_output_tokens as i64,
                summary.success_cache_create_tokens as i64,
                summary.success_cache_read_tokens as i64,
                summary.success_cost,
                summary.client_error_requests as i64,
                summary.server_error_requests as i64,
                summary.model_count as i64,
                summary.success_model_count as i64,
                summary.proxy_backed_requests as i64,
                summary.local_only_requests as i64,
                summary.merged_overlap_requests as i64,
                if summary.has_partial_status_coverage { 1 } else { 0 },
                if summary.has_partial_performance_coverage { 1 } else { 0 },
                summary.materialized_at,
            ],
        )
        .map_err(|e| format!("Failed to upsert unified daily summary: {}", e))?;

        tx.execute(
            "DELETE FROM unified_daily_model_summary WHERE local_date = ?1",
            [local_date],
        )
        .map_err(|e| format!("Failed to clear unified daily model summary: {}", e))?;
        {
            let mut stmt = tx
                .prepare(
                    r#"
                    INSERT INTO unified_daily_model_summary (
                        local_date, model_name, request_count, visible_request_count, total_tokens, visible_total_tokens, input_tokens,
                        visible_input_tokens, output_tokens, visible_output_tokens, cache_create_tokens, visible_cache_create_tokens,
                        cache_read_tokens, visible_cache_read_tokens, total_cost, visible_cost,
                        success_request_count, success_total_tokens, success_input_tokens,
                        success_output_tokens, success_cache_create_tokens,
                        success_cache_read_tokens, success_cost, client_error_requests,
                        server_error_requests, local_only_requests, rate_sum, rate_count, ttft_sum, ttft_count,
                        status_counts_json, materialized_at
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                        ?8, ?9, ?10, ?11, ?12,
                        ?13, ?14, ?15, ?16,
                        ?17, ?18, ?19,
                        ?20, ?21, ?22, ?23, ?24,
                        ?25, ?26, ?27, ?28, ?29,
                        ?30, ?31, ?32
                    )
                    "#,
                )
                .map_err(|e| format!("Failed to prepare unified daily model summary insert: {}", e))?;
            for row in &model_summaries {
                let status_counts_json =
                    serde_json::to_string(&row.status_code_counts).map_err(|e| {
                        format!("Failed to serialize unified model status counts: {}", e)
                    })?;
                stmt.execute(params![
                    row.local_date,
                    row.model_name,
                    row.request_count as i64,
                    row.visible_request_count as i64,
                    row.total_tokens as i64,
                    row.visible_total_tokens as i64,
                    row.input_tokens as i64,
                    row.visible_input_tokens as i64,
                    row.output_tokens as i64,
                    row.visible_output_tokens as i64,
                    row.cache_create_tokens as i64,
                    row.visible_cache_create_tokens as i64,
                    row.cache_read_tokens as i64,
                    row.visible_cache_read_tokens as i64,
                    row.total_cost,
                    row.visible_cost,
                    row.success_request_count as i64,
                    row.success_total_tokens as i64,
                    row.success_input_tokens as i64,
                    row.success_output_tokens as i64,
                    row.success_cache_create_tokens as i64,
                    row.success_cache_read_tokens as i64,
                    row.success_cost,
                    row.client_error_requests as i64,
                    row.server_error_requests as i64,
                    row.local_only_requests as i64,
                    row.rate_sum,
                    row.rate_count as i64,
                    row.ttft_sum,
                    row.ttft_count as i64,
                    status_counts_json,
                    row.materialized_at,
                ])
                .map_err(|e| format!("Failed to insert unified daily model summary: {}", e))?;
            }
        }

        tx.commit().map_err(|e| {
            format!(
                "Failed to commit unified materialization transaction: {}",
                e
            )
        })?;
        Ok(())
    }

    pub fn get_unified_facts_for_dates(
        &self,
        local_dates: &[String],
        tool_filter: &ToolFilter,
    ) -> Result<Vec<MergedRequestFact>, String> {
        if local_dates.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().unwrap();
        let placeholders = std::iter::repeat_n("?", local_dates.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            r#"
            SELECT
                request_key, session_id, project_name, project_path, api_key_prefix, request_base_url,
                tool, timestamp_sec, timestamp_ms, model, input_tokens, output_tokens,
                cache_create_tokens, cache_read_tokens, total_tokens, estimated_cost,
                coverage_origin, status_code, duration_ms, output_tokens_per_second, ttft_ms,
                source_label
            FROM unified_daily_materialized_facts
            WHERE local_date IN ({placeholders})
            ORDER BY timestamp_ms ASC
            "#
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare unified fact query: {}", e))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(local_dates.iter()), |row| {
                Ok(MergedRequestFact {
                    canonical_request_key: row.get(0)?,
                    session_id: row.get(1)?,
                    project_name: row.get(2)?,
                    project_path: row.get(3)?,
                    api_key_prefix: row.get(4)?,
                    request_base_url: row.get(5)?,
                    tool: row.get(6)?,
                    timestamp_sec: row.get(7)?,
                    timestamp_ms: row.get(8)?,
                    model: row.get(9)?,
                    input_tokens: row.get::<_, i64>(10)?.max(0) as u64,
                    output_tokens: row.get::<_, i64>(11)?.max(0) as u64,
                    cache_create_tokens: row.get::<_, i64>(12)?.max(0) as u64,
                    cache_read_tokens: row.get::<_, i64>(13)?.max(0) as u64,
                    total_tokens: row.get::<_, i64>(14)?.max(0) as u64,
                    estimated_cost: row.get(15)?,
                    coverage_origin: CoverageOrigin::from_storage_str(
                        row.get::<_, String>(16)?.as_str(),
                    ),
                    status_code: row.get::<_, Option<i64>>(17)?.map(|v| v as u16),
                    duration_ms: row.get::<_, Option<i64>>(18)?.map(|v| v.max(0) as u64),
                    output_tokens_per_second: row.get(19)?,
                    ttft_ms: row.get::<_, Option<i64>>(20)?.map(|v| v.max(0) as u64),
                    source_label: row.get(21)?,
                })
            })
            .map_err(|e| format!("Failed to query unified materialized facts: {}", e))?;

        let mut facts = Vec::new();
        for row in rows {
            let fact =
                row.map_err(|e| format!("Failed to read unified materialized fact: {}", e))?;
            if matches!(
                tool_filter,
                ToolFilter::Tool(tool) if !tool.trim().is_empty() && fact.tool != *tool
            ) {
                continue;
            }
            facts.push(fact);
        }
        Ok(facts)
    }

    pub fn get_unified_daily_summaries_between(
        &self,
        start_date_inclusive: &str,
        end_date_exclusive: &str,
    ) -> Result<Vec<UnifiedDailySummaryRow>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                r#"
                SELECT
                    local_date, request_count, visible_request_count, total_tokens, visible_total_tokens,
                    input_tokens, visible_input_tokens, output_tokens, visible_output_tokens,
                    cache_create_tokens, visible_cache_create_tokens, cache_read_tokens, visible_cache_read_tokens,
                    total_cost, visible_cost, success_request_count, success_total_tokens, success_input_tokens,
                    success_output_tokens, success_cache_create_tokens, success_cache_read_tokens, success_cost,
                    client_error_requests, server_error_requests, model_count, success_model_count,
                    proxy_backed_requests, local_only_requests, merged_overlap_requests,
                    has_partial_status_coverage, has_partial_performance_coverage, materialized_at
                FROM unified_daily_summary
                WHERE local_date >= ?1 AND local_date < ?2
                ORDER BY local_date ASC
                "#,
            )
            .map_err(|e| format!("Failed to prepare unified daily summary query: {}", e))?;
        let rows = stmt
            .query_map([start_date_inclusive, end_date_exclusive], |row| {
                Ok(UnifiedDailySummaryRow {
                    local_date: row.get(0)?,
                    request_count: row.get::<_, i64>(1)?.max(0) as u64,
                    visible_request_count: row.get::<_, i64>(2)?.max(0) as u64,
                    total_tokens: row.get::<_, i64>(3)?.max(0) as u64,
                    visible_total_tokens: row.get::<_, i64>(4)?.max(0) as u64,
                    input_tokens: row.get::<_, i64>(5)?.max(0) as u64,
                    visible_input_tokens: row.get::<_, i64>(6)?.max(0) as u64,
                    output_tokens: row.get::<_, i64>(7)?.max(0) as u64,
                    visible_output_tokens: row.get::<_, i64>(8)?.max(0) as u64,
                    cache_create_tokens: row.get::<_, i64>(9)?.max(0) as u64,
                    visible_cache_create_tokens: row.get::<_, i64>(10)?.max(0) as u64,
                    cache_read_tokens: row.get::<_, i64>(11)?.max(0) as u64,
                    visible_cache_read_tokens: row.get::<_, i64>(12)?.max(0) as u64,
                    total_cost: row.get(13)?,
                    visible_cost: row.get(14)?,
                    success_request_count: row.get::<_, i64>(15)?.max(0) as u64,
                    success_total_tokens: row.get::<_, i64>(16)?.max(0) as u64,
                    success_input_tokens: row.get::<_, i64>(17)?.max(0) as u64,
                    success_output_tokens: row.get::<_, i64>(18)?.max(0) as u64,
                    success_cache_create_tokens: row.get::<_, i64>(19)?.max(0) as u64,
                    success_cache_read_tokens: row.get::<_, i64>(20)?.max(0) as u64,
                    success_cost: row.get(21)?,
                    client_error_requests: row.get::<_, i64>(22)?.max(0) as u64,
                    server_error_requests: row.get::<_, i64>(23)?.max(0) as u64,
                    model_count: row.get::<_, i64>(24)?.max(0) as u64,
                    success_model_count: row.get::<_, i64>(25)?.max(0) as u64,
                    proxy_backed_requests: row.get::<_, i64>(26)?.max(0) as u64,
                    local_only_requests: row.get::<_, i64>(27)?.max(0) as u64,
                    merged_overlap_requests: row.get::<_, i64>(28)?.max(0) as u64,
                    has_partial_status_coverage: row.get::<_, i64>(29)? != 0,
                    has_partial_performance_coverage: row.get::<_, i64>(30)? != 0,
                    materialized_at: row.get(31)?,
                })
            })
            .map_err(|e| format!("Failed to query unified daily summaries: {}", e))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| format!("Failed to read unified daily summary: {}", e))?);
        }
        Ok(result)
    }

    pub fn get_unified_daily_model_summaries_between(
        &self,
        start_date_inclusive: &str,
        end_date_exclusive: &str,
    ) -> Result<Vec<UnifiedDailyModelSummaryRow>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                r#"
                SELECT
                    local_date, model_name, request_count, visible_request_count, total_tokens, visible_total_tokens, input_tokens,
                    visible_input_tokens, output_tokens, visible_output_tokens, cache_create_tokens, visible_cache_create_tokens,
                    cache_read_tokens, visible_cache_read_tokens, total_cost, visible_cost,
                    success_request_count, success_total_tokens, success_input_tokens,
                    success_output_tokens, success_cache_create_tokens,
                    success_cache_read_tokens, success_cost, client_error_requests,
                    server_error_requests, local_only_requests, rate_sum, rate_count, ttft_sum, ttft_count,
                    status_counts_json, materialized_at
                FROM unified_daily_model_summary
                WHERE local_date >= ?1 AND local_date < ?2
                ORDER BY local_date ASC, model_name ASC
                "#,
            )
            .map_err(|e| format!("Failed to prepare unified daily model summary query: {}", e))?;
        let rows = stmt
            .query_map([start_date_inclusive, end_date_exclusive], |row| {
                let status_counts_json: String = row.get(30)?;
                let status_code_counts: HashMap<u16, u64> =
                    serde_json::from_str(&status_counts_json).unwrap_or_default();
                Ok(UnifiedDailyModelSummaryRow {
                    local_date: row.get(0)?,
                    model_name: row.get(1)?,
                    request_count: row.get::<_, i64>(2)?.max(0) as u64,
                    visible_request_count: row.get::<_, i64>(3)?.max(0) as u64,
                    total_tokens: row.get::<_, i64>(4)?.max(0) as u64,
                    visible_total_tokens: row.get::<_, i64>(5)?.max(0) as u64,
                    input_tokens: row.get::<_, i64>(6)?.max(0) as u64,
                    visible_input_tokens: row.get::<_, i64>(7)?.max(0) as u64,
                    output_tokens: row.get::<_, i64>(8)?.max(0) as u64,
                    visible_output_tokens: row.get::<_, i64>(9)?.max(0) as u64,
                    cache_create_tokens: row.get::<_, i64>(10)?.max(0) as u64,
                    visible_cache_create_tokens: row.get::<_, i64>(11)?.max(0) as u64,
                    cache_read_tokens: row.get::<_, i64>(12)?.max(0) as u64,
                    visible_cache_read_tokens: row.get::<_, i64>(13)?.max(0) as u64,
                    total_cost: row.get(14)?,
                    visible_cost: row.get(15)?,
                    success_request_count: row.get::<_, i64>(16)?.max(0) as u64,
                    success_total_tokens: row.get::<_, i64>(17)?.max(0) as u64,
                    success_input_tokens: row.get::<_, i64>(18)?.max(0) as u64,
                    success_output_tokens: row.get::<_, i64>(19)?.max(0) as u64,
                    success_cache_create_tokens: row.get::<_, i64>(20)?.max(0) as u64,
                    success_cache_read_tokens: row.get::<_, i64>(21)?.max(0) as u64,
                    success_cost: row.get(22)?,
                    client_error_requests: row.get::<_, i64>(23)?.max(0) as u64,
                    server_error_requests: row.get::<_, i64>(24)?.max(0) as u64,
                    local_only_requests: row.get::<_, i64>(25)?.max(0) as u64,
                    rate_sum: row.get(26)?,
                    rate_count: row.get::<_, i64>(27)?.max(0) as u64,
                    ttft_sum: row.get(28)?,
                    ttft_count: row.get::<_, i64>(29)?.max(0) as u64,
                    status_code_counts,
                    materialized_at: row.get(31)?,
                })
            })
            .map_err(|e| format!("Failed to query unified daily model summaries: {}", e))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(
                row.map_err(|e| format!("Failed to read unified daily model summary: {}", e))?,
            );
        }
        Ok(result)
    }
}
