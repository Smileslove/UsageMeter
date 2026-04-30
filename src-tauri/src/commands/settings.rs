//! 设置相关 Tauri 命令

use crate::models::{AppSettings, CurrencySettings, WindowQuota};
use std::fs;

/// 加载应用设置
#[tauri::command]
pub fn load_settings() -> Result<AppSettings, String> {
    let path = AppSettings::settings_path()?;
    if !path.exists() {
        return Ok(AppSettings::default());
    }

    let raw = fs::read_to_string(path).map_err(|e| format!("ERR_READ_SETTINGS: {e}"))?;
    let mut settings: AppSettings =
        serde_json::from_str(&raw).map_err(|e| format!("ERR_PARSE_SETTINGS: {e}"))?;

    // 确保所有窗口配额存在（迁移旧配置）
    migrate_quotas(&mut settings);

    // 确保代理配置有效（迁移旧配置）
    migrate_proxy_config(&mut settings);

    // 确保模型价格配置存在（迁移旧配置）
    migrate_model_pricing(&mut settings);

    // 确保货币配置存在（迁移旧配置）
    migrate_currency(&mut settings);

    // 确保客户端工具配置完整（迁移旧配置）
    migrate_client_tools(&mut settings);

    Ok(settings)
}

/// 保存应用设置
#[tauri::command]
pub fn save_settings(settings: AppSettings) -> Result<(), String> {
    let path = AppSettings::settings_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("ERR_CREATE_SETTINGS_DIR: {e}"))?;
    }

    let content = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("ERR_SERIALIZE_SETTINGS: {e}"))?;
    fs::write(path, content).map_err(|e| format!("ERR_WRITE_SETTINGS: {e}"))
}

/// 确保所有窗口配额存在，添加缺失的默认值
fn migrate_quotas(settings: &mut AppSettings) {
    use std::collections::HashSet;

    let defaults = crate::models::default_quotas();
    let existing_windows: HashSet<_> = settings.quotas.iter().map(|q| q.window.as_str()).collect();

    let missing: Vec<WindowQuota> = defaults
        .into_iter()
        .filter(|d| !existing_windows.contains(d.window.as_str()))
        .collect();

    settings.quotas.extend(missing);
}

/// 确保代理配置有效，修复端口问题
fn migrate_proxy_config(settings: &mut AppSettings) {
    // 修复端口为 0 或无效的情况
    if settings.proxy.port == 0 {
        settings.proxy.port = crate::models::default_proxy_port();
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
