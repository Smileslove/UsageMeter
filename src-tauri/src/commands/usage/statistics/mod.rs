use super::types::{
    MonthActivity, ProxyState, StatisticsMetric, StatisticsQuery, StatisticsSummary, YearActivity,
};
use crate::models::AppSettings;

pub(crate) mod activity;
mod aggregate;
mod daily_summary;
mod hourly_summary;
mod shared;

pub(crate) async fn get_statistics_summary_no_sync(
    query: &StatisticsQuery,
    settings: &AppSettings,
) -> Result<StatisticsSummary, String> {
    if let Some(summary) =
        daily_summary::try_build_statistics_summary_from_daily_summary(query, settings).await?
    {
        return Ok(summary);
    }

    if let Some(summary) =
        hourly_summary::try_build_statistics_summary_from_hourly_cache(query, settings).await?
    {
        return Ok(summary);
    }

    let (start_epoch, end_epoch) = shared::normalize_range(query);
    let include_errors = settings.proxy.include_error_requests;
    let (facts, _) = crate::unified_usage::get_merged_request_facts_no_sync(
        settings,
        Some(start_epoch),
        Some(end_epoch),
        include_errors,
    )
    .await?;
    Ok(aggregate::build_merged_statistics(&facts, query))
}

#[tauri::command]
pub async fn get_statistics_summary(
    query: StatisticsQuery,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<StatisticsSummary, String> {
    get_statistics_summary_no_sync(&query, &settings).await
}

#[tauri::command]
pub async fn get_month_activity(
    app: tauri::AppHandle,
    year: i32,
    month: u8,
    metric: StatisticsMetric,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<MonthActivity, String> {
    let next_month = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month as u32 + 1)
    };
    let (month_start, month_end) = activity::resolve_period_bounds(
        &format!("{year}-{month:02}-01"),
        &format!("{}-{:02}-01", next_month.0, next_month.1),
        &settings,
    )?;
    // 缓存先行：activity 缓存只存完整物化结果（deferred 快照不写缓存），命中即可
    // 直接返回，避免缓存命中时仍跑 count_stale_materialization_days 的逐日统计。
    if let Some(activity) = activity::lookup_month_activity_cached(
        year,
        month,
        &metric,
        month_start,
        month_end,
        &settings,
    )? {
        return Ok(activity);
    }
    let stale =
        crate::unified_usage::count_stale_materialization_days(&settings, month_start, month_end)?;
    if stale > activity::MAX_SYNC_MATERIALIZATION_DAYS {
        activity::spawn_background_activity_materialization(
            app,
            settings.clone(),
            month_start,
            month_end,
            activity::ActivityMaterializationScope {
                kind: "month",
                year,
                month: Some(month),
                metric: metric.clone(),
            },
        );
        return activity::get_month_activity_deferred(year, month, metric, settings).await;
    }
    activity::get_month_activity_impl(year, month, metric, settings).await
}

#[tauri::command]
pub async fn get_year_activity(
    app: tauri::AppHandle,
    year: i32,
    metric: StatisticsMetric,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<YearActivity, String> {
    let (year_start, year_end) = activity::resolve_period_bounds(
        &format!("{year}-01-01"),
        &format!("{}-01-01", year + 1),
        &settings,
    )?;
    // 缓存先行（同 get_month_activity）。
    if let Some(activity) =
        activity::lookup_year_activity_cached(year, &metric, year_start, year_end, &settings)?
    {
        return Ok(activity);
    }
    let stale =
        crate::unified_usage::count_stale_materialization_days(&settings, year_start, year_end)?;
    if stale > activity::MAX_SYNC_MATERIALIZATION_DAYS {
        activity::spawn_background_activity_materialization(
            app,
            settings.clone(),
            year_start,
            year_end,
            activity::ActivityMaterializationScope {
                kind: "year",
                year,
                month: None,
                metric: metric.clone(),
            },
        );
        return activity::get_year_activity_deferred(year, metric, settings).await;
    }
    activity::get_year_activity_impl(year, metric, settings).await
}
