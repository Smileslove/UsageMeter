use rusqlite::{Connection, Row};
use std::collections::HashSet;

use super::super::types::{SessionStats, UsageRecord};
use super::{ProxyDatabase, LEGACY_UNMATCHED_SESSION_ID};
use crate::models::ModelPricingConfig;
use crate::session::{LocalRequestRecord, SessionMeta};

struct OpenCodeLocalMatcher<'a> {
    candidates: &'a [&'a LocalRequestRecord],
    used_request_keys: HashSet<String>,
}

struct ReasonixSessionMatcher<'a> {
    candidates: &'a [SessionMeta],
}

impl<'a> OpenCodeLocalMatcher<'a> {
    fn new(candidates: &'a [&'a LocalRequestRecord]) -> Self {
        Self {
            candidates,
            used_request_keys: HashSet::new(),
        }
    }

    fn match_record(&mut self, proxy_record: &UsageRecord) -> Option<&'a LocalRequestRecord> {
        let compatible = self.compatible_candidates(proxy_record);
        if compatible.is_empty() {
            return None;
        }
        if compatible.len() == 1 {
            let matched = compatible[0];
            self.mark_used(matched);
            return Some(matched);
        }

        let mut scored: Vec<(&LocalRequestRecord, i64)> = compatible
            .into_iter()
            .map(|candidate| (candidate, self.time_delta_secs(proxy_record, candidate)))
            .collect();
        scored.sort_by_key(|(_, delta)| *delta);

        let best = scored[0];
        let second = scored.get(1).copied();
        let clearly_better = second
            .map(|(_, delta)| delta.saturating_sub(best.1) >= 10)
            .unwrap_or(true);
        if best.1 <= 5 && clearly_better {
            self.mark_used(best.0);
            return Some(best.0);
        }

        None
    }

    fn compatible_candidate_count(&self, proxy_record: &UsageRecord) -> usize {
        self.compatible_candidates(proxy_record).len()
    }

    fn compatible_candidates(&self, proxy_record: &UsageRecord) -> Vec<&'a LocalRequestRecord> {
        let proxy_model_key = normalize_opencode_model_key(&proxy_record.model);
        self.candidates
            .iter()
            .copied()
            .filter(|candidate| {
                candidate.input_tokens == proxy_record.input_tokens
                    && candidate.output_tokens == proxy_record.output_tokens
                    && candidate.cache_create_tokens == proxy_record.cache_create_tokens
                    && candidate.cache_read_tokens == proxy_record.cache_read_tokens
                    && candidate.total_tokens == proxy_record.total_tokens
                    && normalize_opencode_model_key(&candidate.model) == proxy_model_key
                    && self.time_delta_secs(proxy_record, candidate) <= 30
                    && !self.is_used(candidate)
            })
            .collect()
    }

    fn time_delta_secs(&self, proxy_record: &UsageRecord, candidate: &LocalRequestRecord) -> i64 {
        let proxy_ts_sec = if proxy_record.request_end_time > 0 {
            proxy_record.request_end_time / 1000
        } else {
            proxy_record.timestamp / 1000
        };
        (proxy_ts_sec - candidate.timestamp).abs()
    }

    fn is_used(&self, candidate: &LocalRequestRecord) -> bool {
        candidate
            .request_key
            .as_ref()
            .map(|key| self.used_request_keys.contains(key))
            .unwrap_or(false)
    }

    fn mark_used(&mut self, candidate: &LocalRequestRecord) {
        if let Some(key) = candidate.request_key.as_ref() {
            self.used_request_keys.insert(key.clone());
        }
    }
}

impl<'a> ReasonixSessionMatcher<'a> {
    fn new(candidates: &'a [SessionMeta]) -> Self {
        Self { candidates }
    }

