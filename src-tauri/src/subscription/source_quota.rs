//! Manual source-level quota queries.
//!
//! This module complements auto-detected relay quota queries with explicit
//! source-bound configurations. The first supported mode is `new_api`, modeled
//! after cc-switch's New API usage template, but normalized into the existing
//! `SubscriptionQuota` / `QuotaTier` structures.

use crate::models::{
    ApiSource, QuotaKind, QuotaTier, SourceQuotaQueryConfig, SourceQuotaQueryType,
    SubscriptionQuota,
};
use crate::net::HttpClientFactory;

fn make_error(source: &ApiSource, msg: String) -> SubscriptionQuota {
    SubscriptionQuota {
        provider: "source-config".to_string(),
        tool: "newapi".to_string(),
        source_tool: None,
        credential_status: "queryFailed".to_string(),
        credential_message: source.display_name.clone(),
        success: false,
        tiers: Vec::new(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        from_cache: false,
        error: Some(msg),
    }
}

fn make_quota(
    _source: &ApiSource,
    tiers: Vec<QuotaTier>,
    plan_name: Option<String>,
) -> SubscriptionQuota {
    SubscriptionQuota {
        provider: "source-config".to_string(),
        tool: "source-config".to_string(),
        source_tool: None,
        credential_status: "valid".to_string(),
        credential_message: plan_name,
        success: true,
        tiers,
        updated_at: chrono::Utc::now().timestamp_millis(),
        from_cache: false,
        error: None,
    }
}

fn normalize_base_url(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_string()
}

fn build_new_api_quota(source: &ApiSource, response: &serde_json::Value) -> SubscriptionQuota {
    let Some(data) = response.get("data") else {
        let msg = response
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Missing data field in new-api response")
            .to_string();
        return make_error(source, msg);
    };

    let quota = data.get("quota").and_then(value_to_f64).unwrap_or(0.0) / 500000.0;
    let used = data.get("used_quota").and_then(value_to_f64).unwrap_or(0.0) / 500000.0;
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

    make_quota(source, vec![tier], plan_name)
}

fn value_to_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|v| v as f64))
        .or_else(|| value.as_u64().map(|v| v as f64))
        .or_else(|| value.as_str().and_then(|v| v.trim().parse::<f64>().ok()))
}

async fn fetch_new_api_quota(
    source: &ApiSource,
    base_url: &str,
    config: &SourceQuotaQueryConfig,
) -> SubscriptionQuota {
    let access_token = config
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let user_id = config
        .user_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let Some(access_token) = access_token else {
        return make_error(source, "Missing new-api access token".to_string());
    };
    let Some(user_id) = user_id else {
        return make_error(source, "Missing new-api user id".to_string());
    };

    let url = format!("{}/api/user/self", normalize_base_url(base_url));
    let client = HttpClientFactory::global().standard();
    let response = match client
        .get(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("New-Api-User", user_id)
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return make_error(source, format!("Network error: {e}")),
    };

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return make_error(source, format!("HTTP {status}: {text}"));
    }

    let body: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return make_error(source, format!("Parse error: {e}")),
    };

    if body.get("success").and_then(|v| v.as_bool()) != Some(true) {
        let msg = body
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("new-api query failed")
            .to_string();
        return make_error(source, msg);
    }

    build_new_api_quota(source, &body)
}

fn lookup_path<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    Some(current)
}

fn first_f64(body: &serde_json::Value, paths: &[&[&str]]) -> Option<f64> {
    paths
        .iter()
        .find_map(|path| lookup_path(body, path).and_then(value_to_f64))
}

fn first_string(body: &serde_json::Value, paths: &[&[&str]]) -> Option<String> {
    paths.iter().find_map(|path| {
        lookup_path(body, path)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    })
}

fn first_bool(body: &serde_json::Value, paths: &[&[&str]]) -> Option<bool> {
    paths
        .iter()
        .find_map(|path| lookup_path(body, path).and_then(|v| v.as_bool()))
}

