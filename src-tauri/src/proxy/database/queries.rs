use super::session;
use super::{ProxyDatabase, ProxyDayDependencySnapshot, ProxyMergeCacheSignature, WindowAggregate};
use crate::models::UsageQueryFilter;

impl ProxyDatabase {
    #[allow(dead_code)]
    pub async fn get_records_since(
        &self,
        cutoff_ms: i64,
    ) -> Result<Vec<crate::proxy::types::UsageRecord>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT timestamp, message_id, input_tokens, output_tokens,
                       cache_create_tokens, cache_read_tokens, model, session_id,
                       request_start_time, request_end_time, duration_ms, output_tokens_per_second,
                       ttft_ms, status_code, estimated_cost, pricing_snapshot_id, cost_locked,
                       api_key_prefix, request_base_url, client_tool, proxy_profile_id,
                       client_detection_method, storage_dedupe_key, canonical_request_key,
                       session_resolution_state, message_id_conflicted
                FROM usage_records
                WHERE timestamp >= ?1
                ORDER BY timestamp DESC
                "#,
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let records = stmt
            .query_map([cutoff_ms], session::usage_record_from_row)
            .map_err(|e| format!("Failed to query records: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect records: {}", e))?;

        Ok(records)
    }

    pub async fn get_records_between_with_source(
        &self,
        start_ms: i64,
        end_ms: i64,
        include_errors: bool,
        usage_filter: &UsageQueryFilter,
    ) -> Result<Vec<crate::proxy::types::UsageRecord>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let status_filter = if include_errors {
            ""
        } else {
            "AND status_code >= 200 AND status_code < 300"
        };

        let (filter_where, filter_params) = Self::build_usage_filter_sql(usage_filter);

        let sql = format!(
            r#"
            SELECT timestamp, message_id, input_tokens, output_tokens,
                   cache_create_tokens, cache_read_tokens, model, session_id,
                   request_start_time, request_end_time, duration_ms, output_tokens_per_second,
                   ttft_ms, status_code, estimated_cost, pricing_snapshot_id, cost_locked,
                   api_key_prefix, request_base_url, client_tool, proxy_profile_id,
                   client_detection_method, storage_dedupe_key, canonical_request_key,
                   session_resolution_state, message_id_conflicted
            FROM usage_records
            WHERE timestamp >= ?1 AND timestamp < ?2
              {status_filter}
              {filter_where}
            ORDER BY timestamp ASC
            "#
        );

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(start_ms), Box::new(end_ms)];
        for p in &filter_params {
            params_vec.push(Box::new(p.clone()));
        }
        let params: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

        let records = stmt
            .query_map(params.as_slice(), session::usage_record_from_row)
            .map_err(|e| format!("Failed to query records: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect records: {}", e))?;

        Ok(records)
    }

    pub async fn get_record_count(&self) -> Result<usize, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM usage_records", [], |row| row.get(0))
            .map_err(|e| format!("Failed to count records: {}", e))?;

        Ok(count as usize)
    }

    pub fn get_merge_cache_signature(&self) -> Result<ProxyMergeCacheSignature, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;
        conn.query_row(
            r#"
            SELECT
                (SELECT COUNT(*) FROM usage_records),
                (SELECT COALESCE(MAX(timestamp), 0) FROM usage_records),
                (SELECT COALESCE(MAX(updated_at), 0) FROM usage_records),
                (SELECT COALESCE(MAX(last_updated), 0) FROM session_stats)
            "#,
            [],
            |row| {
                Ok(ProxyMergeCacheSignature {
                    usage_record_count: row.get::<_, i64>(0)?.max(0) as u64,
                    max_timestamp: row.get::<_, i64>(1)?,
                    max_updated_at: row.get::<_, i64>(2)?,
                    session_stats_max_updated_at: row.get::<_, i64>(3)?,
                })
            },
        )
        .map_err(|e| format!("Failed to compute proxy merge cache signature: {}", e))
    }

    pub fn get_day_dependency_snapshot(
        &self,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<ProxyDayDependencySnapshot, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;
        conn.query_row(
            r#"
            SELECT
                COUNT(*),
                COALESCE(MAX(timestamp), 0),
                COALESCE(MAX(updated_at), 0)
            FROM usage_records
            WHERE timestamp >= ?1 AND timestamp < ?2
            "#,
            rusqlite::params![start_ms, end_ms],
            |row| {
                Ok(ProxyDayDependencySnapshot {
                    record_count: row.get::<_, i64>(0)?.max(0) as u64,
                    max_timestamp_ms: row.get::<_, i64>(1)?,
                    max_updated_at: row.get::<_, i64>(2)?,
                })
            },
        )
        .map_err(|e| format!("Failed to compute proxy day dependency snapshot: {}", e))
    }

    pub fn get_request_time_bounds(&self) -> Result<Option<(i64, i64)>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;
        conn.query_row(
            "SELECT MIN(timestamp), MAX(timestamp) FROM usage_records",
            [],
            |row| {
                let min_ts: Option<i64> = row.get(0)?;
                let max_ts: Option<i64> = row.get(1)?;
                Ok(match (min_ts, max_ts) {
                    (Some(start_ms), Some(end_ms)) => {
                        Some((start_ms / 1000, (end_ms / 1000).saturating_add(1)))
                    }
                    _ => None,
                })
            },
        )
        .map_err(|e| format!("Failed to query proxy request time bounds: {}", e))
    }

    #[allow(dead_code)]
    pub async fn cleanup_old_records(&self, days: i64) -> Result<usize, String> {
        let cutoff = chrono::Utc::now().timestamp_millis() - (days * 24 * 60 * 60 * 1000);
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let affected = conn
            .execute("DELETE FROM usage_records WHERE timestamp < ?1", [cutoff])
            .map_err(|e| format!("Failed to cleanup records: {}", e))?;

        if affected > 0 {
            if let Ok(local_db) = crate::local_usage::LocalUsageDatabase::get_global() {
                let _ = local_db.clear_unified_materialization();
            }
        }

        Ok(affected)
    }

    #[allow(dead_code)]
    pub async fn get_window_stats(&self, cutoff_ms: i64) -> Result<WindowAggregate, String> {
        self.get_window_stats_filtered(cutoff_ms, true).await
    }
}
