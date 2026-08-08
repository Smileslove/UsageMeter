use rusqlite::params;

use super::{LocalUsageDatabase, TimestampSqlColumn};

pub(crate) const MATERIALIZED_FACT_RETENTION_DAYS: i64 = 45;

impl LocalUsageDatabase {
    pub(crate) fn materialized_fact_retention_cutoff_date(
        settings: &crate::models::AppSettings,
    ) -> Result<String, String> {
        let today = Self::today_local_date_with_settings(settings);
        let today_date = chrono::NaiveDate::parse_from_str(&today, "%Y-%m-%d")
            .map_err(|e| format!("Failed to parse materialized retention date: {e}"))?;
        Ok(
            (today_date - chrono::Duration::days(MATERIALIZED_FACT_RETENTION_DAYS))
                .format("%Y-%m-%d")
                .to_string(),
        )
    }

    pub(crate) fn materialized_fact_retention_cutoff_epoch(
        settings: &crate::models::AppSettings,
    ) -> Result<i64, String> {
        let cutoff = Self::materialized_fact_retention_cutoff_date(settings)?;
        Self::local_date_epoch_bounds_with_settings(&cutoff, settings).map(|(start, _)| start)
    }

    /// Performs bounded, idempotent maintenance during the normal global DB open.
    /// Summary rows remain durable; only finalized request-level history beyond the
    /// retention window is evicted.
    pub(super) fn run_startup_maintenance(&self) {
        // Releasing an abandoned reservation is non-destructive and can run
        // even when settings cannot be read; otherwise a crash could keep the
        // rows invisible forever until a later successful settings load.
        match self.recover_stale_sync_outbox_reservations() {
            Ok(recovered) if recovered > 0 => {
                eprintln!("[database] Released {recovered} stale in-flight sync outbox rows");
            }
            Ok(_) => {}
            Err(error) => eprintln!("[database] Stale sync outbox recovery skipped: {error}"),
        }
        let settings = match crate::settings::load_settings_blocking() {
            Ok(settings) => settings,
            Err(error) => {
                // Unknown policy must never be treated as "disabled": deleting
                // pending payloads here could permanently lose remote updates.
                eprintln!(
                    "[database] Startup maintenance skipped after settings load failure: {error}"
                );
                return;
            }
        };
        let mut deleted_rows = 0_u64;
        match self.reconcile_sync_policy(settings.sync.enabled) {
            Ok(removed) if removed > 0 => {
                eprintln!(
                    "[database] Removed {removed} pending sync outbox rows while sync is disabled"
                );
                deleted_rows = deleted_rows.saturating_add(removed);
            }
            Ok(_) => {}
            Err(error) => eprintln!("[database] Sync policy reconciliation skipped: {error}"),
        }
        if settings.sync.enabled {
            let origin_device_id =
                self.get_webdav_sync_state("device_id")
                    .ok()
                    .flatten()
                    .map(|value| crate::models::normalize_sync_device_id(&value))
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| {
                        crate::models::normalize_sync_device_id(
                            &crate::models::default_sync_device_id(),
                        )
                    });
            match self.repair_pending_request_keys(&origin_device_id) {
                Ok(repaired) if repaired > 0 => {
                    eprintln!("[database] Repaired {repaired} pending outbox request keys")
                }
                Ok(_) => {}
                Err(error) => {
                    eprintln!("[database] Startup outbox request-key repair skipped: {error}")
                }
            }
        }
        let Ok(cutoff) = Self::materialized_fact_retention_cutoff_date(&settings) else {
            return;
        };
        match self.evict_materialized_facts_before(&cutoff) {
            Ok(removed) if removed > 0 => {
                eprintln!("[database] Evicted {removed} old materialized request facts");
                deleted_rows = deleted_rows.saturating_add(removed);
            }
            Ok(_) => {}
            Err(error) => eprintln!("[database] Materialized fact retention skipped: {error}"),
        }
        if deleted_rows > 0 {
            // Consolidate startup cleanup into a single checkpoint/VACUUM pass.
            self.compact_after_large_delete(deleted_rows);
        }
    }

    pub fn clear_imported_remote_data(&self) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start imported sync clear: {}", e))?;
        tx.execute("DELETE FROM remote_request_facts", [])
            .map_err(|e| format!("Failed to clear remote request facts: {}", e))?;
        tx.execute("DELETE FROM remote_sessions", [])
            .map_err(|e| format!("Failed to clear remote sessions: {}", e))?;
        tx.execute("DELETE FROM remote_devices", [])
            .map_err(|e| format!("Failed to clear remote devices: {}", e))?;
        tx.execute("DELETE FROM sync_device_cursors", [])
            .map_err(|e| format!("Failed to clear sync device cursors: {}", e))?;
        tx.execute(
            "DELETE FROM webdav_sync_state WHERE state_key LIKE 'imported:%'",
            [],
        )
        .map_err(|e| format!("Failed to clear imported sync state: {}", e))?;
        Self::clear_unified_materialization_tx(&tx, now)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit imported sync clear: {}", e))?;
        // Large cleanup is compacted asynchronously so settings/sync commands do not block on
        // a full-file rewrite. The committed transaction already makes the freed pages visible.
        drop(conn);
        self.compact_after_large_delete(1_000);
        Ok(())
    }

    /// 统计孤立的本地事实（来源文件已消失）。
    pub fn count_orphan_local_facts(&self) -> Result<u64, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM local_request_facts WHERE source_file_present = 0",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count.max(0) as u64)
        .map_err(|e| format!("Failed to count orphan local request facts: {}", e))
    }

    /// 主动清理孤立的本地事实（来源文件已消失）。
    ///
    /// - `older_than_seconds`: 仅清理 `created_at` 早于 `now - older_than_seconds` 的行；
    ///   传 0 表示不限时间，全清。
    ///
    /// 返回删除的事实行数。同时清理掉随之无任何关联事实的 session 摘要与 source 文件行。
    pub fn purge_orphan_facts(&self, older_than_seconds: i64) -> Result<u64, String> {
        let now = chrono::Utc::now().timestamp();
        let cutoff = if older_than_seconds <= 0 {
            now
        } else {
            now - older_than_seconds
        };
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start orphan purge transaction: {}", e))?;
        let settings = crate::settings::load_settings_blocking().unwrap_or_default();
        let today = Self::today_local_date_with_settings(&settings);
        let date_expr =
            Self::business_date_sql_expr_for_timestamp(&settings, TimestampSqlColumn::Timestamp);
        let touched_history_dates: Vec<String> = {
            let mut stmt = tx
                .prepare(&format!(
                    "SELECT DISTINCT {date_expr} AS business_date
                     FROM local_request_facts
                     WHERE source_file_present = 0 AND created_at <= ?1"
                ))
                .map_err(|e| format!("Failed to prepare orphan day query: {}", e))?;
            let rows = stmt
                .query_map(params![cutoff], |row| row.get::<_, String>(0))
                .map_err(|e| format!("Failed to query orphan days: {}", e))?;
            let mut dates = Vec::new();
            for row in rows {
                let date = row.map_err(|e| format!("Failed to read orphan day row: {}", e))?;
                if date < today {
                    dates.push(date);
                }
            }
            dates
        };

        let affected = tx
            .execute(
                "DELETE FROM local_request_facts
                 WHERE source_file_present = 0 AND created_at <= ?1",
                params![cutoff],
            )
            .map_err(|e| format!("Failed to purge orphan request facts: {}", e))?;

        // 清掉孤立的 session 摘要：本身已被软删过（即对应 source_files.deleted_at 非空）
        // 且不再有任何 request fact 引用。
        tx.execute(
            "DELETE FROM local_sessions
             WHERE session_id IN (
                 SELECT session_id FROM local_source_files
                 WHERE deleted_at IS NOT NULL
             )
             AND session_id NOT IN (SELECT DISTINCT session_id FROM local_request_facts)",
            [],
        )
        .map_err(|e| format!("Failed to purge orphan local sessions: {}", e))?;

        // 清掉同样无引用的 source files 软删行
        tx.execute(
            "DELETE FROM local_source_files
             WHERE deleted_at IS NOT NULL
               AND session_id NOT IN (SELECT DISTINCT session_id FROM local_request_facts)",
            [],
        )
        .map_err(|e| format!("Failed to purge orphan local source files: {}", e))?;

        Self::upsert_sync_state(&tx, "last_orphan_purge_at", &now.to_string(), now)?;
        Self::upsert_sync_state(
            &tx,
            "last_orphan_purge_count",
            &(affected as i64).to_string(),
            now,
        )?;
        Self::invalidate_unified_materialization_dates_tx(&tx, &touched_history_dates, now)?;

        tx.commit()
            .map_err(|e| format!("Failed to commit orphan purge: {}", e))?;
        Ok(affected.max(0) as u64)
    }

    /// 清空本地缓存并强制下一次同步从 JSONL 全量重建。
    ///
    /// 主要给用户「重建本地缓存」按钮使用。会清掉 `local_request_facts` /
    /// `local_sessions` / `local_source_files`；不影响 remote_* 表或 outbox 表。
    pub fn truncate_all_local_facts(&self) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start truncate local facts: {}", e))?;
        tx.execute("DELETE FROM local_request_facts", [])
            .map_err(|e| format!("Failed to delete local request facts: {}", e))?;
        tx.execute("DELETE FROM local_sessions", [])
            .map_err(|e| format!("Failed to delete local sessions: {}", e))?;
        tx.execute("DELETE FROM local_source_files", [])
            .map_err(|e| format!("Failed to delete local source files: {}", e))?;
        tx.execute("DELETE FROM local_sync_cursors", [])
            .map_err(|e| format!("Failed to delete local sync cursors: {}", e))?;
        Self::upsert_sync_state(&tx, "last_truncate_local_at", &now.to_string(), now)?;
        Self::clear_unified_materialization_tx(&tx, now)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit truncate local facts: {}", e))?;
        Ok(())
    }
}
