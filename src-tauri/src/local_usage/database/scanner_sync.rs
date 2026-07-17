use crate::models::ToolFilter;
use crate::session::{
    parse_session_file_for_storage, scan_file_backed_session_files, LocalRequestRecord,
    SessionFile, SessionMeta,
};
use rusqlite::params;
use std::collections::{HashMap, HashSet};

use super::{
    outbox, DirtySessionSync, LocalUsageDatabase, SyncExportRequest, SyncExportSession,
    TimestampSqlColumn,
};

impl LocalUsageDatabase {
    /// 收集该会话中"仍在场"（`source_file_present != 0`）的请求事实所覆盖的历史业务日期。
    /// 只用于会话整体消失时的软删路径：即将被软删翻转的行才会真正改变历史日数据，
    /// 已软删过的行不应再次触发历史日失效。
    pub(super) fn collect_history_dates_for_session_tx(
        tx: &rusqlite::Transaction<'_>,
        session_id: &str,
        settings: &crate::models::AppSettings,
        today: &str,
    ) -> Result<HashSet<String>, String> {
        let date_expr =
            Self::business_date_sql_expr_for_timestamp(settings, TimestampSqlColumn::Timestamp);
        let mut stmt = tx
            .prepare(&format!(
                "SELECT DISTINCT {date_expr} AS business_date
                 FROM local_request_facts
                 WHERE session_id = ?1 AND source_file_present != 0"
            ))
            .map_err(|e| format!("Failed to prepare session history day query: {}", e))?;
        let rows = stmt
            .query_map(params![session_id], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query session history days: {}", e))?;
        let mut dates = HashSet::new();
        for row in rows {
            let date = row.map_err(|e| format!("Failed to read session history day row: {}", e))?;
            if date.as_str() < today {
                dates.insert(date);
            }
        }
        Ok(dates)
    }

    fn load_source_fingerprints(
        &self,
        file_role: &str,
        tool: Option<&str>,
    ) -> Result<HashMap<String, String>, String> {
        let conn = self.conn.lock().unwrap();
        let (sql, params_vec): (&str, Vec<&str>) = if let Some(tool) = tool {
            (
                "SELECT session_id, fingerprint
                 FROM local_source_files
                 WHERE file_role = ?1 AND tool = ?2 AND deleted_at IS NULL",
                vec![file_role, tool],
            )
        } else {
            (
                "SELECT session_id, fingerprint
                 FROM local_source_files
                 WHERE file_role = ?1 AND deleted_at IS NULL",
                vec![file_role],
            )
        };
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| format!("Failed to prepare load_source_fingerprints: {}", e))?;
        let mut result = HashMap::new();
        if params_vec.len() == 2 {
            let rows = stmt
                .query_map(params![params_vec[0], params_vec[1]], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| format!("Failed to query source fingerprints: {}", e))?;
            for row in rows {
                let (session_id, fingerprint) =
                    row.map_err(|e| format!("Failed to read source fingerprint row: {}", e))?;
                result.insert(session_id, fingerprint);
            }
        } else {
            let rows = stmt
                .query_map(params![params_vec[0]], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| format!("Failed to query source fingerprints: {}", e))?;
            for row in rows {
                let (session_id, fingerprint) =
                    row.map_err(|e| format!("Failed to read source fingerprint row: {}", e))?;
                result.insert(session_id, fingerprint);
            }
        }
        Ok(result)
    }

