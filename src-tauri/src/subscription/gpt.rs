//! GPT/ChatGPT subscription query implementation

use reqwest::Client;
use serde::Deserialize;

use crate::models::{CredentialStatus, QuotaTier, SubscriptionQueryResult, SubscriptionQuota};
use crate::net::HttpClientFactory;
use crate::proxy::CodexConfigManager;

use super::types::SubscriptionError;

const GPT_USAGE_API: &str = "https://chatgpt.com/backend-api/wham/usage";
const PROVIDER_ID: &str = "gpt";

/// GPT subscription provider
#[derive(Clone)]
pub struct GptSubscriptionProvider {
    client: Client,
}

impl Default for GptSubscriptionProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl GptSubscriptionProvider {
    /// Create a new provider.
    pub fn new() -> Self {
        let client = HttpClientFactory::global().standard();

        Self { client }
    }

    /// Check if ChatGPT OAuth is configured
    pub fn has_chatgpt_oauth(&self) -> bool {
        let manager = CodexConfigManager::new();
        match manager.read_live_snapshot() {
            Ok(snapshot) => snapshot.auth_mode == crate::proxy::CodexAuthMode::ChatGpt,
            Err(_) => false,
        }
    }

    /// Extract the current ChatGPT access token from Codex auth.json.
    ///
    /// UsageMeter intentionally does not refresh or write Codex credentials. Codex owns
    /// refresh-token rotation; using the current access token keeps this query read-only.
    fn extract_credentials(&self) -> Result<GptCredentials, SubscriptionError> {
        let manager = CodexConfigManager::new();
        let snapshot = manager
            .read_live_snapshot()
            .map_err(|_e| SubscriptionError::NoCredentials)?;

        if snapshot.auth_mode != crate::proxy::CodexAuthMode::ChatGpt {
            return Err(SubscriptionError::NoCredentials);
        }

        let auth_content = std::fs::read_to_string(manager.auth_path())
            .map_err(|_e| SubscriptionError::NoCredentials)?;
        let auth: serde_json::Value =
            serde_json::from_str(&auth_content).map_err(|_e| SubscriptionError::NoCredentials)?;
        parse_credentials(&auth)
    }

    /// Fetch subscription quota
    pub async fn fetch_quota(&self) -> SubscriptionQueryResult {
        // Check if ChatGPT OAuth is configured
        if !self.has_chatgpt_oauth() {
            return SubscriptionQueryResult::no_credentials(PROVIDER_ID);
        }

        let credentials = match self.extract_credentials() {
            Ok(t) => t,
            Err(SubscriptionError::NoCredentials) => {
                return SubscriptionQueryResult::no_credentials(PROVIDER_ID);
            }
            Err(e) => {
                return SubscriptionQueryResult::error(
                    PROVIDER_ID,
                    CredentialStatus::QueryFailed {
                        error: e.user_message(),
                    },
                    e.user_message(),
                );
            }
        };

        // Fetch usage data
        match self
            .fetch_usage_api(&credentials.access_token, credentials.account_id.as_deref())
            .await
        {
            Ok(quota) => SubscriptionQueryResult::success(quota),
            Err(e) => {
                let status = match &e {
                    SubscriptionError::TokenExpired => CredentialStatus::Expired,
                    SubscriptionError::ApiError { status, .. }
                        if *status == 401 || *status == 403 =>
                    {
                        CredentialStatus::Expired
                    }
                    _ => CredentialStatus::QueryFailed {
                        error: e.user_message(),
                    },
                };
                SubscriptionQueryResult::error(PROVIDER_ID, status, e.user_message())
            }
        }
    }

