//! Auto-start (开机自启动) 相关 Tauri 命令

use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

/// 启用开机自启动
#[tauri::command]
pub fn enable_autostart(app: AppHandle) -> Result<(), String> {
    app.autolaunch().enable().map_err(|e| format!("{:?}", e))
}

/// 禁用开机自启动
#[tauri::command]
pub fn disable_autostart(app: AppHandle) -> Result<(), String> {
    app.autolaunch().disable().map_err(|e| format!("{:?}", e))
}

/// 检查开机自启动状态
#[tauri::command]
pub fn is_autostart_enabled(app: AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|e| format!("{:?}", e))
}
