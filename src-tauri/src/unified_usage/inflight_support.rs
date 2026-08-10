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

    #[tokio::test]
    async fn waiter_never_acquires_while_key_is_held_forever() {
        // 固化当前无限等待语义：key 永不释放（guard 持锁不 drop）时，
        // 并发等待方在观测窗口内不得获得该 key。该测试为将来给
        // acquire_inflight_key 加超时兜底提供行为基线——届时此断言应改为
        // 期望超时返回，而不是无限等待。
        let _held = acquire_inflight_key("inflight-support-stuck-key").await;

        let mut waiter =
            tokio::spawn(async { acquire_inflight_key("inflight-support-stuck-key").await });

        let outcome = tokio::time::timeout(Duration::from_millis(150), &mut waiter).await;
        assert!(
            outcome.is_err(),
            "waiter acquired the inflight key while it was still held (would hang forever)"
        );

        // 释放 key 后等待方应能正常获得锁并退出，避免后台任务悬空。
        drop(_held);
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter should acquire after the key is finally released")
            .expect("waiter task should not panic");
    }
}
