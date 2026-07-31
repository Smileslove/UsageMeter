//! 多货币汇率命令

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};

use crate::net::HttpClientFactory;

const EXCHANGE_RATE_CACHE_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone)]
struct ExchangeRateCache {
    fetched_at: Instant,
    rates: HashMap<String, f64>,
}

/// open.er-api.com 汇率 API 响应结构
#[derive(Debug, Deserialize)]
struct ExchangeRateResponse {
    result: String,
    #[serde(default)]
    rates: HashMap<String, f64>,
}

fn exchange_rate_cache() -> &'static RwLock<Option<ExchangeRateCache>> {
    static CACHE: OnceLock<RwLock<Option<ExchangeRateCache>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(None))
}

fn filter_requested_rates(
    all_rates: &HashMap<String, f64>,
    currencies: &[String],
) -> HashMap<String, f64> {
    let mut result: HashMap<String, f64> = HashMap::new();
    result.insert("USD".to_string(), 1.0);

    for currency in currencies {
        if currency == "USD" {
            continue;
        }
        if let Some(&rate) = all_rates.get(currency) {
            result.insert(currency.clone(), rate);
        }
    }

    result
}

fn fresh_cached_rates(currencies: &[String]) -> Option<HashMap<String, f64>> {
    let cache = exchange_rate_cache().read().ok()?;
    let entry = cache.as_ref()?;
    if entry.fetched_at.elapsed() > EXCHANGE_RATE_CACHE_TTL {
        return None;
    }
    Some(filter_requested_rates(&entry.rates, currencies))
}

fn fallback_cached_rates(currencies: &[String]) -> Option<HashMap<String, f64>> {
    let cache = exchange_rate_cache().read().ok()?;
    let entry = cache.as_ref()?;
    Some(filter_requested_rates(&entry.rates, currencies))
}

fn store_cached_rates(rates: HashMap<String, f64>) {
    if let Ok(mut cache) = exchange_rate_cache().write() {
        *cache = Some(ExchangeRateCache {
            fetched_at: Instant::now(),
            rates,
        });
    }
}

/// 从 open.er-api.com 获取指定币种的最新汇率
/// 返回以 USD 为基准的汇率：1 USD = rate 目标货币
#[tauri::command]
pub async fn get_exchange_rates(currencies: Vec<String>) -> Result<HashMap<String, f64>, String> {
    if currencies.is_empty() {
        return Ok(HashMap::new());
    }

    if let Some(cached) = fresh_cached_rates(&currencies) {
        return Ok(cached);
    }

    let url = "https://open.er-api.com/v6/latest/USD";
    let client = HttpClientFactory::global().short();

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("ERR_EXCHANGE_RATE_FETCH: {}", e));

    let response = match response {
        Ok(response) => response,
        Err(err) => {
            if let Some(cached) = fallback_cached_rates(&currencies) {
                return Ok(cached);
            }
            return Err(err);
        }
    };

    if !response.status().is_success() {
        if let Some(cached) = fallback_cached_rates(&currencies) {
            return Ok(cached);
        }
        return Err(format!(
            "ERR_EXCHANGE_RATE_STATUS: HTTP {}",
            response.status()
        ));
    }

    let data = response
        .json()
        .await
        .map_err(|e| format!("ERR_EXCHANGE_RATE_PARSE: {}", e));

    let data: ExchangeRateResponse = match data {
        Ok(data) => data,
        Err(err) => {
            if let Some(cached) = fallback_cached_rates(&currencies) {
                return Ok(cached);
            }
            return Err(err);
        }
    };

    if data.result != "success" {
        if let Some(cached) = fallback_cached_rates(&currencies) {
            return Ok(cached);
        }
        return Err("ERR_EXCHANGE_RATE_API: API returned non-success result".to_string());
    }
    store_cached_rates(data.rates.clone());
    Ok(filter_requested_rates(&data.rates, &currencies))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_cache_state<T>(entry: Option<ExchangeRateCache>, f: impl FnOnce() -> T) -> T {
        static TEST_LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        let _test_guard = TEST_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let lock = exchange_rate_cache();
        let previous = {
            let mut guard = lock.write().unwrap();
            std::mem::replace(&mut *guard, entry)
        };
        let result = f();
        *lock.write().unwrap() = previous;
        result
    }

    #[test]
    fn filter_requested_rates_keeps_usd_and_requested_matches() {
        let rates = HashMap::from([
            ("CNY".to_string(), 7.2),
            ("EUR".to_string(), 0.91),
            ("JPY".to_string(), 155.0),
        ]);

        let filtered = filter_requested_rates(
            &rates,
            &["USD".to_string(), "CNY".to_string(), "EUR".to_string()],
        );

        assert_eq!(filtered.get("USD"), Some(&1.0));
        assert_eq!(filtered.get("CNY"), Some(&7.2));
        assert_eq!(filtered.get("EUR"), Some(&0.91));
        assert!(!filtered.contains_key("JPY"));
    }

    #[test]
    fn fresh_cached_rates_requires_unexpired_entry() {
        let rates = HashMap::from([("CNY".to_string(), 7.2)]);

        let fresh = with_cache_state(
            Some(ExchangeRateCache {
                fetched_at: Instant::now() - Duration::from_secs(30),
                rates: rates.clone(),
            }),
            || fresh_cached_rates(&["CNY".to_string()]),
        );
        assert_eq!(fresh.unwrap().get("CNY"), Some(&7.2));

        let stale = with_cache_state(
            Some(ExchangeRateCache {
                fetched_at: Instant::now() - EXCHANGE_RATE_CACHE_TTL - Duration::from_secs(1),
                rates,
            }),
            || fresh_cached_rates(&["CNY".to_string()]),
        );
        assert!(stale.is_none());
    }

    #[test]
    fn fallback_cached_rates_returns_stale_snapshot() {
        let fallback = with_cache_state(
            Some(ExchangeRateCache {
                fetched_at: Instant::now() - EXCHANGE_RATE_CACHE_TTL - Duration::from_secs(5),
                rates: HashMap::from([("CNY".to_string(), 7.2)]),
            }),
            || fallback_cached_rates(&["USD".to_string(), "CNY".to_string()]),
        );

        let fallback = fallback.expect("stale cache should still be usable as fallback");
        assert_eq!(fallback.get("USD"), Some(&1.0));
        assert_eq!(fallback.get("CNY"), Some(&7.2));
    }
}
