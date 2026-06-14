//! Source-bound quota queries.
//!
//! This module executes configured source quota bindings through a unified
//! profile + credential-resolution pipeline. Known relay providers and manual
//! source bindings share the same normalization model, while preserving
//! backward compatibility for legacy `generic_balance` / `new_api` settings.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::models::{
    ApiSource, DetectionConfidence, QuotaKind, SourceCredentialStrategy, SourceQueryProfileId,
    SourceQuotaBindingConfig, SubscriptionQuota,
};
use crate::net::HttpClientFactory;
use crate::subscription::query_profiles::{
    detect_builtin_profile, probe_candidate_profiles, probe_kind, profile_slug,
    SourceQuotaProbeKind,
};
use crate::subscription::relay::{detect_relay_provider, fetch_relay_quota_for_provider};
use crate::subscription::source_quota_executor::{
    execute_profile, ResolvedSourceCredential, SourceQuotaExecutionContext,
};
use crate::subscription::source_quota_util::{
    first_f64, make_source_config_error, normalize_base_url, value_to_f64,
};
use crate::subscription::source_resolver::{
    find_resolved_source_for_base_url, ResolvedRelaySource,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceQuotaBindingRuntimeState {
    pub source_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_profile_id: Option<SourceQueryProfileId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detection_confidence: Option<DetectionConfidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_probe_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_probe_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_verified_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_test_success: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_test_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_test_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tested_profile_id: Option<SourceQueryProfileId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tested_strategy: Option<SourceCredentialStrategy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_tool: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceQuotaProfileRecommendation {
    pub profile_id: SourceQueryProfileId,
    pub confidence: DetectionConfidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceQuotaBindingTestResult {
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempted_profile_id: Option<SourceQueryProfileId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_profile_id: Option<SourceQueryProfileId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_strategy: Option<SourceCredentialStrategy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<SubscriptionQuota>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn runtime_state_with_builtin_recommendation(source: &ApiSource) -> SourceQuotaBindingRuntimeState {
    let recommendation = recommend_query_profile(source);
    SourceQuotaBindingRuntimeState {
        source_id: source.id.clone(),
        recommended_profile_id: recommendation.as_ref().map(|r| r.profile_id),
        detection_confidence: recommendation.as_ref().map(|r| r.confidence),
        last_probe_at: None,
        last_probe_error: None,
        last_verified_at: None,
        last_test_success: None,
        last_test_summary: None,
        last_test_error: None,
        last_tested_profile_id: None,
        last_tested_strategy: None,
        source_tool: None,
    }
}

pub fn merged_runtime_state(
    source: &ApiSource,
    state: Option<&SourceQuotaBindingRuntimeState>,
) -> SourceQuotaBindingRuntimeState {
    let mut merged = runtime_state_with_builtin_recommendation(source);
    if let Some(state) = state {
        if state.recommended_profile_id.is_some() {
            merged.recommended_profile_id = state.recommended_profile_id;
        }
        if state.detection_confidence.is_some() {
            merged.detection_confidence = state.detection_confidence;
        }
        merged.last_probe_at = state.last_probe_at;
        merged.last_probe_error = state.last_probe_error.clone();
        merged.last_verified_at = state.last_verified_at;
        merged.last_test_success = state.last_test_success;
        merged.last_test_summary = state.last_test_summary.clone();
        merged.last_test_error = state.last_test_error.clone();
        merged.last_tested_profile_id = state.last_tested_profile_id;
        merged.last_tested_strategy = state.last_tested_strategy;
        merged.source_tool = state.source_tool.clone();
    }
    merged
}

fn display_name_from_base_url(base_url: &str) -> String {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| base_url.to_string())
}

fn synthetic_live_source(
    resolved_source: &ResolvedRelaySource,
    matched_source: Option<&ApiSource>,
) -> ApiSource {
    ApiSource {
        id: matched_source
            .map(|source| source.id.clone())
            .unwrap_or_else(|| format!("live:{}", normalize_base_url(&resolved_source.base_url))),
        display_name: Some(
            matched_source
                .and_then(|source| source.display_name.clone())
                .unwrap_or_else(|| display_name_from_base_url(&resolved_source.base_url)),
        ),
        base_url: Some(resolved_source.base_url.clone()),
        api_key_prefixes: Vec::new(),
        api_key_notes: HashMap::new(),
        color: matched_source
            .map(|source| source.color.clone())
            .unwrap_or_else(|| "#22c55e".to_string()),
        icon: matched_source.and_then(|source| source.icon.clone()),
        auto_detected: true,
        quota_query: None,
        first_seen_ms: matched_source
            .map(|source| source.first_seen_ms)
            .unwrap_or(0),
        last_seen_ms: matched_source
            .map(|source| source.last_seen_ms)
            .unwrap_or(0),
    }
}

fn automatic_binding_for_profile(profile_id: SourceQueryProfileId) -> SourceQuotaBindingConfig {
    SourceQuotaBindingConfig {
        enabled: true,
        query_profile_id: profile_id,
        credential_strategy: SourceQuotaBindingConfig::default_credential_strategy_for(profile_id),
        manual_api_key: None,
        manual_access_token: None,
        manual_user_id: None,
    }
}

pub fn recommend_query_profile(source: &ApiSource) -> Option<SourceQuotaProfileRecommendation> {
    let base_url = source.base_url.as_deref()?.trim();
    Some(SourceQuotaProfileRecommendation {
        profile_id: detect_builtin_profile(base_url)?,
        confidence: DetectionConfidence::High,
    })
}

fn manual_api_key_from_source(source: &ApiSource) -> Option<String> {
    source
        .api_key_notes
        .get("__quota_api_key")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn resolve_probe_api_key_credential(
    source: &ApiSource,
    binding: Option<&SourceQuotaBindingConfig>,
    resolved_sources: &[ResolvedRelaySource],
) -> Option<ResolvedSourceCredential> {
    if let Some(live_source) = source
        .base_url
        .as_deref()
        .and_then(|base_url| find_resolved_source_for_base_url(resolved_sources, base_url))
    {
        return Some(ResolvedSourceCredential {
            secret: live_source.api_key.clone(),
            user_id: None,
            source_tool: Some(live_source.tool.id().to_string()),
        });
    }

    let manual_api_key = binding
        .and_then(|binding| binding.manual_api_key.clone())
        .or_else(|| manual_api_key_from_source(source));

    manual_api_key.map(|secret| ResolvedSourceCredential {
        secret,
        user_id: None,
        source_tool: None,
    })
}

fn resolve_probe_new_api_credential(
    binding: Option<&SourceQuotaBindingConfig>,
) -> Option<ResolvedSourceCredential> {
    let binding = binding?;
    let secret = binding.manual_access_token.clone()?;
    let user_id = binding.manual_user_id.clone()?;
    Some(ResolvedSourceCredential {
        secret,
        user_id: Some(user_id),
        source_tool: None,
    })
}

fn resolve_probe_credential_for_profile(
    profile_id: SourceQueryProfileId,
    source: &ApiSource,
    binding: Option<&SourceQuotaBindingConfig>,
    resolved_sources: &[ResolvedRelaySource],
) -> Option<ResolvedSourceCredential> {
    match profile_id {
        SourceQueryProfileId::GenericBalanceV1Usage => {
            resolve_probe_api_key_credential(source, binding, resolved_sources)
        }
        SourceQueryProfileId::NewApiUserSelf => resolve_probe_new_api_credential(binding),
        _ => None,
    }
}

async fn probe_profile(
    profile_id: SourceQueryProfileId,
    base_url: &str,
    credential: &ResolvedSourceCredential,
) -> Result<(), String> {
    match probe_kind(profile_id) {
        Some(SourceQuotaProbeKind::GenericBalanceV1Usage) => {
            probe_generic_balance_profile(base_url, credential).await
        }
        Some(SourceQuotaProbeKind::NewApiUserSelf) => {
            probe_new_api_profile(base_url, credential).await
        }
        _ => Err(format!(
            "Unsupported probe profile: {}",
            profile_slug(profile_id)
        )),
    }
}

async fn probe_generic_balance_profile(
    base_url: &str,
    credential: &ResolvedSourceCredential,
) -> Result<(), String> {
    let url = format!("{}/v1/usage", normalize_base_url(base_url));
    let client = HttpClientFactory::global().standard();
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", credential.secret))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("generic probe network error: {e}"))?;

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("generic probe HTTP {status}: {text}"));
    }

    let body: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("generic probe parse error: {e}"))?;
    let remaining = first_f64(
        &body,
        &[&["remaining"], &["quota", "remaining"], &["balance"]],
    );
    if remaining.is_some() {
        Ok(())
    } else {
        Err("generic probe response missing remaining/balance field".to_string())
    }
}

async fn probe_new_api_profile(
    base_url: &str,
    credential: &ResolvedSourceCredential,
) -> Result<(), String> {
    let Some(user_id) = credential.user_id.as_deref() else {
        return Err("new-api probe missing user ID".to_string());
    };

    let url = format!("{}/api/user/self", normalize_base_url(base_url));
    let client = HttpClientFactory::global().standard();
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", credential.secret))
        .header("New-Api-User", user_id)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("new-api probe network error: {e}"))?;

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("new-api probe HTTP {status}: {text}"));
    }

    let body: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("new-api probe parse error: {e}"))?;
    let success = body.get("success").and_then(|v| v.as_bool()) == Some(true);
    let has_quota = body.pointer("/data/quota").and_then(value_to_f64).is_some()
        || body
            .pointer("/data/used_quota")
            .and_then(value_to_f64)
            .is_some();

    if success && has_quota {
        Ok(())
    } else {
        Err("new-api probe response does not match expected shape".to_string())
    }
}

