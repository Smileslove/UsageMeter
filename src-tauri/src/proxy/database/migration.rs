use super::ProxyDatabase;

impl ProxyDatabase {
    pub async fn migrate_to_session_stats(&self) -> Result<usize, String> {
        // 类型别名：简化复杂类型定义
        // UsageRecordRow: 从数据库查询的 usage_record 行（用于迁移）
        type UsageRecordRow = (
            i64,         // id: 记录 ID
            i64,         // timestamp: 请求时间（毫秒）
            String,      // message_id: 消息 ID
            i64,         // duration_ms: 耗时（毫秒）
            i64,         // input_tokens: 输入 Token
            i64,         // output_tokens: 输出 Token
            i64,         // cache_create_tokens: 缓存创建 Token
            i64,         // cache_read_tokens: 缓存读取 Token
            i64,         // request_start_time: 请求开始时间
            i64,         // request_end_time: 请求结束时间
            i64,         // status_code: 状态码
            String,      // model: 模型名称
            Option<f64>, // ttft_ms: TTFT（毫秒）
            Option<f64>, // output_tokens_per_second: 已验证的逐请求速率
        );
        // SessionAggregate: 按 session_id 聚合的统计数据
        type SessionAggregate = (
            i64,                               // total_duration_ms: 总耗时
            i64,                               // total_input_tokens: 总输入
            i64,                               // total_output_tokens: 总输出
            i64,                               // total_cache_create_tokens: 总缓存创建
            i64,                               // total_cache_read_tokens: 总缓存读取
            i64,                               // first_request_time: 最早开始时间
            i64,                               // last_request_time: 最晚结束时间
            i64,                               // success_requests: 成功请求数
            i64,                               // error_requests: 错误请求数
            i64,                               // request_count: 请求数
            std::collections::HashSet<String>, // 模型集合
            Vec<f64>,                          // TTFT 值列表
            Vec<f64>,                          // 逐请求速率值列表
        );

        let now = chrono::Utc::now().timestamp_millis();

        // 检查是否有需要迁移的记录（session_id 为空的历史记录）
        let needs_migration = {
            let conn = self
                .conn
                .lock()
                .map_err(|e| format!("Failed to lock connection: {}", e))?;

            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM usage_records WHERE session_id IS NULL OR session_id = ''",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            drop(conn);
            count
        };

        // 如果没有需要迁移的记录，直接返回
        if needs_migration == 0 {
            return Ok(0);
        }

        eprintln!(
            "[migration] Found {} records without session_id, starting migration...",
            needs_migration
        );

        // 获取没有 session_id 的记录
        let records = {
            let conn = self
                .conn
                .lock()
                .map_err(|e| format!("Failed to lock connection: {}", e))?;

            // 只查询没有 session_id 的记录
            let result: Vec<UsageRecordRow> = conn
                .prepare(
                    r#"
                    SELECT
                        id,
                        timestamp,
                        message_id,
                        duration_ms,
                        input_tokens,
                        output_tokens,
                        cache_create_tokens,
                        cache_read_tokens,
                        request_start_time,
                        request_end_time,
                        status_code,
                        model,
                        ttft_ms,
                        output_tokens_per_second
                    FROM usage_records
                    WHERE session_id IS NULL OR session_id = ''
                    ORDER BY timestamp
                    "#,
                )
                .map_err(|e| format!("Failed to prepare migration query: {}", e))?
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get::<_, String>(11)?,
                        row.get::<_, Option<f64>>(12)?,
                        row.get::<_, Option<f64>>(13)?,
                    ))
                })
                .map_err(|e| format!("Failed to execute migration query: {}", e))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect migration results: {}", e))?;

            result
        }; // conn 在此处被释放

        // 获取 JSONL 会话元数据缓存（使用缓存，60秒内不会重复扫描）
        let all_meta = crate::session::get_all_session_meta_cached();

        // 构建 message_id -> session_id 的映射
        let mut msg_to_session: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for meta in &all_meta {
            for msg_id in &meta.message_ids {
                msg_to_session.insert(msg_id.clone(), meta.session_id.clone());
            }
        }
        eprintln!(
            "[migration] Built mapping for {} message_ids",
            msg_to_session.len()
        );

        // 按 session_id 聚合记录
        // 同时记录需要更新的 record_id
        let mut session_aggregates: std::collections::HashMap<String, SessionAggregate> =
            std::collections::HashMap::new();

        let mut matched = 0;
        let mut unmatched = 0;
        let mut record_updates: Vec<(String, i64, Option<String>)> = Vec::new();
        let mut unmatched_record_ids: Vec<(i64, Option<String>)> = Vec::new();
        let settings = crate::settings::load_settings_blocking().unwrap_or_default();
        let today = Self::today_local_date_with_settings(&settings);
        let mut touched_history_dates = std::collections::HashSet::new();

        for (
            record_id,
            timestamp,
            message_id,
            duration_ms,
            input,
            output,
            cache_create,
            cache_read,
            start_time,
            end_time,
            status_code,
            model,
            ttft_ms,
            output_tokens_per_second,
        ) in records
        {
            let historical_date = {
                let local_date = Self::record_local_date_with_settings(timestamp, &settings);
                (local_date < today).then_some(local_date)
            };

            if let Some(session_id) = msg_to_session.get(&message_id) {
                matched += 1;
                record_updates.push((session_id.clone(), record_id, historical_date));

                let entry = session_aggregates.entry(session_id.clone()).or_insert((
                    0,                                // 计数
                    0,                                // 总耗时
                    0,                                // 总输入
                    0,                                // 总输出
                    0,                                // 总缓存创建
                    0,                                // 总缓存读取
                    i64::MAX,                         // 最早时间
                    0,                                // 最晚时间
                    0,                                // 成功数
                    0,                                // 错误数
                    std::collections::HashSet::new(), // 模型集合
                    Vec::new(),                       // TTFT 值列表
                    Vec::new(),                       // 逐请求速率值列表
                ));

                entry.0 += 1;
                entry.1 += duration_ms;
                entry.2 += input;
                entry.3 += output;
                entry.4 += cache_create;
                entry.5 += cache_read;
                entry.6 = entry.6.min(start_time);
                entry.7 = entry.7.max(end_time);
                if status_code < 400 {
                    entry.8 += 1;
                } else {
                    entry.9 += 1;
                }
                if !model.is_empty() {
                    entry.10.insert(model);
                }
                if let Some(ttft) = ttft_ms {
                    entry.11.push(ttft);
                }
                if let Some(rate) = output_tokens_per_second
                    .filter(|rate| output > 0 && rate.is_finite() && *rate > 0.0)
                {
                    entry.12.push(rate);
                }
            } else {
                unmatched += 1;
                unmatched_record_ids.push((record_id, historical_date));
            }
        }

        eprintln!(
            "[migration] Matched {} records, unmatched {} records",
            matched, unmatched
        );

        // 保存到 session_stats 表（使用增量更新，避免覆盖已有数据）
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to start migration transaction: {}", e))?;

        let mut migrated = 0;

        {
            let mut update_record_stmt = tx
                .prepare("UPDATE usage_records SET session_id = ?1, updated_at = ?2 WHERE id = ?3")
                .map_err(|e| format!("Failed to prepare record migration update: {}", e))?;

            for (session_id, record_id, historical_date) in &record_updates {
                let changed = update_record_stmt
                    .execute(rusqlite::params![session_id, now, record_id])
                    .map_err(|e| format!("Failed to migrate usage record {record_id}: {e}"))?;
                if changed > 0 {
                    if let Some(local_date) = historical_date {
                        touched_history_dates.insert(local_date.clone());
                    }
                }
            }
        }

        if !unmatched_record_ids.is_empty() {
            let mut mark_unmatched_stmt = tx
                .prepare(
                    "UPDATE usage_records SET session_id = ?1, migration_attempted_at = ?2, updated_at = ?2 WHERE id = ?3",
                )
                .map_err(|e| format!("Failed to prepare unmatched migration update: {}", e))?;

            for (record_id, historical_date) in &unmatched_record_ids {
                let changed = mark_unmatched_stmt
                    .execute(rusqlite::params![
                        super::LEGACY_UNMATCHED_SESSION_ID,
                        now,
                        record_id
                    ])
                    .map_err(|e| {
                        format!("Failed to archive unmatched usage record {record_id}: {e}")
                    })?;
                if changed > 0 {
                    if let Some(local_date) = historical_date {
                        touched_history_dates.insert(local_date.clone());
                    }
                }
            }
        }

        if matched == 0 {
            tx.commit()
                .map_err(|e| format!("Failed to commit unmatched migration transaction: {}", e))?;
            drop(conn);
            Self::invalidate_local_materialization_after_proxy_commit(
                &touched_history_dates.into_iter().collect::<Vec<_>>(),
            )?;
            eprintln!(
                "[migration] No records matched; archived {} records as legacy unmatched",
                unmatched
            );
            return Ok(0);
        }

        for (
            session_id,
            (
                count,
                duration,
                input,
                output,
                cache_create,
                cache_read,
                first_time,
                last_time,
                success,
                error,
                models,
                ttfts,
                rates,
            ),
        ) in session_aggregates
        {
            let avg_rate = if !rates.is_empty() {
                rates.iter().sum::<f64>() / rates.len() as f64
            } else {
                0.0
            };

            let avg_ttft = if !ttfts.is_empty() {
                ttfts.iter().sum::<f64>() / ttfts.len() as f64
            } else {
                0.0
            };

            let models_str: String = models.into_iter().collect::<Vec<_>>().join(",");

            // 检查是否已存在该 session
            let exists: bool = tx
                .query_row(
                    "SELECT 1 FROM session_stats WHERE session_id = ?1",
                    [&session_id],
                    |row| row.get::<_, i64>(0),
                )
                .is_ok();

            if exists {
                // 增量更新已存在的记录
                let result = tx.execute(
                    r#"
                    UPDATE session_stats SET
                        total_duration_ms = total_duration_ms + ?2,
                        total_input_tokens = total_input_tokens + ?3,
                        total_output_tokens = total_output_tokens + ?4,
                        total_cache_create_tokens = total_cache_create_tokens + ?5,
                        total_cache_read_tokens = total_cache_read_tokens + ?6,
                        proxy_request_count = proxy_request_count + ?7,
                        success_requests = success_requests + ?8,
                        error_requests = error_requests + ?9,
                        last_request_time = MAX(last_request_time, ?10),
                        first_request_time = COALESCE(first_request_time, ?11),
                        avg_output_tokens_per_second = COALESCE(
                            (SELECT AVG(output_tokens_per_second)
                             FROM usage_records WHERE session_id = ?1), 0
                        ),
                        avg_ttft_ms = COALESCE(
                            (SELECT AVG(ttft_ms) FROM usage_records WHERE session_id = ?1), 0
                        ),
                        last_updated = ?12
                    WHERE session_id = ?1
                    "#,
                    rusqlite::params![
                        session_id,
                        duration,
                        input,
                        output,
                        cache_create,
                        cache_read,
                        count,
                        success,
                        error,
                        last_time,
                        if first_time == i64::MAX {
                            None
                        } else {
                            Some(first_time)
                        },
                        now
                    ],
                );

                if result.is_ok() {
                    migrated += 1;
                }
            } else {
                // 插入新记录
                let result = tx.execute(
                    r#"
                    INSERT INTO session_stats (
                        session_id, total_duration_ms, avg_output_tokens_per_second, avg_ttft_ms,
                        proxy_request_count, success_requests, error_requests,
                        total_input_tokens, total_output_tokens, total_cache_create_tokens, total_cache_read_tokens,
                        models, first_request_time, last_request_time, estimated_cost, last_updated
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 0, ?15
                    )
                    "#,
                    rusqlite::params![
                        session_id,
                        duration,
                        avg_rate,
                        avg_ttft,
                        count,
                        success,
                        error,
                        input,
                        output,
                        cache_create,
                        cache_read,
                        models_str,
                        if first_time == i64::MAX { None } else { Some(first_time) },
                        last_time,
                        now
                    ],
                );

                if result.is_ok() {
                    migrated += 1;
                }
            }
        }

        tx.commit()
            .map_err(|e| format!("Failed to commit migration transaction: {}", e))?;
        drop(conn);
        Self::invalidate_local_materialization_after_proxy_commit(
            &touched_history_dates.into_iter().collect::<Vec<_>>(),
        )?;
        eprintln!(
            "[migration] Migrated {} sessions to session_stats table",
            migrated
        );
        Ok(migrated)
    }

    /// 删除指定来源的请求记录
    pub async fn delete_records_by_source(
        &self,
        api_key_prefixes: &[String],
        base_url: Option<&str>,
    ) -> Result<(), String> {
        if api_key_prefixes.is_empty() {
            return Ok(());
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        // 构建删除 SQL
        let placeholders: Vec<String> = api_key_prefixes.iter().map(|_| "?".to_string()).collect();
        let base_url_val = base_url.unwrap_or("");

        let sql = format!(
            "DELETE FROM usage_records WHERE api_key_prefix IN ({}) AND COALESCE(request_base_url, '') = ?",
            placeholders.join(",")
        );

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare delete statement: {}", e))?;

        // 构建参数
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![];
        for p in api_key_prefixes {
            params.push(p);
        }
        params.push(&base_url_val);

        let deleted = stmt
            .execute(params.as_slice())
            .map_err(|e| format!("Failed to delete records: {}", e))?;

        if deleted > 0 {
            if let Ok(local_db) = crate::local_usage::LocalUsageDatabase::get_global() {
                let _ = local_db.clear_unified_materialization();
            }
        }
        eprintln!("[database] Deleted {} records for source", deleted);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (tempfile::TempDir, ProxyDatabase) {
        let tmpdir = tempfile::tempdir().expect("create temp dir");
        let path = tmpdir.path().join("proxy_data.db");
        let db = ProxyDatabase::new_with_path(&path).expect("open temp db");
        (tmpdir, db)
    }

    fn insert_record(
        db: &ProxyDatabase,
        timestamp: i64,
        message_id: &str,
        session_id: Option<&str>,
        api_key_prefix: Option<&str>,
        request_base_url: Option<&str>,
    ) {
        let conn = db.conn.lock().expect("lock conn");
        conn.execute(
            r#"
            INSERT INTO usage_records (
                timestamp, message_id, storage_dedupe_key, model, session_id,
                api_key_prefix, request_base_url, client_tool, session_resolution_state
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'claude_code', 'known')
            "#,
            rusqlite::params![
                timestamp,
                message_id,
                format!("key-{message_id}"),
                "gpt-4o",
                session_id,
                api_key_prefix,
                request_base_url,
            ],
        )
        .expect("insert usage record");
    }

    #[tokio::test]
    async fn migrate_to_session_stats_empty_db_returns_zero() {
        let (_tmp, db) = temp_db();
        let migrated = db
            .migrate_to_session_stats()
            .await
            .expect("migrate empty db");
        assert_eq!(migrated, 0);

        // 不产生任何 session_stats 行。
        let conn = db.conn.lock().expect("lock conn");
        let stats_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_stats", [], |row| row.get(0))
            .expect("count session stats");
        assert_eq!(stats_count, 0);
    }

    #[tokio::test]
    async fn migrate_to_session_stats_all_records_resolved_is_noop() {
        let (_tmp, db) = temp_db();
        insert_record(&db, 1_000, "m1", Some("sess-1"), None, None);
        insert_record(&db, 2_000, "m2", Some("sess-2"), None, None);

        // 所有记录都有 session_id → needs_migration == 0，直接返回 0，
        // 不触发真实会话目录扫描（保持测试环境隔离）。
        let migrated = db
            .migrate_to_session_stats()
            .await
            .expect("migrate resolved db");
        assert_eq!(migrated, 0);

        let conn = db.conn.lock().expect("lock conn");
        let stats_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_stats", [], |row| row.get(0))
            .expect("count session stats");
        assert_eq!(stats_count, 0);
    }

    #[tokio::test]
    async fn delete_records_by_source_deletes_only_matching_source() {
        let (_tmp, db) = temp_db();
        insert_record(
            &db,
            1_000,
            "m1",
            Some("sess-1"),
            Some("sk-a"),
            Some("https://api.a.com"),
        );
        insert_record(
            &db,
            2_000,
            "m2",
            Some("sess-2"),
            Some("sk-a"),
            Some("https://api.a.com"),
        );
        insert_record(
            &db,
            3_000,
            "m3",
            Some("sess-3"),
            Some("sk-b"),
            Some("https://api.b.com"),
        );
        insert_record(
            &db,
            4_000,
            "m4",
            Some("sess-4"),
            Some("sk-a"),
            Some("https://other.com"),
        );

        db.delete_records_by_source(&["sk-a".to_string()], Some("https://api.a.com"))
            .await
            .expect("delete by source");

        let remaining = db.get_records_since(0).await.expect("query remaining");
        let ids: Vec<&str> = remaining.iter().map(|r| r.message_id.as_str()).collect();
        // get_records_since 按时间戳 DESC 排序（新→旧）。
        assert_eq!(ids, vec!["m4", "m3"]);
        assert_eq!(db.get_record_count().await.expect("count"), 2);
    }

    #[tokio::test]
    async fn delete_records_by_source_matches_multiple_prefixes() {
        let (_tmp, db) = temp_db();
        insert_record(&db, 1_000, "m1", Some("sess-1"), Some("sk-a"), None);
        insert_record(
            &db,
            2_000,
            "m2",
            Some("sess-2"),
            Some("sk-b"),
            Some("https://api.b.com"),
        );
        insert_record(&db, 3_000, "m3", Some("sess-3"), Some("sk-c"), None);

        // base_url=None → 匹配空 base_url 的记录；m1/m3 命中（前缀匹配且 base_url 为空）。
        db.delete_records_by_source(&["sk-a".to_string(), "sk-c".to_string()], None)
            .await
            .expect("delete multiple prefixes");

        let remaining = db.get_records_since(0).await.expect("query remaining");
        let ids: Vec<&str> = remaining.iter().map(|r| r.message_id.as_str()).collect();
        assert_eq!(ids, vec!["m2"]);
    }

    #[tokio::test]
    async fn delete_records_by_source_empty_prefixes_is_noop() {
        let (_tmp, db) = temp_db();
        insert_record(&db, 1_000, "m1", Some("sess-1"), Some("sk-a"), None);

        db.delete_records_by_source(&[], None)
            .await
            .expect("noop delete");
        assert_eq!(db.get_record_count().await.expect("count"), 1);
    }
}
