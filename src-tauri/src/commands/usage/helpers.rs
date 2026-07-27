use crate::{models::AppSettings, unified_usage::MergedRequestFact};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Emitter;

pub(crate) fn usage_window_cutoff_epoch(window: &str, settings: &AppSettings) -> i64 {
    crate::utils::business_time::business_window_cutoff_epoch(window, settings)
}

pub(crate) fn epoch_u64_to_i64_saturating(epoch: u64) -> i64 {
    i64::try_from(epoch).unwrap_or(i64::MAX)
}

pub(crate) fn first_fact_index_in_range(facts: &[MergedRequestFact], cutoff_epoch: i64) -> usize {
    facts.partition_point(|fact| fact.timestamp_sec < cutoff_epoch)
}

static BACKGROUND_SYNC_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// 会话面板事件名：后台同步完成且数据实际发生变化时通知前端静默刷新。
pub(crate) const LOCAL_USAGE_SYNCED_EVENT: &str = "local_usage_synced";

/// 面板命令的快照优先配套：把全盘扫描同步移出请求路径，放到后台执行。
///
/// 同步完成后仅当 local/proxy 合并签名实际变化（有新数据落库）才 emit
/// LOCAL_USAGE_SYNCED_EVENT，前端据此做静默二次刷新；无变化时静默结束，
/// 避免事件风暴。多个面板命令并发调用时只保留一个在途后台任务
/// （ensure_synced_throttled 内部另有 3 秒节流兜底）。
pub(crate) fn spawn_background_local_usage_sync(app: tauri::AppHandle) {
    if BACKGROUND_SYNC_IN_FLIGHT.swap(true, Ordering::AcqRel) {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let result = (|| -> Result<bool, String> {
            let before_local = crate::local_usage::get_local_usage_db()?
                .get_merge_cache_signature()?;
            let before_proxy = crate::proxy::ProxyDatabase::get_global()
                .map(|db| db.get_merge_cache_signature())
                .transpose()?;
            let db = crate::local_usage::ensure_local_usage_synced()?;
            let after_local = db.get_merge_cache_signature()?;
            let after_proxy = crate::proxy::ProxyDatabase::get_global()
                .map(|db| db.get_merge_cache_signature())
                .transpose()?;
            Ok(before_local != after_local || before_proxy != after_proxy)
        })();
        BACKGROUND_SYNC_IN_FLIGHT.store(false, Ordering::Release);
        match result {
            Ok(true) => {
                let _ = app.emit(LOCAL_USAGE_SYNCED_EVENT, ());
            }
            Ok(false) => {}
            Err(err) => {
                eprintln!("[UsageMeter] Background local usage sync failed: {}", err);
            }
        }
    });
}
