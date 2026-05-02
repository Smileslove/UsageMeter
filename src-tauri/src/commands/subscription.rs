//! Tauri commands for subscription queries

use tauri::State;

use crate::models::{CredentialStatus, SubscriptionQueryResult};
use crate::subscription::SubscriptionState;

/// Get subscription quota for a specific provider
#[tauri::command]
pub async fn get_subscription_quota(
    provider: String,
    state: State<'_, SubscriptionState>,
) -> Result<SubscriptionQueryResult, String> {
    // Check cache first
    if let Some(cached) = state.get_cached(&provider).await {
        return Ok(SubscriptionQueryResult::from_cache(cached));
    }

    // Fetch fresh data
    let result = fetch_provider_quota(&provider, &state).await;

    // Update cache if successful
    if result.success {
        if let Some(quota) = &result.quota {
            state.update_cache(quota.clone()).await;
        }
    }

    Ok(result)
}

/// Refresh subscription quota (force refresh, ignore cache)
#[tauri::command]
pub async fn refresh_subscription_quota(
    provider: String,
    state: State<'_, SubscriptionState>,
) -> Result<SubscriptionQueryResult, String> {
    // Clear cache first
    state.clear_cache(&provider).await;

    // Fetch fresh data
    let result = fetch_provider_quota(&provider, &state).await;

    // Update cache if successful
    if result.success {
        if let Some(quota) = &result.quota {
            state.update_cache(quota.clone()).await;
        }
    }

    Ok(result)
}

/// Check if ChatGPT OAuth is configured
#[tauri::command]
pub async fn has_chatgpt_oauth() -> bool {
    // Use tokio::spawn_blocking for file I/O to avoid blocking
    tokio::task::spawn_blocking(|| {
        crate::subscription::GptSubscriptionProvider::new().has_chatgpt_oauth()
    })
    .await
    .unwrap_or(false)
}

/// Clear subscription cache
#[tauri::command]
pub async fn clear_subscription_cache(
    provider: Option<String>,
    state: State<'_, SubscriptionState>,
) -> Result<(), String> {
    match provider {
        Some(p) => state.clear_cache(&p).await,
        None => state.clear_all_cache().await,
    }
    Ok(())
}

/// Fetch quota from provider
async fn fetch_provider_quota(
    provider: &str,
    state: &State<'_, SubscriptionState>,
) -> SubscriptionQueryResult {
    match provider {
        "gpt" => {
            let gpt_provider = state.get_gpt_provider().await;
            gpt_provider.fetch_quota().await
        }
        _ => SubscriptionQueryResult::error(
            provider,
            CredentialStatus::NotConfigured,
            format!("Unknown provider: {}", provider),
        ),
    }
}
