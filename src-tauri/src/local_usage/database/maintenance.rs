use rusqlite::params;

use super::LocalUsageDatabase;

impl LocalUsageDatabase {
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
        // 大批量删除后收缩数据库文件
        conn.execute_batch("VACUUM")
            .map_err(|e| format!("Failed to vacuum after imported sync clear: {}", e))?;
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
        let touched_history_dates: Vec<String> = {
            let mut stmt = tx
                .prepare(
                    "SELECT DISTINCT strftime('%Y-%m-%d', timestamp, 'unixepoch', 'localtime')
                     FROM local_request_facts
                     WHERE source_file_present = 0 AND created_at <= ?1",
                )
                .map_err(|e| format!("Failed to prepare orphan day query: {}", e))?;
            let rows = stmt
                .query_map(params![cutoff], |row| row.get::<_, String>(0))
                .map_err(|e| format!("Failed to query orphan days: {}", e))?;
            let mut dates = Vec::new();
            for row in rows {
                let date = row.map_err(|e| format!("Failed to read orphan day row: {}", e))?;
                if date < Self::today_local_date() {
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
