//! Token caching and automatic refresh

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::types::{ChatGptTokens, SubscriptionError};

/// Token refresh threshold in seconds (refresh 60 seconds before expiry)
const REFRESH_THRESHOLD_SECS: i64 = 60;

/// Cached token entry
#[derive(Clone)]
struct CachedToken {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<i64>,
    account_id: Option<String>,
}

/// Token cache with automatic refresh capability
pub struct TokenCache {
    tokens: Arc<RwLock<HashMap<String, CachedToken>>>,
}

impl Default for TokenCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenCache {
    pub fn new() -> Self {
        Self {
            tokens: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Store tokens for a provider
    pub async fn store_tokens(&self, provider: &str, tokens: ChatGptTokens) {
        if let Some(access_token) = tokens.access_token {
            let mut cache = self.tokens.write().await;
            cache.insert(
                provider.to_string(),
                CachedToken {
                    access_token,
                    refresh_token: tokens.refresh_token,
                    expires_at: tokens.expires_at,
                    account_id: tokens.account_id,
                },
            );
        }
    }

    /// Get valid access token, refreshing if necessary
    pub async fn get_valid_token(&self, provider: &str) -> Result<String, SubscriptionError> {
        let mut cache = self.tokens.write().await;

        if let Some(cached) = cache.get_mut(provider) {
            let now = chrono::Utc::now().timestamp();

            // Case 1: Token has known expiration time
            if let Some(expires_at) = cached.expires_at {
                // Already expired and no refresh token
                if now >= expires_at && cached.refresh_token.is_none() {
                    return Err(SubscriptionError::NoRefreshToken);
                }
                // Need refresh (60 seconds before expiry)
                if now >= expires_at - REFRESH_THRESHOLD_SECS {
                    if let Some(refresh_token) = &cached.refresh_token {
                        match refresh_gpt_token(refresh_token).await {
                            Ok(new_tokens) => {
                                if let Some(access_token) = new_tokens.access_token {
                                    cached.access_token = access_token;
                                    cached.refresh_token = new_tokens.refresh_token;
                                    cached.expires_at = new_tokens.expires_at;
                                    cached.account_id = new_tokens.account_id;
                                } else {
                                    return Err(SubscriptionError::ParseError {
                                        message: "Missing access_token in refresh response"
                                            .to_string(),
                                    });
                                }
                            }
                            Err(e) => {
                                return Err(e);
                            }
                        }
                    }
                }
            } else {
                // Case 2: No expiration time known - try refresh if we have refresh_token
                // This handles ChatGPT tokens which don't include expiry in auth.json
                if let Some(refresh_token) = &cached.refresh_token {
                    match refresh_gpt_token(refresh_token).await {
                        Ok(new_tokens) => {
                            if let Some(access_token) = new_tokens.access_token {
                                cached.access_token = access_token;
                                cached.refresh_token = new_tokens.refresh_token;
                                cached.expires_at = new_tokens.expires_at;
                                cached.account_id = new_tokens.account_id;
                            } else {
                                return Err(SubscriptionError::ParseError {
                                    message: "Missing access_token in refresh response".to_string(),
                                });
                            }
                        }
                        Err(e) => {
                            // Refresh failed, but existing token might still work
                            // Let the API call determine if token is actually expired
                            eprintln!("[TokenCache] Refresh failed: {}, using existing token", e);
                        }
                    }
                }
            }
            return Ok(cached.access_token.clone());
        }

        Err(SubscriptionError::NoCredentials)
    }

    /// Get account ID for a provider
    #[allow(dead_code)]
    pub async fn get_account_id(&self, provider: &str) -> Option<String> {
        let cache = self.tokens.read().await;
        cache.get(provider).and_then(|t| t.account_id.clone())
    }

    /// Clear tokens for a provider
    #[allow(dead_code)]
    pub async fn clear(&self, provider: &str) {
        let mut cache = self.tokens.write().await;
        cache.remove(provider);
    }

    /// Clear all tokens
    #[allow(dead_code)]
    pub async fn clear_all(&self) {
        let mut cache = self.tokens.write().await;
        cache.clear();
    }
}

/// Refresh GPT OAuth token
async fn refresh_gpt_token(refresh_token: &str) -> Result<ChatGptTokens, SubscriptionError> {
    // OpenAI Auth0 OAuth endpoint
    const TOKEN_URL: &str = "https://auth0.openai.com/oauth/token";
    // Client ID from OpenAI's official Codex CLI tool
    const CLIENT_ID: &str = "pdlLIX2Y72MIl2rhLhTE9VV9bN905kBh";

    let client = reqwest::Client::new();

    let response = client
        .post(TOKEN_URL)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": CLIENT_ID
        }))
        .send()
        .await
        .map_err(|e| SubscriptionError::NetworkError {
            message: e.to_string(),
        })?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(SubscriptionError::RefreshFailed {
            message: format!("HTTP {}: {}", status, body),
        });
    }

    let data: serde_json::Value =
        response
            .json()
            .await
            .map_err(|e| SubscriptionError::ParseError {
                message: e.to_string(),
            })?;

    let access_token = data
        .get("access_token")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| SubscriptionError::ParseError {
            message: "Missing access_token in refresh response".to_string(),
        })?;

    // Calculate expiration time from expires_in
    let expires_in = data
        .get("expires_in")
        .and_then(|v| v.as_i64())
        .unwrap_or(3600);
    let expires_at = chrono::Utc::now().timestamp() + expires_in;

    Ok(ChatGptTokens {
        access_token: Some(access_token),
        refresh_token: data
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(String::from),
        account_id: None, // Will be extracted from JWT if needed
        expires_at: Some(expires_at),
    })
}
