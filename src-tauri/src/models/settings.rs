//! 设置和配置数据模型

use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

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
    #[serde(default = "default_proxy_request_timeout_seconds")]
    pub request_timeout_seconds: u64,
    #[serde(default = "default_proxy_streaming_idle_timeout_seconds")]
    pub streaming_idle_timeout_seconds: u64,
}

pub fn default_proxy_port() -> u16 {
    18765
}

pub fn default_include_error_requests() -> bool {
    true
}

pub fn default_proxy_request_timeout_seconds() -> u64 {
    120
}

pub fn default_proxy_streaming_idle_timeout_seconds() -> u64 {
    0
}

impl ProxyConfig {
    pub fn default_config() -> Self {
        Self {
            enabled: false,
            port: 18765,
            auto_start: false,
            include_error_requests: true,
            request_timeout_seconds: default_proxy_request_timeout_seconds(),
            streaming_idle_timeout_seconds: default_proxy_streaming_idle_timeout_seconds(),
        }
    }
}

/// 全局出站网络代理配置
///
/// 与 `ProxyConfig`（本地代理接管 Claude Code）职责完全不同：
/// - `enabled = false`：所有出站 HTTP 请求跟随系统代理（如 Clash/VPN）
/// - `enabled = true`：所有出站请求强制走用户配置的代理地址
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkProxyConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_network_proxy_scheme")]
    pub scheme: String,
    #[serde(default = "default_network_proxy_host")]
    pub host: String,
    #[serde(default = "default_network_proxy_port")]
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

pub fn default_network_proxy_scheme() -> String {
    "http".to_string()
}

pub fn default_network_proxy_host() -> String {
    "127.0.0.1".to_string()
}

pub fn default_network_proxy_port() -> u16 {
    7890
}

impl Default for NetworkProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            scheme: default_network_proxy_scheme(),
            host: default_network_proxy_host(),
            port: default_network_proxy_port(),
            username: None,
            password: None,
        }
    }
}

impl NetworkProxyConfig {
    /// 构造代理 URL，如 "http://127.0.0.1:7890"。
    /// IPv6 host 自动加方括号，如 "http://[::1]:7890"。
    /// scheme 统一小写，与 validate() 保持一致。
    pub fn build_url(&self) -> String {
        let host = self.host.trim();
        let scheme = self.scheme.trim().to_ascii_lowercase();
        if host.contains(':') && !host.starts_with('[') {
            format!("{}://[{}]:{}", scheme, host, self.port)
        } else {
            format!("{}://{}:{}", scheme, host, self.port)
        }
    }

    /// 是否需要 basic auth
    pub fn has_auth(&self) -> bool {
        matches!(&self.username, Some(u) if !u.is_empty())
    }

    /// 校验"启用模式"下配置是否合法。`enabled=false` 时不校验（跟随系统）。
    /// 返回错误标识符，前端可对照 i18n 提示。
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        let scheme = self.scheme.trim().to_ascii_lowercase();
        if !matches!(scheme.as_str(), "http" | "https" | "socks5") {
            return Err("ERR_NETWORK_PROXY_SCHEME".to_string());
        }
        if self.host.trim().is_empty() {
            return Err("ERR_NETWORK_PROXY_HOST".to_string());
        }
        if self.port == 0 {
            return Err("ERR_NETWORK_PROXY_PORT".to_string());
        }
        Ok(())
    }
}

/// 模型价格配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

/// 来源识别的预定义颜色（8 色轮转）
pub const SOURCE_COLORS: &[&str] = &[
    "#3B82F6", "#10B981", "#F59E0B", "#EF4444", "#8B5CF6", "#EC4899", "#06B6D4", "#84CC16",
];

pub const DEFAULT_CLIENT_TOOL: &str = "claude_code";
pub const DEFAULT_CLIENT_DETECTION_METHOD: &str = "legacy_path";

