//! Tauri adapters for application settings.

use crate::models::AppSettings;
use tauri::{AppHandle, Emitter};

pub use crate::settings::{
    load_settings_blocking, save_settings_internal, update_settings_internal, SaveSettingsError,
};

/// Load settings through Tauri's async runtime without exposing persistence
/// details to the command boundary.
#[tauri::command]
pub async fn load_settings() -> Result<AppSettings, String> {
    tauri::async_runtime::spawn_blocking(load_settings_blocking)
        .await
        .map_err(|e| format!("join error: {e}"))?
}

/// Save settings and report a runtime network-proxy reload failure separately.
#[tauri::command]
pub async fn save_settings(app: AppHandle, settings: AppSettings) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || match save_settings_internal(settings) {
        Ok(()) => Ok(()),
        Err(SaveSettingsError::ReloadFailed(err)) => {
            let _ = app.emit("network-proxy-reload-failed", err.clone());
            Err(err)
        }
        Err(SaveSettingsError::Other(err)) => Err(err),
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

/// List installed WSL distributions (Windows only).
#[tauri::command]
pub async fn list_wsl_distros() -> Vec<String> {
    tauri::async_runtime::spawn_blocking(crate::settings::list_wsl_distros_blocking)
        .await
        .unwrap_or_default()
}
