use super::{outbox, LocalUsageDatabase};
use crate::session::project::{GitProject, ProjectResolver};
use rusqlite::params;
use std::collections::{HashMap, HashSet};

impl LocalUsageDatabase {
    /// Runs independently of transcript fingerprints so existing history is backfilled too.
    pub(super) fn refresh_project_paths(
        &self,
        settings: &crate::models::AppSettings,
    ) -> Result<(), String> {
        let paths = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT s.cwd, p.common_dir, p.project_path, p.project_name
                 FROM local_sessions s LEFT JOIN local_project_paths p ON p.cwd = s.cwd
                 WHERE s.cwd IS NOT NULL AND s.cwd != ''",
                )
                .map_err(|e| format!("Failed to prepare project paths: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    let common: Option<String> = row.get(1)?;
                    Ok((
                        row.get::<_, String>(0)?,
                        common
                            .map(|common_dir| {
                                Ok::<_, rusqlite::Error>(GitProject {
                                    common_dir,
                                    path: row.get(2)?,
                                    name: row.get(3)?,
                                })
                            })
                            .transpose()?,
                    ))
                })
                .map_err(|e| format!("Failed to query project paths: {e}"))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to read project paths: {e}"))?
        };
        let mut resolver = ProjectResolver::default();
        for (_, project) in &paths {
            if let Some(project) = project {
                resolver.remember(project);
            }
        }
        // Discover valid primary checkouts first, so a moved primary wins regardless
        // of the order of cwd rows (notably with --separate-git-dir).
        for (cwd, previous) in &paths {
            let _ = resolver.resolve(cwd, previous.as_ref());
        }
        let resolved: Vec<_> = paths
            .into_iter()
            .map(|(cwd, previous)| {
                let project = resolver.resolve(&cwd, previous.as_ref());
                (cwd, previous, project)
            })
            .collect();
        let projects_by_cwd: HashMap<_, _> = resolved
            .iter()
            .filter_map(|(cwd, _, project)| project.as_ref().map(|project| (cwd.as_str(), project)))
            .collect();
        let projects_by_common: HashMap<_, _> = resolved
            .iter()
            .filter_map(|(_, _, project)| {
                project
                    .as_ref()
                    .map(|project| (project.common_dir.as_str(), project))
            })
            .collect();
        let mut changed_sessions = HashMap::new();
        {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT s.session_id, s.cwd, s.project_key, s.project_name,
                        p.common_dir, p.project_path, p.project_name
                 FROM local_sessions s LEFT JOIN local_session_projects p ON p.session_id = s.session_id",
            ).map_err(|e| format!("Failed to prepare session project backfill: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    let common: Option<String> = row.get(4)?;
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        common
                            .map(|common_dir| {
                                Ok::<_, rusqlite::Error>(GitProject {
                                    common_dir,
                                    path: row.get(5)?,
                                    name: row.get(6)?,
                                })
                            })
                            .transpose()?,
                    ))
                })
                .map_err(|e| format!("Failed to query session project backfill: {e}"))?;
            for row in rows {
                let (id, cwd, key, name, bound) =
                    row.map_err(|e| format!("Failed to read session project backfill: {e}"))?;
                let desired = match &bound {
                    // A known session can follow its repository's primary checkout move,
                    // but can never migrate to another repository just because cwd was reused.
                    Some(bound) => projects_by_common
                        .get(bound.common_dir.as_str())
                        .copied()
                        .or(Some(bound)),
                    None => cwd
                        .as_deref()
                        .and_then(|cwd| projects_by_cwd.get(cwd).copied()),
                };
                if let Some(project) = desired {
                    if bound.as_ref() != Some(project)
                        || key.as_deref() != Some(&project.path)
                        || name.as_deref() != Some(&project.name)
                    {
                        changed_sessions.insert(id, project.clone());
                    }
                }
            }
        }
        let mapping_changed = resolved
            .iter()
            .any(|(_, previous, project)| previous != project);
        if changed_sessions.is_empty() && !mapping_changed {
            return Ok(());
        }
        // Reuse the existing export serializer for changed rows, including all usage fields.
        let mut export = if settings.sync.enabled && !changed_sessions.is_empty() {
            Some(self.get_sync_export_data()?)
        } else {
            None
        };
        let device_id = self
            .get_webdav_sync_state("device_id")?
            .map(|id| crate::models::normalize_sync_device_id(&id))
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| {
                crate::models::normalize_sync_device_id(&crate::models::default_sync_device_id())
            });
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start project path refresh: {e}"))?;
        for (cwd, previous, project) in resolved {
            if project != previous {
                if let Some(project) = &project {
                    tx.execute(
                        "INSERT INTO local_project_paths (cwd, common_dir, project_path, project_name)
                         VALUES (?1, ?2, ?3, ?4) ON CONFLICT(cwd) DO UPDATE SET
                         common_dir = excluded.common_dir, project_path = excluded.project_path,
                         project_name = excluded.project_name",
                        params![cwd, project.common_dir, project.path, project.name],
                    ).map_err(|e| format!("Failed to save project path: {e}"))?;
                } else {
                    tx.execute("DELETE FROM local_project_paths WHERE cwd = ?1", [&cwd])
                        .map_err(|e| format!("Failed to remove project path: {e}"))?;
                }
            }
        }
        let today = Self::today_local_date_with_settings(settings);
        let mut history_dates = HashSet::new();
        for (id, project) in &changed_sessions {
            let key = &project.path;
            let name = &project.name;
            history_dates.extend(Self::collect_history_dates_for_session_tx(
                &tx, id, settings, &today,
            )?);
            tx.execute(
                "INSERT INTO local_session_projects (session_id, common_dir, project_path, project_name)
                 VALUES (?1, ?2, ?3, ?4) ON CONFLICT(session_id) DO UPDATE SET
                 common_dir = excluded.common_dir, project_path = excluded.project_path, project_name = excluded.project_name",
                params![id, project.common_dir, key, name],
            ).map_err(|e| format!("Failed to persist session project: {e}"))?;
            // Proxy-only requests have no local request row, but their persisted facts
            // still depend on SessionMeta. Invalidate every cached day for this session.
            let mut stmt = tx.prepare(
                "SELECT DISTINCT local_date FROM unified_daily_materialized_facts WHERE session_id = ?1",
            ).map_err(|e| format!("Failed to prepare cached session dates: {e}"))?;
            let dates = stmt
                .query_map([id], |row| row.get::<_, String>(0))
                .map_err(|e| format!("Failed to query cached session dates: {e}"))?;
            for date in dates {
                history_dates
                    .insert(date.map_err(|e| format!("Failed to read cached session date: {e}"))?);
            }
            tx.execute(
                "UPDATE local_sessions SET project_key = ?2, project_name = ?3,
                 sync_version = sync_version + 1, updated_at = ?4 WHERE session_id = ?1",
                params![id, key, name, now],
            )
            .map_err(|e| format!("Failed to backfill session project: {e}"))?;
            tx.execute(
                "UPDATE local_request_facts SET project_key = ?2, sync_version = sync_version + 1
                 WHERE session_id = ?1 AND project_key IS NOT ?2",
                params![id, key],
            )
            .map_err(|e| format!("Failed to backfill request project: {e}"))?;
            tx.execute("UPDATE local_source_files SET project_key = ?2 WHERE session_id = ?1 AND project_key IS NOT ?2", params![id, key])
                .map_err(|e| format!("Failed to backfill source project: {e}"))?;
        }
        if !changed_sessions.is_empty() {
            // Retention removes per-session facts while keeping summaries. Their session
            // membership is no longer available, so conservatively invalidate evicted days.
            let mut stmt = tx.prepare(
                "SELECT local_date FROM unified_daily_materialization_state WHERE fact_cache_status != 'complete'",
            ).map_err(|e| format!("Failed to prepare evicted history dates: {e}"))?;
            let dates = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| format!("Failed to query evicted history dates: {e}"))?;
            for date in dates {
                history_dates
                    .insert(date.map_err(|e| format!("Failed to read evicted history date: {e}"))?);
            }
        }
        Self::invalidate_unified_materialization_dates_tx(
            &tx,
            &history_dates.into_iter().collect::<Vec<_>>(),
            now,
        )?;
        if let Some(export) = &mut export {
            for session in &mut export.sessions {
                if session.deleted {
                    continue;
                }
                if let Some(project) = changed_sessions.get(&session.session_id) {
                    session.project_key = Some(project.path.clone());
                    session.project_name = Some(project.name.clone());
                    outbox::enqueue_session_export_tx(&tx, &device_id, session, now)?;
                }
            }
            for request in &mut export.requests {
                if request.deleted {
                    continue;
                }
                if let Some(project) = changed_sessions.get(&request.session_id) {
                    request.project_key = Some(project.path.clone());
                    outbox::enqueue_request_export_tx(&tx, &device_id, request, now)?;
                }
            }
        }
        if mapping_changed {
            tx.execute(
                "UPDATE local_sync_state SET state_value = CAST(CAST(state_value AS INTEGER) + 1 AS TEXT), updated_at = ?1
                 WHERE state_key = 'merge_cache_generation'", [now],
            ).map_err(|e| format!("Failed to invalidate project cache: {e}"))?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit project path refresh: {e}"))?;
        Ok(())
    }
}