/// 一个客户端工具接入配置。单端口模式下通过 path_prefix 识别工具身份。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientToolProfile {
    pub id: String,
    pub tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// 单端口共享代理中的路径前缀，如 "claude-code"、"codex"。
    pub path_prefix: String,
    /// 该工具原始目标地址；None 时使用当前 Claude Code 兼容转发目标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_base_url: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub auto_detected: bool,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
    /// 工具图标（lucide 图标名），None 时使用默认工具图标
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientToolSettings {
    #[serde(default = "default_client_tool_profiles")]
    pub profiles: Vec<ClientToolProfile>,
    /// None = 全部工具；Some(tool_id) = 指定工具。
    #[serde(default)]
    pub active_tool_filter: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ToolFilter {
    All,
    Tool(String),
}

impl Default for ClientToolSettings {
    fn default() -> Self {
        Self {
            profiles: default_client_tool_profiles(),
            active_tool_filter: None,
        }
    }
}

impl ClientToolSettings {
    pub fn build_filter(&self) -> ToolFilter {
        match self.active_tool_filter.as_ref() {
            Some(tool) if !tool.trim().is_empty() => ToolFilter::Tool(tool.clone()),
            _ => ToolFilter::All,
        }
    }
}

pub fn default_client_tool_profiles() -> Vec<ClientToolProfile> {
    let now = chrono::Utc::now().timestamp_millis();
    vec![
        ClientToolProfile {
            id: "claude_code".to_string(),
            tool: DEFAULT_CLIENT_TOOL.to_string(),
            display_name: Some("Claude Code".to_string()),
            path_prefix: "claude-code".to_string(),
            target_base_url: None,
            enabled: true,
            auto_detected: false,
            first_seen_ms: now,
            last_seen_ms: now,
            icon: Some("claudecode".to_string()),
        },
        ClientToolProfile {
            id: "codex".to_string(),
            tool: "codex".to_string(),
            display_name: Some("Codex".to_string()),
            path_prefix: "codex".to_string(),
            target_base_url: None,
            enabled: false,
            auto_detected: false,
            first_seen_ms: now,
            last_seen_ms: now,
            icon: Some("codex".to_string()),
        },
        ClientToolProfile {
            id: "cursor".to_string(),
            tool: "cursor".to_string(),
            display_name: Some("Cursor".to_string()),
            path_prefix: "cursor".to_string(),
            target_base_url: None,
            enabled: false,
            auto_detected: false,
            first_seen_ms: now,
            last_seen_ms: now,
            icon: Some("cursor".to_string()),
        },
        ClientToolProfile {
            id: "opencode".to_string(),
            tool: "opencode".to_string(),
            display_name: Some("OpenCode".to_string()),
            path_prefix: "opencode".to_string(),
            target_base_url: None,
            enabled: false,
            auto_detected: false,
            first_seen_ms: now,
            last_seen_ms: now,
            icon: Some("opencode".to_string()),
        },
        ClientToolProfile {
            id: "reasonix".to_string(),
            tool: "reasonix".to_string(),
            display_name: Some("Reasonix".to_string()),
            path_prefix: "reasonix".to_string(),
            target_base_url: None,
            enabled: false,
            auto_detected: false,
            first_seen_ms: now,
            last_seen_ms: now,
            icon: Some("reasonix".to_string()),
        },
    ]
}

/// 一个自动发现的 API 来源（由 Proxy 实际请求行为触发，非手动创建）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApiSource {
    /// 稳定的唯一 ID（由 api_key_prefix + base_url 哈希生成）
    pub id: String,
    /// 用户自定义名称；None 时前端自动生成显示名
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// API 基础地址；None = 官方 Anthropic
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// 与此来源关联的所有 API Key 前缀（支持密钥轮换）
    pub api_key_prefixes: Vec<String>,
    /// API Key 前缀备注，key 为 api_key_prefixes 中的前缀
    #[serde(default)]
    pub api_key_notes: HashMap<String, String>,
    /// 自动分配的十六进制颜色
    pub color: String,
    /// 用户自选图标（lucide 图标名），None 时使用颜色点作为默认展示
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// true = 自动发现，false = 用户手动编辑过
    #[serde(default)]
    pub auto_detected: bool,
    /// 首次发现时间（Unix 毫秒）
    pub first_seen_ms: i64,
    /// 最近使用时间（Unix 毫秒）
    pub last_seen_ms: i64,
}

/// API 来源感知设置
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceAwareSettings {
    /// 已注册的来源列表
    #[serde(default)]
    pub sources: Vec<ApiSource>,
    /// 当前激活的来源过滤器
    /// - None: 显示全部
    /// - Some("__unknown__"): 只看未归因记录
    /// - Some(source_id): 指定来源
    #[serde(default)]
    pub active_source_filter: Option<String>,
}

/// 来源过滤条件
#[derive(Debug, Clone)]
pub enum SourceFilter {
    /// 不过滤，显示全部记录
    All,
    /// 按指定来源过滤
    Source {
        /// 匹配的 API Key 前缀列表
        api_key_prefixes: Vec<String>,
        /// 匹配的 base_url
        base_url: Option<String>,
    },
    /// 只显示未归因记录（不属于任何已定义来源）
    Unknown {
        /// 所有已知来源的 (API Key 前缀, base_url) 组合
        known_pairs: Vec<(String, Option<String>)>,
    },
}

#[derive(Debug, Clone)]
pub struct UsageQueryFilter {
    pub source: SourceFilter,
    pub tool: ToolFilter,
}

