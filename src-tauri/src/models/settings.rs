//! Settings and configuration data models

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProxyConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_proxy_port")]
    pub port: u16,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default = "default_include_error_requests")]
    pub include_error_requests: bool,
}

pub fn default_proxy_port() -> u16 {
    18765
}

pub fn default_include_error_requests() -> bool {
    true
}

impl ProxyConfig {
    pub fn default_config() -> Self {
        Self {
            enabled: false,
            port: 18765,
            auto_start: false,
            include_error_requests: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowQuota {
    pub window: String,
    pub enabled: bool,
    pub token_limit: Option<u64>,
    pub request_limit: Option<u64>,
}

/// 模型价格配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricingConfig {
    /// 模型ID，如 "claude-3-sonnet-20240229" 或 "minimax-m2-5"
    pub model_id: String,
    /// 显示名称（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// 输入价格 $/M tokens
    pub input_price: f64,
    /// 输出价格 $/M tokens
    pub output_price: f64,
    /// 缓存写入价格 $/M（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_price: Option<f64>,
    /// 缓存读取价格 $/M（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_price: Option<f64>,
    /// 来源：api 或 custom
    pub source: String,
    /// 最后更新时间戳
    pub last_updated: i64,
}

/// 模型价格设置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricingSettings {
    /// 匹配方式：fuzzy 或 exact
    #[serde(default = "default_match_mode")]
    pub match_mode: String,
    /// 最后同步时间
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync_time: Option<i64>,
    /// 价格配置列表
    #[serde(default)]
    pub pricings: Vec<ModelPricingConfig>,
}

fn default_match_mode() -> String {
    "fuzzy".to_string()
}

impl Default for ModelPricingSettings {
    fn default() -> Self {
        Self {
            match_mode: "fuzzy".to_string(),
            last_sync_time: None,
            pricings: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default = "default_refresh_interval_seconds")]
    pub refresh_interval_seconds: u64,
    #[serde(default = "default_warning_threshold")]
    pub warning_threshold: u8,
    #[serde(default = "default_critical_threshold")]
    pub critical_threshold: u8,
    #[serde(default = "default_billing_type")]
    pub billing_type: String,
    #[serde(default = "default_quotas")]
    pub quotas: Vec<WindowQuota>,
    #[serde(default = "default_summary_window")]
    pub summary_window: String,
    #[serde(default = "default_data_source")]
    pub data_source: String,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub model_pricing: ModelPricingSettings,
}

pub fn default_locale() -> String {
    "zh-CN".to_string()
}

pub fn default_timezone() -> String {
    "Asia/Shanghai".to_string()
}

pub fn default_refresh_interval_seconds() -> u64 {
    30
}

pub fn default_warning_threshold() -> u8 {
    70
}

pub fn default_critical_threshold() -> u8 {
    90
}

pub fn default_billing_type() -> String {
    "both".to_string()
}

pub fn default_summary_window() -> String {
    "1d".to_string()
}

pub fn default_data_source() -> String {
    "ccusage".to_string()
}

pub fn default_theme() -> String {
    "system".to_string()
}

pub fn default_quotas() -> Vec<WindowQuota> {
    vec![
        WindowQuota {
            window: "5h".to_string(),
            enabled: true,
            token_limit: Some(500_000),
            request_limit: Some(500),
        },
        WindowQuota {
            window: "1d".to_string(),
            enabled: false,
            token_limit: Some(1_000_000),
            request_limit: Some(1_000),
        },
        WindowQuota {
            window: "7d".to_string(),
            enabled: true,
            token_limit: Some(5_000_000),
            request_limit: Some(5_000),
        },
        WindowQuota {
            window: "30d".to_string(),
            enabled: true,
            token_limit: Some(20_000_000),
            request_limit: Some(20_000),
        },
        WindowQuota {
            window: "current_month".to_string(),
            enabled: true,
            token_limit: Some(30_000_000),
            request_limit: Some(30_000),
        },
    ]
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            locale: default_locale(),
            timezone: default_timezone(),
            refresh_interval_seconds: default_refresh_interval_seconds(),
            warning_threshold: default_warning_threshold(),
            critical_threshold: default_critical_threshold(),
            billing_type: default_billing_type(),
            quotas: default_quotas(),
            summary_window: default_summary_window(),
            data_source: default_data_source(),
            proxy: ProxyConfig::default_config(),
            theme: default_theme(),
            model_pricing: ModelPricingSettings::default(),
        }
    }
}

impl AppSettings {
    pub fn settings_path() -> Result<std::path::PathBuf, String> {
        let home = dirs::home_dir().ok_or_else(|| "ERR_HOME_DIR_NOT_FOUND".to_string())?;
        Ok(home.join(".usagemeter").join("settings.json"))
    }
}