    pub fn sync_from_scanner(&self) -> Result<(), String> {
        let persisted_opencode_states = self.load_opencode_db_scan_states()?;
        crate::session::opencode_reader::hydrate_opencode_db_scan_states(
            &persisted_opencode_states,
        );

        self.sync_file_backed_sessions(scan_file_backed_session_files())?;
        if let Some(proxy_db) = crate::proxy::ProxyDatabase::get_global() {
            let reasonix_local_sessions =
                self.get_all_sessions(&ToolFilter::Tool("reasonix".to_string()))?;
            let _ = proxy_db.reconcile_reasonix_records(&reasonix_local_sessions);
        }

        let opencode_sessions = crate::session::opencode_reader::scan_opencode_sessions();
        let opencode_local_records: Vec<crate::session::LocalRequestRecord> = opencode_sessions
            .iter()
            .flat_map(|session| session.requests.iter().cloned())
            .collect();
        let opencode_map: HashMap<String, DirtySessionSync> = opencode_sessions
            .into_iter()
            .map(|session| {
                let project_key = session
                    .meta
                    .project_name
                    .clone()
                    .or(session.meta.cwd.clone())
                    .unwrap_or_else(|| "unknown_project".to_string());
                let key = session.meta.session_id.clone();
                (
                    key.clone(),
                    DirtySessionSync {
                        session_id: key,
                        tool: session.meta.tool.clone(),
                        file_path: session.source_locator.clone(),
                        file_role: "opencode_session".to_string(),
                        file_size: 0,
                        last_modified: session.meta.last_modified,
                        fingerprint: session.fingerprint.to_string(),
                        meta: session.meta,
                        requests: session.requests,
                        project_key,
                    },
                )
            })
            .collect();
        self.sync_dirty_session_map(opencode_map, "opencode_session", Some("opencode"))?;
        if let Some(proxy_db) = crate::proxy::ProxyDatabase::get_global() {
            let _ = proxy_db.reconcile_opencode_records(&opencode_local_records);
        }

        let qoder_map = sessions_to_dirty_map(
            crate::session::qoder_ide_reader::scan_qoder_ide_sessions(),
            "qoder_ide_session",
            |s| (s.meta, s.requests, s.fingerprint, s.source_locator),
        );
        self.sync_dirty_session_map(qoder_map, "qoder_ide_session", Some("qoder_ide"))?;

        let qoder_cn_map = sessions_to_dirty_map(
            crate::session::qoder_ide_reader::scan_qoder_ide_cn_sessions(),
            "qoder_ide_cn_session",
            |s| (s.meta, s.requests, s.fingerprint, s.source_locator),
        );
        self.sync_dirty_session_map(qoder_cn_map, "qoder_ide_cn_session", Some("qoder_ide_cn"))?;

        let qoder_work_map = sessions_to_dirty_map(
            crate::session::qoder_work_reader::scan_qoder_work_sessions(),
            "qoder_work_session",
            |s| (s.meta, s.requests, s.fingerprint, s.source_locator),
        );
        self.sync_dirty_session_map(qoder_work_map, "qoder_work_session", Some("qoder_work"))?;

        let qoder_work_cn_map = sessions_to_dirty_map(
            crate::session::qoder_work_reader::scan_qoder_work_cn_sessions(),
            "qoder_work_cn_session",
            |s| (s.meta, s.requests, s.fingerprint, s.source_locator),
        );
        self.sync_dirty_session_map(
            qoder_work_cn_map,
            "qoder_work_cn_session",
            Some("qoder_work_cn"),
        )?;

        let hermes_map = sessions_to_dirty_map(
            crate::session::scan_hermes_sessions(),
            "hermes_session",
            |s| (s.meta, s.requests, s.fingerprint, s.source_locator),
        );
        self.sync_dirty_session_map(hermes_map, "hermes_session", Some("hermes"))?;

        let opencode_states = crate::session::opencode_reader::get_opencode_db_scan_states();
        let opencode_schema_status = crate::session::opencode_reader::check_opencode_schema();
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start OpenCode DB sync state transaction: {}", e))?;
        Self::persist_opencode_db_scan_states_tx(&tx, &opencode_states, now)?;
        Self::persist_opencode_message_id_conflict_tx(
            &tx,
            &opencode_schema_status.message_id_conflict,
            now,
        )?;
        tx.commit()
            .map_err(|e| format!("Failed to commit OpenCode DB sync state: {}", e))?;

        Ok(())
    }

