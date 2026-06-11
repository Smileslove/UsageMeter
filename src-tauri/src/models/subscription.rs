//! Subscription quota data models
//!
//! Data structures for subscription quota information from official providers.

use serde::{Deserialize, Serialize};

/// 配额类型：时间窗口利用率 vs 余额。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum QuotaKind {
    /// 时间窗口型（5h/周等），主指标是 utilization。
    #[default]
    Window,
    /// 余额型（中转 $ 余额/积分），主指标是 remaining_value。
    Balance,
}

/// Quota tier for a time window (5h or 7d) or a balance.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct QuotaTier {
    /// Tier name: "five_hour" / "seven_day" / 余额币种等
    pub name: String,
    /// 配额类型（默认窗口型）
    #[serde(default)]
    pub kind: QuotaKind,
    /// Usage percentage (0-100)；余额型可为 0
    pub utilization: f64,
    /// Reset time in ISO 8601 format
    pub resets_at: Option<String>,
    /// 余额型：剩余额度/余额
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_value: Option<f64>,
    /// 余额型/套餐：上限/总额
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_value: Option<f64>,
    /// 金额单位：USD / CNY / credits
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// 是否已触顶
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_reached: Option<bool>,
}

/// Subscription quota data for different providers
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionQuota {
    /// Provider identifier ("gpt", "claude", "relay" 等)
    pub provider: String,
    /// Tool identifier：官方为 "codex"/"claude"，中转为供应商 id（如 "deepseek"）
    pub tool: String,
    /// 来源工具：标识该额度属于哪个工具（"claude-code" / "codex" / "opencode"）。
    /// 仅已配置来源额度查询会填充；官方 OAuth 查询为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_tool: Option<String>,
    /// Credential status
    pub credential_status: String,
    /// Credential message
    pub credential_message: Option<String>,
    /// Whether the query was successful
    pub success: bool,
    /// Quota tiers (5h and 7d windows)
    pub tiers: Vec<QuotaTier>,
    /// Last update timestamp (milliseconds)
    pub updated_at: i64,
    /// Whether the data is from cache
    pub from_cache: bool,
    /// Error message
    pub error: Option<String>,
    /// Plan / tier label (e.g. "Free", "Pro"), provider-specific (Gemini)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_label: Option<String>,
    /// Account label such as email or project id, provider-specific (Gemini)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
}

/// Result of configured source quota refresh.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfiguredSourceQuotaQueryResult {
    pub quotas: Vec<SubscriptionQuota>,
    pub attempted_count: usize,
    pub success_count: usize,
    pub failed_count: usize,
    #[serde(default)]
    pub errors: Vec<String>,
    pub queried_at: i64,
}

/// Credential status for subscription queries
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CredentialStatus {
    /// No credentials configured
    NotConfigured,
    /// Credentials valid and ready
    Valid,
    /// Token expired, refresh needed
    Expired,
    /// Refresh failed, re-authentication required
    RefreshFailed { error: String },
    /// Query failed with current credentials
    QueryFailed { error: String },
}

/// Result of a subscription query
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionQueryResult {
    /// Whether the query was successful
    pub success: bool,
    /// Subscription quota data (if successful)
    pub quota: Option<SubscriptionQuota>,
    /// Credential status
    pub credential_status: CredentialStatus,
    /// Error message (if failed)
    pub error: Option<String>,
    /// Query timestamp (milliseconds)
    pub queried_at: i64,
}

impl SubscriptionQueryResult {
    /// Create a successful result
    pub fn success(quota: SubscriptionQuota) -> Self {
        Self {
            success: true,
            quota: Some(quota),
            credential_status: CredentialStatus::Valid,
            error: None,
            queried_at: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Create a result indicating no credentials
    /// Provider parameter retained for future multi-provider support
    #[allow(unused_variables)]
    pub fn no_credentials(provider: &str) -> Self {
        Self {
            success: false,
            quota: None,
            credential_status: CredentialStatus::NotConfigured,
            error: None,
            queried_at: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Create an error result
    /// Provider parameter retained for future multi-provider support
    #[allow(unused_variables)]
    pub fn error(provider: &str, status: CredentialStatus, error: String) -> Self {
        Self {
            success: false,
            quota: None,
            credential_status: status,
            error: Some(error),
            queried_at: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Create a cached result
    pub fn from_cache(quota: SubscriptionQuota) -> Self {
        let mut result = Self::success(quota);
        if let Some(ref mut q) = result.quota {
            q.from_cache = true;
        }
        result
    }
}
