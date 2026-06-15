use super::super::helpers::perf_log;
use super::super::types::{StatisticsBucket, StatisticsQuery, StatisticsSummary};
use super::aggregate::build_merged_statistics;
use super::shared::{
    cache_key_for_source_filter, cache_key_for_tool_filter, fingerprint_pricings, normalize_range,
    normalized_day_boundary_mode,
};
use crate::models::AppSettings;
use crate::proxy::{ProxyDatabase, ProxyMergeCacheSignature};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct HourlySummaryCacheKey {
    local_date: String,
    start_epoch: i64,
    end_epoch: i64,
    timezone: String,
    day_boundary_mode: String,
    include_errors: bool,
    tool_filter: String,
    source_filter: String,
    pricing_match_mode: String,
    pricing_fingerprint: u64,
    local_signature: crate::local_usage::LocalMergeCacheSignature,
    proxy_signature: Option<ProxyMergeCacheSignature>,
}

#[derive(Debug, Clone)]
struct HourlySummaryCacheEntry {
    key: HourlySummaryCacheKey,
    summary: StatisticsSummary,
}

static HOURLY_SUMMARY_CACHE: OnceLock<Mutex<Vec<HourlySummaryCacheEntry>>> = OnceLock::new();
const HOURLY_SUMMARY_CACHE_CAPACITY: usize = 8;

fn hourly_summary_cache() -> &'static Mutex<Vec<HourlySummaryCacheEntry>> {
    HOURLY_SUMMARY_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn lookup_hourly_summary_cache(key: &HourlySummaryCacheKey) -> Option<StatisticsSummary> {
    let cache = hourly_summary_cache();
    let mut guard = cache.lock().unwrap();
    let idx = guard.iter().position(|entry| entry.key == *key)?;
    let entry = guard.remove(idx);
    let summary = entry.summary.clone();
    guard.insert(0, entry);
    Some(summary)
}

fn store_hourly_summary_cache(key: HourlySummaryCacheKey, summary: &StatisticsSummary) {
    let cache = hourly_summary_cache();
    let mut guard = cache.lock().unwrap();
    if let Some(idx) = guard.iter().position(|entry| entry.key == key) {
        guard.remove(idx);
    }
    guard.insert(
        0,
        HourlySummaryCacheEntry {
            key,
            summary: summary.clone(),
        },
    );
    if guard.len() > HOURLY_SUMMARY_CACHE_CAPACITY {
        guard.truncate(HOURLY_SUMMARY_CACHE_CAPACITY);
    }
}

#[cfg(test)]
fn clear_hourly_summary_cache() {
    hourly_summary_cache().lock().unwrap().clear();
}

fn build_hourly_summary_cache_key(
    local_date: &str,
    start_epoch: i64,
    end_epoch: i64,
    settings: &AppSettings,
) -> Result<HourlySummaryCacheKey, String> {
    let local_db = crate::local_usage::get_local_usage_db()?;
    let local_signature = local_db.get_merge_cache_signature()?;
    let proxy_signature = ProxyDatabase::get_global()
        .map(|db| db.get_merge_cache_signature())
        .transpose()?;
    let mut pricings = settings.model_pricing.pricings.clone();
    if let Ok(db) = crate::proxy::ProxyDatabase::new() {
        if let Ok(db_pricings) = db.get_all_model_pricings() {
            pricings.extend(db_pricings);
        }
    }

    Ok(HourlySummaryCacheKey {
        local_date: local_date.to_string(),
        start_epoch,
        end_epoch,
        timezone: settings.timezone.clone(),
        day_boundary_mode: normalized_day_boundary_mode(settings),
        include_errors: settings.proxy.include_error_requests,
        tool_filter: cache_key_for_tool_filter(&settings.client_tools.build_filter()),
        source_filter: cache_key_for_source_filter(&settings.source_aware.build_filter()),
        pricing_match_mode: settings.model_pricing.match_mode.clone(),
        pricing_fingerprint: fingerprint_pricings(&pricings),
        local_signature,
        proxy_signature,
    })
}

fn is_single_historical_day(
    local_date: &str,
    start_epoch: i64,
    end_epoch: i64,
    settings: &AppSettings,
) -> Result<bool, String> {
    let (day_start, day_end) =
        crate::local_usage::LocalUsageDatabase::local_date_epoch_bounds_with_settings(
            local_date, settings,
        )?;
    let today = crate::local_usage::LocalUsageDatabase::today_local_date_with_settings(settings);
    Ok(local_date < today.as_str()
        && start_epoch >= day_start
        && end_epoch <= day_end
        && end_epoch > start_epoch)
}

