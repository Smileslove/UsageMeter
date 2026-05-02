//! Subscription query module
//!
//! Provides subscription quota queries for official providers (GPT, Claude, etc.)

mod gpt;
mod token_cache;
mod types;

pub use gpt::*;
pub use token_cache::TokenCache;

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::models::SubscriptionQuota;

/// Subscription state shared across the application
#[derive(Default)]
pub struct SubscriptionState {
    /// Cached subscription data by provider
    cache: Arc<RwLock<std::collections::HashMap<String, CachedSubscription>>>,
    /// Shared token cache for all providers
    token_cache: Arc<TokenCache>,
    /// GPT provider instance (singleton)
    gpt_provider: Arc<RwLock<Option<GptSubscriptionProvider>>>,
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
        Self::default()
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
}