    fn sync_file_backed_sessions(&self, scanned_sessions: Vec<SessionFile>) -> Result<(), String> {
        let current_ids: HashSet<String> = scanned_sessions
            .iter()
            .map(|session| session.session_id.clone())
            .collect();
        let cached_fingerprints = self.load_source_fingerprints("session_group", None)?;
        let cached_ids: HashSet<String> = cached_fingerprints.keys().cloned().collect();
        let removed_ids: Vec<String> = cached_ids.difference(&current_ids).cloned().collect();

        let mut dirty_sessions: Vec<DirtySessionSync> = scanned_sessions
            .into_iter()
            .filter_map(|session| {
                let fingerprint = session.fingerprint.to_string();
                match cached_fingerprints.get(&session.session_id) {
                    Some(existing) if existing == &fingerprint => None,
                    _ => {
                        let (meta, requests) = match parse_session_file_for_storage(&session) {
                            Ok(parsed) => parsed,
                            Err(err) => {
                                eprintln!(
                                    "[UsageMeter] Failed to sync session {} from {}: {}",
                                    session.session_id, session.file_path, err
                                );
                                return None;
                            }
                        };
                        let project_key = meta
                            .project_name
                            .clone()
                            .or(meta.cwd.clone())
                            .unwrap_or_else(|| "unknown_project".to_string());
                        Some(DirtySessionSync {
                            session_id: session.session_id,
                            tool: session.tool,
                            file_path: session.file_path,
                            file_role: "session_group".to_string(),
                            file_size: session.file_size,
                            last_modified: session.last_modified,
                            fingerprint,
                            meta,
                            requests,
                            project_key,
                        })
                    }
                }
            })
            .collect();
        dirty_sessions.sort_by(|left, right| left.session_id.cmp(&right.session_id));

        self.sync_dirty_sessions(dirty_sessions, removed_ids)
    }

    fn sync_dirty_session_map(
        &self,
        scanned_map: HashMap<String, DirtySessionSync>,
        file_role: &str,
        tool: Option<&str>,
    ) -> Result<(), String> {
        let current_ids: HashSet<String> = scanned_map.keys().cloned().collect();
        let cached_fingerprints = self.load_source_fingerprints(file_role, tool)?;
        let cached_ids: HashSet<String> = cached_fingerprints.keys().cloned().collect();

        let removed_ids: Vec<String> = cached_ids.difference(&current_ids).cloned().collect();
        let mut dirty_ids: Vec<String> = scanned_map
            .iter()
            .filter_map(
                |(session_id, session)| match cached_fingerprints.get(session_id) {
                    Some(existing) if existing == &session.fingerprint => None,
                    _ => Some(session_id.clone()),
                },
            )
            .collect();
        dirty_ids.sort();

        if dirty_ids.is_empty() && removed_ids.is_empty() {
            return Ok(());
        }

        let dirty_sessions: Vec<DirtySessionSync> = dirty_ids
            .into_iter()
            .filter_map(|session_id| scanned_map.get(&session_id).cloned())
            .collect();
        self.sync_dirty_sessions(dirty_sessions, removed_ids)
    }

