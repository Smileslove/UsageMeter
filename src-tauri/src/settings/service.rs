//! 设置相关 Tauri 命令

use crate::models::{AppSettings, CurrencySettings};
use crate::net::HttpClientFactory;
use crate::{app_config::with_config_database, proxy::ProxyDatabase};
use std::fs;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

static SETTINGS_WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// 加载应用设置（同步实现，供 Rust 内部直接调用；macOS 上可能 spawn Keychain 子进程）
pub fn load_settings_blocking() -> Result<AppSettings, String> {
    // 偏好文件版本用于存量迁移判定：0.11.x 及更早（settingsVersion <= 2）落盘的
    // streaming_idle_timeout_seconds=0 是旧默认值而非显式关闭；文件缺失时按旧版
    // 处理，使 DB-only 的旧设置同样进入迁移。迁移作用于最终 settings（在
    // apply_preferences / 默认值之后），保证文件与 DB 两个来源都被覆盖。
    let legacy_file_version = load_preferences_file_version();
    let mut settings = with_config_database(|database| {
        if let Some(mut settings) = database.load_settings()? {
            if let Some(preferences) = load_preferences_file()? {
                apply_preferences(&mut settings, preferences);
            }
            normalize_settings(&mut settings)?;
            migrate_legacy_streaming_idle_timeout(&mut settings, legacy_file_version);
            migrate_legacy_model_pricings(&mut settings)?;
            // Re-save once to remove the old full-snapshot documents after a
            // user upgrades to the split preferences/entity layout.
            database.save_settings(&settings)?;
            write_preferences_file(&settings)?;
            Ok(settings)
        } else {
            let mut settings = load_preferences_file()?.unwrap_or_default();
            normalize_settings(&mut settings)?;
            migrate_legacy_streaming_idle_timeout(&mut settings, legacy_file_version);
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

/// 读取偏好文件的 `settingsVersion`。文件缺失或无法解析时按 1（旧版）处理：
/// 这样 settings.json 缺失、仅 app_config.db 存有旧设置的路径（0.10.x 全字段
/// 存储形态）同样进入存量迁移判定。
fn load_preferences_file_version() -> u64 {
    let path = match AppSettings::settings_path() {
        Ok(path) => path,
        Err(_) => return 1,
    };
    let Ok(raw) = fs::read_to_string(path) else {
        return 1;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return 1;
    };
    value
        .get("settingsVersion")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1)
}

/// 0.11.x 及更早（settingsVersion <= 2，含缺失）写入的
/// `streaming_idle_timeout_seconds: 0` 是「默认无超时」（旧版默认值即为 0），
/// 并非用户显式关闭——settingsVersion: 2 自 0.11.0 起就存在，而彼时默认值仍是 0。
/// 升级为当前默认 300 秒，让存量用户升级后自动获得流式空闲超时保护。
/// settingsVersion 3 起「显式 0 = 关闭」语义才存在，此时必须保持 0 不动。
fn migrate_legacy_streaming_idle_timeout(settings: &mut AppSettings, file_version: u64) {
    if file_version <= 2 && settings.proxy.streaming_idle_timeout_seconds == 0 {
        settings.proxy.streaming_idle_timeout_seconds =
            crate::models::default_proxy_streaming_idle_timeout_seconds();
    }
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

    // Apply the sync policy immediately when the toggle changes. This keeps a
    // disabled account from accumulating payloads until the next app restart;
    // local facts and summaries are never touched by this reconciliation.
    if previous_settings.sync.enabled != settings.sync.enabled {
        match crate::local_usage::get_local_usage_db() {
            Ok(db) => {
                let _ = db.mark_sync_policy_reconcile_pending(true);
                match db.reconcile_sync_policy(settings.sync.enabled) {
                    Ok(removed) if removed > 0 => {
                        eprintln!(
                            "[database] Reconciled {removed} sync outbox rows after settings change"
                        );
                        db.compact_after_large_delete(removed);
                    }
                    Ok(_) => {}
                    Err(err) => eprintln!("[database] Sync policy reconciliation deferred: {err}"),
                }
            }
            Err(err) => eprintln!("[database] Sync policy reconciliation deferred: {err}"),
        }
    }

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
    document.insert("settingsVersion".to_string(), serde_json::json!(3));
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
    migrate_api_sources(settings)?;
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
    // streaming_idle_timeout_seconds 不参与修复：0 是合法的“关闭流式空闲超时”
    // 语义；字段缺省时 serde 反序列化已按 default_proxy_streaming_idle_timeout_seconds()
    // 填充（首次创建默认 300）。迁移若把 0 覆盖为默认值，用户将无法显式关闭该功能。
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

fn migrate_api_sources(settings: &mut AppSettings) -> Result<(), String> {
    // 旧版 16 位十六进制 source id 归一为 8 位新格式，避免与新建来源的
    // 短 id 失配（来源筛选、配额绑定、动态归因都按 id 等值匹配）。
    let old_filter = settings.source_aware.active_source_filter.clone();
    if let Some(filter) = &old_filter {
        let normalized = crate::proxy::normalize_source_id(filter);
        settings.source_aware.active_source_filter = Some(normalized);
    }
    let mut migration_failed = false;
    for source in &mut settings.source_aware.sources {
        let old_id = source.id.clone();
        let new_id = crate::proxy::normalize_source_id(&source.id);
        if old_id != new_id {
            match crate::subscription::source_quota_secrets::migrate_secret_keys(&old_id, &new_id) {
                Ok(()) => source.id = new_id,
                Err(e) => {
                    // 迁移失败：保留旧 id（旧键仍可读），下次保存时 old != new
                    // 会再次触发迁移；不因本地密钥存储异常把配置切到无密钥的新 id。
                    migration_failed = true;
                    eprintln!(
                        "[usagemeter] failed to migrate quota secret keys from {old_id}: {e}; keeping old id"
                    );
                }
            }
        }
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
    // 任一来源迁移失败时，回退筛选器归一，避免 filter(8hex) 与
    // source.id(16hex) 失配导致来源不可见。
    if migration_failed {
        settings.source_aware.active_source_filter = old_filter;
    }
    Ok(())
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
pub fn list_wsl_distros_blocking() -> Vec<String> {
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
        // 显式写 0 表示“关闭流式空闲超时”，迁移不得覆盖
        settings.proxy.streaming_idle_timeout_seconds = 0;

        migrate_proxy_config(&mut settings);

        assert_eq!(settings.proxy.port, crate::models::default_proxy_port());
        assert_eq!(
            settings.proxy.request_timeout_seconds,
            crate::models::default_proxy_request_timeout_seconds()
        );
        // 用户显式关闭的 0 必须保持，迁移不再把它归一化为默认值
        assert_eq!(settings.proxy.streaming_idle_timeout_seconds, 0);
    }

    #[test]
    fn fresh_settings_default_streaming_idle_timeout_to_300() {
        // 首次创建（无设置文件）走 Default / default_config，应为 300 秒
        let settings = AppSettings::default();
        assert_eq!(
            settings.proxy.streaming_idle_timeout_seconds,
            crate::models::default_proxy_streaming_idle_timeout_seconds()
        );
        assert_eq!(settings.proxy.streaming_idle_timeout_seconds, 300);

        let proxy = crate::models::ProxyConfig::default_config();
        assert_eq!(proxy.streaming_idle_timeout_seconds, 300);
    }

    #[test]
    fn deserialize_missing_field_uses_default_but_explicit_zero_stays_zero() {
        // 字段缺省（首次未设置）→ 反序列化按 serde default 填 300
        let settings: AppSettings =
            serde_json::from_value(serde_json::json!({ "proxy": {} })).unwrap();
        assert_eq!(settings.proxy.streaming_idle_timeout_seconds, 300);

        // 用户显式写 0 → 反序列化保持 0（关闭）
        let settings: AppSettings = serde_json::from_value(serde_json::json!({
            "proxy": { "streamingIdleTimeoutSeconds": 0 }
        }))
        .unwrap();
        assert_eq!(settings.proxy.streaming_idle_timeout_seconds, 0);

        // 迁移后仍保持显式 0
        let mut settings = settings;
        normalize_settings(&mut settings).unwrap();
        assert_eq!(settings.proxy.streaming_idle_timeout_seconds, 0);
    }

    #[test]
    fn legacy_settings_zero_streaming_timeout_upgrades_to_default() {
        // 旧版（settingsVersion <= 2，含缺失）的 0 是旧默认值而非显式关闭：
        // version 1 = 0.10.x 及更早（无字段），version 2 = 0.11.x（settingsVersion: 2
        // 自 0.11.0 起存在，彼时默认值仍为 0）。两者都必须升级为当前默认 300。
        for file_version in [0, 1, 2] {
            let mut settings = AppSettings::default();
            settings.proxy.streaming_idle_timeout_seconds = 0;
            migrate_legacy_streaming_idle_timeout(&mut settings, file_version);
            assert_eq!(
                settings.proxy.streaming_idle_timeout_seconds,
                crate::models::default_proxy_streaming_idle_timeout_seconds(),
                "file_version: {file_version}"
            );
        }
    }

    #[test]
    fn modern_settings_explicit_zero_streaming_timeout_stays_zero() {
        // settingsVersion >= 3：0 是「显式关闭」语义，迁移不得覆盖。
        let mut settings = AppSettings::default();
        settings.proxy.streaming_idle_timeout_seconds = 0;
        migrate_legacy_streaming_idle_timeout(&mut settings, 3);
        assert_eq!(settings.proxy.streaming_idle_timeout_seconds, 0);

        // 非零值在任何版本下都不受影响。
        let mut settings = AppSettings::default();
        settings.proxy.streaming_idle_timeout_seconds = 15;
        migrate_legacy_streaming_idle_timeout(&mut settings, 1);
        assert_eq!(settings.proxy.streaming_idle_timeout_seconds, 15);
    }

    #[test]
    fn legacy_settings_file_json_upgrades_streaming_timeout_on_load() {
        // 端到端形状：0.11.x 文件（settingsVersion=2、streaming 0）与 0.10.x 文件
        // （无 settingsVersion、streaming 0）都应得到默认 300；settingsVersion=3 的
        // 显式 0 保持 0。
        let cases = [
            (
                serde_json::json!({ "proxy": { "streamingIdleTimeoutSeconds": 0 } }),
                1,
                300,
            ),
            (
                serde_json::json!({
                    "settingsVersion": 2,
                    "proxy": { "streamingIdleTimeoutSeconds": 0 }
                }),
                2,
                300,
            ),
            (
                serde_json::json!({
                    "settingsVersion": 3,
                    "proxy": { "streamingIdleTimeoutSeconds": 0 }
                }),
                3,
                0,
            ),
        ];
        for (json, file_version, expected) in cases {
            let mut settings: AppSettings = serde_json::from_value(json).unwrap();
            migrate_legacy_streaming_idle_timeout(&mut settings, file_version);
            assert_eq!(
                settings.proxy.streaming_idle_timeout_seconds, expected,
                "file_version: {file_version}"
            );
        }
    }

    #[test]
    fn preferences_file_version_extraction_handles_missing_and_invalid() {
        let _guard = crate::test_support::env_lock();
        let previous_home = std::env::var_os("HOME");
        let dir = tempdir().expect("tempdir");
        std::env::set_var("HOME", dir.path());

        let result = (|| -> Result<(u64, u64, u64), String> {
            // 文件缺失 → 按旧版 1
            let missing = load_preferences_file_version();
            let settings_dir = dir.path().join(".usagemeter");
            fs::create_dir_all(&settings_dir).map_err(|e| e.to_string())?;
            // 非法 JSON → 按旧版 1
            fs::write(settings_dir.join("settings.json"), "not-json").map_err(|e| e.to_string())?;
            let invalid = load_preferences_file_version();
            // 合法 version → 按文件值
            fs::write(
                settings_dir.join("settings.json"),
                serde_json::json!({ "settingsVersion": 2 }).to_string(),
            )
            .map_err(|e| e.to_string())?;
            let valid = load_preferences_file_version();
            Ok((missing, invalid, valid))
        })();

        restore_home(previous_home);
        let (missing, invalid, valid) = result.unwrap();
        assert_eq!(missing, 1);
        assert_eq!(invalid, 1);
        assert_eq!(valid, 2);
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
        assert_eq!(compact_file["settingsVersion"], 3);
        assert!(compact_file.get("gateway").is_none());
        assert!(compact_file.get("sourceAware").is_none());
        assert!(compact_file.get("clientTools").is_none());
    }

    #[test]
    fn legacy_0_11_settings_file_upgrades_streaming_timeout_on_load() {
        // 端到端：0.11.x 用户磁盘形态 = settingsVersion:2 + streaming 0（旧默认）。
        // 首次加载应升级为 300，并写回 settingsVersion:3 固化，二次加载不再变化。
        let _guard = crate::test_support::env_lock();
        let previous_home = std::env::var_os("HOME");
        let dir = tempdir().expect("tempdir");
        std::env::set_var("HOME", dir.path());

        let result = (|| -> Result<(u64, u64, serde_json::Value), String> {
            let settings_dir = dir.path().join(".usagemeter");
            fs::create_dir_all(&settings_dir).map_err(|e| e.to_string())?;
            fs::write(
                settings_dir.join("settings.json"),
                serde_json::json!({
                    "settingsVersion": 2,
                    "proxy": { "streamingIdleTimeoutSeconds": 0 }
                })
                .to_string(),
            )
            .map_err(|e| e.to_string())?;

            let imported = load_settings_blocking()?;
            let first = imported.proxy.streaming_idle_timeout_seconds;
            let reloaded = load_settings_blocking()?;
            let second = reloaded.proxy.streaming_idle_timeout_seconds;
            let compact_file: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(settings_dir.join("settings.json"))
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            Ok((first, second, compact_file))
        })();

        restore_home(previous_home);
        let (first, second, compact_file) = result.unwrap();
        assert_eq!(first, 300);
        assert_eq!(second, 300);
        // 写回固化：settingsVersion bump 到 3，streaming 300，二次加载幂等。
        assert_eq!(compact_file["settingsVersion"], 3);
        assert_eq!(
            compact_file["proxy"]["streamingIdleTimeoutSeconds"],
            serde_json::json!(300)
        );
    }

    #[test]
    fn db_only_legacy_settings_upgrade_streaming_timeout_when_file_missing() {
        // 端到端：0.10.x 全字段存储形态——app_config.db 的 config_documents 含
        // proxy 文档（streaming 0），settings.json 缺失。迁移必须作用于最终设置
        // （DB 来源），升级为 300 而非停留在 0。
        let _guard = crate::test_support::env_lock();
        let previous_home = std::env::var_os("HOME");
        let dir = tempdir().expect("tempdir");
        std::env::set_var("HOME", dir.path());

        let result = (|| -> Result<u64, String> {
            let settings_dir = dir.path().join(".usagemeter");
            fs::create_dir_all(&settings_dir).map_err(|e| e.to_string())?;
            // 先建库（含 schema），再直插 0.10.x 形态的 proxy 文档。
            crate::app_config::with_config_database(|_| Ok(()))?;
            let db_path = settings_dir.join("app_config.db");
            let conn = rusqlite::Connection::open(&db_path)
                .map_err(|e| format!("ERR_OPEN_TEST_DB: {e}"))?;
            conn.execute(
                "INSERT INTO config_documents (document_key, payload_json, revision, updated_at)
                 VALUES ('proxy', ?1, 1, 0)",
                rusqlite::params![serde_json::json!({
                    "streamingIdleTimeoutSeconds": 0
                })
                .to_string()],
            )
            .map_err(|e| format!("ERR_INSERT_TEST_DOCUMENT: {e}"))?;
            drop(conn);
            // settings.json 缺失 → 文件版本按 1（旧版）处理，迁移 DB 来源的 0。
            assert!(!settings_dir.join("settings.json").exists());
            let loaded = load_settings_blocking()?;
            Ok(loaded.proxy.streaming_idle_timeout_seconds)
        })();

        restore_home(previous_home);
        assert_eq!(result.unwrap(), 300);
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
