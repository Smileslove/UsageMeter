use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

static UNIFIED_INFLIGHT_KEYS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn inflight_keys() -> &'static Mutex<HashSet<String>> {
    UNIFIED_INFLIGHT_KEYS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// RAII guard that releases an in-flight query key when the query finishes.
pub(super) struct InflightKeyGuard {
    key: String,
}

impl Drop for InflightKeyGuard {
    fn drop(&mut self) {
        inflight_keys().lock().unwrap().remove(&self.key);
    }
}

/// Wait until no other query is processing the same cache key.
pub(super) async fn acquire_inflight_key(key: &str) -> InflightKeyGuard {
    loop {
        let acquired = {
            let mut guard = inflight_keys().lock().unwrap();
            if guard.contains(key) {
                false
            } else {
                guard.insert(key.to_string());
                true
            }
        };
        if acquired {
            return InflightKeyGuard {
                key: key.to_string(),
            };
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::acquire_inflight_key;
    use std::time::Duration;

    #[tokio::test]
    async fn guard_releases_key_after_drop() {
        let first = acquire_inflight_key("inflight-support-test").await;
        drop(first);

        // A second acquisition would wait forever if Drop failed to remove the key.
        let second = acquire_inflight_key("inflight-support-test").await;
        drop(second);
    }

    #[tokio::test]
    async fn same_key_waits_until_previous_guard_is_dropped() {
        let first = acquire_inflight_key("inflight-support-blocking-test").await;
        let waiter = tokio::spawn(async {
            acquire_inflight_key("inflight-support-blocking-test").await;
        });

        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(!waiter.is_finished());

        drop(first);
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter should acquire after release")
            .expect("waiter task should not panic");
    }
}
