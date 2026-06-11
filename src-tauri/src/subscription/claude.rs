//! Claude (Anthropic) subscription quota query implementation
//!
//! Queries the official Anthropic OAuth usage API to obtain Claude Code
//! subscription quota windows (five_hour / seven_day / ...).
//!
//! Credentials are read from the Claude Code OAuth store and are never
//! written back — UsageMeter only reads them and, when needed, refreshes
//! the access token in-memory for the duration of a single query.

use std::path::PathBuf;

use reqwest::Client;

use crate::models::{CredentialStatus, QuotaTier, SubscriptionQueryResult, SubscriptionQuota};
use crate::net::HttpClientFactory;

use super::types::SubscriptionError;

const CLAUDE_USAGE_API: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
/// Public OAuth client id used by Claude Code.
const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";
const PROVIDER_ID: &str = "claude";
/// Refresh this many seconds before the token's known expiry.
const REFRESH_THRESHOLD_SECS: i64 = 60;

/// Claude OAuth credentials extracted from the local credential store.
#[derive(Debug, Clone)]
struct ClaudeOAuth {
    access_token: String,
    refresh_token: Option<String>,
    /// Expiry timestamp in milliseconds since epoch, if known.
    expires_at_ms: Option<i64>,
}

/// Claude subscription provider.
#[derive(Clone)]
pub struct ClaudeSubscriptionProvider {
    client: Client,
}

impl Default for ClaudeSubscriptionProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeSubscriptionProvider {
    pub fn new() -> Self {
        Self {
            client: HttpClientFactory::global().standard(),
        }
    }

    /// Whether Claude OAuth credentials are available locally.
    pub fn has_claude_oauth(&self) -> bool {
        load_oauth().is_some()
    }

    /// Fetch the Claude subscription quota.
    pub async fn fetch_quota(&self) -> SubscriptionQueryResult {
        let mut creds = match load_oauth() {
            Some(c) => c,
            None => return SubscriptionQueryResult::no_credentials(PROVIDER_ID),
        };

        // 主动刷新（仅当已知接近过期且有 refresh token）。
        // 刷新失败不致命：Claude Code 通常自己维护 token 新鲜，回退用现有
        // access token，让 usage API 以 401 判定真正过期（与 TokenTracker /
        // cc-switch 的「优先使用现有 token」策略一致）。
        let mut already_refreshed = false;
        if token_needs_refresh(&creds) && creds.refresh_token.is_some() {
            if let Ok(refreshed) = self.try_refresh(&creds).await {
                creds = refreshed;
                already_refreshed = true;
            }
        }

        match self.fetch_usage_api(&creds.access_token).await {
            Ok(quota) => SubscriptionQueryResult::success(quota),
            // Reactive refresh: token rejected, try once more with a fresh token.
            Err(SubscriptionError::ApiError { status, .. })
                if (status == 401 || status == 403)
                    && !already_refreshed
                    && creds.refresh_token.is_some() =>
            {
                match self.try_refresh(&creds).await {
                    Ok(refreshed) => match self.fetch_usage_api(&refreshed.access_token).await {
                        Ok(quota) => SubscriptionQueryResult::success(quota),
                        Err(e) => SubscriptionQueryResult::error(
                            PROVIDER_ID,
                            error_status(&e),
                            e.user_message(),
                        ),
                    },
                    Err(_) => SubscriptionQueryResult::error(
                        PROVIDER_ID,
                        CredentialStatus::Expired,
                        SubscriptionError::TokenExpired.user_message(),
                    ),
                }
            }
            Err(e) => {
                SubscriptionQueryResult::error(PROVIDER_ID, error_status(&e), e.user_message())
            }
        }
    }

    /// Refresh the access token using the refresh token.
    async fn try_refresh(&self, creds: &ClaudeOAuth) -> Result<ClaudeOAuth, SubscriptionError> {
        let refresh_token = creds
            .refresh_token
            .clone()
            .ok_or(SubscriptionError::NoRefreshToken)?;

        let response = self
            .client
            .post(CLAUDE_TOKEN_URL)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "client_id": CLAUDE_CLIENT_ID,
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

        let expires_at_ms = data
            .get("expires_in")
            .and_then(|v| v.as_i64())
            .map(|secs| chrono::Utc::now().timestamp_millis() + secs * 1000);

        Ok(ClaudeOAuth {
            access_token,
            refresh_token: data
                .get("refresh_token")
                .and_then(|v| v.as_str())
                .map(String::from)
                .or(creds.refresh_token.clone()),
            expires_at_ms,
        })
    }

