//! 应用更新相关 Tauri 命令
//!
//! 通过 tauri-plugin-updater 检查 GitHub Releases，下载并安装更新。
//! 使用 UpdaterState 在命令调用之间保存 Update 对象（Update 不可序列化，无法跨命令传递）。

use semver::Version;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

use super::usage::ProxyState;

/// 跨命令共享的更新状态（持有 Update 对象供后续下载使用）
pub struct UpdaterState {
    #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
    pub pending_update: Mutex<Option<tauri_plugin_updater::Update>>,
}

impl Default for UpdaterState {
    fn default() -> Self {
        Self {
            #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
            pending_update: Mutex::new(None),
        }
    }
}

/// 返回给前端的更新信息 DTO
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfoDto {
    pub version: String,
    pub current_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// ISO 8601 日期字符串
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

/// 下载进度事件（通过 `update-download-progress` 事件推送给前端）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDownloadProgressEvent {
    pub downloaded_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
}

fn normalize_version_string(version: &str) -> &str {
    version.trim().trim_start_matches(['v', 'V'])
}

pub fn should_suppress_update(update_version: &str, skipped_version: &str) -> bool {
    let skipped_version = normalize_version_string(skipped_version);
    if skipped_version.is_empty() {
        return false;
    }

    let update_version = normalize_version_string(update_version);

    match (
        Version::parse(update_version),
        Version::parse(skipped_version),
    ) {
        (Ok(update), Ok(skipped)) => update <= skipped,
        _ => update_version == skipped_version,
    }
}

/// 构建带代理配置的 Updater 实例（供命令和后台检查共用）
#[cfg(any(target_os = "macos", windows, target_os = "linux"))]
pub fn build_updater(app: &AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    use crate::commands::load_settings;
    use tauri_plugin_updater::UpdaterExt;

    let settings = load_settings().unwrap_or_default();
    let mut builder = app.updater_builder();

    if settings.network_proxy.enabled {
        let proxy_url = format!(
            "{}://{}:{}",
            settings.network_proxy.scheme, settings.network_proxy.host, settings.network_proxy.port
        );
        if let Ok(url) = proxy_url.parse() {
            builder = builder.proxy(url);
        }
    }

    builder
        .build()
        .map_err(|e| format!("ERR_UPDATER_BUILD: {e}"))
}

/// 检查是否有可用更新
///
/// 若有更新，将 Update 对象存入 UpdaterState 供后续下载，并返回版本信息。
/// 返回 None 表示已是最新版本。
/// 检查请求会遵循用户配置的全局网络代理。
#[tauri::command]
pub async fn check_for_update(
    app: AppHandle,
    state: State<'_, UpdaterState>,
) -> Result<Option<UpdateInfoDto>, String> {
    #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
    {
        let updater = build_updater(&app)?;

        match updater.check().await {
            Ok(Some(update)) => {
                let dto = build_dto(&update);
                *state.pending_update.lock().unwrap() = Some(update);
                Ok(Some(dto))
            }
            Ok(None) => {
                *state.pending_update.lock().unwrap() = None;
                Ok(None)
            }
            Err(e) => Err(format!("ERR_UPDATE_CHECK: {e}")),
        }
    }

    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    {
        let _ = (app, state);
        Ok(None)
    }
}

/// 下载并安装更新，安装完成后重启应用
///
/// 通过 `update-download-progress` 事件实时推送下载进度。
/// 调用此命令前必须先成功调用 `check_for_update`。
#[tauri::command]
pub async fn download_and_install_update(
    app: AppHandle,
    proxy_state: State<'_, ProxyState>,
    state: State<'_, UpdaterState>,
) -> Result<(), String> {
    #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
    {
        crate::commands::stop_proxy_runtime_only_inner(&proxy_state).await?;

        let update = state
            .pending_update
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| "ERR_NO_PENDING_UPDATE".to_string())?;

        let app_for_progress = app.clone();
        let mut downloaded_bytes: u64 = 0;

        update
            .download_and_install(
                move |chunk_len, content_length| {
                    downloaded_bytes += chunk_len as u64;
                    let _ = app_for_progress.emit(
                        "update-download-progress",
                        UpdateDownloadProgressEvent {
                            downloaded_bytes,
                            total_bytes: content_length,
                        },
                    );
                },
                || {},
            )
            .await
            .map_err(|e| format!("ERR_UPDATE_INSTALL: {e}"))?;

        app.restart();
    }

    // 仅在非桌面平台（不支持更新）时返回 Ok
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    {
        let _ = (app, proxy_state, state);
        Ok(())
    }
}

/// 跳过指定版本：仅更新 skipped_update_version 字段，不影响其他设置
///
/// 直接读写文件而不经过前端，避免覆盖用户在 UI 中未保存的其他改动。
/// 使用 JSON patch 方式：只修改目标字段，保留其余内容原样。
#[tauri::command]
pub fn skip_update_version(version: String, state: State<'_, UpdaterState>) -> Result<(), String> {
    use std::fs;

    let path = crate::models::AppSettings::settings_path()?;

    // 读取现有 JSON（文件不存在时以空对象起步）
    let raw = if path.exists() {
        fs::read_to_string(&path).map_err(|e| format!("ERR_READ_SETTINGS: {e}"))?
    } else {
        "{}".to_string()
    };

    let mut json: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("ERR_PARSE_SETTINGS: {e}"))?;

    // 仅写入目标字段，其余键保持原样
    if let serde_json::Value::Object(ref mut map) = json {
        map.insert(
            "skippedUpdateVersion".to_string(),
            serde_json::Value::String(version),
        );
    }

    let content =
        serde_json::to_string_pretty(&json).map_err(|e| format!("ERR_SERIALIZE_SETTINGS: {e}"))?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("ERR_CREATE_SETTINGS_DIR: {e}"))?;
    }
    fs::write(&path, content).map_err(|e| format!("ERR_WRITE_SETTINGS: {e}"))?;
    *state.pending_update.lock().unwrap() = None;

    Ok(())
}

/// 将插件 Update 转换为可序列化的 DTO
#[cfg(any(target_os = "macos", windows, target_os = "linux"))]
pub fn build_dto(update: &tauri_plugin_updater::Update) -> UpdateInfoDto {
    UpdateInfoDto {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        body: update.body.clone(),
        // 仅返回稳定的日级日期字符串，避免不同平台对时区日期的解析差异。
        date: update.date.map(|d| d.date().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::should_suppress_update;

    #[test]
    fn suppresses_same_skipped_version() {
        assert!(should_suppress_update("0.6.4", "0.6.4"));
    }

    #[test]
    fn suppresses_older_versions_when_newer_skip_exists() {
        assert!(should_suppress_update("0.6.3", "0.6.4"));
    }

    #[test]
    fn does_not_suppress_newer_versions() {
        assert!(!should_suppress_update("0.6.5", "0.6.4"));
    }

    #[test]
    fn handles_prefixed_versions() {
        assert!(should_suppress_update("v0.6.4", "0.6.4"));
        assert!(!should_suppress_update("v0.6.5", "v0.6.4"));
    }

    #[test]
    fn falls_back_to_exact_match_for_non_semver_strings() {
        assert!(should_suppress_update("build-123", "build-123"));
        assert!(!should_suppress_update("build-124", "build-123"));
    }
}
