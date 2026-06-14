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

#[tauri::command]
pub async fn get_source_quota_profiles(
) -> Result<Vec<crate::subscription::query_profiles::SourceQuotaProfileDescriptor>, String> {
    Ok(crate::subscription::query_profiles::list_profile_descriptors())
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
    use crate::subscription::source_quota::{
        fetch_auto_source_quota_for_resolved_source, fetch_source_quota,
    };
    use crate::subscription::source_resolver::{normalize_base_url, resolve_all_relay_sources};

    // 文件 / 注册表读取放到阻塞线程，避免阻塞 async 运行时。
    let resolved_sources: Vec<crate::subscription::source_resolver::ResolvedRelaySource> =
        tokio::task::spawn_blocking(resolve_all_relay_sources)
            .await
            .map_err(|e| format!("join error: {e}"))?;
    let settings = tokio::task::spawn_blocking(load_settings)
        .await
        .map_err(|e| format!("join error: {e}"))?
        .map_err(|e| format!("Failed to load settings: {e}"))?;
    let sources = settings.source_aware.sources;
    let live_base_urls: std::collections::HashSet<String> = resolved_sources
        .iter()
        .map(|source| normalize_base_url(&source.base_url))
        .collect();

    // 对当前工具真实配置的来源做自动识别：
    // 已知厂商直接走官方解析；未知来源尝试通用 `/v1/usage`，失败则静默跳过。
    #[allow(clippy::redundant_iter_cloned)]
    let relay_futures: Vec<_> = resolved_sources
        .iter()
        .cloned()
        .map(|resolved_source| {
            let matched_source = sources
                .iter()
                .find(|source| {
                    source
                        .base_url
                        .as_deref()
                        .map(|base_url| {
                            normalize_base_url(base_url)
                                == normalize_base_url(&resolved_source.base_url)
                        })
                        .unwrap_or(false)
                })
                .cloned();
            async move {
                fetch_auto_source_quota_for_resolved_source(
                    &resolved_source,
                    matched_source.as_ref(),
                )
                .await
            }
        })
        .collect();

    let source_quota_futures: Vec<_> = sources
        .into_iter()
        .filter(|source| {
            source
                .base_url
                .as_deref()
                .map(|base_url| !live_base_urls.contains(&normalize_base_url(base_url)))
                .unwrap_or(true)
        })
        .filter(|source| {
            source
                .quota_query
                .as_ref()
                .map(|cfg| cfg.enabled)
                .unwrap_or(false)
        })
        .map(|source| {
            let resolved_sources = resolved_sources.clone();
            async move { fetch_source_quota(&source, &resolved_sources).await }
        })
        .collect();

    let mut all_results: Vec<crate::models::SubscriptionQuota> =
        futures::future::join_all(relay_futures)
            .await
            .into_iter()
            .flatten()
            .collect();
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

#[tauri::command]
pub async fn test_source_quota_query(
    source_id: String,
    binding: crate::models::SourceQuotaBindingConfig,
    state: State<'_, SubscriptionState>,
) -> Result<crate::subscription::source_quota::SourceQuotaBindingTestResult, String> {
    use crate::commands::load_settings;
    use crate::subscription::source_quota::{
        probe_source_quota_binding_state, test_source_quota_binding, SourceQuotaBindingRuntimeState,
    };
    use crate::subscription::source_resolver::resolve_all_relay_sources;

    let settings = tokio::task::spawn_blocking(load_settings)
        .await
        .map_err(|e| format!("join error: {e}"))?
        .map_err(|e| format!("Failed to load settings: {e}"))?;
    let resolved_sources = tokio::task::spawn_blocking(resolve_all_relay_sources)
        .await
        .map_err(|e| format!("join error: {e}"))?;

    let source = settings
        .source_aware
        .sources
        .into_iter()
        .find(|source| source.id == source_id)
        .ok_or_else(|| format!("Source not found: {source_id}"))?;
    let probe_state =
        probe_source_quota_binding_state(&source, Some(&binding), &resolved_sources).await;
    let result = test_source_quota_binding(&source, &binding, &resolved_sources).await;
    let runtime_state = SourceQuotaBindingRuntimeState {
        source_id: source.id.clone(),
        recommended_profile_id: probe_state.recommended_profile_id,
        detection_confidence: probe_state.detection_confidence,
        last_probe_at: probe_state.last_probe_at,
        last_probe_error: probe_state.last_probe_error,
        last_verified_at: Some(chrono::Utc::now().timestamp_millis()),
        last_test_success: Some(result.success),
        last_test_summary: result.summary.clone(),
        last_test_error: result.error.clone(),
        last_tested_profile_id: result.attempted_profile_id,
        last_tested_strategy: result.credential_strategy,
        source_tool: result.source_tool.clone(),
    };
    state.update_source_binding_state(runtime_state).await;

    Ok(result)
}

#[tauri::command]
pub async fn get_source_quota_binding_states(
    source_id: Option<String>,
    state: State<'_, SubscriptionState>,
) -> Result<Vec<crate::subscription::source_quota::SourceQuotaBindingRuntimeState>, String> {
    use crate::commands::load_settings;
    use crate::subscription::source_quota::{
        merged_runtime_state, probe_source_quota_binding_state,
    };
    use crate::subscription::source_resolver::{
        find_resolved_source_for_base_url, resolve_all_relay_sources,
    };

    let settings = tokio::task::spawn_blocking(load_settings)
        .await
        .map_err(|e| format!("join error: {e}"))?
        .map_err(|e| format!("Failed to load settings: {e}"))?;
    let resolved_sources = tokio::task::spawn_blocking(resolve_all_relay_sources)
        .await
        .map_err(|e| format!("join error: {e}"))?;
    let runtime_states = state.get_all_source_binding_states().await;
    let filtered_sources: Vec<_> = settings
        .source_aware
        .sources
        .into_iter()
        .filter(|source| {
            source_id
                .as_ref()
                .map(|id| source.id == *id)
                .unwrap_or(true)
        })
        .collect();

    let probe_futures: Vec<_> = filtered_sources
        .into_iter()
        .map(|source| {
            let mut runtime_state = merged_runtime_state(&source, runtime_states.get(&source.id));
            let resolved_sources = resolved_sources.clone();
            async move {
                let needs_update = if let Some(live_source) =
                    source.base_url.as_deref().and_then(|base_url| {
                        find_resolved_source_for_base_url(&resolved_sources, base_url)
                    }) {
                    let probed = probe_source_quota_binding_state(
                        &source,
                        source.quota_query.as_ref(),
                        &resolved_sources,
                    )
                    .await;
                    runtime_state.recommended_profile_id = probed.recommended_profile_id;
                    runtime_state.detection_confidence = probed.detection_confidence;
                    runtime_state.last_probe_at = probed.last_probe_at;
                    runtime_state.last_probe_error = probed.last_probe_error;
                    runtime_state.source_tool = Some(live_source.tool.id().to_string());
                    true
                } else {
                    false
                };
                (runtime_state, needs_update)
            }
        })
        .collect();

    let probed_states = futures::future::join_all(probe_futures).await;
    let mut states = Vec::with_capacity(probed_states.len());
    for (runtime_state, needs_update) in probed_states {
        if needs_update {
            state
                .update_source_binding_state(runtime_state.clone())
                .await;
        }
        states.push(runtime_state);
    }

    Ok(states)
}

#[tauri::command]
pub async fn probe_source_quota_query(
    source_id: String,
    binding: Option<crate::models::SourceQuotaBindingConfig>,
    state: State<'_, SubscriptionState>,
) -> Result<crate::subscription::source_quota::SourceQuotaBindingRuntimeState, String> {
    use crate::commands::load_settings;
    use crate::subscription::source_quota::probe_source_quota_binding_state;
    use crate::subscription::source_resolver::resolve_all_relay_sources;

    let settings = tokio::task::spawn_blocking(load_settings)
        .await
        .map_err(|e| format!("join error: {e}"))?
        .map_err(|e| format!("Failed to load settings: {e}"))?;
    let resolved_sources = tokio::task::spawn_blocking(resolve_all_relay_sources)
        .await
        .map_err(|e| format!("join error: {e}"))?;

    let source = settings
        .source_aware
        .sources
        .into_iter()
        .find(|source| source.id == source_id)
        .ok_or_else(|| format!("Source not found: {source_id}"))?;

    let runtime_state =
        probe_source_quota_binding_state(&source, binding.as_ref(), &resolved_sources).await;
    state
        .update_source_binding_state(runtime_state.clone())
        .await;
    Ok(runtime_state)
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
