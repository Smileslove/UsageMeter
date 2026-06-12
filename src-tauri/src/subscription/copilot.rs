use crate::copilot::{CopilotAuthError, CopilotAuthManager, CopilotUsageResponse, QuotaDetail};
use crate::models::{
    CredentialStatus, QuotaKind, QuotaTier, SubscriptionQueryResult, SubscriptionQuota,
};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct CopilotSubscriptionProvider {
    auth_manager: Arc<RwLock<CopilotAuthManager>>,
}

impl CopilotSubscriptionProvider {
    pub fn new(auth_manager: Arc<RwLock<CopilotAuthManager>>) -> Self {
        Self { auth_manager }
    }

    pub async fn query(&self) -> SubscriptionQueryResult {
        let auth = self.auth_manager.read().await;

        if !auth.is_authenticated().await {
            return SubscriptionQueryResult::no_credentials("copilot");
        }

        match auth.fetch_usage().await {
            Ok(usage) => {
                let login = auth.get_status().await.username;
                SubscriptionQueryResult::success(map_to_quota(usage, login))
            }
            Err(CopilotAuthError::GitHubTokenInvalid) => SubscriptionQueryResult::error(
                "copilot",
                CredentialStatus::Expired,
                "copilot.token_invalid".to_string(),
            ),
            Err(CopilotAuthError::NoCopilotSubscription) => SubscriptionQueryResult::error(
                "copilot",
                CredentialStatus::QueryFailed {
                    error: "no_subscription".to_string(),
                },
                "no_subscription".to_string(),
            ),
            Err(err) => SubscriptionQueryResult::error(
                "copilot",
                CredentialStatus::QueryFailed {
                    error: err.to_string(),
                },
                err.to_string(),
            ),
        }
    }
}

fn map_to_quota(usage: CopilotUsageResponse, login: Option<String>) -> SubscriptionQuota {
    let reset_at = Some(usage.quota_reset_date.clone());
    let tiers = vec![
        map_quota_detail(
            "copilot_premium",
            &usage.quota_snapshots.premium_interactions,
            reset_at.clone(),
        ),
        map_quota_detail(
            "copilot_chat",
            &usage.quota_snapshots.chat,
            reset_at.clone(),
        ),
        map_quota_detail(
            "copilot_completions",
            &usage.quota_snapshots.completions,
            reset_at,
        ),
    ];

    SubscriptionQuota {
        provider: "copilot".to_string(),
        tool: "copilot".to_string(),
        source_tool: None,
        credential_status: "valid".to_string(),
        credential_message: None,
        success: true,
        tiers,
        updated_at: chrono::Utc::now().timestamp_millis(),
        from_cache: false,
        error: None,
        plan_label: Some(usage.copilot_plan),
        account_label: login,
    }
}

fn map_quota_detail(name: &str, detail: &QuotaDetail, resets_at: Option<String>) -> QuotaTier {
    let utilization = if detail.unlimited || detail.entitlement <= 0 {
        0.0
    } else {
        let used = (detail.entitlement - detail.remaining).max(0);
        (used as f64 / detail.entitlement as f64 * 100.0).min(100.0)
    };

    QuotaTier {
        name: name.to_string(),
        kind: QuotaKind::Window,
        utilization,
        resets_at,
        remaining_value: if detail.unlimited {
            None
        } else {
            Some(detail.remaining as f64)
        },
        max_value: if detail.unlimited {
            None
        } else {
            Some(detail.entitlement as f64)
        },
        currency: None,
        limit_reached: Some(!detail.unlimited && detail.remaining <= 0),
    }
}