    fn match_record(&self, proxy_record: &UsageRecord) -> Option<&'a SessionMeta> {
        let compatible = self.compatible_candidates(proxy_record);
        if compatible.len() == 1 {
            compatible.into_iter().next()
        } else {
            None
        }
    }

    fn compatible_candidate_count(&self, proxy_record: &UsageRecord) -> usize {
        self.compatible_candidates(proxy_record).len()
    }

    fn compatible_candidates(&self, proxy_record: &UsageRecord) -> Vec<&'a SessionMeta> {
        let proxy_model_key = crate::models::normalize_model_id(&proxy_record.model);
        let proxy_ts_sec = self.proxy_timestamp_secs(proxy_record);

        self.candidates
            .iter()
            .filter(|meta| meta.tool == "reasonix" && !meta.session_id.trim().is_empty())
            .filter(|meta| {
                meta.models.is_empty()
                    || meta
                        .models
                        .iter()
                        .any(|model| crate::models::normalize_model_id(model) == proxy_model_key)
            })
            .filter(|meta| self.timestamp_matches(proxy_ts_sec, meta))
            .collect()
    }

    fn proxy_timestamp_secs(&self, proxy_record: &UsageRecord) -> i64 {
        if proxy_record.request_end_time > 0 {
            proxy_record.request_end_time / 1000
        } else {
            proxy_record.timestamp / 1000
        }
    }

    fn timestamp_matches(&self, proxy_ts_sec: i64, meta: &SessionMeta) -> bool {
        let start = if meta.start_time > 0 {
            meta.start_time
        } else {
            meta.last_modified.saturating_sub(15)
        };
        let end = meta.end_time.max(meta.last_modified);
        let effective_end = if end > 0 {
            end
        } else {
            start.saturating_add(15)
        };
        let grace = 15;
        proxy_ts_sec >= start.saturating_sub(grace)
            && proxy_ts_sec <= effective_end.saturating_add(grace)
    }
}

fn normalize_opencode_model_key(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed
        .rsplit('/')
        .next()
        .unwrap_or(trimmed)
        .trim()
        .to_ascii_lowercase()
}

pub(super) fn usage_record_from_row(row: &Row<'_>) -> rusqlite::Result<UsageRecord> {
    let input = ProxyDatabase::safe_i64_to_u64(row.get::<_, i64>(2)?);
    let output = ProxyDatabase::safe_i64_to_u64(row.get::<_, i64>(3)?);
    let cache_create = ProxyDatabase::safe_i64_to_u64(row.get::<_, i64>(4)?);
    let cache_read = ProxyDatabase::safe_i64_to_u64(row.get::<_, i64>(5)?);
    Ok(UsageRecord {
        timestamp: row.get::<_, i64>(0)?,
        message_id: row.get(1)?,
        storage_dedupe_key: row.get(22)?,
        canonical_request_key: row.get(23)?,
        input_tokens: input,
        output_tokens: output,
        cache_create_tokens: cache_create,
        cache_read_tokens: cache_read,
        reasoning_tokens: 0,
        total_tokens: input + cache_create + cache_read + output,
        model: row.get(6)?,
        session_id: row.get(7)?,
        session_resolution_state: row.get(24)?,
        message_id_conflicted: row.get::<_, Option<i64>>(25)?.unwrap_or(0) != 0,
        request_start_time: row.get::<_, Option<i64>>(8)?.unwrap_or(0),
        request_end_time: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
        duration_ms: row.get::<_, i64>(10)? as u64,
        output_tokens_per_second: row.get(11)?,
        ttft_ms: row.get::<_, Option<i64>>(12)?.map(|v| v as u64),
        status_code: row.get::<_, i64>(13)? as u16,
        estimated_cost: row.get::<_, Option<f64>>(14)?.unwrap_or(0.0),
        pricing_snapshot_id: row.get(15)?,
        cost_locked: row.get::<_, Option<i64>>(16)?.unwrap_or(0) != 0,
        api_key_prefix: row.get(17)?,
        request_base_url: row.get(18)?,
        client_tool: row
            .get::<_, Option<String>>(19)?
            .unwrap_or_else(|| crate::models::DEFAULT_CLIENT_TOOL.to_string()),
        proxy_profile_id: row.get(20)?,
        client_detection_method: row
            .get::<_, Option<String>>(21)?
            .unwrap_or_else(|| crate::models::DEFAULT_CLIENT_DETECTION_METHOD.to_string()),
    })
}

