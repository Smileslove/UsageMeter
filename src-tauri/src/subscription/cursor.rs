use crate::{
    cursor::{
        client::{CursorClient, CursorFailure},
        sync::current_auth,
    },
    models::{CredentialStatus, QuotaTier, SubscriptionQueryResult, SubscriptionQuota},
};
use serde_json::Value;
use std::sync::OnceLock;
use tokio::sync::Mutex;

static CACHE: OnceLock<Mutex<Option<(String, SubscriptionQuota)>>> = OnceLock::new();

pub(crate) async fn query(force: bool) -> SubscriptionQueryResult {
    match query_inner(force).await {
        Ok(quota) if quota.from_cache => SubscriptionQueryResult::from_cache(quota),
        Ok(quota) => SubscriptionQueryResult::success(quota),
        Err(failure) => SubscriptionQueryResult::error(
            "cursor",
            match failure.code.as_str() {
                "cursor_not_found" | "cursor_not_signed_in" | "cursor_sync_disabled" => {
                    CredentialStatus::NotConfigured
                }
                "cursor_expired" => CredentialStatus::Expired,
                _ => CredentialStatus::QueryFailed {
                    error: failure.code.clone(),
                },
            },
            failure.code,
        ),
    }
}

async fn query_inner(force: bool) -> Result<SubscriptionQuota, CursorFailure> {
    if !crate::cursor::ACCOUNT_API_CONTRACT_VERIFIED {
        return Err("cursor_contract_unverified".into());
    }
    if !crate::settings::load_settings_blocking()?
        .cursor
        .account_sync_enabled
    {
        return Err("cursor_sync_disabled".into());
    }
    let auth = tokio::task::spawn_blocking(current_auth)
        .await
        .map_err(|_| "cursor_auth_unavailable")??;
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if !force {
        if let Some((key, quota)) = cache.lock().await.as_ref() {
            if key == &auth.account_key
                && chrono::Utc::now().timestamp_millis() - quota.updated_at < 5 * 60 * 1000
            {
                let mut quota = quota.clone();
                quota.from_cache = true;
                return Ok(quota);
            }
        }
    }
    let summary = CursorClient::new().fetch_summary(&auth).await?;
    let quota = parse_summary(&summary)?;
    let active = tokio::task::spawn_blocking(current_auth)
        .await
        .map_err(|_| "cursor_auth_unavailable")??;
    if active.account_key != auth.account_key {
        return Err("cursor_identity_mismatch".into());
    }
    if !crate::settings::load_settings_blocking()?
        .cursor
        .account_sync_enabled
    {
        return Err("cursor_sync_disabled".into());
    }
    *cache.lock().await = Some((auth.account_key, quota.clone()));
    Ok(quota)
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|number| number.is_finite() && *number >= 0.0 && *number <= 1e12)
}
fn reset(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        if let Ok(date) = chrono::DateTime::parse_from_rfc3339(text) {
            return Some(date.to_rfc3339());
        }
    }
    let raw = number(value)?;
    chrono::DateTime::from_timestamp_millis((if raw > 1e10 { raw } else { raw * 1000.0 }) as i64)
        .map(|date| date.to_rfc3339())
}

