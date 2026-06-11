//! Gemini CLI subscription (Cloud Code Assist quota) implementation
//!
//! Reads `~/.gemini/oauth_creds.json`, refreshes the Google OAuth access token
//! in memory (never writing back to the user's credential file), then queries
//! the Cloud Code Assist internal endpoints (`loadCodeAssist` +
//! `retrieveUserQuota`) and aggregates the returned buckets into the
//! Pro / Flash / Flash Lite model families.

use std::path::PathBuf;
use std::sync::Arc;

use reqwest::Client;
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::models::{CredentialStatus, QuotaTier, SubscriptionQueryResult, SubscriptionQuota};
use crate::net::HttpClientFactory;

use super::types::SubscriptionError;

const PROVIDER_ID: &str = "gemini";

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const LOAD_CODE_ASSIST_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const RETRIEVE_QUOTA_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";

/// Public OAuth client used by the open-source Gemini CLI installed-app flow.
/// These are not user secrets — they ship inside the CLI binary. They are only
/// used as a fallback when `oauth_creds.json` does not embed its own client.
/// Both can be overridden via environment variables.
const DEFAULT_OAUTH_CLIENT_ID: &str =
    "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
const DEFAULT_OAUTH_CLIENT_SECRET: &str = "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";

const ENV_CLIENT_ID: &str = "USAGEMETER_GEMINI_OAUTH_CLIENT_ID";
const ENV_CLIENT_SECRET: &str = "USAGEMETER_GEMINI_OAUTH_CLIENT_SECRET";

/// Refresh the access token this many seconds before its declared expiry.
const REFRESH_THRESHOLD_SECS: i64 = 60;

/// Tier names emitted for the three Gemini model families.
const TIER_PRO: &str = "gemini_pro";
const TIER_FLASH: &str = "gemini_flash";
const TIER_FLASH_LITE: &str = "gemini_flash_lite";

/// OAuth credentials as stored by the Gemini CLI in `oauth_creds.json`.
#[derive(Debug, Clone, Deserialize, Default)]
struct GeminiOAuthCreds {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    /// Expiry in epoch milliseconds (Gemini CLI convention).
    #[serde(default)]
    expiry_date: Option<i64>,
    email: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
}

/// In-memory access-token cache; never persisted to disk.
#[derive(Clone, Default)]
struct GeminiTokenState {
    access_token: Option<String>,
    /// Expiry in epoch milliseconds.
    expires_at_ms: Option<i64>,
}

/// Gemini subscription/quota provider.
#[derive(Clone)]
pub struct GeminiSubscriptionProvider {
    client: Client,
    token_state: Arc<RwLock<GeminiTokenState>>,
}

impl Default for GeminiSubscriptionProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl GeminiSubscriptionProvider {
    pub fn new() -> Self {
        Self {
            client: HttpClientFactory::global().standard(),
            token_state: Arc::new(RwLock::new(GeminiTokenState::default())),
        }
    }

