use crate::models::{ApiSource, QuotaKind, QuotaTier, SourceQueryProfileId, SubscriptionQuota};
use crate::net::HttpClientFactory;
use crate::subscription::query_profiles::{
    executor_kind, profile_id_for_relay_provider, profile_slug, relay_providers_for_profile,
    SourceQuotaExecutorKind,
};
use crate::subscription::relay::{detect_relay_provider, fetch_relay_quota_for_provider};
use crate::subscription::source_quota_util::{
    first_bool, first_f64, first_string, make_source_config_error, normalize_base_url, value_to_f64,
};

#[derive(Debug, Clone)]
pub struct ResolvedSourceCredential {
    pub secret: String,
    pub user_id: Option<String>,
    pub source_tool: Option<String>,
}

pub struct SourceQuotaExecutionContext<'a> {
    pub source: &'a ApiSource,
    pub base_url: &'a str,
    pub credential: &'a ResolvedSourceCredential,
}

fn make_quota(
    source: &ApiSource,
    source_tool: Option<String>,
    tool: &str,
    tiers: Vec<QuotaTier>,
    plan_name: Option<String>,
    credential_message: Option<String>,
) -> SubscriptionQuota {
    SubscriptionQuota {
        provider: "source-config".to_string(),
        tool: tool.to_string(),
        source_tool,
        credential_status: "valid".to_string(),
        credential_message,
        success: true,
        tiers,
        updated_at: chrono::Utc::now().timestamp_millis(),
        from_cache: false,
        error: None,
        plan_label: plan_name,
        account_label: source.display_name.clone(),
    }
}

const NEW_API_CREDITS_PER_USD: f64 = 500_000.0;

fn build_new_api_quota(
    source: &ApiSource,
    source_tool: Option<String>,
    response: &serde_json::Value,
) -> SubscriptionQuota {
    let Some(data) = response.get("data") else {
        let msg = response
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Missing data field in new-api response")
            .to_string();
        return make_source_config_error(
            source,
            source_tool,
            profile_slug(SourceQueryProfileId::NewApiUserSelf),
            msg,
        );
    };

    // New API 内部计量单位：500,000 credits = $1.00 USD。
    let quota = data.get("quota").and_then(value_to_f64).unwrap_or(0.0) / NEW_API_CREDITS_PER_USD;
    let used =
        data.get("used_quota").and_then(value_to_f64).unwrap_or(0.0) / NEW_API_CREDITS_PER_USD;
    let total = quota + used;
    let remaining = quota.max(0.0);
    let utilization = if total > 0.0 {
        (used / total) * 100.0
    } else {
        0.0
    };
    let plan_name = data
        .get("group")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let tier = QuotaTier {
        name: plan_name.clone().unwrap_or_else(|| "new-api".to_string()),
        kind: QuotaKind::Balance,
        utilization,
        resets_at: None,
        remaining_value: Some(remaining),
        max_value: Some(total),
        currency: Some("USD".to_string()),
        limit_reached: Some(remaining <= 0.0),
    };

    make_quota(
        source,
        source_tool,
        profile_slug(SourceQueryProfileId::NewApiUserSelf),
        vec![tier],
        plan_name,
        source.display_name.clone(),
    )
}

fn build_generic_balance_quota(
    source: &ApiSource,
    source_tool: Option<String>,
    body: &serde_json::Value,
) -> SubscriptionQuota {
    let remaining = first_f64(
        body,
        &[&["remaining"], &["quota", "remaining"], &["balance"]],
    );
    let max_value = first_f64(
        body,
        &[&["total"], &["quota", "total"], &["quota", "limit"]],
    );
    let unit =
        first_string(body, &[&["unit"], &["quota", "unit"]]).unwrap_or_else(|| "USD".to_string());
    let is_valid = first_bool(body, &[&["is_active"], &["isValid"]]).unwrap_or(true);

    let Some(remaining) = remaining else {
        return make_source_config_error(
            source,
            source_tool,
            profile_slug(SourceQueryProfileId::GenericBalanceV1Usage),
            "Missing remaining/balance field in generic usage response".to_string(),
        );
    };

    let tier = QuotaTier {
        name: unit.clone(),
        kind: QuotaKind::Balance,
        utilization: 0.0,
        resets_at: None,
        remaining_value: Some(remaining),
        max_value,
        currency: Some(unit),
        limit_reached: Some(!is_valid || remaining <= 0.0),
    };

    make_quota(
        source,
        source_tool,
        profile_slug(SourceQueryProfileId::GenericBalanceV1Usage),
        vec![tier],
        source.display_name.clone(),
        source.display_name.clone(),
    )
}

