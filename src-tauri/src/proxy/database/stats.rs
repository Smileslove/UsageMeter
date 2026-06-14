use super::{
    ProxyDatabase, StatusCodeDistribution, WindowAggregate, WindowRateStats,
    LEGACY_UNMATCHED_SESSION_ID,
};
use crate::models::ModelPricingConfig;
use crate::proxy::types::SessionStats;

impl ProxyDatabase {
    #[allow(dead_code)]
    pub async fn get_status_code_distribution(
        &self,
        cutoff_ms: i64,
    ) -> Result<Vec<StatusCodeDistribution>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT status_code, COUNT(*) as count
                FROM usage_records
                WHERE timestamp >= ?1
                GROUP BY status_code
                ORDER BY count DESC
                "#,
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let distribution = stmt
            .query_map([cutoff_ms], |row| {
                let status_code: i64 = row.get(0)?;
                let count: i64 = row.get(1)?;
                let category = if (200..300).contains(&status_code) {
                    "success".to_string()
                } else if (400..500).contains(&status_code) {
                    "client_error".to_string()
                } else if status_code >= 500 {
                    "server_error".to_string()
                } else {
                    "other".to_string()
                };
                Ok(StatusCodeDistribution {
                    status_code,
                    count,
                    category,
                })
            })
            .map_err(|e| format!("Failed to query status code distribution: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect status code distribution: {}", e))?;

