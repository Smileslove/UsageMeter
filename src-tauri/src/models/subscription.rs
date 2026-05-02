//! Subscription quota data models
//!
//! Data structures for subscription quota information from official providers.

use serde::{Deserialize, Serialize};

/// Quota tier for a time window (5h or 7d)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaTier {
    /// Tier name: "five_hour" or "seven_day"
    pub name: String,
    /// Usage percentage (0-100)
    pub utilization: f64,
    /// Reset time in ISO 8601 format
    pub resets_at: Option<String>,
}

/// Subscription quota data for different providers
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionQuota {
    /// Provider identifier ("gpt", "claude", etc.)
    pub provider: String,
    /// Tool identifier ("codex" or "codex_oauth")
    pub tool: String,
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
