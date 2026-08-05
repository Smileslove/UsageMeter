//! 设置相关 Tauri 命令

use crate::models::{AppSettings, CurrencySettings};
use crate::net::HttpClientFactory;
use crate::{app_config::with_config_database, proxy::ProxyDatabase};
use std::fs;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use tauri::{AppHandle, Emitter};

static SETTINGS_WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// 加载应用设置（同步实现，供 Rust 内部直接调用；macOS 上可能 spawn Keychain 子进程）
pub fn load_settings_blocking() -> Result<AppSettings, String> {
    let mut settings = with_config_database(|database| {
        if let Some(mut settings) = database.load_settings()? {
            if let Some(preferences) = load_preferences_file()? {
                apply_preferences(&mut settings, preferences);
            }
            normalize_settings(&mut settings)?;
            migrate_legacy_model_pricings(&mut settings)?;
            // Re-save once to remove the old full-snapshot documents after a
            // user upgrades to the split preferences/entity layout.
            database.save_settings(&settings)?;
            write_preferences_file(&settings)?;
            Ok(settings)
        } else {
            let mut settings = load_preferences_file()?.unwrap_or_default();
            normalize_settings(&mut settings)?;
            migrate_legacy_model_pricings(&mut settings)?;

            let previous_settings = settings.clone();
            crate::subscription::source_quota_secrets::persist_settings(
                &mut settings,
                &previous_settings,
            )?;
            database.save_settings(&settings)?;
            write_preferences_file(&settings)?;
            Ok(settings)
        }
    })?;

    crate::subscription::source_quota_secrets::hydrate_settings(&mut settings)?;
    Ok(settings)
}

fn load_preferences_file() -> Result<Option<AppSettings>, String> {
    let path = AppSettings::settings_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).map_err(|e| format!("ERR_READ_SETTINGS: {e}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("ERR_PARSE_SETTINGS: {e}"))?;
    let mut settings: AppSettings =
        serde_json::from_value(value.clone()).map_err(|e| format!("ERR_PARSE_SETTINGS: {e}"))?;

    if let Some(filters) = value.get("filters") {
        settings.source_aware.active_source_filter = filters
            .get("activeSourceFilter")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        settings.client_tools.active_tool_filter = filters
            .get("activeToolFilter")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
    }

    Ok(Some(settings))
}

fn apply_preferences(settings: &mut AppSettings, preferences: AppSettings) {
    settings.locale = preferences.locale;
    settings.timezone = preferences.timezone;
    settings.refresh_interval_seconds = preferences.refresh_interval_seconds;
    settings.summary_window = preferences.summary_window;
    settings.day_boundary_mode = preferences.day_boundary_mode;
    settings.number_format = preferences.number_format;
    settings.proxy = preferences.proxy;
    settings.theme = preferences.theme;
    settings.model_pricing = preferences.model_pricing;
    settings.auto_start = preferences.auto_start;
    settings.currency = preferences.currency;
    settings.sync = preferences.sync;
    settings.network_proxy = preferences.network_proxy;
    settings.auto_check_update = preferences.auto_check_update;
    settings.skipped_update_version = preferences.skipped_update_version;
    settings.wsl_scan = preferences.wsl_scan;
    settings.source_aware.active_source_filter = preferences.source_aware.active_source_filter;
    settings.client_tools.active_tool_filter = preferences.client_tools.active_tool_filter;
}

/// 加载应用设置
#[tauri::command]
pub async fn load_settings() -> Result<AppSettings, String> {
    tauri::async_runtime::spawn_blocking(load_settings_blocking)
        .await
        .map_err(|e| format!("join error: {e}"))?
}

/// 保存应用设置（Tauri 命令）。
///
/// 网络代理 reload 失败时会同时 emit `network-proxy-reload-failed` 事件并返回 Err，
/// 让前端 UI 能感知"已落盘但运行时未应用"的状态。
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

/// 内部错误分类，让命令层能区分"保存失败"与"保存成功但热更新失败"。
pub enum SaveSettingsError {
    /// 序列化、写盘等失败（数据未落盘）。
    Other(String),
    /// 数据已落盘，但 HTTP 客户端热更新失败（运行时仍是旧配置）。
    ReloadFailed(String),
}

impl From<SaveSettingsError> for String {
    fn from(value: SaveSettingsError) -> Self {
        match value {
            SaveSettingsError::Other(s) | SaveSettingsError::ReloadFailed(s) => s,
        }
    }
}

impl std::fmt::Display for SaveSettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveSettingsError::Other(s) | SaveSettingsError::ReloadFailed(s) => f.write_str(s),
        }
    }
}