pub(crate) fn parse_summary(value: &Value) -> Result<SubscriptionQuota, CursorFailure> {
    let plan = &value["individualUsage"]["plan"];
    let resets_at = reset(&value["billingCycleEnd"]);
    let mut tiers = Vec::new();
    for (name, field) in [
        ("cursor_models", "autoPercentUsed"),
        ("cursor_other_models", "apiPercentUsed"),
    ] {
        if let Some(percent) = number(&plan[field]) {
            tiers.push(QuotaTier {
                name: name.into(),
                utilization: percent.clamp(0.0, 100.0),
                utilization_available: Some(true),
                resets_at: resets_at.clone(),
                limit_reached: Some(percent >= 100.0),
                ..Default::default()
            });
        }
    }
    if tiers.is_empty() {
        if let Some(percent) = number(&plan["totalPercentUsed"]).or_else(|| {
            Some(
                number(&plan["used"])? / number(&plan["limit"]).filter(|limit| *limit > 0.0)?
                    * 100.0,
            )
        }) {
            tiers.push(QuotaTier {
                name: "cursor_monthly".into(),
                utilization: percent.clamp(0.0, 100.0),
                utilization_available: Some(true),
                resets_at: resets_at.clone(),
                ..Default::default()
            });
        }
        let used = number(&plan["numRequests"]);
        let limit = number(&plan["maxRequestUsage"]).filter(|limit| *limit > 0.0);
        if let (Some(used), Some(limit)) = (used, limit) {
            tiers.push(QuotaTier {
                name: "cursor_requests".into(),
                utilization: (used / limit * 100.0).clamp(0.0, 100.0),
                utilization_available: Some(true),
                used_value: Some(used),
                max_value: Some(limit),
                remaining_value: Some((limit - used).max(0.0)),
                resets_at: resets_at.clone(),
                ..Default::default()
            });
        }
    }
    // usage-summary used/limit amounts are cents. Individual and team pools are
    // independent; no summing percentages and no default hardcoded plan allowance.
    for (name, pool) in [
        ("cursor_on_demand", &value["individualUsage"]["onDemand"]),
        ("cursor_team_pool", &value["teamUsage"]["pooled"]),
        ("cursor_team_on_demand", &value["teamUsage"]["onDemand"]),
    ] {
        if let Some(used) = number(&pool["used"]) {
            let limit = number(&pool["limit"]).filter(|limit| *limit > 0.0);
            let percentage = limit.map(|limit| used / limit * 100.0);
            tiers.push(QuotaTier {
                name: name.into(),
                utilization: percentage.unwrap_or(0.0).clamp(0.0, 100.0),
                utilization_available: Some(percentage.is_some()),
                used_value: Some(used / 100.0),
                max_value: limit.map(|limit| limit / 100.0),
                remaining_value: limit.map(|limit| (limit - used).max(0.0) / 100.0),
                currency: Some("USD".into()),
                resets_at: resets_at.clone(),
                limit_reached: percentage.map(|percent| percent >= 100.0),
                ..Default::default()
            });
        }
    }
    if tiers.is_empty() {
        return Err("cursor_schema_unsupported".into());
    }
    let plan_label = value["membershipType"]
        .as_str()
        .filter(|name| {
            [
                "pro",
                "pro_plus",
                "business",
                "enterprise",
                "hobby",
                "free",
                "ultra",
            ]
            .contains(name)
        })
        .map(String::from);
    Ok(SubscriptionQuota {
        provider: "cursor".into(),
        tool: "cursor".into(),
        source_tool: None,
        credential_status: "valid".into(),
        credential_message: None,
        success: true,
        tiers,
        updated_at: chrono::Utc::now().timestamp_millis(),
        from_cache: false,
        error: None,
        plan_label,
        account_label: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn cursor_quota_keeps_two_pools_and_unlimited_spend_separate() {
        let quota = parse_summary(&json!({"membershipType":"pro","billingCycleEnd":"2025-07-01T00:00:00Z",
            "individualUsage":{"plan":{"autoPercentUsed":1,"apiPercentUsed":85},"onDemand":{"used":125}},
            "teamUsage":{"pooled":{"used":200,"limit":1000}}})).unwrap();
        assert_eq!(quota.tiers.len(), 4);
        assert_eq!(quota.tiers[0].utilization, 1.0);
        assert_eq!(quota.tiers[1].utilization, 85.0);
        assert_eq!(quota.tiers[2].used_value, Some(1.25));
        assert_eq!(quota.tiers[2].utilization_available, Some(false));
        assert_eq!(quota.tiers[3].utilization, 20.0);
        assert!(parse_summary(&json!({})).is_err());
        assert!(parse_summary(&json!({"membershipType":"pro"})).is_err());
    }
}