    /// Path to the Gemini CLI OAuth credential file.
    fn creds_path() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".gemini").join("oauth_creds.json")
    }

    /// Whether Gemini CLI OAuth credentials are present and usable.
    pub fn has_gemini_oauth(&self) -> bool {
        match Self::read_creds() {
            Ok(creds) => creds.refresh_token.is_some() || creds.access_token.is_some(),
            Err(_) => false,
        }
    }

    /// Read and parse `oauth_creds.json`.
    fn read_creds() -> Result<GeminiOAuthCreds, SubscriptionError> {
        let path = Self::creds_path();
        if !path.exists() {
            return Err(SubscriptionError::NoCredentials);
        }
        let content =
            std::fs::read_to_string(&path).map_err(|_e| SubscriptionError::NoCredentials)?;
        serde_json::from_str(&content).map_err(|e| SubscriptionError::ParseError {
            message: e.to_string(),
        })
    }

    /// Resolve the OAuth client id/secret, preferring file-embedded values, then
    /// environment overrides, then the public Gemini CLI defaults.
    fn resolve_oauth_client(creds: &GeminiOAuthCreds) -> (String, String) {
        let client_id = creds
            .client_id
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                std::env::var(ENV_CLIENT_ID)
                    .ok()
                    .filter(|s| !s.trim().is_empty())
            })
            .unwrap_or_else(|| DEFAULT_OAUTH_CLIENT_ID.to_string());
        let client_secret = creds
            .client_secret
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                std::env::var(ENV_CLIENT_SECRET)
                    .ok()
                    .filter(|s| !s.trim().is_empty())
            })
            .unwrap_or_else(|| DEFAULT_OAUTH_CLIENT_SECRET.to_string());
        (client_id, client_secret)
    }

    /// Return a valid access token, refreshing via the refresh token if the
    /// cached/file token is missing or about to expire.
    async fn ensure_access_token(
        &self,
        creds: &GeminiOAuthCreds,
    ) -> Result<String, SubscriptionError> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let threshold_ms = REFRESH_THRESHOLD_SECS * 1000;

        // 1) In-memory cached token still valid?
        {
            let state = self.token_state.read().await;
            if let (Some(token), Some(exp)) = (&state.access_token, state.expires_at_ms) {
                if now_ms < exp - threshold_ms {
                    return Ok(token.clone());
                }
            }
        }

        // 2) File-provided access token still valid?
        if let Some(token) = creds.access_token.as_ref().filter(|s| !s.is_empty()) {
            if let Some(exp) = creds.expiry_date {
                if now_ms < exp - threshold_ms {
                    let mut state = self.token_state.write().await;
                    state.access_token = Some(token.clone());
                    state.expires_at_ms = Some(exp);
                    return Ok(token.clone());
                }
            }
        }

        // 3) Refresh using the refresh token.
        let refresh_token = creds
            .refresh_token
            .as_ref()
            .filter(|s| !s.is_empty())
            .ok_or(SubscriptionError::NoRefreshToken)?;
        let (client_id, client_secret) = Self::resolve_oauth_client(creds);
        let (access_token, expires_at_ms) = self
            .refresh_access_token(refresh_token, &client_id, &client_secret)
            .await?;

        let mut state = self.token_state.write().await;
        state.access_token = Some(access_token.clone());
        state.expires_at_ms = Some(expires_at_ms);
        Ok(access_token)
    }

    /// Exchange a refresh token for a new access token. Returns
    /// `(access_token, expires_at_ms)`.
    async fn refresh_access_token(
        &self,
        refresh_token: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<(String, i64), SubscriptionError> {
        let params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
            ("client_secret", client_secret),
        ];

        let response = self
            .client
            .post(TOKEN_URL)
            .form(&params)
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

        let expires_in = data
            .get("expires_in")
            .and_then(|v| v.as_i64())
            .unwrap_or(3600);
        let expires_at_ms = chrono::Utc::now().timestamp_millis() + expires_in * 1000;

        Ok((access_token, expires_at_ms))
    }

    /// Call `loadCodeAssist` and return `(tier_id, project_id)` (both optional).
    async fn load_code_assist(
        &self,
        access_token: &str,
    ) -> Result<(Option<String>, Option<String>), SubscriptionError> {
        let body = serde_json::json!({
            "metadata": {
                "ideType": "IDE_UNSPECIFIED",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI"
            }
        });

        let response = self
            .client
            .post(LOAD_CODE_ASSIST_URL)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .json(&body)
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

        let data: serde_json::Value =
            response
                .json()
                .await
                .map_err(|e| SubscriptionError::ParseError {
                    message: e.to_string(),
                })?;

        let tier_id = data
            .pointer("/currentTier/id")
            .and_then(|v| v.as_str())
            .map(String::from);
        let project_id = data
            .get("cloudaicompanionProject")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(String::from);

        Ok((tier_id, project_id))
    }

    /// Call `retrieveUserQuota` and return the raw bucket list.
    async fn retrieve_quota(
        &self,
        access_token: &str,
        project_id: Option<&str>,
    ) -> Result<Vec<QuotaBucket>, SubscriptionError> {
        let body = match project_id {
            Some(p) => serde_json::json!({ "project": p }),
            None => serde_json::json!({}),
        };

        let response = self
            .client
            .post(RETRIEVE_QUOTA_URL)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .json(&body)
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

        let data: serde_json::Value =
            response
                .json()
                .await
                .map_err(|e| SubscriptionError::ParseError {
                    message: e.to_string(),
                })?;

        Ok(parse_buckets(&data))
    }

    /// Fetch the Gemini quota, performing the full credential → token → quota flow.
    pub async fn fetch_quota(&self) -> SubscriptionQueryResult {
        let creds = match Self::read_creds() {
            Ok(c) => c,
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

        let access_token = match self.ensure_access_token(&creds).await {
            Ok(t) => t,
            Err(e) => {
                let status = match &e {
                    SubscriptionError::NoRefreshToken | SubscriptionError::TokenExpired => {
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

        // loadCodeAssist is best-effort: a failure shouldn't block the quota read.
        let (tier_id, project_id) = match self.load_code_assist(&access_token).await {
            Ok(v) => v,
            Err(SubscriptionError::ApiError { status, message })
                if status == 401 || status == 403 =>
            {
                return SubscriptionQueryResult::error(
                    PROVIDER_ID,
                    CredentialStatus::Expired,
                    SubscriptionError::ApiError { status, message }.user_message(),
                );
            }
            Err(_) => (None, None),
        };

        let buckets = match self
            .retrieve_quota(&access_token, project_id.as_deref())
            .await
        {
            Ok(b) => b,
            Err(e) => {
                let status = match &e {
                    SubscriptionError::ApiError { status, .. }
                        if *status == 401 || *status == 403 =>
                    {
                        CredentialStatus::Expired
                    }
                    _ => CredentialStatus::QueryFailed {
                        error: e.user_message(),
                    },
                };
                return SubscriptionQueryResult::error(PROVIDER_ID, status, e.user_message());
            }
        };

        let tiers = build_tiers(&buckets);
        let plan_label = tier_id.as_deref().map(tier_id_to_label);
        let account_label = creds
            .email
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| creds.id_token.as_deref().and_then(extract_email_from_jwt))
            .or(project_id);

        let quota = SubscriptionQuota {
            provider: PROVIDER_ID.to_string(),
            tool: "gemini".to_string(),
            source_tool: None,
            credential_status: "valid".to_string(),
            credential_message: None,
            success: true,
            tiers,
            updated_at: chrono::Utc::now().timestamp_millis(),
            from_cache: false,
            error: None,
            plan_label,
            account_label,
        };

        SubscriptionQueryResult::success(quota)
    }
}

/// A normalized quota bucket from `retrieveUserQuota`.
#[derive(Debug, Clone)]
struct QuotaBucket {
    model_id: String,
    /// Fraction of quota remaining (0.0 - 1.0).
    remaining_fraction: f64,
    reset_time: Option<String>,
}

/// Read a string field tolerating both camelCase and snake_case spellings.
fn get_str<'a>(obj: &'a serde_json::Value, camel: &str, snake: &str) -> Option<&'a str> {
    obj.get(camel)
        .or_else(|| obj.get(snake))
        .and_then(|v| v.as_str())
}

/// Read an f64 field tolerating both camelCase and snake_case spellings.
fn get_f64(obj: &serde_json::Value, camel: &str, snake: &str) -> Option<f64> {
    obj.get(camel)
        .or_else(|| obj.get(snake))
        .and_then(|v| v.as_f64())
}

/// Parse the `buckets` array from a `retrieveUserQuota` response.
fn parse_buckets(data: &serde_json::Value) -> Vec<QuotaBucket> {
    let Some(arr) = data.get("buckets").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for item in arr {
        let Some(model_id_raw) = get_str(item, "modelId", "model_id") else {
            continue;
        };
        let model_id = model_id_raw.trim_end_matches("_vertex").to_string();

        let remaining_fraction = match get_f64(item, "remainingFraction", "remaining_fraction") {
            Some(f) => f.clamp(0.0, 1.0),
            None => {
                // Fall back to remaining amount: <= 0 means exhausted, otherwise skip.
                match get_f64(item, "remainingAmount", "remaining_amount") {
                    Some(amt) if amt <= 0.0 => 0.0,
                    _ => continue,
                }
            }
        };

        let reset_time = get_str(item, "resetTime", "reset_time").map(String::from);

        out.push(QuotaBucket {
            model_id,
            remaining_fraction,
            reset_time,
        });
    }
    out
}

/// Classify a model id into one of the three Gemini families.
/// Returns `None` for ignored families (e.g. legacy `gemini-2.0-flash`).
fn classify_family(model_id: &str) -> Option<&'static str> {
    let id = model_id.to_lowercase();
    if id.contains("2.0") {
        return None;
    }
    if id.contains("flash-lite") || id.contains("flash_lite") {
        Some(TIER_FLASH_LITE)
    } else if id.contains("flash") {
        Some(TIER_FLASH)
    } else if id.contains("pro") {
        Some(TIER_PRO)
    } else {
        None
    }
}