/// 真正的保存逻辑。供非 Tauri 上下文（后台同步、代理服务器内部）复用。
///
/// 所有设置写入都通过同一把进程内锁串行化，避免多个全量设置快照同时写盘时
/// 互相破坏临时文件。需要基于最新设置做局部修改时，应优先使用
/// `update_settings_internal`，让“读取—修改—保存”整个过程处于同一临界区。
pub fn save_settings_internal(mut settings: AppSettings) -> Result<(), SaveSettingsError> {
    let _guard = SETTINGS_WRITE_LOCK
        .lock()
        .map_err(|_| SaveSettingsError::Other("ERR_SETTINGS_WRITE_LOCK_POISONED".to_string()))?;
    let previous_settings = load_settings_blocking().unwrap_or_default();
    // Gateway credentials and routing metadata are owned by the dedicated
    // gateway mutation commands. Generic full-settings writers commonly hold
    // an older UI/background snapshot, so allowing them to replace this
    // namespace would reintroduce lost updates even though disk writes are
    // serialized.
    settings.gateway = previous_settings.gateway.clone();
    save_settings_locked(settings, &previous_settings)
}

/// 基于磁盘上的最新设置执行原子局部更新。
///
/// 修改闭包返回的业务结果会与最终落盘设置一起返回，调用方可据此刷新运行时快照。
pub fn update_settings_internal<T, F>(mutate: F) -> Result<(T, AppSettings), SaveSettingsError>
where
    F: FnOnce(&mut AppSettings) -> Result<T, String>,
{
    let _guard = SETTINGS_WRITE_LOCK
        .lock()
        .map_err(|_| SaveSettingsError::Other("ERR_SETTINGS_WRITE_LOCK_POISONED".to_string()))?;
    let previous_settings = load_settings_blocking().map_err(SaveSettingsError::Other)?;
    let mut settings = previous_settings.clone();
    let result = mutate(&mut settings).map_err(SaveSettingsError::Other)?;
    save_settings_locked(settings.clone(), &previous_settings)?;
    Ok((result, settings))
}

fn save_settings_locked(
    mut settings: AppSettings,
    previous_settings: &AppSettings,
) -> Result<(), SaveSettingsError> {
    normalize_settings(&mut settings).map_err(SaveSettingsError::Other)?;
    migrate_legacy_model_pricings(&mut settings).map_err(SaveSettingsError::Other)?;
    crate::subscription::source_quota_secrets::persist_settings(&mut settings, previous_settings)
        .map_err(SaveSettingsError::Other)?;
    with_config_database(|database| database.save_settings(&settings))
        .map_err(SaveSettingsError::Other)?;
    write_preferences_file(&settings).map_err(SaveSettingsError::Other)?;

    if previous_settings.day_boundary_mode != settings.day_boundary_mode {
        reset_day_boundary_caches().map_err(SaveSettingsError::Other)?;
    }

    if let Err(err) = HttpClientFactory::global().reload(&settings.network_proxy) {
        eprintln!("[UsageMeter] {err}");
        return Err(SaveSettingsError::ReloadFailed(err));
    }
    Ok(())
}

/// Imports the pre-database pricing vector into its existing authoritative table.
/// The vector is then removed from application settings to prevent two sources of truth.
fn migrate_legacy_model_pricings(settings: &mut AppSettings) -> Result<bool, String> {
    if settings.model_pricing.pricings.is_empty() {
        return Ok(false);
    }
    let database = ProxyDatabase::new_pricing_store()?;
    database.upsert_model_pricings(&settings.model_pricing.pricings)?;
    settings.model_pricing.pricings.clear();
    Ok(true)
}