        Ok(distribution)
    }

    pub async fn get_window_stats_filtered(
        &self,
        cutoff_ms: i64,
        include_errors: bool,
    ) -> Result<WindowAggregate, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let stats = if include_errors {
            conn.query_row(
                r#"
                SELECT
                    COUNT(*) as request_count,
                    COALESCE(SUM(input_tokens + cache_create_tokens + cache_read_tokens + output_tokens), 0) as total_tokens,
                    COALESCE(SUM(input_tokens), 0) as input_tokens,
                    COALESCE(SUM(output_tokens), 0) as output_tokens,
                    COALESCE(SUM(cache_create_tokens), 0) as cache_create_tokens,
                    COALESCE(SUM(cache_read_tokens), 0) as cache_read_tokens,
                    COALESCE(SUM(CASE WHEN status_code >= 200 AND status_code < 300 THEN 1 ELSE 0 END), 0) as status_2xx,
                    COALESCE(SUM(CASE WHEN status_code >= 400 AND status_code < 500 THEN 1 ELSE 0 END), 0) as status_4xx,
                    COALESCE(SUM(CASE WHEN status_code >= 500 AND status_code < 600 THEN 1 ELSE 0 END), 0) as status_5xx
                FROM usage_records
                WHERE timestamp >= ?1
                "#,
                [cutoff_ms],
                |row| {
                    Ok(WindowAggregate {
                        request_count: row.get(0)?,
                        total_tokens: row.get(1)?,
                        input_tokens: row.get(2)?,
                        output_tokens: row.get(3)?,
                        cache_create_tokens: row.get(4)?,
                        cache_read_tokens: row.get(5)?,
                        status_2xx: row.get(6)?,
                        status_4xx: row.get(7)?,
                        status_5xx: row.get(8)?,
                    })
                },
            )
        } else {
            conn.query_row(
                r#"
                SELECT
                    COUNT(*) as request_count,
                    COALESCE(SUM(input_tokens + cache_create_tokens + cache_read_tokens + output_tokens), 0) as total_tokens,
                    COALESCE(SUM(input_tokens), 0) as input_tokens,
                    COALESCE(SUM(output_tokens), 0) as output_tokens,
                    COALESCE(SUM(cache_create_tokens), 0) as cache_create_tokens,
                    COALESCE(SUM(cache_read_tokens), 0) as cache_read_tokens,
                    COUNT(*) as status_2xx,
                    0 as status_4xx,
                    0 as status_5xx
                FROM usage_records
                WHERE timestamp >= ?1
                  AND status_code >= 200 AND status_code < 300
                "#,
                [cutoff_ms],
                |row| {
                    Ok(WindowAggregate {
                        request_count: row.get(0)?,
                        total_tokens: row.get(1)?,
                        input_tokens: row.get(2)?,
                        output_tokens: row.get(3)?,
                        cache_create_tokens: row.get(4)?,
                        cache_read_tokens: row.get(5)?,
                        status_2xx: row.get(6)?,
                        status_4xx: row.get(7)?,
                        status_5xx: row.get(8)?,
                    })
                },
            )
        }
        .map_err(|e| format!("Failed to get window stats: {}", e))?;

        Ok(stats)
    }

    #[allow(dead_code)]
    pub async fn get_session_stats(
        &self,
        session_id: &str,
        _pricings: &[ModelPricingConfig],
        _match_mode: &str,
    ) -> Result<Option<SessionStats>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT
                    session_id,
                    COUNT(*) as total_requests,
                    CASE
                        WHEN COUNT(DISTINCT COALESCE(NULLIF(client_tool, ''), ?2)) = 1
                        THEN MIN(COALESCE(NULLIF(client_tool, ''), ?2))
                        ELSE 'mixed'
                    END as session_tool,
                    COALESCE(SUM(input_tokens), 0) as total_input_tokens,
                    COALESCE(SUM(output_tokens), 0) as total_output_tokens,
                    COALESCE(SUM(cache_create_tokens), 0) as total_cache_create_tokens,
                    COALESCE(SUM(cache_read_tokens), 0) as total_cache_read_tokens,
                    COALESCE(SUM(duration_ms), 0) as total_duration_ms,
                    MIN(request_start_time) as first_request_time,
                    MAX(request_end_time) as last_request_time,
                    GROUP_CONCAT(DISTINCT model) as models,
                    AVG(ttft_ms) as avg_ttft_ms,
                    SUM(CASE WHEN status_code < 400 THEN 1 ELSE 0 END) as success_requests,
                    SUM(CASE WHEN status_code >= 400 THEN 1 ELSE 0 END) as error_requests,
                    COALESCE(SUM(estimated_cost), 0) as estimated_cost
                FROM usage_records
                WHERE session_id = ?1
                GROUP BY session_id
                "#,
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let result = stmt.query_row(
            rusqlite::params![session_id, crate::models::DEFAULT_CLIENT_TOOL],
            |row| {
                let total_output_tokens: i64 = row.get(3)?;
                let total_duration_ms: i64 = row.get(7)?;
                let models_str: String = row.get::<_, String>(10)?;
                let total_input_tokens: i64 = row.get(3)?;
                let total_cache_create_tokens: i64 = row.get(5)?;
                let total_cache_read_tokens: i64 = row.get(6)?;

                let avg_rate = if total_duration_ms > 0 {
                    (total_output_tokens as f64) / (total_duration_ms as f64 / 1000.0)
                } else {
                    0.0
                };

                Ok(SessionStats {
                    session_id: row.get(0)?,
                    tool: row.get::<_, String>(2)?,
                    total_requests: row.get::<_, i64>(1)? as u64,
                    total_input_tokens: total_input_tokens as u64,
                    total_output_tokens: total_output_tokens as u64,
                    total_cache_create_tokens: total_cache_create_tokens as u64,
                    total_cache_read_tokens: total_cache_read_tokens as u64,
                    total_duration_ms: total_duration_ms as u64,
                    avg_output_tokens_per_second: avg_rate,
                    first_request_time: row.get::<_, Option<i64>>(8)?.unwrap_or(0),
                    last_request_time: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
                    models: if models_str.is_empty() {
                        Vec::new()
                    } else {
                        models_str.split(',').map(|s| s.to_string()).collect()
                    },
                    avg_ttft_ms: row.get::<_, Option<f64>>(11)?.unwrap_or(0.0),
                    success_requests: row.get::<_, i64>(12)? as u64,
                    error_requests: row.get::<_, i64>(13)? as u64,
                    estimated_cost: row.get::<_, f64>(14)?,
                    is_cost_estimated: true,
                    usage_fully_covered: true,
                    covered_requests: row.get::<_, i64>(1)? as u64,
                    uncovered_requests: 0,
                    cwd: None,
                    project_name: None,
                    project_identity: None,
                    topic: None,
                    last_prompt: None,
                    session_name: None,
                    scope: None,
                    wsl_distro: None,
                })
            },
        );

        match result {
            Ok(stats) => Ok(Some(stats)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to get session stats: {}", e)),
        }
    }

    #[allow(dead_code)]
    pub async fn get_all_sessions(
        &self,
        limit: i64,
        _pricings: &[ModelPricingConfig],
        _match_mode: &str,
    ) -> Result<Vec<SessionStats>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT
                    session_id,
                    COUNT(*) as total_requests,
                    CASE
                        WHEN COUNT(DISTINCT COALESCE(NULLIF(client_tool, ''), ?3)) = 1
                        THEN MIN(COALESCE(NULLIF(client_tool, ''), ?3))
                        ELSE 'mixed'
                    END as session_tool,
                    COALESCE(SUM(input_tokens), 0) as total_input_tokens,
                    COALESCE(SUM(output_tokens), 0) as total_output_tokens,
                    COALESCE(SUM(cache_create_tokens), 0) as total_cache_create_tokens,
                    COALESCE(SUM(cache_read_tokens), 0) as total_cache_read_tokens,
                    COALESCE(SUM(duration_ms), 0) as total_duration_ms,
                    MIN(request_start_time) as first_request_time,
                    MAX(request_end_time) as last_request_time,
                    GROUP_CONCAT(DISTINCT model) as models,
                    AVG(ttft_ms) as avg_ttft_ms,
                    SUM(CASE WHEN status_code < 400 THEN 1 ELSE 0 END) as success_requests,
                    SUM(CASE WHEN status_code >= 400 THEN 1 ELSE 0 END) as error_requests,
                    COALESCE(SUM(estimated_cost), 0) as estimated_cost
                FROM usage_records
                WHERE session_id IS NOT NULL
                  AND session_id != ''
                  AND session_id != ?2
                GROUP BY session_id
                ORDER BY MAX(request_end_time) DESC
                LIMIT ?1
                "#,
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let sessions = stmt
            .query_map(
                rusqlite::params![
                    limit,
                    LEGACY_UNMATCHED_SESSION_ID,
                    crate::models::DEFAULT_CLIENT_TOOL
                ],
                |row| {
                    let total_output_tokens: i64 = row.get(4)?;
                    let total_duration_ms: i64 = row.get(7)?;
                    let models_str: String = row.get::<_, String>(10)?;
                    let total_input_tokens: i64 = row.get(3)?;
                    let total_cache_create_tokens: i64 = row.get(5)?;
                    let total_cache_read_tokens: i64 = row.get(6)?;

                    let avg_rate = if total_duration_ms > 0 {
                        (total_output_tokens as f64) / (total_duration_ms as f64 / 1000.0)
                    } else {
                        0.0
                    };

                    Ok(SessionStats {
                        session_id: row.get(0)?,
                        tool: row.get::<_, String>(2)?,
                        total_requests: row.get::<_, i64>(1)? as u64,
                        total_input_tokens: total_input_tokens as u64,
                        total_output_tokens: total_output_tokens as u64,
                        total_cache_create_tokens: total_cache_create_tokens as u64,
                        total_cache_read_tokens: total_cache_read_tokens as u64,
                        total_duration_ms: total_duration_ms as u64,
                        avg_output_tokens_per_second: avg_rate,
                        first_request_time: row.get::<_, Option<i64>>(8)?.unwrap_or(0),
                        last_request_time: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
                        models: if models_str.is_empty() {
                            Vec::new()
                        } else {
                            models_str.split(',').map(|s| s.to_string()).collect()
                        },
                        avg_ttft_ms: row.get::<_, Option<f64>>(11)?.unwrap_or(0.0),
                        success_requests: row.get::<_, i64>(12)? as u64,
                        error_requests: row.get::<_, i64>(13)? as u64,
                        estimated_cost: row.get::<_, f64>(14)?,
                        is_cost_estimated: true,
                        usage_fully_covered: true,
                        covered_requests: row.get::<_, i64>(1)? as u64,
                        uncovered_requests: 0,
                        cwd: None,
                        project_name: None,
                        project_identity: None,
                        topic: None,
                        last_prompt: None,
                        session_name: None,
                        scope: None,
                        wsl_distro: None,
                    })
                },
            )
            .map_err(|e| format!("Failed to query sessions: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect sessions: {}", e))?;

        Ok(sessions)
    }

    pub async fn get_window_rate_stats(&self, cutoff_ms: i64) -> Result<WindowRateStats, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let stats = conn
            .query_row(
                r#"
                SELECT
                    COUNT(*) as request_count,
                    COALESCE(SUM(output_tokens), 0) as total_output_tokens,
                    COALESCE(SUM(duration_ms), 0) as total_duration_ms,
                    CASE
                        WHEN SUM(duration_ms) > 0
                        THEN SUM(output_tokens) * 1000.0 / SUM(duration_ms)
                        ELSE 0
                    END as avg_rate
                FROM usage_records
                WHERE timestamp >= ?1
                  AND duration_ms > 0
                  AND output_tokens_per_second IS NOT NULL
                "#,
                [cutoff_ms],
                |row| {
                    Ok(WindowRateStats {
                        request_count: row.get(0)?,
                        total_output_tokens: row.get(1)?,
                        total_duration_ms: row.get(2)?,
                        avg_output_tokens_per_second: row.get(3)?,
                    })
                },
            )
            .map_err(|e| format!("Failed to get window rate stats: {}", e))?;

        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::types::UsageRecord;

    fn temp_db() -> (tempfile::TempDir, ProxyDatabase) {
        let tmpdir = tempfile::tempdir().expect("create temp dir");
        let path = tmpdir.path().join("proxy_data.db");
        let db = ProxyDatabase::new_with_path(&path).expect("open temp db");
        (tmpdir, db)
    }

    fn insert_usage_record(
        db: &ProxyDatabase,
        record: &UsageRecord,
        session_resolution_state: &str,
    ) {
        let now = chrono::Utc::now().timestamp();
        let conn = db.conn.lock().expect("lock conn");
        conn.execute(
            r#"
            INSERT INTO usage_records (
                timestamp, message_id, storage_dedupe_key, canonical_request_key,
                input_tokens, output_tokens, cache_create_tokens, cache_read_tokens,
                model, session_id, session_resolution_state, message_id_conflicted,
                request_start_time, request_end_time, duration_ms, output_tokens_per_second,
                ttft_ms, status_code, estimated_cost, pricing_snapshot_id, cost_locked,
                api_key_prefix, request_base_url, client_tool, proxy_profile_id,
                client_detection_method, created_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28
            )
            "#,
            rusqlite::params![
                record.timestamp,
                record.message_id,
                record
                    .storage_dedupe_key
                    .clone()
                    .unwrap_or_else(|| format!("{}:{}", record.client_tool, record.message_id)),
                record
                    .canonical_request_key
                    .clone()
                    .unwrap_or_else(|| format!("{}:{}", record.client_tool, record.message_id)),
                record.input_tokens as i64,
                record.output_tokens as i64,
                record.cache_create_tokens as i64,
                record.cache_read_tokens as i64,
                record.model,
                record.session_id,
                session_resolution_state,
                if record.message_id_conflicted { 1 } else { 0 },
                record.request_start_time,
                record.request_end_time,
                record.duration_ms as i64,
                record.output_tokens_per_second,
                record.ttft_ms.map(|v| v as i64),
                record.status_code as i64,
                record.estimated_cost,
                record.pricing_snapshot_id,
                if record.cost_locked { 1 } else { 0 },
                record.api_key_prefix,
                record.request_base_url,
                record.client_tool,
                record.proxy_profile_id,
                record.client_detection_method,
                now,
                now,
            ],
        )
        .expect("insert usage record");
    }

    #[test]
    fn session_stats_sum_estimated_costs_across_models() {
        let (_tmp, db) = temp_db();
        insert_usage_record(
            &db,
            &UsageRecord {
                timestamp: 1_715_000_000_000,
                message_id: "msg-1".to_string(),
                model: "claude-3-5-sonnet".to_string(),
                session_id: Some("sess-1".to_string()),
                input_tokens: 100,
                output_tokens: 200,
                estimated_cost: 1.25,
                client_tool: "claude_code".to_string(),
                duration_ms: 1000,
                request_start_time: 1_715_000_000_000,
                request_end_time: 1_715_000_001_000,
                ..Default::default()
            },
            "known",
        );
        insert_usage_record(
            &db,
            &UsageRecord {
                timestamp: 1_715_000_002_000,
                message_id: "msg-2".to_string(),
                model: "gpt-4.1".to_string(),
                session_id: Some("sess-1".to_string()),
                input_tokens: 50,
                output_tokens: 75,
                estimated_cost: 2.75,
                client_tool: "claude_code".to_string(),
                duration_ms: 500,
                request_start_time: 1_715_000_002_000,
                request_end_time: 1_715_000_002_500,
                ..Default::default()
            },
            "known",
        );

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let stats = runtime
            .block_on(db.get_session_stats("sess-1", &[], "exact"))
            .expect("get stats")
            .expect("session stats");

        assert!((stats.estimated_cost - 4.0).abs() < f64::EPSILON);
        assert_eq!(stats.models.len(), 2);
    }

    #[test]
    fn session_stats_mark_mixed_tool_when_session_has_multiple_tools() {
        let (_tmp, db) = temp_db();
        insert_usage_record(
            &db,
            &UsageRecord {
                timestamp: 1_715_000_000_000,
                message_id: "msg-a".to_string(),
                model: "claude-3-5-sonnet".to_string(),
                session_id: Some("sess-mixed".to_string()),
                estimated_cost: 1.0,
                client_tool: "claude_code".to_string(),
                request_start_time: 1_715_000_000_000,
                request_end_time: 1_715_000_001_000,
                ..Default::default()
            },
            "known",
        );
        insert_usage_record(
            &db,
            &UsageRecord {
                timestamp: 1_715_000_002_000,
                message_id: "msg-b".to_string(),
                model: "gpt-4.1".to_string(),
                session_id: Some("sess-mixed".to_string()),
                estimated_cost: 2.0,
                client_tool: "codex".to_string(),
                request_start_time: 1_715_000_002_000,
                request_end_time: 1_715_000_003_000,
                ..Default::default()
            },
            "known",
        );

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let stats = runtime
            .block_on(db.get_session_stats("sess-mixed", &[], "exact"))
            .expect("get stats")
            .expect("session stats");

        assert_eq!(stats.tool, "mixed");
        assert!((stats.estimated_cost - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn migration_normalizes_legacy_opencode_native_session_ids() {
        let (_tmp, db) = temp_db();
        insert_usage_record(
            &db,
            &UsageRecord {
                timestamp: 1_715_000_000_000,
                message_id: "msg-conflict".to_string(),
                canonical_request_key: Some("opencode:opencode::sess-old|msg-conflict".to_string()),
                model: "gpt-4.1".to_string(),
                session_id: Some("opencode::sess-old".to_string()),
                input_tokens: 10,
                output_tokens: 20,
                estimated_cost: 0.5,
                client_tool: "opencode".to_string(),
                duration_ms: 1000,
                request_start_time: 1_715_000_000_000,
                request_end_time: 1_715_000_001_000,
                ..Default::default()
            },
            "known",
        );
        insert_usage_record(
            &db,
            &UsageRecord {
                timestamp: 1_715_000_002_000,
                message_id: "msg-new".to_string(),
                canonical_request_key: Some(
                    "opencode:opencode::native::sess-old|msg-new".to_string(),
                ),
                model: "gpt-4.1".to_string(),
                session_id: Some("opencode::native::sess-old".to_string()),
                input_tokens: 5,
                output_tokens: 7,
                estimated_cost: 0.25,
                client_tool: "opencode".to_string(),
                duration_ms: 500,
                request_start_time: 1_715_000_002_000,
                request_end_time: 1_715_000_002_500,
                ..Default::default()
            },
            "known",
        );

        {
            let conn = db.conn.lock().expect("lock conn");
            conn.execute(
                "INSERT INTO session_stats (
                    session_id, total_duration_ms, avg_output_tokens_per_second, avg_ttft_ms,
                    proxy_request_count, success_requests, error_requests, total_input_tokens,
                    total_output_tokens, total_cache_create_tokens, total_cache_read_tokens,
                    models, first_request_time, last_request_time, estimated_cost, last_updated
                 ) VALUES ('opencode::sess-old', 1000, 20.0, 0, 1, 1, 0, 10, 20, 0, 0, 'gpt-4.1', 1, 2, 0.5, 1)",
                [],
            )
            .expect("insert old session stats");
            conn.execute(
                "INSERT INTO session_stats (
                    session_id, total_duration_ms, avg_output_tokens_per_second, avg_ttft_ms,
                    proxy_request_count, success_requests, error_requests, total_input_tokens,
                    total_output_tokens, total_cache_create_tokens, total_cache_read_tokens,
                    models, first_request_time, last_request_time, estimated_cost, last_updated
                 ) VALUES ('opencode::native::sess-old', 500, 14.0, 0, 1, 1, 0, 5, 7, 0, 0, 'gpt-4.1', 3, 4, 0.25, 1)",
                [],
            )
            .expect("insert new session stats");

            ProxyDatabase::migrate_schema(&conn).expect("rerun migration");
        }

        let conn = db.conn.lock().expect("lock conn");
        let legacy_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_records WHERE session_id = 'opencode::sess-old'",
                [],
                |row| row.get(0),
            )
            .expect("count legacy records");
        assert_eq!(legacy_count, 0);

        let normalized_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_records WHERE session_id = 'opencode::native::sess-old'",
                [],
                |row| row.get(0),
            )
            .expect("count normalized records");
        assert_eq!(normalized_count, 2);

        let canonical_key: String = conn
            .query_row(
                "SELECT canonical_request_key FROM usage_records WHERE message_id = 'msg-conflict'",
                [],
                |row| row.get(0),
            )
            .expect("read normalized key");
        assert_eq!(
            canonical_key,
            "opencode:opencode::native::sess-old|msg-conflict"
        );

        let old_stats_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_stats WHERE session_id = 'opencode::sess-old'",
                [],
                |row| row.get(0),
            )
            .expect("count old stats");
        assert_eq!(old_stats_count, 0);

        let (request_count, input_tokens, output_tokens, estimated_cost): (i64, i64, i64, f64) = conn
            .query_row(
                "SELECT proxy_request_count, total_input_tokens, total_output_tokens, estimated_cost
                 FROM session_stats WHERE session_id = 'opencode::native::sess-old'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read rebuilt stats");
        assert_eq!(request_count, 2);
        assert_eq!(input_tokens, 15);
        assert_eq!(output_tokens, 27);
        assert!((estimated_cost - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn ensure_daily_rollup_mode_current_resets_rollups_on_mode_mismatch() {
        let (_tmp, db) = temp_db();
        {
            let conn = db.conn.lock().expect("lock conn");
            conn.execute(
                "INSERT INTO daily_summary (date, request_count, finalized_at) VALUES ('2026-06-01', 5, 1)",
                [],
            )
            .expect("insert daily summary");
            conn.execute(
                "INSERT INTO model_usage (date, model, request_count) VALUES ('2026-06-01', 'claude-sonnet-4', 5)",
                [],
            )
            .expect("insert model usage");
            conn.execute(
                "INSERT INTO daily_rollup_state (state_key, state_value, updated_at)
                 VALUES ('day_boundary_mode', 'night_owl', 1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                [],
            )
            .expect("insert mismatched rollup mode");
        }

        db.ensure_daily_rollup_mode_current()
            .expect("ensure current rollup mode");

        let conn = db.conn.lock().expect("lock conn");
        let summary_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM daily_summary", [], |row| row.get(0))
            .expect("count daily summary");
        let model_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM model_usage", [], |row| row.get(0))
            .expect("count model usage");
        let stored_mode: String = conn
            .query_row(
                "SELECT state_value FROM daily_rollup_state WHERE state_key = 'day_boundary_mode'",
                [],
                |row| row.get(0),
            )
            .expect("load stored mode");

        assert_eq!(summary_count, 0);
        assert_eq!(model_count, 0);
        assert_eq!(stored_mode, "standard");
    }
}