impl SourceAwareSettings {
    /// 根据当前设置构建 SourceFilter
    pub fn build_filter(&self) -> SourceFilter {
        match &self.active_source_filter {
            None => SourceFilter::All,
            Some(filter) if filter == "__unknown__" => {
                let known_pairs: Vec<(String, Option<String>)> = self
                    .sources
                    .iter()
                    .flat_map(|s| {
                        s.api_key_prefixes
                            .iter()
                            .cloned()
                            .map(|prefix| (prefix, s.base_url.clone()))
                    })
                    .collect();
                SourceFilter::Unknown { known_pairs }
            }
            Some(source_id) => {
                // 查找对应的来源
                self.sources
                    .iter()
                    .find(|s| &s.id == source_id)
                    .map(|source| SourceFilter::Source {
                        api_key_prefixes: source.api_key_prefixes.clone(),
                        base_url: source.base_url.clone(),
                    })
                    .unwrap_or(SourceFilter::All)
            }
        }
    }
}

/// 多货币设置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CurrencySettings {
    /// 当前显示币种，如 "USD"、"CNY"
    #[serde(default = "default_display_currency")]
    pub display_currency: String,
    /// 用户追踪的币种汇率，key=币种代码, value=1USD兑换该币种数量
    #[serde(default = "default_exchange_rates")]
    pub exchange_rates: HashMap<String, f64>,
    /// 用户选择追踪的币种列表（用于同步过滤）
    #[serde(default = "default_tracked_currencies")]
    pub tracked_currencies: Vec<String>,
    /// 最后汇率更新时间
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_rate_update: Option<i64>,
}

pub fn default_display_currency() -> String {
    "USD".to_string()
}

pub fn default_exchange_rates() -> HashMap<String, f64> {
    let mut rates = HashMap::new();
    rates.insert("USD".to_string(), 1.0);
    rates
}

pub fn default_tracked_currencies() -> Vec<String> {
    vec!["USD".to_string()]
}

impl Default for CurrencySettings {
    fn default() -> Self {
        Self {
            display_currency: default_display_currency(),
            exchange_rates: default_exchange_rates(),
            tracked_currencies: default_tracked_currencies(),
            last_rate_update: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeSettings {
    #[serde(default = "default_theme_appearance")]
    pub appearance: String,
    #[serde(default = "default_light_palette")]
    pub light_palette: String,
    #[serde(default = "default_dark_palette")]
    pub dark_palette: String,
}

impl Default for ThemeSettings {
    fn default() -> Self {
        Self {
            appearance: default_theme_appearance(),
            light_palette: default_light_palette(),
            dark_palette: default_dark_palette(),
        }
    }
}

fn deserialize_theme_settings<'de, D>(deserializer: D) -> Result<ThemeSettings, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ThemeValue {
        Legacy(String),
        Structured(ThemeSettings),
    }

    match ThemeValue::deserialize(deserializer)? {
        ThemeValue::Legacy(value) => Ok(match value.as_str() {
            "light" => ThemeSettings {
                appearance: "light".to_string(),
                ..ThemeSettings::default()
            },
            "dark" => ThemeSettings {
                appearance: "dark".to_string(),
                ..ThemeSettings::default()
            },
            "system" => ThemeSettings::default(),
            _ => ThemeSettings::default(),
        }),
        ThemeValue::Structured(value) => Ok(value),
    }
}

pub fn default_theme_appearance() -> String {
    "system".to_string()
}

pub fn default_light_palette() -> String {
    "cloud".to_string()
}

pub fn default_dark_palette() -> String {
    "midnight".to_string()
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
    #[serde(default = "default_summary_window")]
    pub summary_window: String,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(
        default = "default_theme",
        deserialize_with = "deserialize_theme_settings"
    )]
    pub theme: ThemeSettings,
    #[serde(default)]
    pub model_pricing: ModelPricingSettings,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub source_aware: SourceAwareSettings,
    #[serde(default)]
    pub client_tools: ClientToolSettings,
    #[serde(default)]
    pub currency: CurrencySettings,
    #[serde(default)]
    pub sync: SyncSettings,
    #[serde(default)]
    pub network_proxy: NetworkProxyConfig,
    #[serde(default = "default_auto_check_update")]
    pub auto_check_update: bool,
    #[serde(default)]
    pub skipped_update_version: String,
    #[serde(default)]
    pub wsl_scan: WslScanSettings,
}