/// Writes only stable user preferences. Configurable entity collections live
/// in `app_config.db`; existing credential locations are unchanged pending the
/// separate SecretStore migration.
fn write_preferences_file(settings: &AppSettings) -> Result<(), String> {
    let mut value =
        serde_json::to_value(settings).map_err(|e| format!("ERR_SERIALIZE_SETTINGS: {e}"))?;
    let document = value
        .as_object_mut()
        .ok_or_else(|| "ERR_SETTINGS_NOT_OBJECT".to_string())?;
    let active_source_filter = settings.source_aware.active_source_filter.clone();
    let active_tool_filter = settings.client_tools.active_tool_filter.clone();

    document.remove("gateway");
    document.remove("sourceAware");
    document.remove("clientTools");
    document.insert("settingsVersion".to_string(), serde_json::json!(2));
    document.insert(
        "filters".to_string(),
        serde_json::json!({
            "activeSourceFilter": active_source_filter,
            "activeToolFilter": active_tool_filter,
        }),
    );

    let content =
        serde_json::to_string_pretty(&value).map_err(|e| format!("ERR_SERIALIZE_SETTINGS: {e}"))?;
    let path = AppSettings::settings_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("ERR_CREATE_SETTINGS_DIR: {e}"))?;
    }
    atomic_write(&path, &content)?;
    set_private_file_permissions(&path);
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) {}

fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "ERR_SETTINGS_PARENT_DIR_NOT_FOUND".to_string())?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "ERR_SETTINGS_FILE_NAME_INVALID".to_string())?;
    let temp_path = parent.join(format!("{file_name}.tmp"));

    fs::write(&temp_path, content).map_err(|e| format!("ERR_WRITE_SETTINGS: {e}"))?;
    fs::rename(&temp_path, path).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        format!("ERR_RENAME_SETTINGS: {error}")
    })
}

fn normalize_settings(settings: &mut AppSettings) -> Result<(), String> {
    migrate_proxy_config(settings);
    migrate_model_pricing(settings);
    migrate_currency(settings);
    migrate_client_tools(settings);
    migrate_api_sources(settings);
    migrate_sync(settings)?;
    migrate_day_boundary(settings);
    migrate_gateway(settings);
    Ok(())
}

/// Older builds may have written a local-key metadata record without its
/// verifier. Keep the profile visible, but never accept that unverifiable key.
fn migrate_gateway(settings: &mut AppSettings) {
    for profile in &mut settings.gateway.profiles {
        for key in &mut profile.local_keys {
            if key.secret_hash.is_empty() || key.secret_salt.is_empty() {
                key.enabled = false;
            }
        }
    }
}

/// 确保代理配置有效，修复端口问题
fn migrate_proxy_config(settings: &mut AppSettings) {
    // 修复端口为 0 或无效的情况
    if settings.proxy.port == 0 {
        settings.proxy.port = crate::models::default_proxy_port();
    }
    if settings.proxy.request_timeout_seconds == 0 {
        settings.proxy.request_timeout_seconds =
            crate::models::default_proxy_request_timeout_seconds();
    }
    if settings.proxy.streaming_idle_timeout_seconds == 0 {
        settings.proxy.streaming_idle_timeout_seconds =
            crate::models::default_proxy_streaming_idle_timeout_seconds();
    }
}

/// 确保模型价格配置存在
fn migrate_model_pricing(settings: &mut AppSettings) {
    if settings.model_pricing.match_mode.is_empty() {
        settings.model_pricing.match_mode = "fuzzy".to_string();
    }
}

/// 确保货币配置存在且有效（迁移旧配置）
fn migrate_currency(settings: &mut AppSettings) {
    if settings.currency.display_currency.is_empty() {
        settings.currency = CurrencySettings::default();
        return;
    }
    if !settings.currency.exchange_rates.contains_key("USD") {
        settings
            .currency
            .exchange_rates
            .insert("USD".to_string(), 1.0);
    }
    if !settings
        .currency
        .tracked_currencies
        .contains(&"USD".to_string())
    {
        settings
            .currency
            .tracked_currencies
            .insert(0, "USD".to_string());
    }
    // 确保显示货币在追踪列表中
    if !settings
        .currency
        .tracked_currencies
        .contains(&settings.currency.display_currency)
    {
        settings.currency.display_currency = "USD".to_string();
    }
}