pub async fn probe_source_quota_binding_state(
    source: &ApiSource,
    binding: Option<&SourceQuotaBindingConfig>,
    resolved_sources: &[ResolvedRelaySource],
) -> SourceQuotaBindingRuntimeState {
    let mut state = runtime_state_with_builtin_recommendation(source);
    let now = chrono::Utc::now().timestamp_millis();
    state.last_probe_at = Some(now);

    if state.recommended_profile_id.is_some() {
        state.last_probe_error = None;
        return state;
    }

    let Some(base_url) = source
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        state.last_probe_error = Some("Missing source base URL".to_string());
        return state;
    };

    let mut errors = Vec::new();

    for profile_id in probe_candidate_profiles() {
        if let Some(credential) =
            resolve_probe_credential_for_profile(*profile_id, source, binding, resolved_sources)
        {
            match probe_profile(*profile_id, base_url, &credential).await {
                Ok(()) => {
                    state.recommended_profile_id = Some(*profile_id);
                    state.detection_confidence = Some(DetectionConfidence::High);
                    state.source_tool = credential.source_tool.clone();
                    state.last_probe_error = None;
                    return state;
                }
                Err(err) => errors.push(err),
            }
        } else {
            errors.push(format!(
                "{} probe missing credential",
                profile_slug(*profile_id)
            ));
        }
    }

    state.last_probe_error = Some(errors.join(" | "));
    state
}

