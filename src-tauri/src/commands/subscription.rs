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

/// Check if Claude OAuth credentials are available
#[tauri::command]
pub async fn has_claude_oauth() -> bool {
    // Credential lookup may touch the filesystem / macOS keychain.
    tokio::task::spawn_blocking(|| {
        crate::subscription::ClaudeSubscriptionProvider::new().has_claude_oauth()
    })
    .await
    .unwrap_or(false)
}

/// Check if Gemini CLI OAuth credentials are present
#[tauri::command]
pub async fn has_gemini_oauth() -> bool {
    tokio::task::spawn_blocking(|| {
        crate::subscription::GeminiSubscriptionProvider::new().has_gemini_oauth()
    })
    .await
    .unwrap_or(false)
}

/// 查询多工具**已配置来源**的第三方中转额度/余额（A 静默降级）。
///
/// 流程：逐工具解析真实上游 base_url + 完整 api_key（Claude Code / Codex / OpenCode）→
/// 仅保留可识别的中转供应商 → 并发查询并归一化，成功者打上 `source_tool` 后返回。
/// 任一工具缺凭据 / 不可识别 / 查询失败都静默跳过（不抛错、不影响其它工具、不持久化 key）。
/// 返回空 Vec 表示无任何可查已配置来源，前端回落限额生存 T3 基线。
///
/// 只要工具配置中存在可识别来源且能读到凭据，就自动查询；缺凭据/查询失败都静默降级返回空 Vec。
#[tauri::command]
pub async fn get_configured_source_quotas(
) -> Result<crate::models::ConfiguredSourceQuotaQueryResult, String> {
    use crate::commands::load_settings;
    use crate::subscription::relay::{detect_relay_provider, fetch_relay_quota};
    use crate::subscription::source_quota::fetch_source_quota;
    use crate::subscription::source_resolver::resolve_all_relay_sources;

    // 文件 / 注册表读取放到阻塞线程，避免阻塞 async 运行时。
    let sources: Vec<crate::subscription::source_resolver::ResolvedRelaySource> =
        tokio::task::spawn_blocking(resolve_all_relay_sources)
            .await
            .map_err(|e| format!("join error: {e}"))?;
    let settings = tokio::task::spawn_blocking(load_settings)
        .await
        .map_err(|e| format!("join error: {e}"))?
        .map_err(|e| format!("Failed to load settings: {e}"))?;

    // 仅查询可识别的中转来源；多工具并发。
    let relay_futures: Vec<_> = sources
        .into_iter()
        .filter(|s| detect_relay_provider(&s.base_url).is_some())
        .map(|s| async move {
            let mut quota = fetch_relay_quota(&s.base_url, &s.api_key).await;
            quota.source_tool = Some(s.tool.id().to_string());
            quota
        })
        .collect();

    let source_quota_futures: Vec<_> = settings
        .source_aware
        .sources
        .into_iter()
        .filter(|source| {
            source
                .quota_query
                .as_ref()
                .map(|cfg| cfg.enabled)
                .unwrap_or(false)
        })
        .map(|source| async move { fetch_source_quota(&source).await })
        .collect();

    let mut all_results: Vec<crate::models::SubscriptionQuota> =
        futures::future::join_all(relay_futures).await;
    all_results.extend(
        futures::future::join_all(source_quota_futures)
            .await
            .into_iter(),
    );

    let attempted_count = all_results.len();
    let failed_count = all_results.iter().filter(|q| !q.success).count();
    let success_count = attempted_count.saturating_sub(failed_count);
    let errors = all_results
        .iter()
        .filter(|q| !q.success)
        .filter_map(|q| {
            let label = q.source_tool.as_deref().unwrap_or(q.tool.as_str());
            q.error
                .as_ref()
                .or(q.credential_message.as_ref())
                .map(|err| format!("{label}: {err}"))
        })
        .collect::<Vec<_>>();
    let quotas = all_results
        .into_iter()
        .filter(|q| q.success)
        .collect::<Vec<_>>();

    Ok(crate::models::ConfiguredSourceQuotaQueryResult {
        quotas,
        attempted_count,
        success_count,
        failed_count,
        errors,
        queried_at: chrono::Utc::now().timestamp_millis(),
    })
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
        "claude" => {
            crate::subscription::ClaudeSubscriptionProvider::new()
                .fetch_quota()
                .await
        }
        "gemini" => {
            let gemini_provider = state.get_gemini_provider().await;
            gemini_provider.fetch_quota().await
        }
        "copilot" => {
            let copilot_provider = state.get_copilot_provider().await;
            copilot_provider.query().await
        }
        _ => SubscriptionQueryResult::error(
            provider,
            CredentialStatus::NotConfigured,
            format!("Unknown provider: {}", provider),
        ),
    }
}
