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
pub fn get_sync_status(settings: AppSettings) -> Result<SyncStatus, String> {
    crate::sync::get_status(&settings.sync)
}

#[tauri::command]
pub fn list_sync_devices() -> Result<Vec<crate::local_usage::RemoteSyncDevice>, String> {
    let db = crate::local_usage::ensure_local_usage_synced()?;
    db.list_remote_devices()
}

#[tauri::command]
pub fn remove_sync_device(device_id: String) -> Result<(), String> {
    let db = crate::local_usage::ensure_local_usage_synced()?;
    db.remove_remote_device(&device_id)
}

#[tauri::command]
pub fn clear_imported_sync_data() -> Result<(), String> {
    let db = crate::local_usage::ensure_local_usage_synced()?;
    db.clear_imported_remote_data()
}

/// 获取当前设备在同步状态 DB 中存储的 device_id（首次同步后自动生成的值）。
/// 用于在前端 device_id 输入框为空时，将后端实际使用的 ID 同步回 UI。
#[tauri::command]
pub fn get_active_sync_device_id() -> Result<Option<String>, String> {
    let db = crate::local_usage::ensure_local_usage_synced()?;
    let device_id = db
        .get_webdav_sync_state("device_id")?
        .map(|v| crate::models::normalize_sync_device_id(&v))
        .filter(|v| !v.is_empty());
    Ok(device_id)
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