fn resolve_api_key_credential(
    source: &ApiSource,
    binding: &SourceQuotaBindingConfig,
    resolved_sources: &[ResolvedRelaySource],
) -> Result<ResolvedSourceCredential, String> {
    let live_source = source
        .base_url
        .as_deref()
        .and_then(|base_url| find_resolved_source_for_base_url(resolved_sources, base_url));

    match binding.credential_strategy {
        SourceCredentialStrategy::ToolLiveApiKey => {
            let Some(live_source) = live_source else {
                return Err("Missing live tool API key".to_string());
            };
            Ok(ResolvedSourceCredential {
                secret: live_source.api_key.clone(),
                user_id: None,
                source_tool: Some(live_source.tool.id().to_string()),
            })
        }
        SourceCredentialStrategy::ManualApiKey => {
            let Some(secret) = binding.manual_api_key.as_ref() else {
                return Err("Missing manual API key".to_string());
            };
            Ok(ResolvedSourceCredential {
                secret: secret.clone(),
                user_id: None,
                source_tool: None,
            })
        }
        SourceCredentialStrategy::ToolLiveApiKeyThenManualApiKey => {
            if let Some(live_source) = live_source {
                return Ok(ResolvedSourceCredential {
                    secret: live_source.api_key.clone(),
                    user_id: None,
                    source_tool: Some(live_source.tool.id().to_string()),
                });
            }
            let Some(secret) = binding.manual_api_key.as_ref() else {
                return Err("Missing API key: no live key and no manual fallback".to_string());
            };
            Ok(ResolvedSourceCredential {
                secret: secret.clone(),
                user_id: None,
                source_tool: None,
            })
        }
        SourceCredentialStrategy::ManualAccessTokenUserId => {
            let Some(secret) = binding.manual_access_token.as_ref() else {
                return Err("Missing access token".to_string());
            };
            let Some(user_id) = binding.manual_user_id.as_ref() else {
                return Err("Missing user ID".to_string());
            };
            Ok(ResolvedSourceCredential {
                secret: secret.clone(),
                user_id: Some(user_id.clone()),
                source_tool: None,
            })
        }
    }
}