/// Aggregate buckets into Pro / Flash / Flash Lite tiers, keeping the most
/// constrained (lowest remaining) bucket per family.
fn build_tiers(buckets: &[QuotaBucket]) -> Vec<QuotaTier> {
    // (lowest_remaining_fraction, reset_time)
    let mut pro: Option<(f64, Option<String>)> = None;
    let mut flash: Option<(f64, Option<String>)> = None;
    let mut flash_lite: Option<(f64, Option<String>)> = None;

    for bucket in buckets {
        let Some(family) = classify_family(&bucket.model_id) else {
            continue;
        };
        let slot = match family {
            TIER_PRO => &mut pro,
            TIER_FLASH => &mut flash,
            TIER_FLASH_LITE => &mut flash_lite,
            _ => continue,
        };
        match slot {
            Some((frac, _)) if *frac <= bucket.remaining_fraction => {}
            _ => *slot = Some((bucket.remaining_fraction, bucket.reset_time.clone())),
        }
    }

    let mut tiers = Vec::new();
    for (name, slot) in [
        (TIER_PRO, pro),
        (TIER_FLASH, flash),
        (TIER_FLASH_LITE, flash_lite),
    ] {
        if let Some((frac, reset_time)) = slot {
            tiers.push(QuotaTier {
                name: name.to_string(),
                kind: crate::models::QuotaKind::Window,
                utilization: ((1.0 - frac) * 100.0).clamp(0.0, 100.0),
                resets_at: reset_time,
                ..Default::default()
            });
        }
    }
    tiers
}

