//! Token caching and automatic refresh

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::types::{ChatGptTokens, SubscriptionError};

use crate::net::HttpClientFactory;

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
        let now = chrono::Utc::now().timestamp();
        self.get_valid_token_with_refresher(provider, now, refresh_gpt_token_boxed)
            .await
    }

    /// 可测内部实现：注入当前时间与刷新器，隔离全局 HTTP client 与真实时钟。
    ///
    /// 行为与原 `get_valid_token` 完全一致；`get_valid_token` 以真实时钟与默认
    /// `refresh_gpt_token` 调用本方法。
    ///
    /// `refresher` 使用 HRTB 约束：闭包返回的 boxed future 生命周期与输入
    /// `&str` 绑定，允许 async fn 与测试闭包统一注入。
    async fn get_valid_token_with_refresher<F>(
        &self,
        provider: &str,
        now_sec: i64,
        refresher: F,
    ) -> Result<String, SubscriptionError>
    where
        F: for<'a> Fn(
            &'a str,
        ) -> Pin<
            Box<dyn Future<Output = Result<ChatGptTokens, SubscriptionError>> + Send + 'a>,
        >,
    {
        let mut cache = self.tokens.write().await;

        if let Some(cached) = cache.get_mut(provider) {
            // Case 1: Token has known expiration time
            if let Some(expires_at) = cached.expires_at {
                // Already expired and no refresh token
                if now_sec >= expires_at && cached.refresh_token.is_none() {
                    return Err(SubscriptionError::NoRefreshToken);
                }
                // Need refresh (60 seconds before expiry)
                if now_sec >= expires_at - REFRESH_THRESHOLD_SECS {
                    if let Some(refresh_token) = &cached.refresh_token {
                        match refresher(refresh_token).await {
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
                    match refresher(refresh_token).await {
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

    let client = HttpClientFactory::global().standard();

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

/// 将 async fn 包装为 HRTB 兼容的 boxed future 刷新器，供 `get_valid_token` 使用。
fn refresh_gpt_token_boxed(
    refresh_token: &str,
) -> Pin<Box<dyn Future<Output = Result<ChatGptTokens, SubscriptionError>> + Send + '_>> {
    Box::pin(refresh_gpt_token(refresh_token))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(access: &str, refresh: Option<&str>, expires_at: Option<i64>) -> ChatGptTokens {
        ChatGptTokens {
            access_token: Some(access.to_string()),
            refresh_token: refresh.map(str::to_string),
            account_id: Some("acct-1".to_string()),
            expires_at,
        }
    }

    /// 总是成功的假刷新器，可追踪被调用的 refresh token。
    fn ok_refresher(
        calls: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        new_access: &'static str,
    ) -> impl for<'a> Fn(
        &'a str,
    ) -> Pin<
        Box<dyn Future<Output = Result<ChatGptTokens, SubscriptionError>> + Send + 'a>,
    > {
        let calls = calls.clone();
        move |refresh_token: &str| {
            calls.lock().unwrap().push(refresh_token.to_string());
            let mut t = tokens(new_access, Some("new-refresh"), Some(9_999));
            t.account_id = Some("acct-new".to_string());
            Box::pin(async move { Ok(t) })
        }
    }

    #[tokio::test]
    async fn get_valid_token_no_cache_returns_no_credentials() {
        let cache = TokenCache::new();
        let err = cache
            .get_valid_token_with_refresher("chatgpt", 1_000, unexpected_refresher())
            .await
            .expect_err("expected NoCredentials");
        assert!(matches!(err, SubscriptionError::NoCredentials));
    }

    #[tokio::test]
    async fn store_tokens_ignores_missing_access_token() {
        let cache = TokenCache::new();
        cache
            .store_tokens(
                "chatgpt",
                ChatGptTokens {
                    access_token: None,
                    refresh_token: Some("rt".to_string()),
                    account_id: None,
                    expires_at: Some(1_000),
                },
            )
            .await;
        let err = cache
            .get_valid_token_with_refresher("chatgpt", 1_000, unexpected_refresher())
            .await
            .expect_err("no cached token");
        assert!(matches!(err, SubscriptionError::NoCredentials));
    }

    #[tokio::test]
    async fn get_valid_token_expired_without_refresh_returns_no_refresh_token() {
        let cache = TokenCache::new();
        cache
            .store_tokens("chatgpt", tokens("old-token", None, Some(1_000)))
            .await;
        let err = cache
            .get_valid_token_with_refresher("chatgpt", 1_000, unexpected_refresher())
            .await
            .expect_err("expected NoRefreshToken");
        assert!(matches!(err, SubscriptionError::NoRefreshToken));
    }

    #[tokio::test]
    async fn get_valid_token_refreshes_at_threshold_boundary() {
        let cache = TokenCache::new();
        let expires_at = 1_000;
        cache
            .store_tokens(
                "chatgpt",
                tokens("old-token", Some("rt-1"), Some(expires_at)),
            )
            .await;

        // now == expires_at - 60：恰好进入刷新窗口 → 刷新。
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let token = cache
            .get_valid_token_with_refresher(
                "chatgpt",
                expires_at - 60,
                ok_refresher(&calls, "new-1"),
            )
            .await
            .expect("refreshed at boundary");
        assert_eq!(token, "new-1");
        assert_eq!(calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn get_valid_token_fresh_token_returns_without_refresh() {
        let cache = TokenCache::new();
        let expires_at = 1_000;
        cache
            .store_tokens("chatgpt", tokens("still-fresh", None, Some(expires_at)))
            .await;

        // now < expires_at - 60：不刷新直接返回，即使没有 refresh token。
        let token = cache
            .get_valid_token_with_refresher("chatgpt", expires_at - 61, unexpected_refresher())
            .await
            .expect("fresh token returned");
        assert_eq!(token, "still-fresh");
    }

    #[tokio::test]
    async fn get_valid_token_refresh_success_updates_cached_token() {
        let cache = TokenCache::new();
        let expires_at = 1_000;
        cache
            .store_tokens(
                "chatgpt",
                tokens("old-token", Some("rt-1"), Some(expires_at)),
            )
            .await;

        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let token = cache
            .get_valid_token_with_refresher(
                "chatgpt",
                expires_at,
                ok_refresher(&calls, "new-token"),
            )
            .await
            .expect("refresh succeeds");
        assert_eq!(token, "new-token");
        assert_eq!(calls.lock().unwrap()[0], "rt-1");

        // 刷新后缓存已更新：远离刷新窗口再次调用，直接返回新 token 且不再刷新。
        let token = cache
            .get_valid_token_with_refresher("chatgpt", 9_000, unexpected_refresher())
            .await
            .expect("updated token returned");
        assert_eq!(token, "new-token");
    }

    #[tokio::test]
    async fn get_valid_token_refresh_failure_keeps_existing_token() {
        let cache = TokenCache::new();
        let expires_at = 1_000;
        cache
            .store_tokens(
                "chatgpt",
                tokens("old-token", Some("rt-1"), Some(expires_at)),
            )
            .await;

        // 刷新失败：返回错误，但缓存中的旧 token 保留。
        let err = cache
            .get_valid_token_with_refresher("chatgpt", expires_at, |_| {
                Box::pin(async {
                    Err(SubscriptionError::RefreshFailed {
                        message: "HTTP 401".to_string(),
                    })
                })
            })
            .await
            .expect_err("refresh failed");
        assert!(matches!(err, SubscriptionError::RefreshFailed { .. }));

        // 之后在非刷新窗口再次获取，仍能取到旧 token。
        let token = cache
            .get_valid_token_with_refresher("chatgpt", expires_at - 100, unexpected_refresher())
            .await
            .expect("old token kept");
        assert_eq!(token, "old-token");
    }

    #[tokio::test]
    async fn get_valid_token_refresh_missing_access_token_returns_parse_error() {
        let cache = TokenCache::new();
        let expires_at = 1_000;
        cache
            .store_tokens(
                "chatgpt",
                tokens("old-token", Some("rt-1"), Some(expires_at)),
            )
            .await;

        let err = cache
            .get_valid_token_with_refresher("chatgpt", expires_at, |_| {
                Box::pin(async {
                    Ok(ChatGptTokens {
                        access_token: None,
                        refresh_token: None,
                        account_id: None,
                        expires_at: None,
                    })
                })
            })
            .await
            .expect_err("missing access token");
        assert!(matches!(err, SubscriptionError::ParseError { .. }));
    }

    #[tokio::test]
    async fn get_valid_token_without_expiry_refreshes_when_refresh_token_present() {
        let cache = TokenCache::new();
        cache
            .store_tokens("chatgpt", tokens("old-token", Some("rt-1"), None))
            .await;

        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let token = cache
            .get_valid_token_with_refresher("chatgpt", 5_000, ok_refresher(&calls, "refreshed"))
            .await
            .expect("refresh without expiry");
        assert_eq!(token, "refreshed");
        assert_eq!(calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn get_valid_token_without_expiry_and_refresh_returns_existing_token() {
        let cache = TokenCache::new();
        cache
            .store_tokens("chatgpt", tokens("old-token", None, None))
            .await;

        let token = cache
            .get_valid_token_with_refresher("chatgpt", 5_000, unexpected_refresher())
            .await
            .expect("existing token returned");
        assert_eq!(token, "old-token");
    }

    /// 不应被调用的刷新器：一旦被调用即返回网络错误（各“must not refresh”断言
    /// 因此会以 expect 失败的方式暴露回归）。
    fn unexpected_refresher() -> impl for<'a> Fn(
        &'a str,
    ) -> Pin<
        Box<dyn Future<Output = Result<ChatGptTokens, SubscriptionError>> + Send + 'a>,
    > {
        |_| {
            Box::pin(async {
                Err(SubscriptionError::NetworkError {
                    message: "unexpected refresh call".to_string(),
                })
            })
        }
    }
}