    /// Fetch usage from GPT API
    async fn fetch_usage_api(
        &self,
        access_token: &str,
        account_id: Option<&str>,
    ) -> Result<SubscriptionQuota, SubscriptionError> {
        let response = usage_request(&self.client, access_token, account_id)
            .send()
            .await
            .map_err(|e| SubscriptionError::NetworkError {
                message: e.to_string(),
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(SubscriptionError::ApiError {
                status: status.as_u16(),
                message: body,
            });
        }

        // Get raw text first for debugging
        let text = response
            .text()
            .await
            .map_err(|e| SubscriptionError::ParseError {
                message: e.to_string(),
            })?;

        parse_usage_response(&text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GptCredentials {
    access_token: String,
    account_id: Option<String>,
}

fn parse_credentials(auth: &serde_json::Value) -> Result<GptCredentials, SubscriptionError> {
    let tokens = auth.get("tokens").ok_or(SubscriptionError::NoCredentials)?;
    let access_token = tokens
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(String::from)
        .ok_or(SubscriptionError::NoCredentials)?;
    let account_id = tokens
        .get("account_id")
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(String::from)
        .or_else(|| extract_account_id_from_jwt(&access_token));

    Ok(GptCredentials {
        access_token,
        account_id,
    })
}

fn usage_request<'a>(
    client: &'a Client,
    access_token: &'a str,
    account_id: Option<&'a str>,
) -> reqwest::RequestBuilder {
    let request = client
        .get(GPT_USAGE_API)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("User-Agent", "codex-cli")
        .header("Accept", "application/json");
    match account_id.filter(|value| !value.trim().is_empty()) {
        Some(account_id) => request.header("ChatGPT-Account-ID", account_id),
        None => request,
    }
}

fn parse_usage_response(text: &str) -> Result<SubscriptionQuota, SubscriptionError> {
    let data: GptUsageResponse =
        serde_json::from_str(text).map_err(|e| SubscriptionError::ParseError {
            message: e.to_string(),
        })?;

    let mut tiers = Vec::new();

    if let Some(primary) = data.rate_limit.primary_window {
        tiers.push(window_tier(primary, data.rate_limit.limit_reached));
    }

    if let Some(secondary) = data.rate_limit.secondary_window {
        tiers.push(window_tier(secondary, data.rate_limit.limit_reached));
    }

    Ok(SubscriptionQuota {
        provider: PROVIDER_ID.to_string(),
        tool: "codex_oauth".to_string(),
        source_tool: None,
        credential_status: "valid".to_string(),
        credential_message: None,
        success: true,
        tiers,
        updated_at: chrono::Utc::now().timestamp_millis(),
        from_cache: false,
        error: None,
        plan_label: data.plan_type,
        account_label: None,
    })
}

fn window_tier(window: GptWindow, limit_reached: Option<bool>) -> QuotaTier {
    QuotaTier {
        name: window_seconds_to_name(window.limit_window_seconds),
        utilization: window.used_percent,
        resets_at: window
            .reset_at
            .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0).map(|dt| dt.to_rfc3339())),
        limit_reached,
        ..Default::default()
    }
}

/// Convert window seconds to tier name
fn window_seconds_to_name(seconds: i64) -> String {
    match seconds {
        18000 => "five_hour".to_string(),
        604800 => "seven_day".to_string(),
        _ => {
            // Dynamic calculation for other windows
            let hours = seconds / 3600;
            if hours >= 24 {
                format!("{}_day", hours / 24)
            } else {
                format!("{}_hour", hours)
            }
        }
    }
}

/// GPT Usage API response structure
#[derive(Debug, Deserialize)]
struct GptUsageResponse {
    rate_limit: GptRateLimit,
    plan_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GptRateLimit {
    primary_window: Option<GptWindow>,
    secondary_window: Option<GptWindow>,
    limit_reached: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct GptWindow {
    used_percent: f64,
    limit_window_seconds: i64,
    reset_at: Option<i64>,
}

/// Extract account ID from JWT token
fn extract_account_id_from_jwt(token: &str) -> Option<String> {
    use base64::Engine;

    let payload = token.split('.').nth(1)?;
    let mut value = payload.replace('-', "+").replace('_', "/");
    while !value.len().is_multiple_of(4) {
        value.push('=');
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value.as_bytes())
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    json.pointer("/https://api.openai.com/auth/chatgpt_account_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_request_includes_the_selected_chatgpt_account() {
        let request = usage_request(&Client::new(), "access-token", Some("account-123"))
            .build()
            .expect("request should build");
        assert_eq!(
            request.headers().get("ChatGPT-Account-ID").unwrap(),
            "account-123"
        );

        let request_without_account = usage_request(&Client::new(), "access-token", Some(" "))
            .build()
            .expect("request should build");
        assert!(request_without_account
            .headers()
            .get("ChatGPT-Account-ID")
            .is_none());
    }

    #[test]
    fn parses_current_wham_usage_windows() {
        let quota = parse_usage_response(
            r#"{
                "plan_type": "plus",
                "rate_limit": {
                    "limit_reached": false,
                    "primary_window": {
                        "used_percent": 89,
                        "limit_window_seconds": 18000,
                        "reset_at": 1790446126
                    },
                    "secondary_window": {
                        "used_percent": 66,
                        "limit_window_seconds": 604800,
                        "reset_at": 1790735884
                    }
                }
            }"#,
        )
        .expect("current wham response should parse");

        assert_eq!(quota.plan_label.as_deref(), Some("plus"));
        assert_eq!(quota.tiers.len(), 2);
        assert_eq!(quota.tiers[0].name, "five_hour");
        assert_eq!(quota.tiers[0].utilization, 89.0);
        assert_eq!(quota.tiers[0].limit_reached, Some(false));
        assert_eq!(quota.tiers[1].name, "seven_day");
        assert_eq!(quota.tiers[1].utilization, 66.0);
    }

    #[test]
    fn credentials_require_an_access_token_and_keep_the_account_id() {
        let auth = serde_json::json!({
            "tokens": { "access_token": "access", "account_id": "account-123" }
        });
        assert_eq!(
            parse_credentials(&auth).expect("credentials should parse"),
            GptCredentials {
                access_token: "access".to_string(),
                account_id: Some("account-123".to_string()),
            }
        );
        assert!(parse_credentials(&serde_json::json!({ "tokens": {} })).is_err());
    }
}