fn resolve_source_credential(
    source: &ApiSource,
    binding: &SourceQuotaBindingConfig,
    resolved_sources: &[ResolvedRelaySource],
) -> Result<ResolvedSourceCredential, String> {
    match binding.query_profile_id {
        SourceQueryProfileId::NewApiUserSelf => match binding.credential_strategy {
            SourceCredentialStrategy::ManualAccessTokenUserId => {
                resolve_api_key_credential(source, binding, resolved_sources)
            }
            _ => Err("New API profile requires access token + user ID".to_string()),
        },
        _ => resolve_api_key_credential(source, binding, resolved_sources),
    }
}

pub async fn execute_source_quota_binding(
    source: &ApiSource,
    binding: &SourceQuotaBindingConfig,
    resolved_sources: &[ResolvedRelaySource],
) -> SubscriptionQuota {
    let Some(base_url) = source
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return make_source_config_error(
            source,
            None,
            profile_slug(binding.query_profile_id),
            "Missing source base URL".to_string(),
        );
    };

    let credential = match resolve_source_credential(source, binding, resolved_sources) {
        Ok(credential) => credential,
        Err(err) => {
            return make_source_config_error(
                source,
                None,
                profile_slug(binding.query_profile_id),
                err,
            );
        }
    };

    execute_profile(
        binding.query_profile_id,
        SourceQuotaExecutionContext {
            source,
            base_url,
            credential: &credential,
        },
    )
    .await
}

pub async fn fetch_source_quota(
    source: &ApiSource,
    resolved_sources: &[ResolvedRelaySource],
) -> SubscriptionQuota {
    let Some(binding) = source.quota_query.as_ref().filter(|cfg| cfg.enabled) else {
        return make_source_config_error(
            source,
            None,
            "source-config",
            "Quota query is disabled".to_string(),
        );
    };

    execute_source_quota_binding(source, binding, resolved_sources).await
}

pub async fn fetch_auto_source_quota_for_resolved_source(
    resolved_source: &ResolvedRelaySource,
    matched_source: Option<&ApiSource>,
) -> Option<SubscriptionQuota> {
    if let Some(provider) = detect_relay_provider(&resolved_source.base_url) {
        let mut quota = fetch_relay_quota_for_provider(
            provider,
            &resolved_source.base_url,
            &resolved_source.api_key,
        )
        .await;
        quota.source_tool = Some(resolved_source.tool.id().to_string());
        if quota.credential_message.is_none() {
            quota.credential_message =
                matched_source.and_then(|source| source.display_name.clone());
        }
        if quota.account_label.is_none() {
            quota.account_label = matched_source.and_then(|source| source.display_name.clone());
        }
        return quota.success.then_some(quota);
    }

    let source = synthetic_live_source(resolved_source, matched_source);
    let resolved_sources = std::slice::from_ref(resolved_source);
    let runtime_state = probe_source_quota_binding_state(&source, None, resolved_sources).await;
    let profile_id = runtime_state.recommended_profile_id?;
    let binding = automatic_binding_for_profile(profile_id);
    let quota = execute_source_quota_binding(&source, &binding, resolved_sources).await;
    quota.success.then_some(quota)
}

fn build_test_summary(quota: &SubscriptionQuota) -> Option<String> {
    let balance_tier = quota
        .tiers
        .iter()
        .find(|tier| tier.kind == QuotaKind::Balance);
    if let Some(tier) = balance_tier {
        let value = tier.remaining_value?;
        let currency = tier.currency.as_deref().unwrap_or("");
        return Some(format!("{currency} {:.2}", value).trim().to_string());
    }

    let window_tier = quota.tiers.first()?;
    // Keep this summary compact and locale-neutral because it may be surfaced
    // by runtime state consumers outside the current settings UI.
    Some(format!(
        "{} {:.0}%",
        window_tier.name,
        (100.0 - window_tier.utilization).clamp(0.0, 100.0)
    ))
}

