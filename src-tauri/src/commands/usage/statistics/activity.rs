use super::super::types::{DayActivity, MonthActivity, StatisticsMetric, YearActivity};
use super::daily_summary::{
    can_use_unified_daily_summary, day_activity_from_summary_row,
    load_day_activity_from_summary_with_hot_overlay, next_business_date,
};
use super::shared::{
    cache_key_for_source_filter, cache_key_for_tool_filter, collect_day_activity_from_facts,
    fingerprint_pricings, month_day_count, normalized_day_boundary_mode, DayAccumulatorMap,
};
use crate::models::AppSettings;
use crate::proxy::{ProxyDatabase, ProxyMergeCacheSignature};
use chrono::NaiveDate;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::Emitter;

/// stale 天数阈值：超过该值则把历史物化移出请求路径，改为后台物化 + 延迟快照。
pub(crate) const MAX_SYNC_MATERIALIZATION_DAYS: usize = 7;

/// 后台物化完成事件名，前端 Statistics.vue 监听后做静默刷新。
pub(crate) const ACTIVITY_MATERIALIZATION_DONE_EVENT: &str = "activity_materialization_done";

static ACTIVITY_MATERIALIZATION_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// 最新 pending 后台物化视图 scope（P2-4 后端 latest-scope 方案）。
///
/// in-flight 只允许一个后台任务，但用户可能在任务运行期间切到另一个月/年视图
/// （stale>7）再次调用 spawn —— 该调用会被 in-flight guard 短路、无法获得自己的
/// 后台任务。若任务完成时仍 emit 启动时捕获的旧 scope，前端按 payload 维度匹配
/// 当前视图会失败而不刷新（物化事件 `activity_materialization_done` 就此丢失）。
/// 因此 spawn 时（无论抢占是否成功）都把最新请求的 scope 记到这里；唯一在跑的
/// 任务完成时 `take()` 消费它并 emit，保证 emit 的总是用户最后等待的视图，且
/// emit 后清空不残留。注意：物化本身按各请求的 [start,end) 执行，latest scope
/// 只影响事件 payload 的维度，不影响物化数据。
static LATEST_PENDING_ACTIVITY_SCOPE: Mutex<Option<ActivityMaterializationScope>> =
    Mutex::new(None);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ActivityCacheKey {
    kind: ActivityCacheKind,
    start_epoch: i64,
    end_epoch: i64,
    day_boundary_mode: String,
    timezone: String,
    include_errors: bool,
    tool_filter: String,
    source_filter: String,
    pricing_match_mode: String,
    pricing_fingerprint: u64,
    local_signature: crate::local_usage::LocalMergeCacheSignature,
    proxy_signature: Option<ProxyMergeCacheSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ActivityCacheKind {
    Month {
        year: i32,
        month: u8,
        metric: StatisticsMetric,
    },
    Year {
        year: i32,
        metric: StatisticsMetric,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum ActivityCacheValue {
    Month(MonthActivity),
    Year(YearActivity),
}

#[derive(Debug, Clone)]
struct ActivityCacheEntry {
    key: ActivityCacheKey,
    value: ActivityCacheValue,
}

static ACTIVITY_CACHE: OnceLock<Mutex<Vec<ActivityCacheEntry>>> = OnceLock::new();
const ACTIVITY_CACHE_CAPACITY: usize = 8;

fn activity_cache() -> &'static Mutex<Vec<ActivityCacheEntry>> {
    ACTIVITY_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

pub(crate) fn lookup_activity_cache(key: &ActivityCacheKey) -> Option<ActivityCacheValue> {
    let cache = activity_cache();
    let mut guard = cache.lock().unwrap();
    let idx = guard.iter().position(|entry| entry.key == *key)?;
    let entry = guard.remove(idx);
    let value = entry.value.clone();
    guard.insert(0, entry);
    Some(value)
}

fn store_activity_cache(key: ActivityCacheKey, value: ActivityCacheValue) {
    let cache = activity_cache();
    let mut guard = cache.lock().unwrap();
    if let Some(idx) = guard.iter().position(|entry| entry.key == key) {
        guard.remove(idx);
    }
    guard.insert(0, ActivityCacheEntry { key, value });
    if guard.len() > ACTIVITY_CACHE_CAPACITY {
        guard.truncate(ACTIVITY_CACHE_CAPACITY);
    }
}

pub(crate) fn clear_activity_cache() {
    activity_cache().lock().unwrap().clear();
}

/// 便捷缓存查询（Month 维度）：构造缓存键并命中返回完整结果。
///
/// 供命令层在 `count_stale_materialization_days`（较重：逐日读 state/snapshot）
/// 之前先尝试缓存命中，避免缓存命中时仍跑 stale 统计。命中结果必然来自完整
/// 物化（deferred 快照不写缓存），可直接返回。
pub(crate) fn lookup_month_activity_cached(
    year: i32,
    month: u8,
    metric: &StatisticsMetric,
    start_epoch: i64,
    end_epoch: i64,
    settings: &AppSettings,
) -> Result<Option<MonthActivity>, String> {
    let cache_key = build_activity_cache_key(
        ActivityCacheKind::Month {
            year,
            month,
            metric: metric.clone(),
        },
        start_epoch,
        end_epoch,
        settings,
    )?;
    Ok(match lookup_activity_cache(&cache_key) {
        Some(ActivityCacheValue::Month(activity)) => Some(activity),
        _ => None,
    })
}

/// 便捷缓存查询（Year 维度），语义同 `lookup_month_activity_cached`。
pub(crate) fn lookup_year_activity_cached(
    year: i32,
    metric: &StatisticsMetric,
    start_epoch: i64,
    end_epoch: i64,
    settings: &AppSettings,
) -> Result<Option<YearActivity>, String> {
    let cache_key = build_activity_cache_key(
        ActivityCacheKind::Year {
            year,
            metric: metric.clone(),
        },
        start_epoch,
        end_epoch,
        settings,
    )?;
    Ok(match lookup_activity_cache(&cache_key) {
        Some(ActivityCacheValue::Year(activity)) => Some(activity),
        _ => None,
    })
}

pub(super) fn resolve_period_bounds(
    start_label: &str,
    end_label: &str,
    settings: &AppSettings,
) -> Result<(i64, i64), String> {
    let start = crate::utils::business_time::business_date_epoch_bounds(start_label, settings)
        .map(|(value, _)| value)?;
    let end = crate::utils::business_time::business_date_epoch_bounds(end_label, settings)
        .map(|(value, _)| value)?;
    Ok((start, end))
}

fn build_activity_cache_key(
    kind: ActivityCacheKind,
    start_epoch: i64,
    end_epoch: i64,
    settings: &AppSettings,
) -> Result<ActivityCacheKey, String> {
    let local_db = crate::local_usage::get_local_usage_db()?;
    let local_signature = local_db.get_merge_cache_signature()?;
    let proxy_signature = ProxyDatabase::get_global()
        .map(|db| db.get_merge_cache_signature())
        .transpose()?;
    let pricings = crate::proxy::ProxyDatabase::get_global()
        .and_then(|db| db.get_all_model_pricings().ok())
        .unwrap_or_default();

    Ok(ActivityCacheKey {
        kind,
        start_epoch,
        end_epoch,
        day_boundary_mode: normalized_day_boundary_mode(settings),
        timezone: settings.timezone.clone(),
        include_errors: settings.proxy.include_error_requests,
        tool_filter: cache_key_for_tool_filter(&settings.client_tools.build_filter()),
        source_filter: cache_key_for_source_filter(&settings.source_aware.build_filter()),
        pricing_match_mode: settings.model_pricing.match_mode.clone(),
        pricing_fingerprint: fingerprint_pricings(&pricings),
        local_signature,
        proxy_signature,
    })
}

async fn load_activity_day_map(
    start_epoch: i64,
    end_epoch: i64,
    include_errors: bool,
    settings: &AppSettings,
    skip_history_ensure: bool,
) -> Result<HashMap<String, DayActivity>, String> {
    if can_use_unified_daily_summary(settings) {
        if skip_history_ensure {
            return load_day_activity_from_summary_skipping_ensure(
                start_epoch,
                end_epoch,
                include_errors,
                settings,
            )
            .await;
        }
        return load_day_activity_from_summary_with_hot_overlay(
            start_epoch,
            end_epoch,
            include_errors,
            settings,
        )
        .await;
    }

    let mut day_map: DayAccumulatorMap = HashMap::new();
    let (facts, _coverage) = crate::unified_usage::get_merged_request_facts_no_sync(
        settings,
        Some(start_epoch),
        Some(end_epoch),
        include_errors,
    )
    .await?;
    collect_day_activity_from_facts(&facts, &mut day_map, settings);
    let mut days_by_date = HashMap::new();
    for (date, (acc, models)) in day_map {
        let error_requests = acc.client_error_requests + acc.server_error_requests;
        days_by_date.insert(
            date.clone(),
            DayActivity {
                date,
                request_count: acc.request_count,
                total_tokens: acc.total_tokens,
                input_tokens: acc.input_tokens,
                output_tokens: acc.output_tokens,
                cache_create_tokens: acc.cache_create_tokens,
                cache_read_tokens: acc.cache_read_tokens,
                cost: acc.cost,
                model_count: models.len() as u64,
                success_requests: Some(acc.success_requests),
                error_requests: Some(error_requests),
            },
        );
    }

    Ok(days_by_date)
}

/// 与 `daily_summary::load_day_activity_from_summary_with_hot_overlay` 等价，但
/// **跳过历史物化 ensure**：直接读现有 summary + 今日 hot overlay，缺失日由调用方补零。
/// 供 deferred 变体在后台物化进行中返回快速快照，避免请求路径阻塞在重建上。
async fn load_day_activity_from_summary_skipping_ensure(
    start_epoch: i64,
    end_epoch: i64,
    include_errors: bool,
    settings: &AppSettings,
) -> Result<HashMap<String, DayActivity>, String> {
    let local_db = crate::local_usage::get_local_usage_db()?;
    let start_date =
        crate::utils::business_time::business_date_for_timestamp(start_epoch, settings);
    let end_date = crate::utils::business_time::business_date_for_timestamp(
        end_epoch.saturating_sub(1),
        settings,
    );
    let today_date =
        crate::local_usage::LocalUsageDatabase::today_local_date_with_settings(settings);
    let mut by_date = HashMap::new();

    if start_date < today_date {
        let summary_end = if end_date < today_date {
            next_business_date(&end_date, settings)?
        } else {
            today_date.clone()
        };
        let rows = local_db.get_unified_daily_summaries_between(&start_date, &summary_end)?;
        for row in rows {
            by_date.insert(
                row.local_date.clone(),
                day_activity_from_summary_row(&row, include_errors),
            );
        }
    }

    let (today_start, _) =
        crate::local_usage::LocalUsageDatabase::local_date_epoch_bounds_with_settings(
            &today_date,
            settings,
        )?;
    if end_epoch > today_start && start_epoch < end_epoch {
        let hot_start = start_epoch.max(today_start);
        if end_epoch > hot_start {
            let (facts, _coverage) = crate::unified_usage::get_merged_request_facts_no_sync(
                settings,
                Some(hot_start),
                Some(end_epoch),
                include_errors,
            )
            .await?;
            let mut day_map: DayAccumulatorMap = HashMap::new();
            collect_day_activity_from_facts(&facts, &mut day_map, settings);
            for (date, (acc, models)) in day_map {
                let error_requests = acc.client_error_requests + acc.server_error_requests;
                by_date.insert(
                    date.clone(),
                    DayActivity {
                        date,
                        request_count: acc.request_count,
                        total_tokens: acc.total_tokens,
                        input_tokens: acc.input_tokens,
                        output_tokens: acc.output_tokens,
                        cache_create_tokens: acc.cache_create_tokens,
                        cache_read_tokens: acc.cache_read_tokens,
                        cost: acc.cost,
                        model_count: models.len() as u64,
                        success_requests: Some(acc.success_requests),
                        error_requests: Some(error_requests),
                    },
                );
            }
        }
    }

    Ok(by_date)
}

async fn get_month_activity_inner(
    year: i32,
    month: u8,
    metric: StatisticsMetric,
    settings: AppSettings,
    skip_history_ensure: bool,
    cache_result: bool,
) -> Result<MonthActivity, String> {
    let day_count = month_day_count(year, month);
    let next_month = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month as u32 + 1)
    };
    let (month_start, month_end) = resolve_period_bounds(
        &format!("{year}-{month:02}-01"),
        &format!("{}-{:02}-01", next_month.0, next_month.1),
        &settings,
    )?;
    let cache_key = build_activity_cache_key(
        ActivityCacheKind::Month {
            year,
            month,
            metric: metric.clone(),
        },
        month_start,
        month_end,
        &settings,
    )?;
    if let Some(ActivityCacheValue::Month(activity)) = lookup_activity_cache(&cache_key) {
        return Ok(activity);
    }

    let include_errors = settings.proxy.include_error_requests;
    let days_by_date = load_activity_day_map(
        month_start,
        month_end,
        include_errors,
        &settings,
        skip_history_ensure,
    )
    .await?;

    let mut days = Vec::new();
    for day in 1..=day_count {
        let Some(date) = NaiveDate::from_ymd_opt(year, month as u32, day) else {
            continue;
        };
        let key = date.format("%Y-%m-%d").to_string();
        days.push(days_by_date.get(&key).cloned().unwrap_or(DayActivity {
            date: key,
            ..Default::default()
        }));
    }

    let activity = MonthActivity {
        year,
        month,
        timezone: settings.timezone.clone(),
        metric: metric.clone(),
        days,
    };
    if cache_result {
        store_activity_cache(cache_key, ActivityCacheValue::Month(activity.clone()));
    }
    Ok(activity)
}

pub(crate) async fn get_month_activity_impl(
    year: i32,
    month: u8,
    metric: StatisticsMetric,
    settings: AppSettings,
) -> Result<MonthActivity, String> {
    get_month_activity_inner(year, month, metric, settings, false, true).await
}

/// 延迟变体：跳过历史物化 ensure，直接读现有 summary + 今日 hot overlay，缺失日补零。
/// 供 stale 天数超阈值、后台物化进行中时返回快速快照；物化完成后前端收到
/// `activity_materialization_done` 事件再静默刷新。
///
/// 注意：**不写 activity 缓存**。物化后台任务成功后会 clear_activity_cache 再 emit 事件；
/// 若 deferred 把“全零/部分”快照写入缓存，可能落在 clear 之后被前端命中，导致物化
/// 完成后仍长期显示旧 partial（缓存无 TTL）。
pub(crate) async fn get_month_activity_deferred(
    year: i32,
    month: u8,
    metric: StatisticsMetric,
    settings: AppSettings,
) -> Result<MonthActivity, String> {
    get_month_activity_inner(year, month, metric, settings, true, false).await
}

async fn get_year_activity_inner(
    year: i32,
    metric: StatisticsMetric,
    settings: AppSettings,
    skip_history_ensure: bool,
    cache_result: bool,
) -> Result<YearActivity, String> {
    let (year_start, year_end) = resolve_period_bounds(
        &format!("{year}-01-01"),
        &format!("{}-01-01", year + 1),
        &settings,
    )?;
    let cache_key = build_activity_cache_key(
        ActivityCacheKind::Year {
            year,
            metric: metric.clone(),
        },
        year_start,
        year_end,
        &settings,
    )?;
    if let Some(ActivityCacheValue::Year(activity)) = lookup_activity_cache(&cache_key) {
        return Ok(activity);
    }

    let include_errors = settings.proxy.include_error_requests;
    let days_by_date = load_activity_day_map(
        year_start,
        year_end,
        include_errors,
        &settings,
        skip_history_ensure,
    )
    .await?;

    let Some(mut date) = NaiveDate::from_ymd_opt(year, 1, 1) else {
        return Ok(YearActivity {
            year,
            timezone: settings.timezone,
            metric,
            days: Vec::new(),
        });
    };
    let Some(end_date) = NaiveDate::from_ymd_opt(year + 1, 1, 1) else {
        return Ok(YearActivity {
            year,
            timezone: settings.timezone,
            metric,
            days: Vec::new(),
        });
    };

    let mut days = Vec::new();
    while date < end_date {
        let key = date.format("%Y-%m-%d").to_string();
        days.push(days_by_date.get(&key).cloned().unwrap_or(DayActivity {
            date: key,
            ..Default::default()
        }));
        let Some(next_date) = date.succ_opt() else {
            break;
        };
        date = next_date;
    }

    let activity = YearActivity {
        year,
        timezone: settings.timezone.clone(),
        metric: metric.clone(),
        days,
    };
    if cache_result {
        store_activity_cache(cache_key, ActivityCacheValue::Year(activity.clone()));
    }
    Ok(activity)
}

pub(crate) async fn get_year_activity_impl(
    year: i32,
    metric: StatisticsMetric,
    settings: AppSettings,
) -> Result<YearActivity, String> {
    get_year_activity_inner(year, metric, settings, false, true).await
}

/// 延迟变体：跳过历史物化 ensure，直接读现有 summary + 今日 hot overlay，缺失日补零。
/// 供 stale 天数超阈值、后台物化进行中时返回快速快照；物化完成后前端收到
/// `activity_materialization_done` 事件再静默刷新。
///
/// 注意：**不写 activity 缓存**（原因同 get_month_activity_deferred）。
pub(crate) async fn get_year_activity_deferred(
    year: i32,
    metric: StatisticsMetric,
    settings: AppSettings,
) -> Result<YearActivity, String> {
    get_year_activity_inner(year, metric, settings, true, false).await
}

/// 后台物化的视图维度描述，随 `activity_materialization_done` 事件 payload 下发，
/// 前端据此判断是否与当前视图匹配再决定是否静默刷新。
#[derive(Clone)]
pub(crate) struct ActivityMaterializationScope {
    pub(crate) kind: &'static str,
    pub(crate) year: i32,
    pub(crate) month: Option<u8>,
    pub(crate) metric: StatisticsMetric,
}

/// RAII 幂等 guard：`spawn_background_activity_materialization` 抢占成功后在任务体内持有，
/// Drop 时（含 panic）复位 in-flight 标记，避免任务异常导致后台物化永久短路。
struct MaterializationInFlightGuard;

impl MaterializationInFlightGuard {
    fn acquire() -> Option<Self> {
        if ACTIVITY_MATERIALIZATION_IN_FLIGHT.swap(true, Ordering::AcqRel) {
            None
        } else {
            Some(Self)
        }
    }
}

impl Drop for MaterializationInFlightGuard {
    fn drop(&mut self) {
        ACTIVITY_MATERIALIZATION_IN_FLIGHT.store(false, Ordering::Release);
    }
}

/// 把历史物化移出请求路径、放到后台执行（stale 天数超过 MAX_SYNC_MATERIALIZATION_DAYS 时由命令调用）。
///
/// 幂等防重：多个命令并发调用时只保留一个在途后台任务（AtomicBool swap 判定；
/// guard 在任务体内持有，任务结束才复位）。物化成功后清空 activity 缓存（否则
/// 前端永远命中旧 partial），随后 emit `activity_materialization_done` 通知前端
/// 静默刷新；失败时 eprintln 并 emit ok:false，前端可据此决定是否保留旧快照。
/// emit 失败按 helpers.rs 惯例用 `let _ =` 忽略。
///
/// 事件 payload 使用 **latest pending scope**（P2-4）：任务运行期间用户切视图再
/// 触发 spawn 时，即使该 spawn 被 in-flight 短路，也会更新
/// `LATEST_PENDING_ACTIVITY_SCOPE`；任务完成时消费并 emit 该最新 scope，前端当前
/// 视图因此能收到匹配的事件。作为备选，前端也可放宽匹配（按 kind+year 匹配即
/// 刷新），届时本方案不影响行为。
pub(crate) fn spawn_background_activity_materialization(
    app: tauri::AppHandle,
    settings: AppSettings,
    start_epoch: i64,
    end_epoch: i64,
    scope: ActivityMaterializationScope,
) {
    // 无论抢占是否成功，都把该视图记为「最新 pending scope」：在途任务完成时
    // 按它 emit，前端当前视图因此能收到匹配事件（即使本次 spawn 被短路）。
    *LATEST_PENDING_ACTIVITY_SCOPE.lock().unwrap() = Some(scope.clone());

    let Some(guard) = MaterializationInFlightGuard::acquire() else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        // guard 移入任务体内持有：任务结束（含 unwind）才复位 in-flight 标记，
        // 保证任一时刻只有一个后台物化任务在跑，latest scope 也仅由它消费。
        let _guard = guard;
        let result = crate::unified_usage::ensure_materialized_history_no_sync(
            &settings,
            start_epoch,
            end_epoch,
        )
        .await;
        // 完成时消费最新 pending scope（若运行期间被新视图覆盖则 emit 新 scope），
        // 并在 emit 后清空，避免残留旧视图干扰后续事件。
        let emit_scope = LATEST_PENDING_ACTIVITY_SCOPE
            .lock()
            .unwrap()
            .take()
            .unwrap_or(scope);
        match result {
            Ok(()) => {
                clear_activity_cache();
                let _ = app.emit(
                    ACTIVITY_MATERIALIZATION_DONE_EVENT,
                    serde_json::json!({
                        "kind": emit_scope.kind,
                        "year": emit_scope.year,
                        "month": emit_scope.month,
                        "metric": emit_scope.metric,
                        "ok": true,
                    }),
                );
            }
            Err(err) => {
                eprintln!(
                    "[UsageMeter] Background activity materialization failed: {}",
                    err
                );
                let _ = app.emit(
                    ACTIVITY_MATERIALIZATION_DONE_EVENT,
                    serde_json::json!({
                        "kind": emit_scope.kind,
                        "year": emit_scope.year,
                        "month": emit_scope.month,
                        "metric": emit_scope.metric,
                        "ok": false,
                    }),
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_local_signature(version: i64) -> crate::local_usage::LocalMergeCacheSignature {
        crate::local_usage::LocalMergeCacheSignature {
            merge_cache_generation: version,
            unified_materialization_invalidation_version: version,
        }
    }

    fn sample_month_activity() -> MonthActivity {
        MonthActivity {
            year: 2026,
            month: 6,
            timezone: "Asia/Shanghai".to_string(),
            metric: StatisticsMetric::Cost,
            days: vec![DayActivity {
                date: "2026-06-14".to_string(),
                request_count: 12,
                ..Default::default()
            }],
        }
    }

    fn sample_activity_key(
        local_signature: crate::local_usage::LocalMergeCacheSignature,
    ) -> ActivityCacheKey {
        let settings = AppSettings::default();
        ActivityCacheKey {
            kind: ActivityCacheKind::Month {
                year: 2026,
                month: 6,
                metric: StatisticsMetric::Cost,
            },
            start_epoch: 1,
            end_epoch: 2,
            day_boundary_mode: normalized_day_boundary_mode(&settings),
            timezone: settings.timezone,
            include_errors: settings.proxy.include_error_requests,
            tool_filter: "all".to_string(),
            source_filter: "all".to_string(),
            pricing_match_mode: settings.model_pricing.match_mode,
            pricing_fingerprint: 0,
            local_signature,
            proxy_signature: None,
        }
    }

    #[test]
    fn activity_cache_hit_requires_matching_signature() {
        clear_activity_cache();
        let key = sample_activity_key(sample_local_signature(1));
        let activity = sample_month_activity();
        store_activity_cache(key.clone(), ActivityCacheValue::Month(activity.clone()));

        let cached = lookup_activity_cache(&key);
        assert!(
            matches!(cached, Some(ActivityCacheValue::Month(value)) if value.days[0].request_count == 12)
        );

        let changed_key = sample_activity_key(sample_local_signature(2));
        assert!(lookup_activity_cache(&changed_key).is_none());
    }
}
