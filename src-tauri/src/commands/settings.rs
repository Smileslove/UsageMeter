//! 设置相关 Tauri 命令

use crate::models::{AppSettings, CurrencySettings};
use crate::net::HttpClientFactory;
use std::fs;
use tauri::{AppHandle, Emitter};

/// 加载应用设置
#[tauri::command]
pub fn load_settings() -> Result<AppSettings, String> {
    let path = AppSettings::settings_path()?;
    if !path.exists() {
        let mut settings = AppSettings::default();
        normalize_settings(&mut settings)?;
        return Ok(settings);
    }

    let raw = fs::read_to_string(path).map_err(|e| format!("ERR_READ_SETTINGS: {e}"))?;
    let mut settings: AppSettings =
        serde_json::from_str(&raw).map_err(|e| format!("ERR_PARSE_SETTINGS: {e}"))?;

    normalize_settings(&mut settings)?;

    Ok(settings)
}

/// 保存应用设置（Tauri 命令）。
///
/// 网络代理 reload 失败时会同时 emit `network-proxy-reload-failed` 事件并返回 Err，
/// 让前端 UI 能感知"已落盘但运行时未应用"的状态。
#[tauri::command]
pub fn save_settings(app: AppHandle, settings: AppSettings) -> Result<(), String> {
    match save_settings_internal(settings) {
        Ok(()) => Ok(()),
        Err(SaveSettingsError::ReloadFailed(err)) => {
            let _ = app.emit("network-proxy-reload-failed", err.clone());
            Err(err)
        }
        Err(SaveSettingsError::Other(err)) => Err(err),
    }
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
pub fn save_settings_internal(settings: AppSettings) -> Result<(), SaveSettingsError> {
    let mut settings = settings;
    normalize_settings(&mut settings).map_err(SaveSettingsError::Other)?;
    let path = AppSettings::settings_path().map_err(SaveSettingsError::Other)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| SaveSettingsError::Other(format!("ERR_CREATE_SETTINGS_DIR: {e}")))?;
    }

    let content = serde_json::to_string_pretty(&settings)
        .map_err(|e| SaveSettingsError::Other(format!("ERR_SERIALIZE_SETTINGS: {e}")))?;
    fs::write(path, content)
        .map_err(|e| SaveSettingsError::Other(format!("ERR_WRITE_SETTINGS: {e}")))?;

    if let Err(err) = HttpClientFactory::global().reload(&settings.network_proxy) {
        eprintln!("[UsageMeter] {err}");
        return Err(SaveSettingsError::ReloadFailed(err));
    }
    Ok(())
}

fn normalize_settings(settings: &mut AppSettings) -> Result<(), String> {
    migrate_proxy_config(settings);
    migrate_model_pricing(settings);
    migrate_currency(settings);
    migrate_client_tools(settings);
    migrate_api_sources(settings);
    migrate_sync(settings)?;
    Ok(())
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
            quota_query.access_token = quota_query
                .access_token
                .as_ref()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty());
            quota_query.user_id = quota_query
                .user_id
                .as_ref()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty());
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

/// 列出所有已安装的 WSL 发行版（仅 Windows 生效）。
#[tauri::command]
pub fn list_wsl_distros() -> Vec<String> {
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
        assert!(settings.proxy.enabled);
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
}
