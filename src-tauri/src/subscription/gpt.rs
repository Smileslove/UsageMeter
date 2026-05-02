//! GPT/ChatGPT subscription query implementation

use std::sync::Arc;

use reqwest::Client;
use serde::Deserialize;

use crate::models::{CredentialStatus, QuotaTier, SubscriptionQueryResult, SubscriptionQuota};
use crate::proxy::CodexConfigManager;

use super::token_cache::TokenCache;
use super::types::{ChatGptTokens, SubscriptionError};

const GPT_USAGE_API: &str = "https://chatgpt.com/backend-api/wham/usage";
const PROVIDER_ID: &str = "gpt";

/// GPT subscription provider
#[derive(Clone)]
pub struct GptSubscriptionProvider {
    client: Client,
    token_cache: Arc<TokenCache>,
}

impl Default for GptSubscriptionProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl GptSubscriptionProvider {
    /// Create a new provider with its own token cache
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            token_cache: Arc::new(TokenCache::new()),
        }
    }

    /// Create a provider with a shared token cache
    pub fn with_token_cache(token_cache: Arc<TokenCache>) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            token_cache,
        }
    }

    /// Check if ChatGPT OAuth is configured
    pub fn has_chatgpt_oauth(&self) -> bool {
        let manager = CodexConfigManager::new();
        match manager.read_live_snapshot() {
            Ok(snapshot) => snapshot.auth_mode == crate::proxy::CodexAuthMode::ChatGpt,
            Err(_) => false,
        }
    }

    /// Extract ChatGPT tokens from Codex auth.json
    fn extract_tokens(&self) -> Result<ChatGptTokens, SubscriptionError> {
        let manager = CodexConfigManager::new();
        let snapshot = manager
            .read_live_snapshot()
            .map_err(|_e| SubscriptionError::NoCredentials)?;

        if snapshot.auth_mode != crate::proxy::CodexAuthMode::ChatGpt {
            return Err(SubscriptionError::NoCredentials);
        }

        let auth = snapshot.auth_json.ok_or(SubscriptionError::NoCredentials)?;
        let tokens = auth.get("tokens").ok_or(SubscriptionError::NoCredentials)?;

        // Extract account ID from JWT if not present
        let account_id = tokens
            .get("account_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| {
                tokens
                    .get("access_token")
                    .and_then(|v| v.as_str())
                    .and_then(extract_account_id_from_jwt)
            });

        Ok(ChatGptTokens {
            access_token: tokens
                .get("access_token")
                .and_then(|v| v.as_str())
                .map(String::from),
            refresh_token: tokens
                .get("refresh_token")
                .and_then(|v| v.as_str())
                .map(String::from),
            account_id,
            expires_at: None, // ChatGPT tokens don't include expiry in auth.json
        })
    }

    /// Fetch subscription quota
    pub async fn fetch_quota(&self) -> SubscriptionQueryResult {
        // Check if ChatGPT OAuth is configured
        if !self.has_chatgpt_oauth() {
            return SubscriptionQueryResult::no_credentials(PROVIDER_ID);
        }

        // Extract tokens
        let tokens = match self.extract_tokens() {
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

        // Store tokens in cache
        self.token_cache
            .store_tokens(PROVIDER_ID, tokens.clone())
            .await;

        // Get valid access token (will refresh if needed)
        let access_token = match self.token_cache.get_valid_token(PROVIDER_ID).await {
            Ok(t) => t,
            Err(e) => {
                let status = match &e {
                    SubscriptionError::TokenExpired | SubscriptionError::NoRefreshToken => {
                        CredentialStatus::Expired
                    }
                    SubscriptionError::RefreshFailed { .. } => CredentialStatus::RefreshFailed {
                        error: e.user_message(),
                    },
                    _ => CredentialStatus::QueryFailed {
                        error: e.user_message(),
                    },
                };
                return SubscriptionQueryResult::error(PROVIDER_ID, status, e.user_message());
            }
        };

        // Fetch usage data
        match self.fetch_usage_api(&access_token).await {
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
    ) -> Result<SubscriptionQuota, SubscriptionError> {
        let response = self
            .client
            .get(GPT_USAGE_API)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("User-Agent", "codex-cli")
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| {
                eprintln!("[Subscription] Network error: {}", e);
                SubscriptionError::NetworkError {
                    message: e.to_string(),
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            eprintln!("[Subscription] API error ({}): {}", status, body);
            return Err(SubscriptionError::ApiError {
                status: status.as_u16(),
                message: body,
            });
        }

        // Get raw text first for debugging
        let text = response.text().await.map_err(|e| {
            eprintln!("[Subscription] Failed to read response: {}", e);
            SubscriptionError::ParseError {
                message: e.to_string(),
            }
        })?;

        let data: GptUsageResponse = serde_json::from_str(&text).map_err(|e| {
            eprintln!(
                "[Subscription] Failed to parse JSON: {}\nResponse: {}",
                e, text
            );
            SubscriptionError::ParseError {
                message: format!("{}: {}", e, text),
            }
        })?;

        // Convert to SubscriptionQuota
        let mut tiers = Vec::new();

        // Process primary window (usually 5 hours)
        if let Some(primary) = data.rate_limit.primary_window {
            tiers.push(QuotaTier {
                name: window_seconds_to_name(primary.limit_window_seconds),
                utilization: primary.used_percent,
                resets_at: primary.reset_at.map(|ts| {
                    chrono::DateTime::from_timestamp(ts, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                }),
            });
        }

        // Process secondary window (usually 7 days)
        if let Some(secondary) = data.rate_limit.secondary_window {
            tiers.push(QuotaTier {
                name: window_seconds_to_name(secondary.limit_window_seconds),
                utilization: secondary.used_percent,
                resets_at: secondary.reset_at.map(|ts| {
                    chrono::DateTime::from_timestamp(ts, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                }),
            });
        }

        Ok(SubscriptionQuota {
            provider: PROVIDER_ID.to_string(),
            tool: "codex_oauth".to_string(),
            credential_status: "valid".to_string(),
            credential_message: None,
            success: true,
            tiers,
            updated_at: chrono::Utc::now().timestamp_millis(),
            from_cache: false,
            error: None,
        })
    }

    /// Clear token cache
    #[allow(dead_code)]
    pub async fn clear_cache(&self) {
        self.token_cache.clear(PROVIDER_ID).await;
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
}

#[derive(Debug, Deserialize)]
struct GptRateLimit {
    primary_window: Option<GptWindow>,
    secondary_window: Option<GptWindow>,
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
