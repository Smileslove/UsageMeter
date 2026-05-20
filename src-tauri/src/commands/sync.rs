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
