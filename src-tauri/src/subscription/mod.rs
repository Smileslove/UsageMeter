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
        if let Some(cached) = cache.get(provider) {
            let now = chrono::Utc::now().timestamp_millis();
            if now - cached.cached_at < CACHE_VALIDITY_MS {
                return Some(cached.quota.clone());
            }
        }
        None
    }

    /// Update cache with new subscription data
    pub async fn update_cache(&self, quota: SubscriptionQuota) {
        let mut cache = self.cache.write().await;
        let provider = quota.provider.clone();
        cache.insert(
            provider,
            CachedSubscription {
                quota,
                cached_at: chrono::Utc::now().timestamp_millis(),
            },
        );
    }

    /// Clear cache for a specific provider
    pub async fn clear_cache(&self, provider: &str) {
        let mut cache = self.cache.write().await;
        cache.remove(provider);
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