/// WSL 被动扫描设置（仅在 Windows 上生效）。
///
/// 开启后，UsageMeter 会经 UNC 路径 `\\wsl$\<distro>\home\<user>\...` 额外扫描
/// WSL 发行版内的 Claude Code / Codex transcript 以及 OpenCode 本地数据，纳入统计。默认关闭，避免无 WSL
/// 用户每次刷新都唤醒发行版。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WslScanSettings {
    /// 是否启用 WSL 被动扫描。
    #[serde(default)]
    pub enabled: bool,
    /// 手动指定的发行版列表；为空时自动枚举（`wsl.exe -l -q`）。
    #[serde(default)]
    pub distros: Vec<String>,
    /// 手动指定的扫描根（UNC 路径），用于自动探测失败时的兜底。
    #[serde(default)]
    pub extra_roots: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_sync_provider")]
    pub provider: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub sync_password: String,
    #[serde(default = "default_sync_device_id")]
    pub device_id: String,
    #[serde(default = "default_sync_interval_minutes")]
    pub interval_minutes: u64,
    #[serde(default, alias = "syncOnStartup")]
    pub auto_sync: bool,
    #[serde(default)]
    pub include_session_text: bool,
}

pub fn default_sync_provider() -> String {
    "webdav".to_string()
}

pub fn default_sync_interval_minutes() -> u64 {
    15
}

pub fn default_sync_device_id() -> String {
    let os = if cfg!(target_os = "macos") {
        "mac"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "device"
    };
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let home = dirs::home_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(os.as_bytes());
    hasher.update(host.as_bytes());
    hasher.update(home.as_bytes());
    let digest = hasher.finalize();
    format!("{os}-{}", hex_prefix(&digest, 8))
}

fn hex_prefix(bytes: &[u8], len: usize) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(len * 2);
    for byte in bytes.iter().take(len) {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_sync_provider(),
            url: String::new(),
            username: String::new(),
            password: String::new(),
            sync_password: String::new(),
            device_id: default_sync_device_id(),
            interval_minutes: default_sync_interval_minutes(),
            auto_sync: false,
            include_session_text: false,
        }
    }
}

pub const SYNC_DEVICE_ID_MIN_LEN: usize = 3;
pub const SYNC_DEVICE_ID_MAX_LEN: usize = 48;

pub fn normalize_sync_device_id(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut last_was_dash = false;
    for ch in value.trim().chars() {
        let next = if ch.is_ascii_alphanumeric() {
            Some(ch.to_ascii_lowercase())
        } else if matches!(ch, '-' | '_' | '.') {
            Some(ch)
        } else {
            Some('-')
        };
        if let Some(ch) = next {
            if ch == '-' {
                if last_was_dash {
                    continue;
                }
                last_was_dash = true;
            } else {
                last_was_dash = false;
            }
            normalized.push(ch);
            if normalized.len() >= SYNC_DEVICE_ID_MAX_LEN {
                break;
            }
        }
    }
    normalized.trim_matches('-').to_string()
}

pub fn validate_sync_device_id(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("ERR_SYNC_DEVICE_ID_REQUIRED".to_string());
    }
    if value.len() < SYNC_DEVICE_ID_MIN_LEN {
        return Err("ERR_SYNC_DEVICE_ID_TOO_SHORT".to_string());
    }
    if value.len() > SYNC_DEVICE_ID_MAX_LEN {
        return Err("ERR_SYNC_DEVICE_ID_TOO_LONG".to_string());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_' | '.'))
    {
        return Err("ERR_SYNC_DEVICE_ID_INVALID".to_string());
    }
    if !value
        .chars()
        .any(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
    {
        return Err("ERR_SYNC_DEVICE_ID_INVALID".to_string());
    }
    Ok(())
}

pub fn default_locale() -> String {
    "zh-CN".to_string()
}

pub fn default_auto_check_update() -> bool {
    true
}

pub fn default_timezone() -> String {
    "Asia/Shanghai".to_string()
}

pub fn default_refresh_interval_seconds() -> u64 {
    30
}

pub fn default_summary_window() -> String {
    "24h".to_string()
}

pub fn default_theme() -> ThemeSettings {
    ThemeSettings::default()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            locale: default_locale(),
            timezone: default_timezone(),
            refresh_interval_seconds: default_refresh_interval_seconds(),
            summary_window: default_summary_window(),
            proxy: ProxyConfig::default_config(),
            theme: default_theme(),
            model_pricing: ModelPricingSettings::default(),
            auto_start: false,
            source_aware: SourceAwareSettings::default(),
            client_tools: ClientToolSettings::default(),
            currency: CurrencySettings::default(),
            sync: SyncSettings::default(),
            network_proxy: NetworkProxyConfig::default(),
            auto_check_update: default_auto_check_update(),
            skipped_update_version: String::new(),
            wsl_scan: WslScanSettings::default(),
        }
    }
}

impl AppSettings {
    pub fn settings_path() -> Result<std::path::PathBuf, String> {
        let home = dirs::home_dir().ok_or_else(|| "ERR_HOME_DIR_NOT_FOUND".to_string())?;
        Ok(home.join(".usagemeter").join("settings.json"))
    }
}