fn migrate_client_tools(settings: &mut AppSettings) {
    let defaults = crate::models::default_client_tool_profiles();
    for default_profile in defaults {
        if !settings
            .client_tools
            .profiles
            .iter()
            .any(|profile| profile.tool == default_profile.tool)
        {
            settings.client_tools.profiles.push(default_profile);
        }
    }
    if settings
        .client_tools
        .profiles
        .iter()
        .any(|profile| profile.enabled)
    {
        settings.proxy.enabled = true;
    }
}

fn migrate_api_sources(settings: &mut AppSettings) {
    for source in &mut settings.source_aware.sources {
        if let Some(quota_query) = &mut source.quota_query {
            if quota_query.manual_api_key.is_none() {
                quota_query.manual_api_key = source
                    .api_key_notes
                    .get("__quota_api_key")
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty());
            }
            quota_query.normalize();
        }
    }
}

fn migrate_sync(settings: &mut AppSettings) -> Result<(), String> {
    if settings.sync.provider.trim().is_empty() {
        settings.sync.provider = crate::models::default_sync_provider();
    }
    if settings.sync.interval_minutes == 0 {
        settings.sync.interval_minutes = crate::models::default_sync_interval_minutes();
    }
    let normalized_device_id = crate::models::normalize_sync_device_id(&settings.sync.device_id);
    settings.sync.device_id = if normalized_device_id.is_empty() {
        crate::models::default_sync_device_id()
    } else {
        normalized_device_id
    };
    crate::models::validate_sync_device_id(&settings.sync.device_id)?;
    settings.sync.include_session_text = false;
    Ok(())
}

fn migrate_day_boundary(settings: &mut AppSettings) {
    settings.day_boundary_mode =
        crate::utils::business_time::normalize_day_boundary_mode(&settings.day_boundary_mode);
}

fn reset_day_boundary_caches() -> Result<(), String> {
    if let Ok(local_db) = crate::local_usage::LocalUsageDatabase::get_global() {
        local_db.clear_unified_materialization()?;
    }
    if let Some(proxy_db) = crate::proxy::ProxyDatabase::get_global() {
        proxy_db.clear_daily_rollups()?;
    }
    crate::unified_usage::clear_runtime_caches();
    Ok(())
}

/// 列出所有已安装的 WSL 发行版（仅 Windows 生效）。
#[tauri::command]
pub async fn list_wsl_distros() -> Vec<String> {
    tauri::async_runtime::spawn_blocking(list_wsl_distros_blocking)
        .await
        .unwrap_or_default()
}