    /// Call the Anthropic OAuth usage API and normalize the response.
    async fn fetch_usage_api(
        &self,
        access_token: &str,
    ) -> Result<SubscriptionQuota, SubscriptionError> {
        let response = self
            .client
            .get(CLAUDE_USAGE_API)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("anthropic-beta", OAUTH_BETA_HEADER)
            .header("Accept", "application/json")
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

        let value: serde_json::Value =
            response
                .json()
                .await
                .map_err(|e| SubscriptionError::ParseError {
                    message: e.to_string(),
                })?;

        let tiers = parse_claude_usage(&value);

        Ok(SubscriptionQuota {
            provider: PROVIDER_ID.to_string(),
            tool: "claude_oauth".to_string(),
            source_tool: None,
            credential_status: "valid".to_string(),
            credential_message: None,
            success: true,
            tiers,
            updated_at: chrono::Utc::now().timestamp_millis(),
            from_cache: false,
            error: None,
            plan_label: None,
            account_label: None,
        })
    }
}

/// Map a fetch error to a credential status for the UI.
fn error_status(e: &SubscriptionError) -> CredentialStatus {
    match e {
        SubscriptionError::TokenExpired => CredentialStatus::Expired,
        SubscriptionError::ApiError { status, .. } if *status == 401 || *status == 403 => {
            CredentialStatus::Expired
        }
        _ => CredentialStatus::QueryFailed {
            error: e.user_message(),
        },
    }
}

/// Whether the access token should be refreshed proactively.
fn token_needs_refresh(creds: &ClaudeOAuth) -> bool {
    match creds.expires_at_ms {
        Some(expires_at_ms) => {
            let now_ms = chrono::Utc::now().timestamp_millis();
            now_ms >= expires_at_ms - REFRESH_THRESHOLD_SECS * 1000
        }
        // Unknown expiry: don't refresh proactively, rely on reactive 401 handling.
        None => false,
    }
}

