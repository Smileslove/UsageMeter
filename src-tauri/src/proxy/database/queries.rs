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
                       session_resolution_state, message_id_conflicted, ingress_kind,
                       gateway_profile_id, gateway_caller_label, usage_source, gateway_request_id
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
                   session_resolution_state, message_id_conflicted, ingress_kind,
                   gateway_profile_id, gateway_caller_label, usage_source, gateway_request_id
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
            "SELECT COALESCE(CAST(state_value AS INTEGER), 0)
             FROM daily_rollup_state
             WHERE state_key = 'merge_cache_generation'",
            [],
            |row| {
                Ok(ProxyMergeCacheSignature {
                    merge_cache_generation: row.get(0)?,
                })
            },
        )
        .map_err(|e| format!("Failed to load proxy merge cache signature: {}", e))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{SourceFilter, ToolFilter, UsageQueryFilter};

    fn temp_db() -> (tempfile::TempDir, ProxyDatabase) {
        let tmpdir = tempfile::tempdir().expect("create temp dir");
        let path = tmpdir.path().join("proxy_data.db");
        let db = ProxyDatabase::new_with_path(&path).expect("open temp db");
        (tmpdir, db)
    }

    /// 直接 SQL 插入 usage_records（触发 generation 触发器），返回插入行数。
    fn insert_record(
        db: &ProxyDatabase,
        timestamp: i64,
        message_id: &str,
        model: &str,
        session_id: Option<&str>,
        status_code: u16,
        input_tokens: u64,
        output_tokens: u64,
        api_key_prefix: Option<&str>,
        request_base_url: Option<&str>,
        client_tool: &str,
        estimated_cost: f64,
    ) {
        let conn = db.conn.lock().expect("lock conn");
        conn.execute(
            r#"
            INSERT INTO usage_records (
                timestamp, message_id, storage_dedupe_key, model, session_id, status_code,
                input_tokens, output_tokens, api_key_prefix, request_base_url, client_tool,
                estimated_cost, session_resolution_state
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'known'
            )
            "#,
            rusqlite::params![
                timestamp,
                message_id,
                format!("{client_tool}:{message_id}"),
                model,
                session_id,
                status_code,
                input_tokens,
                output_tokens,
                api_key_prefix,
                request_base_url,
                client_tool,
                estimated_cost,
            ],
        )
        .expect("insert usage record");
    }

    #[tokio::test]
    async fn get_records_since_empty_db_returns_empty_list() {
        let (_tmp, db) = temp_db();
        let records = db
            .get_records_since(0)
            .await
            .expect("query records on empty db");
        assert!(records.is_empty());
    }

    #[tokio::test]
    async fn get_records_since_includes_boundary_and_sorts_descending() {
        let (_tmp, db) = temp_db();
        insert_record(
            &db, 1_000, "m1", "gpt-4o", None, 200, 10, 10, None, None, "codex", 0.0,
        );
        insert_record(
            &db, 2_000, "m2", "gpt-4o", None, 200, 10, 10, None, None, "codex", 0.0,
        );
        insert_record(
            &db, 3_000, "m3", "gpt-4o", None, 200, 10, 10, None, None, "codex", 0.0,
        );

        let records = db.get_records_since(2_000).await.expect("query records");
        // timestamp >= cutoff：边界 2000 应包含；排序按 timestamp DESC。
        let ids: Vec<&str> = records.iter().map(|r| r.message_id.as_str()).collect();
        assert_eq!(ids, vec!["m3", "m2"]);
    }

    #[tokio::test]
    async fn get_records_between_with_source_uses_half_open_interval() {
        let (_tmp, db) = temp_db();
        insert_record(
            &db, 1_000, "m1", "gpt-4o", None, 200, 10, 10, None, None, "codex", 0.0,
        );
        insert_record(
            &db, 2_000, "m2", "gpt-4o", None, 200, 10, 10, None, None, "codex", 0.0,
        );
        insert_record(
            &db, 3_000, "m3", "gpt-4o", None, 200, 10, 10, None, None, "codex", 0.0,
        );

        let filter = UsageQueryFilter {
            source: SourceFilter::All,
            tool: ToolFilter::All,
        };
        let records = db
            .get_records_between_with_source(1_000, 3_000, true, &filter)
            .await
            .expect("query records");
        // 半开区间 [start, end)：包含 1000，不包含 3000；按 timestamp ASC。
        let ids: Vec<&str> = records.iter().map(|r| r.message_id.as_str()).collect();
        assert_eq!(ids, vec!["m1", "m2"]);
    }

    #[tokio::test]
    async fn get_records_between_with_source_filters_by_source_and_tool() {
        let (_tmp, db) = temp_db();
        insert_record(
            &db,
            1_000,
            "m1",
            "gpt-4o",
            None,
            200,
            10,
            10,
            Some("sk-a"),
            Some("https://api.a.com"),
            "codex",
            0.0,
        );
        insert_record(
            &db,
            2_000,
            "m2",
            "gpt-4o",
            None,
            200,
            10,
            10,
            Some("sk-a"),
            Some("https://api.a.com"),
            "claude_code",
            0.0,
        );
        insert_record(
            &db,
            3_000,
            "m3",
            "gpt-4o",
            None,
            200,
            10,
            10,
            Some("sk-b"),
            Some("https://api.b.com"),
            "codex",
            0.0,
        );

        let filter = UsageQueryFilter {
            source: SourceFilter::Source {
                api_key_prefixes: vec!["sk-a".to_string()],
                base_url: Some("https://api.a.com".to_string()),
            },
            tool: ToolFilter::Tool("codex".to_string()),
        };
        let records = db
            .get_records_between_with_source(0, i64::MAX, true, &filter)
            .await
            .expect("query filtered records");
        let ids: Vec<&str> = records.iter().map(|r| r.message_id.as_str()).collect();
        assert_eq!(ids, vec!["m1"]);
    }

    #[tokio::test]
    async fn get_records_between_with_source_excludes_errors_when_requested() {
        let (_tmp, db) = temp_db();
        insert_record(
            &db, 1_000, "ok", "gpt-4o", None, 200, 10, 10, None, None, "codex", 0.0,
        );
        insert_record(
            &db, 2_000, "err4", "gpt-4o", None, 404, 10, 10, None, None, "codex", 0.0,
        );
        insert_record(
            &db, 3_000, "err5", "gpt-4o", None, 500, 10, 10, None, None, "codex", 0.0,
        );

        let filter = UsageQueryFilter {
            source: SourceFilter::All,
            tool: ToolFilter::All,
        };
        let only_success = db
            .get_records_between_with_source(0, i64::MAX, false, &filter)
            .await
            .expect("query success only");
        let ids: Vec<&str> = only_success.iter().map(|r| r.message_id.as_str()).collect();
        assert_eq!(ids, vec!["ok"]);

        let with_errors = db
            .get_records_between_with_source(0, i64::MAX, true, &filter)
            .await
            .expect("query with errors");
        assert_eq!(with_errors.len(), 3);
    }

    #[tokio::test]
    async fn get_window_stats_empty_db_returns_zeroed_aggregate() {
        let (_tmp, db) = temp_db();
        let stats = db.get_window_stats(0).await.expect("empty window stats");
        assert_eq!(stats.request_count, 0);
        assert_eq!(stats.total_tokens, 0);
        assert_eq!(stats.status_2xx, 0);
        assert_eq!(stats.status_4xx, 0);
        assert_eq!(stats.status_5xx, 0);
    }

    #[tokio::test]
    async fn get_window_stats_aggregates_status_buckets_and_tokens() {
        let (_tmp, db) = temp_db();
        insert_record(
            &db, 1_000, "ok", "gpt-4o", None, 200, 100, 200, None, None, "codex", 0.0,
        );
        insert_record(
            &db, 2_000, "err4", "gpt-4o", None, 404, 50, 50, None, None, "codex", 0.0,
        );
        insert_record(
            &db, 3_000, "err5", "gpt-4o", None, 500, 10, 10, None, None, "codex", 0.0,
        );

        let stats = db.get_window_stats(0).await.expect("window stats");
        assert_eq!(stats.request_count, 3);
        // total_tokens = input + cache_create + cache_read + output
        assert_eq!(stats.total_tokens, 100 + 200 + 50 + 50 + 10 + 10);
        assert_eq!(stats.status_2xx, 1);
        assert_eq!(stats.status_4xx, 1);
        assert_eq!(stats.status_5xx, 1);

        // cutoff 边界：>= cutoff 才计入。
        let partial = db
            .get_window_stats(2_000)
            .await
            .expect("window stats from cutoff");
        assert_eq!(partial.request_count, 2);
    }

    #[tokio::test]
    async fn cleanup_old_records_deletes_old_keeps_recent() {
        let (_tmp, db) = temp_db();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let day_ms = 24 * 60 * 60 * 1000;
        insert_record(
            &db,
            now_ms - 40 * day_ms,
            "old",
            "gpt-4o",
            None,
            200,
            10,
            10,
            None,
            None,
            "codex",
            0.0,
        );
        insert_record(
            &db,
            now_ms - 10 * day_ms,
            "recent",
            "gpt-4o",
            None,
            200,
            10,
            10,
            None,
            None,
            "codex",
            0.0,
        );

        let deleted = db
            .cleanup_old_records(30)
            .await
            .expect("cleanup old records");
        assert_eq!(deleted, 1);

        let remaining = db.get_records_since(0).await.expect("query remaining");
        let ids: Vec<&str> = remaining.iter().map(|r| r.message_id.as_str()).collect();
        assert_eq!(ids, vec!["recent"]);
        assert_eq!(db.get_record_count().await.expect("count"), 1);
    }

    #[tokio::test]
    async fn cleanup_old_records_nothing_old_returns_zero() {
        let (_tmp, db) = temp_db();
        let now_ms = chrono::Utc::now().timestamp_millis();
        insert_record(
            &db, now_ms, "recent", "gpt-4o", None, 200, 10, 10, None, None, "codex", 0.0,
        );

        let deleted = db
            .cleanup_old_records(30)
            .await
            .expect("cleanup with nothing old");
        assert_eq!(deleted, 0);
        assert_eq!(db.get_record_count().await.expect("count"), 1);
    }

    #[test]
    fn get_merge_cache_signature_starts_at_one_and_tracks_delete_trigger() {
        let (_tmp, db) = temp_db();
        let initial = db
            .get_merge_cache_signature()
            .expect("initial signature")
            .merge_cache_generation;
        assert_eq!(initial, 1);

        insert_record(
            &db, 1_000, "m1", "gpt-4o", None, 200, 10, 10, None, None, "codex", 0.0,
        );
        let after_insert = db
            .get_merge_cache_signature()
            .expect("signature after insert")
            .merge_cache_generation;
        assert_eq!(after_insert, initial + 1);

        {
            let conn = db.conn.lock().expect("lock conn");
            conn.execute("DELETE FROM usage_records WHERE message_id = 'm1'", [])
                .expect("delete usage record");
        }
        let after_delete = db
            .get_merge_cache_signature()
            .expect("signature after delete")
            .merge_cache_generation;
        assert_eq!(after_delete, after_insert + 1);
    }

    #[test]
    fn get_request_time_bounds_empty_db_returns_none() {
        let (_tmp, db) = temp_db();
        let bounds = db.get_request_time_bounds().expect("time bounds");
        assert!(bounds.is_none());
    }

    #[test]
    fn get_request_time_bounds_returns_second_span_around_min_max() {
        let (_tmp, db) = temp_db();
        insert_record(
            &db, 1_500, "m1", "gpt-4o", None, 200, 10, 10, None, None, "codex", 0.0,
        );
        insert_record(
            &db, 2_500, "m2", "gpt-4o", None, 200, 10, 10, None, None, "codex", 0.0,
        );

        let bounds = db.get_request_time_bounds().expect("time bounds");
        let (start, end) = bounds.expect("non-empty bounds");
        assert_eq!(start, 1);
        assert_eq!(end, 3); // max_ts/1000 + 1
    }

    #[test]
    fn get_day_dependency_snapshot_counts_and_tracks_extremes_in_range() {
        let (_tmp, db) = temp_db();
        insert_record(
            &db, 1_000, "m1", "gpt-4o", None, 200, 10, 10, None, None, "codex", 0.0,
        );
        insert_record(
            &db, 2_000, "m2", "gpt-4o", None, 200, 10, 10, None, None, "codex", 0.0,
        );
        insert_record(
            &db, 3_000, "m3", "gpt-4o", None, 200, 10, 10, None, None, "codex", 0.0,
        );

        let snap = db
            .get_day_dependency_snapshot(1_000, 3_000)
            .expect("day dependency snapshot");
        assert_eq!(snap.record_count, 2);
        assert_eq!(snap.max_timestamp_ms, 2_000);

        let empty = db
            .get_day_dependency_snapshot(5_000, 6_000)
            .expect("empty range snapshot");
        assert_eq!(empty.record_count, 0);
        assert_eq!(empty.max_timestamp_ms, 0);
        assert_eq!(empty.max_updated_at, 0);
    }
}
