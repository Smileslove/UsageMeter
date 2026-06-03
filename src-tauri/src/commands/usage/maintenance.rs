#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalUsageMaintenanceStats {
    pub total_local_facts: u64,
    pub orphan_local_facts: u64,
}

/// 获取本地缓存维护状态（用于设置页展示）。
#[tauri::command]
pub async fn get_local_usage_maintenance_stats() -> Result<LocalUsageMaintenanceStats, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let db = crate::local_usage::ensure_local_usage_synced()?;
        let total = db.count_local_request_facts()?;
        let orphan = db.count_orphan_local_facts()?;
        Ok::<_, String>(LocalUsageMaintenanceStats {
            total_local_facts: total,
            orphan_local_facts: orphan,
        })
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

/// 检查 OpenCode 数据库 schema 兼容性（用于设置页告知用户）。
#[tauri::command]
pub async fn get_opencode_schema_status(
) -> Result<crate::session::opencode_reader::OpenCodeSchemaStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut status = crate::session::opencode_reader::check_opencode_schema();
        if let Ok(db) = crate::local_usage::ensure_local_usage_synced() {
            status.persisted_compatibility_mode = db
                .get_local_sync_state("opencode_db_schema_mode")
                .ok()
                .flatten();
            status.message_id_conflict.has_conflict = db
                .get_local_sync_state("opencode_message_id_conflict_has_conflict")
                .ok()
                .flatten()
                .map(|value| value == "1")
                .unwrap_or(status.message_id_conflict.has_conflict);
            status.message_id_conflict.conflict_count = db
                .get_local_sync_state("opencode_message_id_conflict_count")
                .ok()
                .flatten()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(status.message_id_conflict.conflict_count);
            status.message_id_conflict.sample_ids = db
                .get_local_sync_state("opencode_message_id_conflict_sample_ids")
                .ok()
                .flatten()
                .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
                .unwrap_or_else(|| status.message_id_conflict.sample_ids.clone());
        }
        Ok(status)
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

/// 清理孤立的本地事实（来源文件已消失的请求记录）。
///
/// `older_than_days`：仅清理 `created_at` 早于该天数的孤立行；传 0 表示全部清理。
#[tauri::command]
pub async fn purge_orphan_local_facts(older_than_days: u32) -> Result<u64, String> {
    let seconds = (older_than_days as i64).saturating_mul(86400);
    tauri::async_runtime::spawn_blocking(move || {
        let db = crate::local_usage::ensure_local_usage_synced()?;
        db.purge_orphan_facts(seconds)
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

/// 重建本地缓存：清空所有 local_* 表，然后强制从 JSONL 全量重新解析。
#[tauri::command]
pub async fn rebuild_local_usage_cache() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        let db = crate::local_usage::LocalUsageDatabase::get_global()?;
        db.truncate_all_local_facts()?;
        db.sync_from_scanner()?;
        Ok::<_, String>(())
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}