/// Parse the Anthropic usage response into quota tiers.
///
/// Forward-compatible: every top-level entry whose value is an object carrying a
/// numeric `utilization` becomes a tier named by its key. This automatically
/// includes future windows (e.g. new `seven_day_*` variants) and naturally
/// excludes non-window fields like `extra_usage` (which has no `utilization`).
fn parse_claude_usage(value: &serde_json::Value) -> Vec<QuotaTier> {
    let Some(obj) = value.as_object() else {
        return Vec::new();
    };

    let mut tiers: Vec<QuotaTier> = obj
        .iter()
        .filter_map(|(key, val)| {
            let entry = val.as_object()?;
            let utilization = entry.get("utilization").and_then(|u| u.as_f64())?;
            let resets_at = entry
                .get("resets_at")
                .and_then(|r| r.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(String::from);
            Some(QuotaTier {
                name: key.clone(),
                utilization,
                resets_at,
                ..Default::default()
            })
        })
        .collect();

    tiers.sort_by_key(|tier| (tier_rank(&tier.name), tier.name.clone()));
    tiers
}

/// Stable ordering so the primary (5h) window always comes first.
fn tier_rank(name: &str) -> u8 {
    match name {
        "five_hour" => 0,
        "seven_day" => 1,
        "seven_day_sonnet" => 2,
        "seven_day_opus" => 3,
        _ => 10,
    }
}

/// Read Claude OAuth credentials from the local credential store.
///
/// Resolution order:
/// 1. `~/.claude/.credentials.json` (Linux and some macOS setups)
/// 2. macOS Keychain item `Claude Code-credentials` (best effort)
fn load_oauth() -> Option<ClaudeOAuth> {
    if let Some(raw) = read_credentials_file() {
        if let Some(creds) = parse_credentials_blob(&raw) {
            return Some(creds);
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(raw) = read_macos_keychain() {
            if let Some(creds) = parse_credentials_blob(&raw) {
                return Some(creds);
            }
        }
    }

    None
}

fn credentials_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
        .join(".credentials.json")
}

fn read_credentials_file() -> Option<String> {
    std::fs::read_to_string(credentials_path()).ok()
}

#[cfg(target_os = "macos")]
fn read_macos_keychain() -> Option<String> {
    let output = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Parse a credential blob (`{ "claudeAiOauth": { ... } }`) into [`ClaudeOAuth`].
fn parse_credentials_blob(raw: &str) -> Option<ClaudeOAuth> {
    let json: serde_json::Value = serde_json::from_str(raw).ok()?;
    // The OAuth object is usually under `claudeAiOauth`; some variants use
    // `claude.ai_oauth`. Tolerate the credentials being at the top level too.
    let oauth = json
        .get("claudeAiOauth")
        .or_else(|| json.get("claude.ai_oauth"))
        .unwrap_or(&json);

    let access_token = oauth
        .get("accessToken")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(String::from)?;

    let refresh_token = oauth
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(String::from);

    let expires_at_ms = oauth.get("expiresAt").and_then(|v| v.as_i64());

    Some(ClaudeOAuth {
        access_token,
        refresh_token,
        expires_at_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_and_unknown_windows_and_skips_extra_usage() {
        let value = serde_json::json!({
            "five_hour": { "utilization": 45.5, "resets_at": "2026-06-08T12:00:00Z" },
            "seven_day": { "utilization": 30, "resets_at": "2026-06-14T00:00:00Z" },
            "seven_day_opus": { "utilization": 10, "resets_at": "2026-06-14T00:00:00Z" },
            // A future/unknown window must still be picked up (forward compatible).
            "thirty_day": { "utilization": 5, "resets_at": "2026-07-01T00:00:00Z" },
            // extra_usage has no `utilization` -> must be ignored.
            "extra_usage": { "monthly_limit": 100, "used_credits": 12 }
        });

        let tiers = parse_claude_usage(&value);

        // extra_usage skipped, four windows kept.
        assert_eq!(tiers.len(), 4);

        // Deterministic ordering: five_hour, seven_day, seven_day_opus, then unknown.
        let names: Vec<&str> = tiers.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["five_hour", "seven_day", "seven_day_opus", "thirty_day"]
        );

        let five_hour = &tiers[0];
        assert_eq!(five_hour.utilization, 45.5);
        assert_eq!(five_hour.resets_at.as_deref(), Some("2026-06-08T12:00:00Z"));
    }

    #[test]
    fn missing_resets_at_is_tolerated() {
        let value = serde_json::json!({
            "five_hour": { "utilization": 12 }
        });
        let tiers = parse_claude_usage(&value);
        assert_eq!(tiers.len(), 1);
        assert_eq!(tiers[0].utilization, 12.0);
        assert!(tiers[0].resets_at.is_none());
    }

    #[test]
    fn empty_or_non_object_yields_no_tiers() {
        assert!(parse_claude_usage(&serde_json::json!([])).is_empty());
        assert!(parse_claude_usage(&serde_json::json!("nope")).is_empty());
        assert!(parse_claude_usage(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn parses_credentials_blob_with_wrapper() {
        let raw = r#"{
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-abc",
                "refreshToken": "sk-ant-ort01-def",
                "expiresAt": 1717000000000
            }
        }"#;
        let creds = parse_credentials_blob(raw).expect("should parse");
        assert_eq!(creds.access_token, "sk-ant-oat01-abc");
        assert_eq!(creds.refresh_token.as_deref(), Some("sk-ant-ort01-def"));
        assert_eq!(creds.expires_at_ms, Some(1717000000000));
    }

    #[test]
    fn credentials_blob_requires_access_token() {
        let raw = r#"{ "claudeAiOauth": { "refreshToken": "x" } }"#;
        assert!(parse_credentials_blob(raw).is_none());
    }

    #[test]
    fn credentials_blob_tolerates_alternate_key() {
        let raw = r#"{ "claude.ai_oauth": { "accessToken": "tok" } }"#;
        let creds = parse_credentials_blob(raw).expect("alternate key should parse");
        assert_eq!(creds.access_token, "tok");
    }

    #[test]
    fn token_needs_refresh_logic() {
        let now_ms = chrono::Utc::now().timestamp_millis();
        // Already expired -> needs refresh.
        assert!(token_needs_refresh(&ClaudeOAuth {
            access_token: "a".into(),
            refresh_token: Some("r".into()),
            expires_at_ms: Some(now_ms - 1000),
        }));
        // Far in the future -> no refresh.
        assert!(!token_needs_refresh(&ClaudeOAuth {
            access_token: "a".into(),
            refresh_token: Some("r".into()),
            expires_at_ms: Some(now_ms + 3_600_000),
        }));
        // Unknown expiry -> no proactive refresh.
        assert!(!token_needs_refresh(&ClaudeOAuth {
            access_token: "a".into(),
            refresh_token: Some("r".into()),
            expires_at_ms: None,
        }));
    }
}
