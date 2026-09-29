use crate::session::{
    parse_session_file_for_storage, scan_file_backed_session_files, LocalRequestRecord,
    SessionFile, SessionMeta,
};
use rusqlite::{params, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

#[cfg(test)]
use crate::models::ToolFilter;

use super::{
    outbox, DirtySessionSync, LocalUsageDatabase, SyncExportRequest, SyncExportSession,
    TimestampSqlColumn,
};

type RemovedSessionContext = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    i64,
);
type RemovedFact = (
    String,
    String,
    Option<String>,
    i64,
    Option<String>,
    String,
    String,
);

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
        let stage_started = Instant::now();
        let persisted_opencode_states = self.load_opencode_db_scan_states()?;
        log_sync_stage(
            "load_opencode_state",
            stage_started,
            persisted_opencode_states.stores.len(),
        );

        let stage_started = Instant::now();
        crate::session::opencode_reader::hydrate_opencode_db_scan_states(
            &persisted_opencode_states,
        );
        log_sync_stage(
            "hydrate_opencode_state",
            stage_started,
            persisted_opencode_states.stores.len(),
        );

        let stage_started = Instant::now();
        let (file_backed_sessions, unavailable_tools) = scan_file_backed_session_files();
        log_sync_stage(
            "file_backed_scan",
            stage_started,
            file_backed_sessions.len(),
        );

        let file_backed_count = file_backed_sessions.len();
        let stage_started = Instant::now();
        self.sync_file_backed_sessions(file_backed_sessions, &unavailable_tools)?;
        log_sync_stage("file_backed_sync", stage_started, file_backed_count);

        let stage_started = Instant::now();
        let proxy_db = crate::proxy::ProxyDatabase::get_global();
        log_sync_stage(
            "proxy_db_acquire",
            stage_started,
            usize::from(proxy_db.is_some()),
        );
        // ReasonX 本地扫描链路已移除：此处不再做 reasonix 会话的代理记录回填
        // （reconcile_reasonix_records 已删除，代理记录保持独立展示）。
        // proxy_db 仍被下方的 opencode reconcile 使用。

        let stage_started = Instant::now();
        let opencode_sessions = crate::session::opencode_reader::scan_opencode_sessions();
        log_sync_stage("opencode_scan", stage_started, opencode_sessions.len());

        let stage_started = Instant::now();
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
        log_sync_stage("opencode_map", stage_started, opencode_map.len());

        let opencode_count = opencode_map.len();
        let stage_started = Instant::now();
        self.sync_dirty_session_map(opencode_map, "opencode_session", Some("opencode"))?;
        log_sync_stage("opencode_sync", stage_started, opencode_count);

        if let Some(proxy_db) = proxy_db.as_ref() {
            let opencode_record_count = opencode_local_records.len();
            let stage_started = Instant::now();
            let _ = proxy_db.reconcile_opencode_records(&opencode_local_records);
            log_sync_stage("opencode_reconcile", stage_started, opencode_record_count);
        }

        let stage_started = Instant::now();
        let qoder_sessions = crate::session::qoder_ide_reader::scan_qoder_ide_sessions();
        log_sync_stage("qoder_ide_scan", stage_started, qoder_sessions.len());
        let stage_started = Instant::now();
        let qoder_map = sessions_to_dirty_map(qoder_sessions, "qoder_ide_session", |s| {
            (s.meta, s.requests, s.fingerprint, s.source_locator)
        });
        let qoder_count = qoder_map.len();
        self.sync_dirty_session_map(qoder_map, "qoder_ide_session", Some("qoder_ide"))?;
        log_sync_stage("qoder_ide_sync", stage_started, qoder_count);

        let stage_started = Instant::now();
        let qoder_cn_sessions = crate::session::qoder_ide_reader::scan_qoder_ide_cn_sessions();
        log_sync_stage("qoder_ide_cn_scan", stage_started, qoder_cn_sessions.len());
        let stage_started = Instant::now();
        let qoder_cn_map = sessions_to_dirty_map(qoder_cn_sessions, "qoder_ide_cn_session", |s| {
            (s.meta, s.requests, s.fingerprint, s.source_locator)
        });
        let qoder_cn_count = qoder_cn_map.len();
        self.sync_dirty_session_map(qoder_cn_map, "qoder_ide_cn_session", Some("qoder_ide_cn"))?;
        log_sync_stage("qoder_ide_cn_sync", stage_started, qoder_cn_count);

        // 两路 Qoder Work 扫描读取完全独立的 app/CLI 目录，结果也属于不同 tool
        // 命名空间。并行执行冷解析以隐藏较短一侧的 I/O/JSON 开销；后续 SQLite
        // 同步仍保持串行，避免扩大事务并发面。
        let stage_started = Instant::now();
        let (qoder_work_sessions, qoder_work_cn_sessions) = std::thread::scope(|scope| {
            let global_scan = scope.spawn(|| {
                let started = Instant::now();
                let sessions = crate::session::qoder_work_reader::scan_qoder_work_sessions();
                log_sync_stage("qoder_work_scan", started, sessions.len());
                sessions
            });
            let cn_scan = scope.spawn(|| {
                let started = Instant::now();
                let sessions = crate::session::qoder_work_reader::scan_qoder_work_cn_sessions();
                log_sync_stage("qoder_work_cn_scan", started, sessions.len());
                sessions
            });
            let global_sessions = global_scan
                .join()
                .map_err(|_| "Qoder Work scanner thread panicked".to_string())?;
            let cn_sessions = cn_scan
                .join()
                .map_err(|_| "Qoder Work CN scanner thread panicked".to_string())?;
            Ok::<_, String>((global_sessions, cn_sessions))
        })?;
        log_sync_stage(
            "qoder_work_parallel_scan",
            stage_started,
            qoder_work_sessions.len() + qoder_work_cn_sessions.len(),
        );

        let stage_started = Instant::now();
        let qoder_work_map =
            sessions_to_dirty_map(qoder_work_sessions, "qoder_work_session", |s| {
                (s.meta, s.requests, s.fingerprint, s.source_locator)
            });
        let qoder_work_count = qoder_work_map.len();
        self.sync_dirty_session_map(qoder_work_map, "qoder_work_session", Some("qoder_work"))?;
        log_sync_stage("qoder_work_sync", stage_started, qoder_work_count);

        let stage_started = Instant::now();
        let qoder_work_cn_map =
            sessions_to_dirty_map(qoder_work_cn_sessions, "qoder_work_cn_session", |s| {
                (s.meta, s.requests, s.fingerprint, s.source_locator)
            });
        let qoder_work_cn_count = qoder_work_cn_map.len();
        self.sync_dirty_session_map(
            qoder_work_cn_map,
            "qoder_work_cn_session",
            Some("qoder_work_cn"),
        )?;
        log_sync_stage("qoder_work_cn_sync", stage_started, qoder_work_cn_count);

        let stage_started = Instant::now();
        let hermes_sessions = crate::session::scan_hermes_sessions();
        log_sync_stage("hermes_scan", stage_started, hermes_sessions.len());
        let stage_started = Instant::now();
        let hermes_map = sessions_to_dirty_map(hermes_sessions, "hermes_session", |s| {
            (s.meta, s.requests, s.fingerprint, s.source_locator)
        });
        let hermes_count = hermes_map.len();
        self.sync_dirty_session_map(hermes_map, "hermes_session", Some("hermes"))?;
        log_sync_stage("hermes_sync", stage_started, hermes_count);

        let stage_started = Instant::now();
        let opencode_states = crate::session::opencode_reader::get_opencode_db_scan_states();
        let opencode_schema_status = crate::session::opencode_reader::check_opencode_schema();
        log_sync_stage(
            "collect_opencode_state",
            stage_started,
            opencode_states.stores.len(),
        );

        let state_count = opencode_states.stores.len();
        let stage_started = Instant::now();
        let now = chrono::Utc::now().timestamp();
        {
            let conn = self.conn.lock().unwrap();
            let tx = conn.unchecked_transaction().map_err(|e| {
                format!("Failed to start OpenCode DB sync state transaction: {}", e)
            })?;
            Self::persist_opencode_db_scan_states_tx(&tx, &opencode_states, now)?;
            Self::persist_opencode_message_id_conflict_tx(
                &tx,
                &opencode_schema_status.message_id_conflict,
                now,
            )?;
            tx.commit()
                .map_err(|e| format!("Failed to commit OpenCode DB sync state: {}", e))?;
        }
        log_sync_stage("persist_opencode_state", stage_started, state_count);

        // Passive attribution is read-only and shares the existing scanner cadence. A missing
        // or unsupported tool configuration must never block local usage ingestion.
        if let Ok(settings) = crate::settings::load_settings_blocking() {
            if self
                .observe_passive_attribution(&settings, chrono::Utc::now().timestamp_millis())
                .unwrap_or(false)
            {
                crate::unified_usage::clear_runtime_caches();
            }
        }

        Ok(())
    }

    fn sync_file_backed_sessions(
        &self,
        scanned_sessions: Vec<SessionFile>,
        unavailable_tools: &[&str],
    ) -> Result<(), String> {
        let current_ids: HashSet<String> = scanned_sessions
            .iter()
            .map(|session| session.session_id.clone())
            .collect();
        let cached_fingerprints = self.load_source_fingerprints("session_group", None)?;
        let cached_ids: HashSet<String> = cached_fingerprints.keys().cloned().collect();
        let mut protected_ids = HashSet::new();
        for tool in unavailable_tools {
            protected_ids.extend(
                self.load_source_fingerprints("session_group", Some(tool))?
                    .into_keys(),
            );
        }
        let removed_ids: Vec<String> = cached_ids
            .difference(&current_ids)
            .filter(|session_id| !protected_ids.contains(*session_id))
            .cloned()
            .collect();

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
                                if session.tool == crate::session::constants::TOOL_DEEPSEEK_HARNESS {
                                    eprintln!(
                                        "[UsageMeter] Failed to sync DeepSeek Harness session: {err}"
                                    );
                                } else {
                                    eprintln!(
                                        "[UsageMeter] Failed to sync session {} from {}: {}",
                                        session.session_id, session.file_path, err
                                    );
                                }
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
        // Scanner failures must not abort local fact ingestion. A missing settings
        // snapshot disables outbox generation for this pass; startup reconciliation
        // will retry once the authoritative configuration is readable.
        let settings = crate::settings::load_settings_blocking().unwrap_or_default();
        let sync_enabled = settings.sync.enabled;
        let today = Self::today_local_date_with_settings(&settings);
        let mut touched_history_dates: HashSet<String> = HashSet::new();

        for session_id in &removed_ids {
            let session_context: Option<RemovedSessionContext> = tx
                .query_row(
                    "SELECT tool, project_key, project_name, scope, start_time, end_time
                     FROM local_sessions WHERE session_id = ?1",
                    params![session_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    },
                )
                .optional()
                .map_err(|e| format!("Failed to read removed session metadata: {e}"))?;
            let session_context = match session_context {
                Some(context) => Some(context),
                None => tx
                    .query_row(
                        "SELECT tool, project_key, NULL, NULL, 0, 0
                         FROM local_source_files
                         WHERE session_id = ?1 ORDER BY id DESC LIMIT 1",
                        params![session_id],
                        |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                                row.get(5)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(|e| format!("Failed to read removed session source metadata: {e}"))?,
            };
            if let Some((tool, project_key, project_name, scope, start_time, end_time)) =
                session_context
            {
                tx.execute(
                    "INSERT INTO local_session_tombstones (
                        session_id, tool, project_key, project_name, scope,
                        start_time, end_time, deleted_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                     ON CONFLICT(session_id) DO UPDATE SET
                        tool = excluded.tool,
                        project_key = excluded.project_key,
                        project_name = excluded.project_name,
                        scope = excluded.scope,
                        start_time = excluded.start_time,
                        end_time = excluded.end_time,
                        deleted_at = excluded.deleted_at,
                        updated_at = excluded.updated_at",
                    params![
                        session_id,
                        tool.as_str(),
                        project_key.as_deref(),
                        project_name.as_deref(),
                        scope.as_deref(),
                        start_time,
                        end_time,
                        now,
                    ],
                )
                .map_err(|e| format!("Failed to persist removed session tombstone: {e}"))?;
                if sync_enabled {
                    outbox::enqueue_session_export_tx(
                        &tx,
                        &origin_device_id,
                        &SyncExportSession {
                            deleted: true,
                            session_id: session_id.clone(),
                            tool,
                            project_key,
                            project_name,
                            scope,
                            start_time,
                            end_time,
                            request_count: 0,
                            total_input_tokens: 0,
                            total_output_tokens: 0,
                            total_cache_create_tokens: 0,
                            total_cache_read_tokens: 0,
                            total_tokens: 0,
                            total_reasoning_tokens: 0,
                            total_elapsed_ms: 0,
                            explicit_cost: None,
                            explicit_cost_currency: None,
                            estimated: false,
                            usage_sources: Default::default(),
                            model_list: Vec::new(),
                        },
                        now,
                    )?;
                }
            }
            // 只统计即将被软删翻转的行（source_file_present != 0）覆盖的历史日期
            touched_history_dates.extend(Self::collect_history_dates_for_session_tx(
                &tx, session_id, &settings, &today,
            )?);
            let removed_facts: Vec<RemovedFact> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT tool, session_id, request_key, timestamp, message_id,
                                dedupe_key, model
                         FROM local_request_facts
                         WHERE session_id = ?1 AND source_file_present != 0",
                    )
                    .map_err(|e| format!("Failed to prepare removed fact tombstones: {e}"))?;
                let rows = stmt
                    .query_map(params![session_id], |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                        ))
                    })
                    .map_err(|e| format!("Failed to query removed fact tombstones: {e}"))?;
                rows.collect::<Result<_, _>>()
                    .map_err(|e| format!("Failed to read removed fact tombstone: {e}"))?
            };
            // 软删同时 bump sync_version 让物化/导出侧感知；守卫避免对已软删行重复 bump
            tx.execute(
                "UPDATE local_request_facts
                 SET source_file_present = 0,
                     sync_version = sync_version + 1
                 WHERE session_id = ?1 AND source_file_present != 0",
                params![session_id],
            )
            .map_err(|e| format!("Failed to soft-delete local request facts: {}", e))?;
            if sync_enabled {
                for (
                    tool,
                    fact_session_id,
                    request_key,
                    timestamp,
                    message_id,
                    dedupe_key,
                    model,
                ) in removed_facts
                {
                    outbox::enqueue_request_export_tx(
                        &tx,
                        &origin_device_id,
                        &SyncExportRequest {
                            deleted: true,
                            request_key: request_key.unwrap_or_else(|| {
                                format!("{}:{}", tool.as_str(), dedupe_key.as_str())
                            }),
                            session_id: fact_session_id,
                            tool,
                            project_key: None,
                            timestamp,
                            message_id,
                            dedupe_key,
                            model,
                            input_tokens: 0,
                            output_tokens: 0,
                            cache_create_tokens: 0,
                            cache_read_tokens: 0,
                            total_tokens: 0,
                            request_count: 0,
                            explicit_estimated_cost: None,
                            is_subagent: false,
                            source_kind: "local_usage_tombstone".to_string(),
                        },
                        now,
                    )?;
                }
            }
            tx.execute(
                "UPDATE local_source_files
                 SET deleted_at = ?2,
                     deletion_reason = 'missing'
                 WHERE session_id = ?1 AND deleted_at IS NULL",
                params![session_id, now],
            )
            .map_err(|e| format!("Failed to mark local source file removed: {}", e))?;
            // Reasonix 本地扫描链路已移除（v27）：会话文件进入回收站或被删除后，
            // 不再有 reasonix 本地会话行，无需专用清理。
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

            tx.execute(
                "DELETE FROM local_session_tombstones WHERE session_id = ?1",
                params![session_id.as_str()],
            )
            .map_err(|e| format!("Failed to clear restored session tombstone: {e}"))?;

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

            // A format migration can move one logical session from session.vN.jsonl
            // to session.vN+1.jsonl. Keep only its selected generation active.
            tx.execute(
                "UPDATE local_source_files
                 SET deleted_at = ?4, deletion_reason = 'superseded'
                 WHERE session_id = ?1 AND file_role = ?2 AND file_path != ?3 AND deleted_at IS NULL",
                params![session_id.as_str(), file_role.as_str(), file_path.as_str(), now],
            )
            .map_err(|e| format!("Failed to retire previous session source file: {e}"))?;

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
            let usage_sources_json = serde_json::to_string(&meta.usage_sources)
                .map_err(|e| format!("Failed to serialize session usage sources: {}", e))?;
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
                    total_reasoning_tokens, total_elapsed_ms, explicit_cost, explicit_cost_currency,
                    estimated, usage_sources_json, model_list_json, source_kind, sync_version, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                          ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, 1, ?29)",
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
                    meta.total_reasoning_tokens as i64,
                    meta.total_elapsed_ms as i64,
                    meta.explicit_cost,
                    meta.explicit_cost_currency.as_deref(),
                    meta.estimated,
                    usage_sources_json.as_str(),
                    model_list_json.as_str(),
                    meta.source.as_str(),
                    now
                ],
            )
            .map_err(|e| format!("Failed to insert local session row: {}", e))?;
            let session_export = SyncExportSession {
                deleted: false,
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
                total_reasoning_tokens: meta.total_reasoning_tokens,
                total_elapsed_ms: meta.total_elapsed_ms,
                explicit_cost: meta.explicit_cost,
                explicit_cost_currency: meta.explicit_cost_currency.clone(),
                estimated: meta.estimated,
                usage_sources: meta.usage_sources.clone(),
                model_list: meta.models.clone(),
            };
            if sync_enabled {
                outbox::enqueue_session_export_tx(&tx, &origin_device_id, &session_export, now)?;
            }

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
                    deleted: false,
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
                if sync_enabled {
                    outbox::enqueue_request_export_tx(
                        &tx,
                        &origin_device_id,
                        &request_export,
                        now,
                    )?;
                }
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
                    if sync_enabled {
                        let tombstone = tx
                            .query_row(
                                "SELECT session_id, request_key, message_id, model, project_key
                                 FROM local_request_facts
                                 WHERE tool = ?1 AND dedupe_key = ?2",
                                params![tool.as_str(), stale_key.as_str()],
                                |row| {
                                    Ok((
                                        row.get::<_, String>(0)?,
                                        row.get::<_, Option<String>>(1)?,
                                        row.get::<_, Option<String>>(2)?,
                                        row.get::<_, String>(3)?,
                                        row.get::<_, Option<String>>(4)?,
                                    ))
                                },
                            )
                            .map_err(|e| format!("Failed to load stale fact tombstone: {e}"))?;
                        let request_key = tombstone
                            .1
                            .unwrap_or_else(|| format!("{}:{}", tool.as_str(), stale_key.as_str()));
                        outbox::enqueue_request_export_tx(
                            &tx,
                            &origin_device_id,
                            &SyncExportRequest {
                                deleted: true,
                                request_key,
                                session_id: tombstone.0,
                                tool: tool.clone(),
                                project_key: tombstone.4,
                                timestamp: *old_timestamp,
                                message_id: tombstone.2,
                                dedupe_key: stale_key.clone(),
                                model: tombstone.3,
                                input_tokens: 0,
                                output_tokens: 0,
                                cache_create_tokens: 0,
                                cache_read_tokens: 0,
                                total_tokens: 0,
                                request_count: 0,
                                explicit_estimated_cost: None,
                                is_subagent: false,
                                source_kind: "local_usage_tombstone".to_string(),
                            },
                            now,
                        )?;
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

fn log_sync_stage(stage: &str, started: Instant, result_size: usize) {
    #[cfg(test)]
    println!(
        "SYNC_STAGE stage={} elapsed_ms={:.3} result_size={}",
        stage,
        started.elapsed().as_secs_f64() * 1000.0,
        result_size
    );

    #[cfg(not(test))]
    let _ = (stage, started, result_size);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::env_lock;
    use std::fs;
    use std::io::Write;
    use std::path::Path;

    fn temp_db() -> (tempfile::TempDir, LocalUsageDatabase) {
        let tmpdir = tempfile::tempdir().expect("create temp dir");
        let path = tmpdir.path().join("local_usage.db");
        let db = LocalUsageDatabase::new_with_path(&path).expect("open temp db");
        (tmpdir, db)
    }

    /// 写入可被 Claude source 解析的 transcript（assistant 消息 JSONL）。
    fn write_claude_transcript(path: &Path, msgs: &[(&str, u64)]) {
        let mut file = fs::File::create(path).unwrap();
        for (index, (msg_id, tokens)) in msgs.iter().enumerate() {
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "type": "assistant",
                    "timestamp": 1_700_000_000 + index as i64,
                    "message": {
                        "id": msg_id,
                        "model": "claude-3-7-sonnet",
                        "usage": { "input_tokens": tokens, "output_tokens": tokens }
                    }
                })
            )
            .unwrap();
        }
        file.flush().unwrap();
    }

    fn make_file_backed_session(
        id: &str,
        path: &Path,
        last_modified: i64,
        fingerprint: u64,
    ) -> SessionFile {
        let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        SessionFile {
            session_id: id.to_string(),
            tool: "claude_code".to_string(),
            project_path: "project".to_string(),
            file_path: path.to_string_lossy().to_string(),
            transcript_paths: vec![path.to_string_lossy().to_string()],
            file_size: size,
            last_modified,
            fingerprint,
        }
    }

    fn claude_facts(db: &LocalUsageDatabase) -> Vec<LocalRequestRecord> {
        db.get_request_records_in_range(0, i64::MAX, &ToolFilter::Tool("claude_code".to_string()))
            .expect("load claude facts")
    }

    #[test]
    fn sync_file_backed_sessions_skips_unchanged_fingerprint_without_reparse() {
        let _guard = env_lock();
        let (_tmp, db) = temp_db();
        let transcript = _tmp.path().join("s1.jsonl");
        write_claude_transcript(&transcript, &[("m1", 10), ("m2", 20)]);

        db.sync_file_backed_sessions(
            vec![make_file_backed_session("proj::s1", &transcript, 100, 111)],
            &[],
        )
        .expect("first sync");
        assert_eq!(claude_facts(&db).len(), 2);

        // 磁盘内容已完全变化（新消息 m9），但 fingerprint 字段相同 → 必须跳过解析
        write_claude_transcript(&transcript, &[("m9", 90)]);
        db.sync_file_backed_sessions(
            vec![make_file_backed_session("proj::s1", &transcript, 100, 111)],
            &[],
        )
        .expect("second sync with unchanged fingerprint");

        let facts = claude_facts(&db);
        assert_eq!(facts.len(), 2, "unchanged fingerprint must not re-parse");
        assert!(facts.iter().any(|r| r.message_id == "m1"));
        assert!(
            facts.iter().all(|r| r.message_id != "m9"),
            "unchanged fingerprint must not re-read disk content"
        );
    }

    #[test]
    fn sync_file_backed_sessions_reparses_when_fingerprint_changes() {
        let _guard = env_lock();
        let (_tmp, db) = temp_db();
        let transcript = _tmp.path().join("s1.jsonl");
        write_claude_transcript(&transcript, &[("m1", 10), ("m2", 20)]);

        db.sync_file_backed_sessions(
            vec![make_file_backed_session("proj::s1", &transcript, 100, 111)],
            &[],
        )
        .expect("first sync");
        assert_eq!(claude_facts(&db).len(), 2);

        // 追加一条消息 + fingerprint 变化 → 触发重新解析（upsert，不重复写行）
        write_claude_transcript(&transcript, &[("m1", 10), ("m2", 20), ("m3", 30)]);
        db.sync_file_backed_sessions(
            vec![make_file_backed_session("proj::s1", &transcript, 100, 222)],
            &[],
        )
        .expect("second sync with changed fingerprint");

        let facts = claude_facts(&db);
        assert_eq!(facts.len(), 3, "re-parse must upsert, not duplicate rows");
        assert!(facts.iter().any(|r| r.message_id == "m3"));

        // local_source_files 中 fingerprint 已推进
        let conn = db.conn.lock().unwrap();
        let stored: String = conn
            .query_row(
                "SELECT fingerprint FROM local_source_files WHERE session_id = 'proj::s1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, "222");
    }

    #[test]
    fn sync_file_backed_sessions_detects_removed_ids_and_soft_deletes() {
        let _guard = env_lock();
        let (_tmp, db) = temp_db();
        let transcript = _tmp.path().join("s1.jsonl");
        write_claude_transcript(&transcript, &[("m1", 10)]);

        db.sync_file_backed_sessions(
            vec![make_file_backed_session("proj::s1", &transcript, 100, 111)],
            &[],
        )
        .expect("first sync");
        assert_eq!(claude_facts(&db).len(), 1);

        // 扫描结果不再包含该会话 → removed_ids 走软删路径
        db.sync_file_backed_sessions(vec![], &[])
            .expect("sync without session");

        let conn = db.conn.lock().unwrap();
        let present: i64 = conn
            .query_row(
                "SELECT source_file_present FROM local_request_facts WHERE message_id = 'm1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(present, 0, "removed session facts must be soft-deleted");
        let tombstoned: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM local_session_tombstones WHERE session_id = 'proj::s1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tombstoned, 1);
        let marked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM local_source_files WHERE session_id = 'proj::s1' AND deleted_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marked, 1);
    }

    #[test]
    fn unavailable_file_source_does_not_soft_delete_existing_facts() {
        let _guard = env_lock();
        let (_tmp, db) = temp_db();
        let transcript = _tmp.path().join("s1.jsonl");
        write_claude_transcript(&transcript, &[("m1", 10)]);
        db.sync_file_backed_sessions(
            vec![make_file_backed_session("proj::s1", &transcript, 100, 111)],
            &[],
        )
        .unwrap();

        db.sync_file_backed_sessions(vec![], &["claude_code"])
            .unwrap();
        let facts = claude_facts(&db);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].source_file_present, Some(true));
    }

    #[test]
    fn changing_selected_generation_retires_previous_source_path() {
        let _guard = env_lock();
        let (_tmp, db) = temp_db();
        let old_path = _tmp.path().join("session.v3.jsonl");
        let new_path = _tmp.path().join("session.v4.jsonl");
        write_claude_transcript(&old_path, &[("m1", 10)]);
        write_claude_transcript(&new_path, &[("m1", 10)]);
        db.sync_file_backed_sessions(
            vec![make_file_backed_session("proj::s1", &old_path, 100, 111)],
            &[],
        )
        .unwrap();
        db.sync_file_backed_sessions(
            vec![make_file_backed_session("proj::s1", &new_path, 100, 222)],
            &[],
        )
        .unwrap();

        let conn = db.conn.lock().unwrap();
        let active: i64 = conn.query_row(
            "SELECT COUNT(*) FROM local_source_files WHERE session_id = 'proj::s1' AND deleted_at IS NULL",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(active, 1);
        drop(conn);
        assert_eq!(claude_facts(&db).len(), 1);
    }
}
