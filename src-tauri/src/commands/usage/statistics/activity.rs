use super::super::types::{DayActivity, MonthActivity, StatisticsMetric, YearActivity};
use super::daily_summary::{
    can_use_unified_daily_summary, load_day_activity_from_summary_with_hot_overlay,
};
use super::shared::{
    cache_key_for_source_filter, cache_key_for_tool_filter, collect_day_activity_from_facts,
    fingerprint_pricings, month_day_count, normalized_day_boundary_mode, DayAccumulatorMap,
};
use crate::models::AppSettings;
use crate::proxy::{ProxyDatabase, ProxyMergeCacheSignature};
use chrono::NaiveDate;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ActivityCacheKey {
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
enum ActivityCacheValue {
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

fn lookup_activity_cache(key: &ActivityCacheKey) -> Option<ActivityCacheValue> {
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

#[cfg(test)]
pub(crate) fn clear_activity_cache() {
    activity_cache().lock().unwrap().clear();
}

fn resolve_period_bounds(
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
    let mut pricings = settings.model_pricing.pricings.clone();
    if let Some(db) = crate::proxy::ProxyDatabase::get_global() {
        if let Ok(db_pricings) = db.get_all_model_pricings() {
            pricings.extend(db_pricings);
        }
    }

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
) -> Result<HashMap<String, DayActivity>, String> {
    if can_use_unified_daily_summary(settings) {
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

pub(crate) async fn get_month_activity_impl(
    year: i32,
    month: u8,
    metric: StatisticsMetric,
    settings: AppSettings,
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
    let days_by_date =
        load_activity_day_map(month_start, month_end, include_errors, &settings).await?;

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
    store_activity_cache(cache_key, ActivityCacheValue::Month(activity.clone()));
    Ok(activity)
}

pub(crate) async fn get_year_activity_impl(
    year: i32,
    metric: StatisticsMetric,
    settings: AppSettings,
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
    let days_by_date =
        load_activity_day_map(year_start, year_end, include_errors, &settings).await?;

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
    store_activity_cache(cache_key, ActivityCacheValue::Year(activity.clone()));
    Ok(activity)
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