fn list_wsl_distros_blocking() -> Vec<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let output = match std::process::Command::new("wsl.exe")
            .args(["-l", "-q"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            Ok(out) if out.status.success() => out,
            _ => return Vec::new(),
        };

        let units: Vec<u16> = output
            .stdout
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let text = String::from_utf16_lossy(&units);

        text.lines()
            .map(|line| line.trim().trim_end_matches('\r').to_string())
            .filter(|name| {
                !name.is_empty()
                    && name.len() <= 64
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AppSettings, CurrencySettings, SyncSettings};
    use std::collections::HashMap;
    use std::ffi::OsString;
    use tempfile::tempdir;

    fn restore_home(previous: Option<OsString>) {
        match previous {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn migrate_proxy_config_fills_missing_timeout_fields() {
        let mut settings = AppSettings::default();
        settings.proxy.port = 0;
        settings.proxy.request_timeout_seconds = 0;
        settings.proxy.streaming_idle_timeout_seconds = 0;

        migrate_proxy_config(&mut settings);

        assert_eq!(settings.proxy.port, crate::models::default_proxy_port());
        assert_eq!(
            settings.proxy.request_timeout_seconds,
            crate::models::default_proxy_request_timeout_seconds()
        );
        assert_eq!(
            settings.proxy.streaming_idle_timeout_seconds,
            crate::models::default_proxy_streaming_idle_timeout_seconds()
        );
    }

    #[test]
    fn normalize_settings_restores_invalid_legacy_values() {
        let mut settings = AppSettings::default();
        settings.model_pricing.match_mode.clear();
        settings.currency = CurrencySettings {
            display_currency: "CNY".to_string(),
            exchange_rates: HashMap::new(),
            tracked_currencies: vec![],
            last_rate_update: None,
        };
        settings.client_tools.profiles.clear();
        settings.proxy.enabled = false;
        settings.sync = SyncSettings {
            provider: String::new(),
            device_id: "Invalid Device ID".to_string(),
            interval_minutes: 0,
            include_session_text: true,
            ..SyncSettings::default()
        };

        normalize_settings(&mut settings).unwrap();

        assert_eq!(settings.model_pricing.match_mode, "fuzzy");
        assert!(settings.currency.exchange_rates.contains_key("USD"));
        assert_eq!(settings.currency.display_currency, "USD");
        assert_eq!(
            settings.currency.tracked_currencies,
            vec!["USD".to_string()]
        );
        assert!(settings
            .client_tools
            .profiles
            .iter()
            .any(|profile| profile.tool == "claude_code"));
        assert!(!settings.proxy.enabled);
        assert_eq!(
            settings.sync.provider,
            crate::models::default_sync_provider()
        );
        assert_eq!(
            settings.sync.interval_minutes,
            crate::models::default_sync_interval_minutes()
        );
        assert_eq!(settings.sync.device_id, "invalid-device-id");
        assert!(!settings.sync.include_session_text);
    }

    #[test]
    fn normalize_settings_generates_default_sync_device_id_when_empty() {
        let mut settings = AppSettings::default();
        settings.sync.device_id.clear();

        normalize_settings(&mut settings).unwrap();

        assert_eq!(
            settings.sync.device_id,
            crate::models::default_sync_device_id()
        );
    }

    #[test]
    fn first_load_migrates_legacy_json_into_preferences_and_entity_storage() {
        let _guard = crate::test_support::env_lock();
        let previous_home = std::env::var_os("HOME");
        let dir = tempdir().expect("tempdir");
        std::env::set_var("HOME", dir.path());

        let result = (|| -> Result<(AppSettings, AppSettings, serde_json::Value), String> {
            let settings_dir = dir.path().join(".usagemeter");
            fs::create_dir_all(&settings_dir).map_err(|e| e.to_string())?;
            let mut legacy = AppSettings {
                locale: "en-US".to_string(),
                ..AppSettings::default()
            };
            fs::write(
                settings_dir.join("settings.json"),
                serde_json::to_string(&legacy).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;

            let imported = load_settings_blocking()?;
            legacy.locale = "zh-CN".to_string();
            fs::write(
                settings_dir.join("settings.json"),
                serde_json::to_string(&legacy).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let reloaded = load_settings_blocking()?;
            let compact_file = serde_json::from_str(
                &fs::read_to_string(settings_dir.join("settings.json"))
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            Ok((imported, reloaded, compact_file))
        })();

        restore_home(previous_home);
        let (imported, reloaded, compact_file) = result.unwrap();
        assert_eq!(imported.locale, "en-US");
        assert_eq!(reloaded.locale, "zh-CN");
        assert!(dir.path().join(".usagemeter/app_config.db").exists());
        assert_eq!(compact_file["settingsVersion"], 2);
        assert!(compact_file.get("gateway").is_none());
        assert!(compact_file.get("sourceAware").is_none());
        assert!(compact_file.get("clientTools").is_none());
    }

    #[test]
    fn legacy_model_pricings_move_to_proxy_database() {
        let _guard = crate::test_support::env_lock();
        let previous_home = std::env::var_os("HOME");
        let dir = tempdir().expect("tempdir");
        std::env::set_var("HOME", dir.path());

        let result = (|| -> Result<(bool, AppSettings, Option<crate::models::ModelPricingConfig>), String> {
            let mut settings = AppSettings::default();
            settings.model_pricing.pricings.push(crate::models::ModelPricingConfig {
                model_id: "legacy-model".to_string(),
                display_name: Some("Legacy Model".to_string()),
                input_price: 1.0,
                output_price: 2.0,
                cache_write_price: None,
                cache_read_price: None,
                source: "custom".to_string(),
                last_updated: 1,
            });

            let migrated = migrate_legacy_model_pricings(&mut settings)?;
            let pricing = ProxyDatabase::new_pricing_store()?.get_model_pricing("legacy-model")?;
            Ok((migrated, settings, pricing))
        })();

        restore_home(previous_home);
        let (migrated, settings, pricing) = result.unwrap();
        assert!(migrated);
        assert!(settings.model_pricing.pricings.is_empty());
        assert_eq!(pricing.unwrap().output_price, 2.0);
    }
}