fn fallback_request_identity(record: &UsageRecord) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}:{}:{}",
        record.client_tool,
        record.session_id.clone().unwrap_or_default(),
        record.timestamp / 1000,
        record.model,
        record.input_tokens,
        record.output_tokens,
        record.cache_create_tokens,
        record.cache_read_tokens,
        record.total_tokens
    )
}

pub(super) fn computed_canonical_request_key(record: &UsageRecord) -> String {
    if let Some(key) = record.canonical_request_key.as_ref() {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if record.message_id.trim().is_empty() {
        fallback_request_identity(record)
    } else {
        format!("{}:{}", record.client_tool, record.message_id)
    }
}

pub(super) fn computed_session_resolution_state(record: &UsageRecord) -> String {
    if let Some(state) = record.session_resolution_state.as_ref() {
        let trimmed = state.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if record
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .is_some()
    {
        "known".to_string()
    } else {
        "unknown".to_string()
    }
}

impl ProxyDatabase {
    /// 通过 message_id 列表查询会话统计信息
    ///
    /// 用于将 JSONL 会话文件中的消息与代理数据库记录关联
    /// 返回聚合后的统计数据：总耗时、总输出 Token、平均生成速率等
    #[allow(dead_code)]
    pub async fn get_session_stats_by_message_ids(
        &self,
        message_ids: &[String],
        pricings: &[ModelPricingConfig],
        match_mode: &str,
    ) -> Option<SessionStats> {
        if message_ids.is_empty() {
            return None;
        }

        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return None,
        };

        // 构建 IN 子句参数
        let placeholders: Vec<String> = message_ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            r#"
            SELECT
                COUNT(*) as total_requests,
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
                SUM(CASE WHEN status_code >= 400 THEN 1 ELSE 0 END) as error_requests
            FROM usage_records
            WHERE message_id IN ({})
            "#,
            placeholders.join(", ")
        );

        let params: Vec<&dyn rusqlite::types::ToSql> = message_ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();

        let result = conn.query_row(&sql, params.as_slice(), |row| {
            let total_output_tokens: i64 = row.get(2)?;
            let total_duration_ms: i64 = row.get(5)?;
            let models_str: String = row.get::<_, String>(8)?;
            let total_input_tokens: i64 = row.get(1)?;
            let total_cache_create_tokens: i64 = row.get(3)?;
            let total_cache_read_tokens: i64 = row.get(4)?;

            // 计算平均生成速率
            let avg_rate = if total_duration_ms > 0 {
                (total_output_tokens as f64) / (total_duration_ms as f64 / 1000.0)
            } else {
                0.0
            };

            // 获取第一个模型用于定价
            let first_model = models_str.split(',').next().unwrap_or("");

            // 计算估算费用
            let estimated_cost = crate::models::estimate_session_cost(
                total_input_tokens as u64,
                total_output_tokens as u64,
                total_cache_create_tokens as u64,
                total_cache_read_tokens as u64,
                first_model,
                pricings,
                match_mode,
            );

            Ok(SessionStats {
                session_id: String::new(), // 调用方会填充
                tool: crate::models::DEFAULT_CLIENT_TOOL.to_string(),
                total_requests: row.get::<_, i64>(0)? as u64,
                total_input_tokens: total_input_tokens as u64,
                total_output_tokens: total_output_tokens as u64,
                total_cache_create_tokens: total_cache_create_tokens as u64,
                total_cache_read_tokens: total_cache_read_tokens as u64,
                total_duration_ms: total_duration_ms as u64,
                avg_output_tokens_per_second: avg_rate,
                first_request_time: row.get::<_, Option<i64>>(6)?.unwrap_or(0),
                last_request_time: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
                models: if models_str.is_empty() {
                    Vec::new()
                } else {
                    models_str.split(',').map(|s| s.to_string()).collect()
                },
                avg_ttft_ms: row.get::<_, Option<f64>>(9)?.unwrap_or(0.0),
                success_requests: row.get::<_, i64>(10)? as u64,
                error_requests: row.get::<_, i64>(11)? as u64,
                estimated_cost,
                is_cost_estimated: true,
                usage_fully_covered: true,
                covered_requests: row.get::<_, i64>(0)? as u64,
                uncovered_requests: 0,
                cwd: None,
                project_name: None,
                project_identity: None,
                topic: None,
                last_prompt: None,
                session_name: None,
                wsl_distro: None,
            })
        });

        match result {
            Ok(stats) if stats.total_requests > 0 => Some(stats),
            Ok(_) => None, // 没有找到记录
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => {
                eprintln!("Failed to get session stats by message_ids: {}", e);
                None
            }
        }
    }

    // ========== session_stats 表操作 ==========

    fn upsert_session_stats_for_record(
        conn: &Connection,
        session_id: &str,
        record: &UsageRecord,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp_millis();

        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM session_stats WHERE session_id = ?1",
                [session_id],
                |row| row.get::<_, i64>(0),
            )
            .is_ok();

        if exists {
            conn.execute(
                r#"
                UPDATE session_stats SET
                    total_duration_ms = total_duration_ms + ?2,
                    total_input_tokens = total_input_tokens + ?3,
                    total_output_tokens = total_output_tokens + ?4,
                    total_cache_create_tokens = total_cache_create_tokens + ?5,
                    total_cache_read_tokens = total_cache_read_tokens + ?6,
                    proxy_request_count = proxy_request_count + 1,
                    success_requests = success_requests + CASE WHEN ?7 < 400 THEN 1 ELSE 0 END,
                    error_requests = error_requests + CASE WHEN ?7 >= 400 THEN 1 ELSE 0 END,
                    last_request_time = MAX(last_request_time, ?8),
                    first_request_time = COALESCE(first_request_time, ?9),
                    last_updated = ?10
                WHERE session_id = ?1
                "#,
                rusqlite::params![
                    session_id,
                    record.duration_ms as i64,
                    record.input_tokens as i64,
                    record.output_tokens as i64,
                    record.cache_create_tokens as i64,
                    record.cache_read_tokens as i64,
                    record.status_code as i64,
                    record.request_end_time,
                    record.request_start_time,
                    now
                ],
            )
            .map_err(|e| format!("Failed to update session stats: {}", e))?;

            conn.execute(
                r#"
                UPDATE session_stats SET
                    avg_output_tokens_per_second = CASE
                        WHEN total_duration_ms > 0 THEN total_output_tokens * 1000.0 / total_duration_ms
                        ELSE 0
                    END
                WHERE session_id = ?1
                "#,
                [session_id],
            )
            .map_err(|e| format!("Failed to update avg rate: {}", e))?;
        } else {
            conn.execute(
                r#"
                INSERT INTO session_stats (
                    session_id, total_duration_ms, total_input_tokens, total_output_tokens,
                    total_cache_create_tokens, total_cache_read_tokens, proxy_request_count,
                    success_requests, error_requests, first_request_time, last_request_time,
                    avg_output_tokens_per_second, last_updated, models, estimated_cost
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, 1,
                    CASE WHEN ?7 < 400 THEN 1 ELSE 0 END,
                    CASE WHEN ?7 >= 400 THEN 1 ELSE 0 END,
                    ?8, ?9,
                    CASE WHEN ?2 > 0 THEN ?4 * 1000.0 / ?2 ELSE 0 END,
                    ?10, ?11, 0
                )
                "#,
                rusqlite::params![
                    session_id,
                    record.duration_ms as i64,
                    record.input_tokens as i64,
                    record.output_tokens as i64,
                    record.cache_create_tokens as i64,
                    record.cache_read_tokens as i64,
                    record.status_code as i64,
                    record.request_start_time,
                    record.request_end_time,
                    now,
                    record.model.clone()
                ],
            )
            .map_err(|e| format!("Failed to insert session stats: {}", e))?;
        }

        Ok(())
    }

    /// 增量更新会话统计（新请求产生时调用）
    ///
    /// 如果会话不存在则创建新记录，否则增量更新
    pub async fn update_session_stats_incremental(
        &self,
        record: &UsageRecord,
    ) -> Result<(), String> {
        if record.client_tool == "opencode" && computed_session_resolution_state(record) != "known"
        {
            return Ok(());
        }

        // 如果没有 session_id，尝试从 JSONL 获取；无匹配时使用请求时间窗口作为回退
        let session_id = match &record.session_id {
            Some(id) if !id.is_empty() => id.clone(),
            _ => {
                match self.find_session_id_by_message_id(&record.message_id).await {
                    Some(id) => id,
                    None => {
                        // 无法匹配 JSONL 的请求也保留在 session_stats 中，
                        // 后续 JSONL 重新扫描时可通过 message_id 回填正确值
                        LEGACY_UNMATCHED_SESSION_ID.to_string()
                    }
                }
            }
        };

        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;
        Self::upsert_session_stats_for_record(&conn, &session_id, record)
    }

    /// 通过 message_id 查找对应的 session_id（从 JSONL 文件）
    async fn find_session_id_by_message_id(&self, message_id: &str) -> Option<String> {
        // 使用 session 模块的缓存索引查找（O(1) 时间复杂度）
        crate::session::find_session_id_by_message_id(message_id)
    }

    pub fn reconcile_opencode_records(
        &self,
        local_records: &[LocalRequestRecord],
    ) -> Result<usize, String> {
        let local_candidates: Vec<&LocalRequestRecord> = local_records
            .iter()
            .filter(|record| record.tool == "opencode")
            .collect();
        if local_candidates.is_empty() {
            return Ok(0);
        }

        let now = chrono::Utc::now().timestamp_millis();
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;
        let unresolved_records: Vec<(i64, UsageRecord)> = {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT timestamp, message_id, input_tokens, output_tokens,
                           cache_create_tokens, cache_read_tokens, model, session_id,
                           request_start_time, request_end_time, duration_ms, output_tokens_per_second,
                           ttft_ms, status_code, estimated_cost, pricing_snapshot_id, cost_locked,
                           api_key_prefix, request_base_url, client_tool, proxy_profile_id,
                           client_detection_method, storage_dedupe_key, canonical_request_key,
                           session_resolution_state, message_id_conflicted, id
                    FROM usage_records
                    WHERE client_tool = 'opencode'
                      AND (session_resolution_state IS NULL OR session_resolution_state != 'known')
                    ORDER BY timestamp ASC
                    "#,
                )
                .map_err(|e| format!("Failed to prepare unresolved OpenCode query: {}", e))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, i64>(26)?, usage_record_from_row(row)?))
                })
                .map_err(|e| format!("Failed to query unresolved OpenCode records: {}", e))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect unresolved OpenCode records: {}", e))?
        };

        if unresolved_records.is_empty() {
            return Ok(0);
        }

        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to start OpenCode reconciliation transaction: {}", e))?;
        let mut updated = 0usize;
        let mut local_matcher = OpenCodeLocalMatcher::new(&local_candidates);

        for (row_id, record) in unresolved_records {
            let mut resolved_record = record.clone();
            let resolution_state;

            match local_matcher.match_record(&record) {
                Some(local_match) => {
                    let conflicted = local_match
                        .request_key
                        .as_deref()
                        .map(|key| key.contains('|'))
                        .unwrap_or(false);
                    resolution_state = "known".to_string();
                    resolved_record.session_id = Some(local_match.session_id.clone());
                    resolved_record.canonical_request_key = Some(
                        crate::unified_usage::canonical_request_key_for_local(local_match),
                    );
                    resolved_record.session_resolution_state = Some(resolution_state.clone());
                    resolved_record.message_id_conflicted = conflicted;
                    resolved_record.model = local_match.model.clone();
                }
                None if local_matcher.compatible_candidate_count(&record) == 0 => {
                    resolution_state = "unmatched".to_string();
                }
                None => {
                    resolution_state = "ambiguous".to_string();
                }
            }

            if resolved_record.session_resolution_state.as_deref() != Some(&resolution_state) {
                resolved_record.session_resolution_state = Some(resolution_state.clone());
            }

            tx.execute(
                "UPDATE usage_records
                 SET session_id = ?1,
                     model = ?2,
                     canonical_request_key = ?3,
                     session_resolution_state = ?4,
                     message_id_conflicted = ?5,
                     migration_attempted_at = ?6,
                     updated_at = ?6
                 WHERE id = ?7",
                rusqlite::params![
                    &resolved_record.session_id,
                    &resolved_record.model,
                    &resolved_record
                        .canonical_request_key
                        .clone()
                        .unwrap_or_else(|| computed_canonical_request_key(&resolved_record)),
                    &resolution_state,
                    if resolved_record.message_id_conflicted {
                        1
                    } else {
                        0
                    },
                    now,
                    row_id
                ],
            )
            .map_err(|e| format!("Failed to update OpenCode proxy reconciliation row: {}", e))?;

            // 仅统计成功解析的记录（unmatched/ambiguous 不计入）
            if resolution_state == "known" {
                updated += 1;
                // 在同一事务内更新 session_stats，保证与 usage_records 原子提交：
                // Transaction 实现了 Deref<Target = Connection>，可直接传入 &tx。
                if let Some(session_id) = resolved_record.session_id.as_deref() {
                    Self::upsert_session_stats_for_record(&tx, session_id, &resolved_record)?;
                }
            }
        }

        // tx.commit() 消耗 tx，conn 上的可变借用随之释放；
        // conn（MutexGuard）在函数末尾的作用域结束时自动 drop，无需手动释放。
        tx.commit()
            .map_err(|e| format!("Failed to commit OpenCode reconciliation: {}", e))?;

        Ok(updated)
    }

    pub fn reconcile_reasonix_records(
        &self,
        local_sessions: &[SessionMeta],
    ) -> Result<usize, String> {
        let reasonix_sessions: Vec<SessionMeta> = local_sessions
            .iter()
            .filter(|meta| meta.tool == "reasonix")
            .cloned()
            .collect();
        if reasonix_sessions.is_empty() {
            return Ok(0);
        }

        let now = chrono::Utc::now().timestamp_millis();
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;
        let unresolved_records: Vec<(i64, UsageRecord)> = {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT timestamp, message_id, input_tokens, output_tokens,
                           cache_create_tokens, cache_read_tokens, model, session_id,
                           request_start_time, request_end_time, duration_ms, output_tokens_per_second,
                           ttft_ms, status_code, estimated_cost, pricing_snapshot_id, cost_locked,
                           api_key_prefix, request_base_url, client_tool, proxy_profile_id,
                           client_detection_method, storage_dedupe_key, canonical_request_key,
                           session_resolution_state, message_id_conflicted, id
                    FROM usage_records
                    WHERE client_tool = 'reasonix'
                      AND (session_resolution_state IS NULL OR session_resolution_state != 'known')
                    ORDER BY timestamp ASC
                    "#,
                )
                .map_err(|e| format!("Failed to prepare unresolved Reasonix query: {}", e))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, i64>(26)?, usage_record_from_row(row)?))
                })
                .map_err(|e| format!("Failed to query unresolved Reasonix records: {}", e))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect unresolved Reasonix records: {}", e))?
        };

        if unresolved_records.is_empty() {
            return Ok(0);
        }

        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to start Reasonix reconciliation transaction: {}", e))?;
        let mut updated = 0usize;
        let matcher = ReasonixSessionMatcher::new(&reasonix_sessions);

        for (row_id, record) in unresolved_records {
            let mut resolved_record = record.clone();
            let resolution_state;

            match matcher.match_record(&record) {
                Some(session_meta) => {
                    resolution_state = "known".to_string();
                    resolved_record.session_id = Some(session_meta.session_id.clone());
                    resolved_record.session_resolution_state = Some(resolution_state.clone());
                }
                None if matcher.compatible_candidate_count(&record) == 0 => {
                    resolution_state = "unmatched".to_string();
                }
                None => {
                    resolution_state = "ambiguous".to_string();
                }
            }

            if resolved_record.session_resolution_state.as_deref() != Some(&resolution_state) {
                resolved_record.session_resolution_state = Some(resolution_state.clone());
            }

            tx.execute(
                "UPDATE usage_records
                 SET session_id = ?1,
                     canonical_request_key = ?2,
                     session_resolution_state = ?3,
                     migration_attempted_at = ?4,
                     updated_at = ?4
                 WHERE id = ?5",
                rusqlite::params![
                    &resolved_record.session_id,
                    &resolved_record
                        .canonical_request_key
                        .clone()
                        .unwrap_or_else(|| computed_canonical_request_key(&resolved_record)),
                    &resolution_state,
                    now,
                    row_id
                ],
            )
            .map_err(|e| format!("Failed to update Reasonix proxy reconciliation row: {}", e))?;

            if resolution_state == "known" {
                updated += 1;
                if let Some(session_id) = resolved_record.session_id.as_deref() {
                    Self::upsert_session_stats_for_record(&tx, session_id, &resolved_record)?;
                }
            }
        }

        tx.commit()
            .map_err(|e| format!("Failed to commit Reasonix reconciliation: {}", e))?;

        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_reasonix_session(
        session_id: &str,
        model: &str,
        start_time: i64,
        end_time: i64,
    ) -> SessionMeta {
        SessionMeta {
            session_id: session_id.to_string(),
            tool: "reasonix".to_string(),
            models: vec![model.to_string()],
            start_time,
            end_time,
            last_modified: end_time,
            ..Default::default()
        }
    }

    fn make_reasonix_record(model: &str, request_end_time_ms: i64) -> UsageRecord {
        UsageRecord {
            client_tool: "reasonix".to_string(),
            model: model.to_string(),
            request_end_time: request_end_time_ms,
            timestamp: request_end_time_ms,
            ..Default::default()
        }
    }

    #[test]
    fn reasonix_matcher_resolves_unique_session_by_model_and_time_range() {
        let sessions = vec![
            make_reasonix_session(
                "reasonix::sess-a",
                "deepseek-v4-pro",
                1_700_000_000,
                1_700_000_030,
            ),
            make_reasonix_session(
                "reasonix::sess-b",
                "deepseek-v4-pro",
                1_700_000_100,
                1_700_000_130,
            ),
        ];
        let matcher = ReasonixSessionMatcher::new(&sessions);
        let record = make_reasonix_record("deepseek-v4-pro", 1_700_000_025_000);

        let matched = matcher
            .match_record(&record)
            .map(|meta| meta.session_id.clone());

        assert_eq!(matched.as_deref(), Some("reasonix::sess-a"));
    }

    #[test]
    fn reasonix_matcher_keeps_ambiguous_overlap_unresolved() {
        let sessions = vec![
            make_reasonix_session(
                "reasonix::sess-a",
                "deepseek-v4-pro",
                1_700_000_000,
                1_700_000_040,
            ),
            make_reasonix_session(
                "reasonix::sess-b",
                "deepseek-v4-pro",
                1_700_000_010,
                1_700_000_050,
            ),
        ];
        let matcher = ReasonixSessionMatcher::new(&sessions);
        let record = make_reasonix_record("deepseek-v4-pro", 1_700_000_020_000);

        assert!(matcher.match_record(&record).is_none());
        assert_eq!(matcher.compatible_candidate_count(&record), 2);
    }
}