fn build_generic_balance_quota(source: &ApiSource, body: &serde_json::Value) -> SubscriptionQuota {
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
        return make_error(
            source,
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

    make_quota(source, vec![tier], source.display_name.clone())
}

async fn fetch_generic_balance_quota(source: &ApiSource, base_url: &str) -> SubscriptionQuota {
    let url = format!("{}/v1/usage", normalize_base_url(base_url));
    let api_key = source
        .api_key_notes
        .get("__quota_api_key")
        .map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(api_key) = api_key else {
        return make_error(source, "Missing generic balance API key".to_string());
    };

    let client = HttpClientFactory::global().standard();
    let response = match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return make_error(source, format!("Network error: {e}")),
    };

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return make_error(source, format!("HTTP {status}: {text}"));
    }

    let body: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return make_error(source, format!("Parse error: {e}")),
    };

    build_generic_balance_quota(source, &body)
}

pub async fn fetch_source_quota(source: &ApiSource) -> SubscriptionQuota {
    let Some(base_url) = source
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return make_error(source, "Missing source base URL".to_string());
    };
    let Some(config) = source.quota_query.as_ref().filter(|cfg| cfg.enabled) else {
        return make_error(source, "Quota query is disabled".to_string());
    };

    match config.query_type {
        SourceQuotaQueryType::NewApi => fetch_new_api_quota(source, base_url, config).await,
        SourceQuotaQueryType::GenericBalance => fetch_generic_balance_quota(source, base_url).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn source_with_config() -> ApiSource {
        ApiSource {
            id: "src_demo".to_string(),
            display_name: Some("Demo".to_string()),
            base_url: Some("https://example.com".to_string()),
            api_key_prefixes: vec!["sk-demo".to_string()],
            api_key_notes: HashMap::new(),
            color: "#000000".to_string(),
            icon: None,
            auto_detected: false,
            quota_query: Some(SourceQuotaQueryConfig {
                enabled: true,
                query_type: SourceQuotaQueryType::NewApi,
                access_token: Some("token".to_string()),
                user_id: Some("42".to_string()),
            }),
            first_seen_ms: 0,
            last_seen_ms: 0,
        }
    }

    #[test]
    fn builds_new_api_balance_tier() {
        let source = source_with_config();
        let body = serde_json::json!({
            "success": true,
            "data": {
                "group": "pro",
                "quota": 1_000_000,
                "used_quota": 500_000
            }
        });
        let quota = build_new_api_quota(&source, &body);
        assert!(quota.success);
        assert_eq!(quota.tool, "source-config");
        assert_eq!(quota.source_tool.as_deref(), Some("src_demo"));
        assert_eq!(quota.credential_message.as_deref(), Some("pro"));
        assert_eq!(quota.tiers.len(), 1);
        assert_eq!(quota.tiers[0].kind, QuotaKind::Balance);
        assert_eq!(quota.tiers[0].remaining_value, Some(2.0));
        assert_eq!(quota.tiers[0].max_value, Some(3.0));
    }

    #[test]
    fn missing_data_becomes_error() {
        let source = source_with_config();
        let quota = build_new_api_quota(&source, &serde_json::json!({ "success": true }));
        assert!(!quota.success);
        assert!(quota.error.is_some());
    }

    #[test]
    fn generic_balance_uses_common_fallback_fields() {
        let mut source = source_with_config();
        source.display_name = Some("Gateway".to_string());
        source
            .api_key_notes
            .insert("__quota_api_key".to_string(), "sk-live".to_string());
        let quota = build_generic_balance_quota(
            &source,
            &serde_json::json!({
                "quota": { "remaining": 12.5, "unit": "USD" },
                "isValid": true
            }),
        );
        assert!(quota.success);
        assert_eq!(quota.credential_message.as_deref(), Some("Gateway"));
        assert_eq!(quota.tiers[0].remaining_value, Some(12.5));
        assert_eq!(quota.tiers[0].currency.as_deref(), Some("USD"));
    }
}
