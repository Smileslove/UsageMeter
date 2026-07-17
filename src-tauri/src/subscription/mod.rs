//! Subscription query module
//!
//! Provides subscription quota queries for official providers (GPT, Claude, etc.)

mod claude;
mod copilot;
mod gemini;
mod gpt;
pub mod query_profiles;
pub mod relay;
pub mod source_quota;
pub mod source_quota_executor;
pub mod source_quota_secrets;
pub mod source_quota_util;
pub mod source_resolver;
mod token_cache;
mod types;

pub use claude::ClaudeSubscriptionProvider;
pub use copilot::CopilotSubscriptionProvider;
pub use gemini::GeminiSubscriptionProvider;
pub use gpt::*;
pub use token_cache::TokenCache;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::copilot::CopilotAuthManager;
use crate::models::SubscriptionQuota;
use crate::subscription::source_quota::SourceQuotaBindingRuntimeState;

/// Subscription state shared across the application
pub struct SubscriptionState {
    /// Cached subscription data by provider
    cache: Arc<RwLock<std::collections::HashMap<String, CachedSubscription>>>,
    /// Shared token cache for all providers
    token_cache: Arc<TokenCache>,
    /// GPT provider instance (singleton)
    gpt_provider: Arc<RwLock<Option<GptSubscriptionProvider>>>,
    /// Gemini provider instance (singleton, keeps in-memory token cache)
    gemini_provider: Arc<RwLock<Option<GeminiSubscriptionProvider>>>,
    /// Copilot auth manager shared with commands
    copilot_auth: Arc<RwLock<CopilotAuthManager>>,
    /// Ephemeral per-source quota binding state (recommendations, last tests).
    source_binding_states:
        Arc<RwLock<std::collections::HashMap<String, SourceQuotaBindingRuntimeState>>>,
    /// 缓存写入序号：cached_at 毫秒级时间戳可能相同，用单调递增序号
    /// 确保“最近更新的条目优先”这一语义是确定性的。
    cache_seq: AtomicU64,
}

impl Default for SubscriptionState {
    fn default() -> Self {
        Self::new()
    }
}

/// Cached subscription data with timestamp
#[derive(Clone)]
struct CachedSubscription {
    quota: SubscriptionQuota,
    cached_at: i64,
    /// 写入序号（同一毫秒内多次写入时按序号判定新旧）。
    seq: u64,
}

fn cache_key_for_provider(provider: &str) -> String {
    let normalized = provider.trim();
    format!("{normalized}::{normalized}")
}

fn cache_key_for_quota(quota: &SubscriptionQuota) -> String {
    let source_tool = quota.source_tool.as_deref().unwrap_or("").trim();
    let tool = quota.tool.trim();
    let provider = quota.provider.trim();
    if source_tool.is_empty() {
        format!("{provider}::{tool}")
    } else {
        format!("{provider}::{tool}::{source_tool}")
    }
}

/// Cache validity duration in milliseconds (5 minutes)
const CACHE_VALIDITY_MS: i64 = 5 * 60 * 1000;

impl SubscriptionState {
    pub fn new() -> Self {
        Self::new_with_copilot(Arc::new(RwLock::new(CopilotAuthManager::new(
            crate::utils::usagemeter_dir().unwrap_or_default(),
        ))))
    }

    pub fn new_with_copilot(copilot_auth: Arc<RwLock<CopilotAuthManager>>) -> Self {
        Self {
            cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
            token_cache: Arc::new(TokenCache::new()),
            gpt_provider: Arc::new(RwLock::new(None)),
            gemini_provider: Arc::new(RwLock::new(None)),
            copilot_auth,
            source_binding_states: Arc::new(RwLock::new(std::collections::HashMap::new())),
            cache_seq: AtomicU64::new(0),
        }
    }

    /// Get or create GPT provider instance with shared token cache
    pub async fn get_gpt_provider(&self) -> GptSubscriptionProvider {
        let mut provider = self.gpt_provider.write().await;
        if provider.is_none() {
            *provider = Some(GptSubscriptionProvider::with_token_cache(
                self.token_cache.clone(),
            ));
        }
        provider.clone().unwrap()
    }

