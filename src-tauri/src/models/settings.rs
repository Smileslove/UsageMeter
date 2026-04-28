//! 设置和配置数据模型

use serde::{Deserialize, Serialize};
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
    ]
}

/// 一个自动发现的 API 来源（由 Proxy 实际请求行为触发，非手动创建）
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub source_aware: SourceAwareSettings,
    #[serde(default)]
    pub client_tools: ClientToolSettings,
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
    "24h".to_string()
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
            window: "24h".to_string(),
            enabled: true,
            token_limit: Some(1_000_000),
            request_limit: Some(1_000),
        },
        WindowQuota {
            window: "today".to_string(),
            enabled: true,
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
            auto_start: false,
            source_aware: SourceAwareSettings::default(),
            client_tools: ClientToolSettings::default(),
        }
    }
}

impl AppSettings {
    pub fn settings_path() -> Result<std::path::PathBuf, String> {
        let home = dirs::home_dir().ok_or_else(|| "ERR_HOME_DIR_NOT_FOUND".to_string())?;
        Ok(home.join(".usagemeter").join("settings.json"))
    }
}