fn materialization_state_matches(
    state: &crate::local_usage::UnifiedDayMaterializationState,
    snapshot: &crate::local_usage::UnifiedDayLocalSnapshot,
    proxy_snapshot: crate::proxy::ProxyDayDependencySnapshot,
    pricing_fingerprint: u64,
    settings: &AppSettings,
) -> bool {
    state.is_finalized
        && state.day_boundary_mode == normalized_day_boundary_mode(settings)
        && state.pricing_fingerprint == pricing_fingerprint
        && state.local_request_count == snapshot.local_request_count
        && state.local_max_sync_version == snapshot.local_max_sync_version
        && state.local_max_timestamp == snapshot.local_max_timestamp
        && state.remote_request_count == snapshot.remote_request_count
        && state.remote_max_export_seq == snapshot.remote_max_export_seq
        && state.remote_max_timestamp == snapshot.remote_max_timestamp
        && state.proxy_record_count == proxy_snapshot.record_count
        && state.proxy_max_timestamp_ms == proxy_snapshot.max_timestamp_ms
        && state.proxy_max_updated_at == proxy_snapshot.max_updated_at
}

fn try_load_ready_historical_day(
    local_date: &str,
    start_epoch: i64,
    end_epoch: i64,
    settings: &AppSettings,
) -> Result<Option<Vec<crate::unified_usage::MergedRequestFact>>, String> {
    let local_db = crate::local_usage::get_local_usage_db()?;
    let state = local_db.get_unified_day_materialization_state(local_date)?;
    let Some(state) = state else {
        return Ok(None);
    };
    let mut pricings = settings.model_pricing.pricings.clone();
    if let Ok(db) = crate::proxy::ProxyDatabase::new() {
        if let Ok(db_pricings) = db.get_all_model_pricings() {
            pricings.extend(db_pricings);
        }
    }
    let pricing_fingerprint = fingerprint_pricings(&pricings);
    let (boundary_start, boundary_end) =
        crate::local_usage::LocalUsageDatabase::local_date_epoch_bounds_with_settings(
            local_date, settings,
        )?;
    let local_snapshot =
        local_db.get_unified_day_local_snapshot_with_settings(local_date, settings)?;
    let proxy_snapshot = ProxyDatabase::get_global()
        .map(|db| {
            db.get_day_dependency_snapshot(
                boundary_start.saturating_mul(1000),
                boundary_end.saturating_mul(1000),
            )
        })
        .transpose()?
        .unwrap_or_default();
    if !materialization_state_matches(
        &state,
        &local_snapshot,
        proxy_snapshot,
        pricing_fingerprint,
        settings,
    ) {
        return Ok(None);
    }

    let tool_filter = settings.client_tools.build_filter();
    let source_filter = settings.source_aware.build_filter();
    let include_errors = settings.proxy.include_error_requests;
    let dates = vec![local_date.to_string()];
    let mut facts = local_db.get_unified_facts_for_dates(&dates, &tool_filter)?;
    facts.retain(|fact| {
        fact.timestamp_sec >= start_epoch
            && fact.timestamp_sec < end_epoch
            && crate::unified_usage::matches_source_filter(fact, &source_filter)
            && (include_errors || fact.status_code.map(|code| code < 300).unwrap_or(true))
    });
    Ok(Some(facts))
}