pub async fn test_source_quota_binding(
    source: &ApiSource,
    binding: &SourceQuotaBindingConfig,
    resolved_sources: &[ResolvedRelaySource],
) -> SourceQuotaBindingTestResult {
    let recommendation = recommend_query_profile(source);
    let quota = execute_source_quota_binding(source, binding, resolved_sources).await;
    let attempted_profile_id = Some(binding.query_profile_id);
    let credential_strategy = Some(binding.credential_strategy);
    let source_tool = quota.source_tool.clone();

    if quota.success {
        SourceQuotaBindingTestResult {
            success: true,
            attempted_profile_id,
            recommended_profile_id: recommendation.as_ref().map(|r| r.profile_id),
            credential_strategy,
            source_tool,
            summary: build_test_summary(&quota),
            quota: Some(quota),
            error: None,
        }
    } else {
        SourceQuotaBindingTestResult {
            success: false,
            attempted_profile_id,
            recommended_profile_id: recommendation.as_ref().map(|r| r.profile_id),
            credential_strategy,
            source_tool,
            summary: None,
            quota: None,
            error: quota.error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn source_with_binding(binding: SourceQuotaBindingConfig) -> ApiSource {
        ApiSource {
            id: "src_demo".to_string(),
            display_name: Some("Demo".to_string()),
            base_url: Some("https://api.deepseek.com".to_string()),
            api_key_prefixes: vec!["sk-demo".to_string()],
            api_key_notes: HashMap::new(),
            color: "#000000".to_string(),
            icon: None,
            auto_detected: false,
            quota_query: Some(binding),
            first_seen_ms: 0,
            last_seen_ms: 0,
        }
    }

    fn source_with_base_url(base_url: &str) -> ApiSource {
        ApiSource {
            id: "src_demo".to_string(),
            display_name: Some("Demo".to_string()),
            base_url: Some(base_url.to_string()),
            api_key_prefixes: vec!["sk-demo".to_string()],
            api_key_notes: HashMap::new(),
            color: "#000000".to_string(),
            icon: None,
            auto_detected: false,
            quota_query: None,
            first_seen_ms: 0,
            last_seen_ms: 0,
        }
    }

    #[test]
    fn recommends_builtin_profile_from_base_url() {
        let source = source_with_binding(SourceQuotaBindingConfig {
            enabled: true,
            query_profile_id: SourceQueryProfileId::OfficialDeepSeekBalance,
            credential_strategy: SourceCredentialStrategy::ToolLiveApiKeyThenManualApiKey,
            manual_api_key: None,
            manual_access_token: None,
            manual_user_id: None,
        });
        let recommendation = recommend_query_profile(&source).expect("recommendation");
        assert_eq!(
            recommendation.profile_id,
            SourceQueryProfileId::OfficialDeepSeekBalance
        );
        assert_eq!(recommendation.confidence, DetectionConfidence::High);
    }

    #[test]
    fn resolves_live_api_key_before_manual_fallback() {
        let source = source_with_binding(SourceQuotaBindingConfig {
            enabled: true,
            query_profile_id: SourceQueryProfileId::OfficialDeepSeekBalance,
            credential_strategy: SourceCredentialStrategy::ToolLiveApiKeyThenManualApiKey,
            manual_api_key: Some("manual-key".to_string()),
            manual_access_token: None,
            manual_user_id: None,
        });
        let resolved_sources = vec![ResolvedRelaySource {
            tool: crate::subscription::source_resolver::ToolKind::Codex,
            base_url: "https://api.deepseek.com/".to_string(),
            api_key: "live-key".to_string(),
        }];
        let credential = resolve_source_credential(
            &source,
            source.quota_query.as_ref().unwrap(),
            &resolved_sources,
        )
        .expect("credential");
        assert_eq!(credential.secret, "live-key");
        assert_eq!(credential.source_tool.as_deref(), Some("codex"));
    }

    #[test]
    fn new_api_requires_manual_access_token_user_id() {
        let source = source_with_binding(SourceQuotaBindingConfig {
            enabled: true,
            query_profile_id: SourceQueryProfileId::NewApiUserSelf,
            credential_strategy: SourceCredentialStrategy::ToolLiveApiKeyThenManualApiKey,
            manual_api_key: None,
            manual_access_token: Some("token".to_string()),
            manual_user_id: Some("42".to_string()),
        });
        let err = resolve_source_credential(&source, source.quota_query.as_ref().unwrap(), &[])
            .expect_err("should reject invalid strategy");
        assert!(err.contains("requires access token"));
    }

    #[test]
    fn merged_runtime_state_preserves_builtin_recommendation() {
        let source = source_with_base_url("https://api.deepseek.com");
        let merged = merged_runtime_state(&source, None);
        assert_eq!(
            merged.recommended_profile_id,
            Some(SourceQueryProfileId::OfficialDeepSeekBalance)
        );
        assert_eq!(merged.detection_confidence, Some(DetectionConfidence::High));
    }

    #[tokio::test]
    async fn probe_unknown_source_without_credentials_returns_probe_error() {
        let source = source_with_base_url("https://relay.example.com/v1");
        let state = probe_source_quota_binding_state(&source, None, &[]).await;
        assert_eq!(state.recommended_profile_id, None);
        assert!(state.last_probe_error.is_some());
    }
}