    /// Get or create the Gemini provider instance (preserves token cache)
    pub async fn get_gemini_provider(&self) -> GeminiSubscriptionProvider {
        let mut provider = self.gemini_provider.write().await;
        if provider.is_none() {
            *provider = Some(GeminiSubscriptionProvider::new());
        }
        provider.clone().unwrap()
    }

    pub async fn get_copilot_provider(&self) -> CopilotSubscriptionProvider {
        CopilotSubscriptionProvider::new(self.copilot_auth.clone())
    }

    /// Get cached subscription if still valid
    pub async fn get_cached(&self, provider: &str) -> Option<SubscriptionQuota> {
        let cache = self.cache.read().await;
        let cache_key = cache_key_for_provider(provider);
        let now = chrono::Utc::now().timestamp_millis();

        if let Some(cached) = cache.get(&cache_key) {
            if now - cached.cached_at < CACHE_VALIDITY_MS {
                return Some(cached.quota.clone());
            }
        }

        let prefix = format!("{}::", provider.trim());
        cache
            .iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .filter(|(_, cached)| now - cached.cached_at < CACHE_VALIDITY_MS)
            // 时间戳相同（同一毫秒写入）时按写入序号取最新，避免 HashMap 迭代顺序带来的不确定性。
            .max_by_key(|(_, cached)| (cached.cached_at, cached.seq))
            .map(|(_, cached)| cached.quota.clone())
    }

    /// Update cache with new subscription data
    pub async fn update_cache(&self, quota: SubscriptionQuota) {
        let mut cache = self.cache.write().await;
        let provider = cache_key_for_quota(&quota);
        cache.insert(
            provider,
            CachedSubscription {
                quota,
                cached_at: chrono::Utc::now().timestamp_millis(),
                seq: self.cache_seq.fetch_add(1, Ordering::Relaxed),
            },
        );
    }

    /// Clear cache for a specific provider
    pub async fn clear_cache(&self, provider: &str) {
        let mut cache = self.cache.write().await;
        cache.retain(|key, _| !key.starts_with(&format!("{}::", cache_key_for_provider(provider))));
    }

    /// Clear all cached data
    pub async fn clear_all_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    pub async fn get_all_source_binding_states(
        &self,
    ) -> std::collections::HashMap<String, SourceQuotaBindingRuntimeState> {
        self.source_binding_states.read().await.clone()
    }

    pub async fn update_source_binding_state(&self, state: SourceQuotaBindingRuntimeState) {
        self.source_binding_states
            .write()
            .await
            .insert(state.source_id.clone(), state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SubscriptionQuota;

    #[tokio::test]
    async fn cache_separates_relay_and_source_tool_variants() {
        let state = SubscriptionState::new();
        let now = chrono::Utc::now().timestamp_millis();

        let deepseek = SubscriptionQuota {
            provider: "relay".to_string(),
            tool: "deepseek".to_string(),
            source_tool: Some("codex".to_string()),
            credential_status: "valid".to_string(),
            credential_message: None,
            success: true,
            tiers: Vec::new(),
            updated_at: now,
            from_cache: false,
            error: None,
            plan_label: None,
            account_label: None,
        };
        let openrouter = SubscriptionQuota {
            provider: "relay".to_string(),
            tool: "openrouter".to_string(),
            source_tool: Some("claude-code".to_string()),
            credential_status: "valid".to_string(),
            credential_message: None,
            success: true,
            tiers: Vec::new(),
            updated_at: now,
            from_cache: false,
            error: None,
            plan_label: None,
            account_label: None,
        };
        state.update_cache(deepseek.clone()).await;
        state.update_cache(openrouter).await;

        let cached = state.get_cached("relay").await.expect("relay cache");
        assert_eq!(cached.tool, "openrouter");
        assert_eq!(cached.source_tool.as_deref(), Some("claude-code"));
        assert_ne!(cached.tool, "relay");
        assert_ne!(cached.tool, deepseek.tool);
    }
}