    pub(super) fn sync_dirty_sessions(
        &self,
        dirty_sessions: Vec<DirtySessionSync>,
        removed_ids: Vec<String>,
    ) -> Result<(), String> {
        let dirty_session_count = dirty_sessions.len();
        let removed_session_count = removed_ids.len();

        let now = chrono::Utc::now().timestamp();
        let origin_device_id = self
            .get_webdav_sync_state("device_id")?
            .map(|value| crate::models::normalize_sync_device_id(&value))
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                crate::models::normalize_sync_device_id(&crate::models::default_sync_device_id())
            });
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start local usage transaction: {}", e))?;
        let settings = crate::commands::load_settings_blocking().unwrap_or_default();
        let today = Self::today_local_date_with_settings(&settings);
        let mut touched_history_dates: HashSet<String> = HashSet::new();

        for session_id in &removed_ids {
            // 只统计即将被软删翻转的行（source_file_present != 0）覆盖的历史日期
            touched_history_dates.extend(Self::collect_history_dates_for_session_tx(
                &tx, session_id, &settings, &today,
            )?);
            // 软删同时 bump sync_version 让物化/导出侧感知；守卫避免对已软删行重复 bump
            tx.execute(
                "UPDATE local_request_facts
                 SET source_file_present = 0,
                     sync_version = sync_version + 1
                 WHERE session_id = ?1 AND source_file_present != 0",
                params![session_id],
            )
            .map_err(|e| format!("Failed to soft-delete local request facts: {}", e))?;
            tx.execute(
                "UPDATE local_source_files
                 SET deleted_at = ?2,
                     deletion_reason = 'missing'
                 WHERE session_id = ?1 AND deleted_at IS NULL",
                params![session_id, now],
            )
            .map_err(|e| format!("Failed to mark local source file removed: {}", e))?;
        }

        for dirty_session in dirty_sessions {
            let DirtySessionSync {
                session_id,
                tool,
                file_path,
                file_role,
                file_size,
                last_modified,
                fingerprint,
                meta,
                requests,
                project_key,
            } = dirty_session;

            // dedupe_key → 旧 timestamp：
            // 既用于软删差集，也用于变更行的"日期迁移"检测（旧时间戳所在历史日同样需要失效）
            let existing_facts: HashMap<String, i64> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT dedupe_key, timestamp
                         FROM local_request_facts
                         WHERE session_id = ?1",
                    )
                    .map_err(|e| format!("Failed to prepare existing dedupe_key query: {}", e))?;
                let rows = stmt
                    .query_map(params![session_id.as_str()], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                    })
                    .map_err(|e| format!("Failed to query existing dedupe_keys: {}", e))?;
                let mut facts = HashMap::new();
                for row in rows {
                    let (key, timestamp) =
                        row.map_err(|e| format!("Failed to read existing dedupe_key row: {}", e))?;
                    facts.insert(key, timestamp);
                }
                facts
            };
            tx.execute(
                "DELETE FROM local_sessions WHERE session_id = ?1",
                params![session_id.as_str()],
            )
            .map_err(|e| format!("Failed to clear stale local session row: {}", e))?;

            tx.execute(
                "INSERT INTO local_source_files (
                    tool, session_id, project_key, file_path, file_role, file_size,
                    mtime_epoch, fingerprint, last_scanned_at, last_synced_at, sync_status,
                    deleted_at, deletion_reason
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'ready', NULL, NULL)
                ON CONFLICT(file_path) DO UPDATE SET
                    tool = excluded.tool,
                    session_id = excluded.session_id,
                    project_key = excluded.project_key,
                    file_role = excluded.file_role,
                    file_size = excluded.file_size,
                    mtime_epoch = excluded.mtime_epoch,
                    fingerprint = excluded.fingerprint,
                    last_scanned_at = excluded.last_scanned_at,
                    last_synced_at = excluded.last_synced_at,
                    sync_status = 'ready',
                    deleted_at = NULL,
                    deletion_reason = NULL",
                params![
                    tool.as_str(),
                    session_id.as_str(),
                    project_key.as_str(),
                    file_path.as_str(),
                    file_role.as_str(),
                    file_size as i64,
                    last_modified,
                    fingerprint,
                    now,
                    now
                ],
            )
            .map_err(|e| format!("Failed to upsert local source row: {}", e))?;

            let model_list_json = serde_json::to_string(&meta.models)
                .map_err(|e| format!("Failed to serialize model list: {}", e))?;
            let total_tokens = meta.total_input_tokens
                + meta.total_output_tokens
                + meta.total_cache_create_tokens
                + meta.total_cache_read_tokens;

            tx.execute(
                "INSERT INTO local_sessions (
                    session_id, tool, project_key, cwd, project_name, topic, last_prompt,
                    session_name, scope, primary_file_path, file_size, last_modified, start_time, end_time,
                    request_count, total_input_tokens, total_output_tokens,
                    total_cache_create_tokens, total_cache_read_tokens, total_tokens,
                    model_list_json, source_kind, sync_version, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                          ?16, ?17, ?18, ?19, ?20, ?21, ?22, 1, ?23)",
                params![
                    meta.session_id.as_str(),
                    meta.tool.as_str(),
                    project_key.as_str(),
                    meta.cwd.as_deref(),
                    meta.project_name.as_deref(),
                    meta.topic.as_deref(),
                    meta.last_prompt.as_deref(),
                    meta.session_name.as_deref(),
                    meta.scope.as_deref(),
                    meta.file_path.as_str(),
                    meta.file_size as i64,
                    meta.last_modified,
                    meta.start_time,
                    meta.end_time,
                    meta.message_count as i64,
                    meta.total_input_tokens as i64,
                    meta.total_output_tokens as i64,
                    meta.total_cache_create_tokens as i64,
                    meta.total_cache_read_tokens as i64,
                    total_tokens as i64,
                    model_list_json.as_str(),
                    meta.source.as_str(),
                    now
                ],
            )
            .map_err(|e| format!("Failed to insert local session row: {}", e))?;
            let session_export = SyncExportSession {
                session_id: meta.session_id.clone(),
                tool: meta.tool.clone(),
                project_key: Some(project_key.clone()),
                project_name: meta.project_name.clone(),
                scope: meta.scope.clone(),
                start_time: meta.start_time,
                end_time: meta.end_time,
                request_count: meta.message_count,
                total_input_tokens: meta.total_input_tokens,
                total_output_tokens: meta.total_output_tokens,
                total_cache_create_tokens: meta.total_cache_create_tokens,
                total_cache_read_tokens: meta.total_cache_read_tokens,
                total_tokens,
                model_list: meta.models.clone(),
            };
            outbox::enqueue_session_export_tx(&tx, &origin_device_id, &session_export, now)?;

            let mut seen_dedupe_keys: HashSet<String> = HashSet::new();
            for (idx, request) in requests.iter().enumerate() {
                let request_identity = if request.message_id.trim().is_empty() {
                    format!(
                        "ts:{}:idx:{}:model:{}:tokens:{}",
                        request.timestamp, idx, request.model, request.total_tokens
                    )
                } else {
                    request.message_id.clone()
                };
                let dedupe_key = format!("{}:{}", request.session_id, request_identity);
                let request_id = format!("{}:{}", request.tool, dedupe_key);
                let request_key = if let Some(key) = request
                    .request_key
                    .as_ref()
                    .map(|value| value.trim())
                    .filter(|value| !value.is_empty())
                {
                    key.to_string()
                } else if request.message_id.trim().is_empty() {
                    format!(
                        "{}:{}:{}:{}:{}:{}:{}:{}:{}",
                        request.tool,
                        request.session_id,
                        request.timestamp,
                        request.model,
                        request.input_tokens,
                        request.output_tokens,
                        request.cache_create_tokens,
                        request.cache_read_tokens,
                        request.total_tokens
                    )
                } else {
                    format!("{}:{}", request.tool, request.message_id)
                };
                seen_dedupe_keys.insert(dedupe_key.clone());
                // 内容守卫：仅当任一内容列与 excluded 不同才执行 UPDATE（可空列用 IS NOT 做
                // NULL 安全比较）。JSONL 是 append-only 的，历史日的行重解析后内容必然不变，
                // 守卫使其既不 bump sync_version、也不触发历史日物化失效与 outbox 重导出。
                let changed_rows = tx.execute(
                    "INSERT INTO local_request_facts (
                        request_id, session_id, tool, project_key, timestamp, message_id, dedupe_key,
                        request_key, model, input_tokens, output_tokens, reasoning_tokens,
                        cache_create_tokens, cache_read_tokens, total_tokens, request_count,
                        explicit_estimated_cost, source_offset, event_index, is_subagent,
                        raw_event_kind, sync_version, created_at, source_file_path, source_file_present
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                              ?16, ?17, NULL, ?18, ?19, 'request', 1, ?20, ?21, 1)
                    ON CONFLICT(tool, dedupe_key) DO UPDATE SET
                        session_id = excluded.session_id,
                        project_key = excluded.project_key,
                        timestamp = excluded.timestamp,
                        message_id = excluded.message_id,
                        request_key = excluded.request_key,
                        model = excluded.model,
                        input_tokens = excluded.input_tokens,
                        output_tokens = excluded.output_tokens,
                        reasoning_tokens = excluded.reasoning_tokens,
                        cache_create_tokens = excluded.cache_create_tokens,
                        cache_read_tokens = excluded.cache_read_tokens,
                        total_tokens = excluded.total_tokens,
                        request_count = excluded.request_count,
                        explicit_estimated_cost = excluded.explicit_estimated_cost,
                        event_index = excluded.event_index,
                        is_subagent = excluded.is_subagent,
                        sync_version = sync_version + 1,
                        source_file_path = excluded.source_file_path,
                        source_file_present = 1
                    WHERE local_request_facts.session_id != excluded.session_id
                       OR local_request_facts.project_key IS NOT excluded.project_key
                       OR local_request_facts.timestamp != excluded.timestamp
                       OR local_request_facts.message_id IS NOT excluded.message_id
                       OR local_request_facts.request_key IS NOT excluded.request_key
                       OR local_request_facts.model != excluded.model
                       OR local_request_facts.input_tokens != excluded.input_tokens
                       OR local_request_facts.output_tokens != excluded.output_tokens
                       OR local_request_facts.reasoning_tokens != excluded.reasoning_tokens
                       OR local_request_facts.cache_create_tokens != excluded.cache_create_tokens
                       OR local_request_facts.cache_read_tokens != excluded.cache_read_tokens
                       OR local_request_facts.total_tokens != excluded.total_tokens
                       OR local_request_facts.request_count != excluded.request_count
                       OR local_request_facts.explicit_estimated_cost IS NOT excluded.explicit_estimated_cost
                       OR local_request_facts.event_index IS NOT excluded.event_index
                       OR local_request_facts.is_subagent != excluded.is_subagent
                       OR local_request_facts.source_file_path IS NOT excluded.source_file_path
                       OR local_request_facts.source_file_present != 1",
                    params![
                        request_id.as_str(),
                        request.session_id.as_str(),
                        request.tool.as_str(),
                        project_key.as_str(),
                        request.timestamp,
                        request.message_id.as_str(),
                        dedupe_key.as_str(),
                        request_key.as_str(),
                        request.model.as_str(),
                        request.input_tokens as i64,
                        request.output_tokens as i64,
                        request.reasoning_tokens as i64,
                        request.cache_create_tokens as i64,
                        request.cache_read_tokens as i64,
                        request.total_tokens as i64,
                        request.request_count as i64,
                        request.explicit_estimated_cost,
                        idx as i64,
                        if request.is_subagent { 1 } else { 0 },
                        now,
                        file_path.as_str()
                    ],
                )
                .map_err(|e| format!("Failed to upsert local request fact: {}", e))?;

                // 内容未变（changed_rows == 0）的行不产生任何写副作用：
                // 不收集历史日期、不重复入队 outbox（入队会重置 uploaded_at 导致重复导出）
                if changed_rows == 0 {
                    continue;
                }

                let date = crate::utils::business_time::business_date_for_timestamp(
                    request.timestamp,
                    &settings,
                );
                if date < today {
                    touched_history_dates.insert(date);
                }
                // 日期迁移：更新前的旧时间戳若落在另一历史日，该日同样需要重物化
                if let Some(old_timestamp) = existing_facts.get(&dedupe_key) {
                    if *old_timestamp != request.timestamp {
                        let old_date = crate::utils::business_time::business_date_for_timestamp(
                            *old_timestamp,
                            &settings,
                        );
                        if old_date < today {
                            touched_history_dates.insert(old_date);
                        }
                    }
                }

                let request_export = SyncExportRequest {
                    request_key: request_key.clone(),
                    session_id: request.session_id.clone(),
                    tool: request.tool.clone(),
                    project_key: Some(project_key.clone()),
                    timestamp: request.timestamp,
                    message_id: if request.message_id.trim().is_empty() {
                        None
                    } else {
                        Some(request.message_id.clone())
                    },
                    dedupe_key: dedupe_key.clone(),
                    model: request.model.clone(),
                    input_tokens: request.input_tokens,
                    output_tokens: request.output_tokens,
                    cache_create_tokens: request.cache_create_tokens,
                    cache_read_tokens: request.cache_read_tokens,
                    total_tokens: request.total_tokens,
                    request_count: request.request_count,
                    explicit_estimated_cost: request.explicit_estimated_cost,
                    is_subagent: request.is_subagent,
                    source_kind: "local_usage".to_string(),
                };
                outbox::enqueue_request_export_tx(&tx, &origin_device_id, &request_export, now)?;
            }

            for (stale_key, old_timestamp) in existing_facts
                .iter()
                .filter(|(key, _)| !seen_dedupe_keys.contains(*key))
            {
                // 软删同时 bump sync_version 让物化/导出侧感知；守卫避免对已软删行重复 bump
                let changed_rows = tx
                    .execute(
                        "UPDATE local_request_facts
                         SET source_file_present = 0,
                             sync_version = sync_version + 1
                         WHERE tool = ?1 AND dedupe_key = ?2 AND source_file_present != 0",
                        params![tool.as_str(), stale_key.as_str()],
                    )
                    .map_err(|e| format!("Failed to soft-mark stale local request fact: {}", e))?;
                // 只有真实翻转的软删才让旧时间戳所在历史日失效
                if changed_rows > 0 {
                    let old_date = crate::utils::business_time::business_date_for_timestamp(
                        *old_timestamp,
                        &settings,
                    );
                    if old_date < today {
                        touched_history_dates.insert(old_date);
                    }
                }
            }
        }

        Self::upsert_sync_state(&tx, "last_sync_completed_at", &now.to_string(), now)?;
        Self::upsert_sync_state(
            &tx,
            "last_dirty_session_count",
            &dirty_session_count.to_string(),
            now,
        )?;
        Self::upsert_sync_state(
            &tx,
            "last_removed_session_count",
            &removed_session_count.to_string(),
            now,
        )?;
        Self::upsert_sync_state(&tx, "last_sync_mode", "session_rebuild_v1", now)?;
        Self::invalidate_unified_materialization_dates_tx(
            &tx,
            &touched_history_dates.into_iter().collect::<Vec<_>>(),
            now,
        )?;

        tx.commit()
            .map_err(|e| format!("Failed to commit local usage sync: {}", e))?;
        Ok(())
    }
}

/// 将任意带有 `(SessionMeta, Vec<LocalRequestRecord>, u64, String)` 字段的会话列表
/// 转换为 `DirtySessionSync` map，消除各 Qoder 变体的重复构建逻辑。
fn sessions_to_dirty_map<T>(
    sessions: Vec<T>,
    file_role: &str,
    extract: impl Fn(T) -> (SessionMeta, Vec<LocalRequestRecord>, u64, String),
) -> HashMap<String, DirtySessionSync> {
    sessions
        .into_iter()
        .map(|session| {
            let (meta, requests, fingerprint, source_locator) = extract(session);
            let project_key = meta
                .project_name
                .clone()
                .or(meta.cwd.clone())
                .unwrap_or_else(|| "unknown_project".to_string());
            let key = meta.session_id.clone();
            (
                key.clone(),
                DirtySessionSync {
                    session_id: key,
                    tool: meta.tool.clone(),
                    file_path: source_locator,
                    file_role: file_role.to_string(),
                    file_size: meta.file_size,
                    last_modified: meta.last_modified,
                    fingerprint: fingerprint.to_string(),
                    meta,
                    requests,
                    project_key,
                },
            )
        })
        .collect()
}