pub(super) async fn try_build_statistics_summary_from_hourly_cache(
    query: &StatisticsQuery,
    settings: &AppSettings,
) -> Result<Option<StatisticsSummary>, String> {
    if !matches!(query.bucket, StatisticsBucket::Hour) {
        return Ok(None);
    }

    let (start_epoch, end_epoch) = normalize_range(query);
    let local_date =
        crate::utils::business_time::business_date_for_timestamp(start_epoch, settings);
    if !is_single_historical_day(&local_date, start_epoch, end_epoch, settings)? {
        return Ok(None);
    }

    let cache_key = build_hourly_summary_cache_key(&local_date, start_epoch, end_epoch, settings)?;
    if let Some(summary) = lookup_hourly_summary_cache(&cache_key) {
        perf_log(
            "statistics_hourly_cache_hit",
            format!(
                "date={} range={}..{} models={} trend_points={}",
                local_date,
                start_epoch,
                end_epoch,
                summary.models.len(),
                summary.trend.len(),
            ),
        );
        return Ok(Some(summary));
    }

    let facts = if let Some(facts) =
        try_load_ready_historical_day(&local_date, start_epoch, end_epoch, settings)?
    {
        perf_log(
            "statistics_hourly_materialized_hit",
            format!(
                "date={} range={}..{} facts={}",
                local_date,
                start_epoch,
                end_epoch,
                facts.len(),
            ),
        );
        facts
    } else {
        crate::unified_usage::ensure_materialized_history_no_sync(settings, start_epoch, end_epoch)
            .await?;
        let local_db = crate::local_usage::get_local_usage_db()?;
        let tool_filter = settings.client_tools.build_filter();
        let source_filter = settings.source_aware.build_filter();
        let include_errors = settings.proxy.include_error_requests;
        let dates = vec![local_date.clone()];
        let mut facts = local_db.get_unified_facts_for_dates(&dates, &tool_filter)?;
        facts.retain(|fact| {
            fact.timestamp_sec >= start_epoch
                && fact.timestamp_sec < end_epoch
                && crate::unified_usage::matches_source_filter(fact, &source_filter)
                && (include_errors || fact.status_code.map(|code| code < 300).unwrap_or(true))
        });
        facts
    };

    let summary = build_merged_statistics(facts, query);
    store_hourly_summary_cache(cache_key, &summary);
    perf_log(
        "statistics_hourly_cache_store",
        format!(
            "date={} range={}..{} models={} trend_points={}",
            local_date,
            start_epoch,
            end_epoch,
            summary.models.len(),
            summary.trend.len(),
        ),
    );
    Ok(Some(summary))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::usage::types::{StatisticsRange, StatisticsTrendPoint};

    fn sample_summary() -> StatisticsSummary {
        StatisticsSummary {
            generated_at_epoch: 1,
            source: "proxy-merged".to_string(),
            capability: Default::default(),
            range: StatisticsRange {
                start_epoch: 10,
                end_epoch: 20,
                timezone: "Asia/Shanghai".to_string(),
                bucket: "hour".to_string(),
            },
            totals: Default::default(),
            trend: vec![StatisticsTrendPoint {
                start_epoch: 10,
                label: "06-03 00:00".to_string(),
                ..Default::default()
            }],
            models: Vec::new(),
            performance: None,
            status: None,
            insights: Vec::new(),
        }
    }

    fn sample_local_signature(
        invalidation_version: i64,
    ) -> crate::local_usage::LocalMergeCacheSignature {
        crate::local_usage::LocalMergeCacheSignature {
            local_request_count: 1,
            local_max_sync_version: invalidation_version,
            local_max_timestamp: 1,
            remote_request_count: 0,
            remote_max_export_seq: 0,
            remote_max_timestamp: 0,
            local_session_max_updated_at: 0,
            remote_session_max_imported_at: 0,
            unified_materialization_invalidation_version: invalidation_version,
        }
    }

    fn sample_cache_key(
        local_signature: crate::local_usage::LocalMergeCacheSignature,
    ) -> HourlySummaryCacheKey {
        let settings = AppSettings::default();
        HourlySummaryCacheKey {
            local_date: "2026-06-03".to_string(),
            start_epoch: 10,
            end_epoch: 20,
            timezone: settings.timezone.clone(),
            day_boundary_mode: normalized_day_boundary_mode(&settings),
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
    fn hourly_summary_cache_hit_requires_matching_signature() {
        clear_hourly_summary_cache();
        let key = sample_cache_key(sample_local_signature(1));
        let summary = sample_summary();
        store_hourly_summary_cache(key.clone(), &summary);

        let cached = lookup_hourly_summary_cache(&key);
        assert!(matches!(cached, Some(value) if value.trend.len() == 1));

        let changed_key = sample_cache_key(sample_local_signature(2));
        assert!(lookup_hourly_summary_cache(&changed_key).is_none());
    }

    #[test]
    fn single_historical_day_detection_rejects_cross_day_ranges() {
        let settings = AppSettings::default();
        let local_date =
            crate::local_usage::LocalUsageDatabase::today_local_date_with_settings(&settings);
        let (start, end) =
            crate::local_usage::LocalUsageDatabase::local_date_epoch_bounds_with_settings(
                &local_date,
                &settings,
            )
            .unwrap();
        assert!(!is_single_historical_day(&local_date, start, end, &settings).unwrap());
    }
}