/// Map a Cloud Code Assist tier id into a human-friendly plan label.
fn tier_id_to_label(tier_id: &str) -> String {
    let normalized = tier_id.trim().to_lowercase();
    let base = normalized.strip_suffix("-tier").unwrap_or(&normalized);
    match base {
        "free" => "Free".to_string(),
        "legacy" => "Legacy".to_string(),
        "standard" => "Standard".to_string(),
        "enterprise" => "Enterprise".to_string(),
        "g1-pro" => "Pro".to_string(),
        "g1-ultra" => "Ultra".to_string(),
        other if !other.is_empty() => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => other.to_string(),
            }
        }
        _ => tier_id.to_string(),
    }
}

/// Extract the `email` claim from a JWT id token.
fn extract_email_from_jwt(token: &str) -> Option<String> {
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
    json.get("email")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_family_matches_expected() {
        assert_eq!(classify_family("gemini-2.5-pro"), Some(TIER_PRO));
        assert_eq!(classify_family("gemini-3-pro-preview"), Some(TIER_PRO));
        assert_eq!(classify_family("gemini-2.5-flash"), Some(TIER_FLASH));
        assert_eq!(
            classify_family("gemini-2.5-flash-lite"),
            Some(TIER_FLASH_LITE)
        );
        // legacy 2.0 family ignored
        assert_eq!(classify_family("gemini-2.0-flash"), None);
        assert_eq!(classify_family("text-embedding-004"), None);
    }

    #[test]
    fn parse_buckets_handles_both_casings_and_vertex_suffix() {
        let data = serde_json::json!({
            "buckets": [
                { "modelId": "gemini-2.5-pro_vertex", "remainingFraction": 0.4, "resetTime": "2026-06-08T00:00:00Z" },
                { "model_id": "gemini-2.5-flash", "remaining_fraction": 0.9 },
                { "modelId": "gemini-2.5-flash-lite", "remainingAmount": 0 }
            ]
        });
        let buckets = parse_buckets(&data);
        assert_eq!(buckets.len(), 3);
        assert_eq!(buckets[0].model_id, "gemini-2.5-pro");
        assert!((buckets[0].remaining_fraction - 0.4).abs() < 1e-9);
        assert!((buckets[2].remaining_fraction - 0.0).abs() < 1e-9);
    }

    #[test]
    fn build_tiers_keeps_lowest_remaining_per_family() {
        let buckets = vec![
            QuotaBucket {
                model_id: "gemini-2.5-pro".into(),
                remaining_fraction: 0.8,
                reset_time: Some("a".into()),
            },
            QuotaBucket {
                model_id: "gemini-3-pro-preview".into(),
                remaining_fraction: 0.3,
                reset_time: Some("b".into()),
            },
            QuotaBucket {
                model_id: "gemini-2.5-flash".into(),
                remaining_fraction: 0.5,
                reset_time: None,
            },
        ];
        let tiers = build_tiers(&buckets);
        let pro = tiers.iter().find(|t| t.name == TIER_PRO).unwrap();
        // lowest remaining (0.3) → utilization 70%
        assert!((pro.utilization - 70.0).abs() < 1e-6);
        assert_eq!(pro.resets_at.as_deref(), Some("b"));
        let flash = tiers.iter().find(|t| t.name == TIER_FLASH).unwrap();
        assert!((flash.utilization - 50.0).abs() < 1e-6);
        // no flash-lite bucket → no tier
        assert!(tiers.iter().all(|t| t.name != TIER_FLASH_LITE));
    }

    #[test]
    fn tier_id_to_label_maps_known_tiers() {
        assert_eq!(tier_id_to_label("free-tier"), "Free");
        assert_eq!(tier_id_to_label("g1-pro-tier"), "Pro");
        assert_eq!(tier_id_to_label("standard-tier"), "Standard");
        assert_eq!(tier_id_to_label("custom"), "Custom");
    }

    #[test]
    fn resolve_oauth_client_prefers_file_values() {
        let creds = GeminiOAuthCreds {
            client_id: Some("file-id".into()),
            client_secret: Some("file-secret".into()),
            ..Default::default()
        };
        let (id, secret) = GeminiSubscriptionProvider::resolve_oauth_client(&creds);
        assert_eq!(id, "file-id");
        assert_eq!(secret, "file-secret");
    }

    #[test]
    fn resolve_oauth_client_falls_back_to_defaults() {
        let creds = GeminiOAuthCreds::default();
        let (id, secret) = GeminiSubscriptionProvider::resolve_oauth_client(&creds);
        // env may override in some shells; only assert non-empty defaults present
        assert!(!id.is_empty());
        assert!(!secret.is_empty());
    }
}
