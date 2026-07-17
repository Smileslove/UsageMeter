//! WebDAV sync commands.

use crate::models::AppSettings;
use crate::sync::{RotateSyncPasswordPayload, SyncStatus, WebDavCredentials};

#[tauri::command]
pub async fn test_webdav_connection(
    settings: AppSettings,
    credentials: WebDavCredentials,
) -> Result<(), String> {
    let credentials = resolve_credentials(&settings, credentials);
    crate::sync::test_connection(settings.sync, credentials).await
}

#[tauri::command]
pub async fn sync_now(
    settings: AppSettings,
    credentials: WebDavCredentials,
) -> Result<SyncStatus, String> {
    let credentials = resolve_credentials(&settings, credentials);
    crate::sync::sync_now(settings.sync, credentials).await
}

#[tauri::command]
pub async fn rotate_sync_password(
    settings: AppSettings,
    credentials: WebDavCredentials,
    payload: RotateSyncPasswordPayload,
) -> Result<(), String> {
    let credentials = resolve_credentials(&settings, credentials);
    crate::sync::rotate_sync_password(settings.sync, credentials, payload).await
}

#[tauri::command]
pub async fn get_sync_status(settings: AppSettings) -> Result<SyncStatus, String> {
    tauri::async_runtime::spawn_blocking(move || crate::sync::get_status(&settings.sync))
        .await
        .map_err(|e| format!("join error: {e}"))?
}

#[tauri::command]
pub async fn list_sync_devices() -> Result<Vec<crate::local_usage::RemoteSyncDevice>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        // 仅读取设备元数据表，无需先触发全量本地扫描
        let db = crate::local_usage::get_local_usage_db()?;
        db.list_remote_devices()
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

#[tauri::command]
pub async fn remove_sync_device(device_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let db = crate::local_usage::get_local_usage_db()?;
        db.remove_remote_device(&device_id)
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

#[tauri::command]
pub async fn clear_imported_sync_data() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        let db = crate::local_usage::get_local_usage_db()?;
        db.clear_imported_remote_data()
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

/// 获取当前设备在同步状态 DB 中存储的 device_id（首次同步后自动生成的值）。
/// 用于在前端 device_id 输入框为空时，将后端实际使用的 ID 同步回 UI。
#[tauri::command]
pub async fn get_active_sync_device_id() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let db = crate::local_usage::get_local_usage_db()?;
        let device_id = db
            .get_webdav_sync_state("device_id")?
            .map(|v| crate::models::normalize_sync_device_id(&v))
            .filter(|v| !v.is_empty());
        Ok(device_id)
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

fn resolve_credentials(
    settings: &AppSettings,
    mut credentials: WebDavCredentials,
) -> WebDavCredentials {
    if credentials.password.is_empty() {
        credentials.password = settings.sync.password.clone();
    }
    if credentials.sync_password.is_empty() {
        credentials.sync_password = settings.sync.sync_password.clone();
    }
    credentials
}