async fn execute_new_api(ctx: SourceQuotaExecutionContext<'_>) -> SubscriptionQuota {
    let Some(user_id) = ctx.credential.user_id.as_deref() else {
        return make_source_config_error(
            ctx.source,
            ctx.credential.source_tool.clone(),
            profile_slug(SourceQueryProfileId::NewApiUserSelf),
            "Missing new-api user id".to_string(),
        );
    };

    let url = format!("{}/api/user/self", normalize_base_url(ctx.base_url));
    let client = HttpClientFactory::global().standard();
    let response = match client
        .get(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", ctx.credential.secret))
        .header("New-Api-User", user_id)
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return make_source_config_error(
                ctx.source,
                ctx.credential.source_tool.clone(),
                profile_slug(SourceQueryProfileId::NewApiUserSelf),
                format!("Network error: {e}"),
            );
        }
    };

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return make_source_config_error(
            ctx.source,
            ctx.credential.source_tool.clone(),
            profile_slug(SourceQueryProfileId::NewApiUserSelf),
            format!("HTTP {status}: {text}"),
        );
    }

    let body: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return make_source_config_error(
                ctx.source,
                ctx.credential.source_tool.clone(),
                profile_slug(SourceQueryProfileId::NewApiUserSelf),
                format!("Parse error: {e}"),
            );
        }
    };

    if body.get("success").and_then(|v| v.as_bool()) != Some(true) {
        let msg = body
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("new-api query failed")
            .to_string();
        return make_source_config_error(
            ctx.source,
            ctx.credential.source_tool.clone(),
            profile_slug(SourceQueryProfileId::NewApiUserSelf),
            msg,
        );
    }

    build_new_api_quota(ctx.source, ctx.credential.source_tool.clone(), &body)
}

async fn execute_generic_balance(ctx: SourceQuotaExecutionContext<'_>) -> SubscriptionQuota {
    let url = format!("{}/v1/usage", normalize_base_url(ctx.base_url));
    let client = HttpClientFactory::global().standard();
    let response = match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", ctx.credential.secret))
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return make_source_config_error(
                ctx.source,
                ctx.credential.source_tool.clone(),
                profile_slug(SourceQueryProfileId::GenericBalanceV1Usage),
                format!("Network error: {e}"),
            );
        }
    };

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return make_source_config_error(
            ctx.source,
            ctx.credential.source_tool.clone(),
            profile_slug(SourceQueryProfileId::GenericBalanceV1Usage),
            format!("HTTP {status}: {text}"),
        );
    }

    let body: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return make_source_config_error(
                ctx.source,
                ctx.credential.source_tool.clone(),
                profile_slug(SourceQueryProfileId::GenericBalanceV1Usage),
                format!("Parse error: {e}"),
            );
        }
    };

    build_generic_balance_quota(ctx.source, ctx.credential.source_tool.clone(), &body)
}

async fn execute_relay(
    profile_id: SourceQueryProfileId,
    ctx: SourceQuotaExecutionContext<'_>,
) -> SubscriptionQuota {
    if relay_providers_for_profile(profile_id).is_empty() {
        return make_source_config_error(
            ctx.source,
            ctx.credential.source_tool.clone(),
            profile_slug(profile_id),
            "Profile does not map to relay provider".to_string(),
        );
    }

    let actual_provider = detect_relay_provider(ctx.base_url);
    let Some(actual_provider) = actual_provider else {
        return make_source_config_error(
            ctx.source,
            ctx.credential.source_tool.clone(),
            profile_slug(profile_id),
            format!("Unsupported relay base_url: {}", ctx.base_url),
        );
    };

    if profile_id_for_relay_provider(actual_provider) != Some(profile_id) {
        return make_source_config_error(
            ctx.source,
            ctx.credential.source_tool.clone(),
            profile_slug(profile_id),
            "Bound profile does not match detected provider".to_string(),
        );
    }

    let mut quota =
        fetch_relay_quota_for_provider(actual_provider, ctx.base_url, &ctx.credential.secret).await;
    quota.provider = "source-config".to_string();
    quota.source_tool = ctx.credential.source_tool.clone();
    quota.account_label = ctx.source.display_name.clone();
    if quota.credential_message.is_none() {
        quota.credential_message = ctx.source.display_name.clone();
    }
    quota
}

pub async fn execute_profile(
    profile_id: SourceQueryProfileId,
    ctx: SourceQuotaExecutionContext<'_>,
) -> SubscriptionQuota {
    match executor_kind(profile_id) {
        Some(SourceQuotaExecutorKind::GenericBalanceV1Usage) => execute_generic_balance(ctx).await,
        Some(SourceQuotaExecutorKind::NewApiUserSelf) => execute_new_api(ctx).await,
        Some(SourceQuotaExecutorKind::RelayProvider) => execute_relay(profile_id, ctx).await,
        None => make_source_config_error(
            ctx.source,
            ctx.credential.source_tool.clone(),
            "unknown",
            format!("Unsupported profile: {}", profile_slug(profile_id)),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{SourceCredentialStrategy, SourceQuotaBindingConfig};

    fn source() -> ApiSource {
        ApiSource {
            id: "src_demo".to_string(),
            display_name: Some("Demo".to_string()),
            base_url: Some("https://api.deepseek.com".to_string()),
            api_key_prefixes: vec!["sk-demo".to_string()],
            api_key_notes: std::collections::HashMap::new(),
            color: "#000000".to_string(),
            icon: None,
            auto_detected: false,
            quota_query: Some(SourceQuotaBindingConfig {
                enabled: true,
                query_profile_id: SourceQueryProfileId::GenericBalanceV1Usage,
                credential_strategy: SourceCredentialStrategy::ManualApiKey,
                manual_api_key: Some("sk".to_string()),
                manual_access_token: None,
                manual_user_id: None,
            }),
            first_seen_ms: 0,
            last_seen_ms: 0,
        }
    }

    #[test]
    fn generic_balance_builder_uses_common_fields() {
        let quota = build_generic_balance_quota(
            &source(),
            Some("codex".to_string()),
            &serde_json::json!({
                "quota": { "remaining": 12.5, "unit": "USD" },
                "isValid": true
            }),
        );
        assert!(quota.success);
        assert_eq!(quota.tiers[0].remaining_value, Some(12.5));
    }
}
