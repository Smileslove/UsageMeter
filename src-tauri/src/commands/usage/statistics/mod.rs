use super::types::{
    MonthActivity, ProxyState, StatisticsMetric, StatisticsQuery, StatisticsSummary, YearActivity,
};
use crate::models::AppSettings;

mod activity;
mod aggregate;
mod daily_summary;
mod hourly_summary;
mod shared;

#[tauri::command]
pub async fn get_statistics_summary(
    query: StatisticsQuery,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<StatisticsSummary, String> {
    if let Some(summary) =
        daily_summary::try_build_statistics_summary_from_daily_summary(&query, &settings).await?
    {
        return Ok(summary);
    }

    if let Some(summary) =
        hourly_summary::try_build_statistics_summary_from_hourly_cache(&query, &settings).await?
    {
        return Ok(summary);
    }

    let (start_epoch, end_epoch) = shared::normalize_range(&query);
    let include_errors = settings.proxy.include_error_requests;
    let (facts, _) = crate::unified_usage::get_merged_request_facts_no_sync(
        &settings,
        Some(start_epoch),
        Some(end_epoch),
        include_errors,
    )
    .await?;
    let summary = aggregate::build_merged_statistics(&facts, &query);
    Ok(summary)
}

#[tauri::command]
pub async fn get_month_activity(
    year: i32,
    month: u8,
    metric: StatisticsMetric,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<MonthActivity, String> {
    activity::get_month_activity_impl(year, month, metric, settings).await
}

#[tauri::command]
pub async fn get_year_activity(
    year: i32,
    metric: StatisticsMetric,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<YearActivity, String> {
    activity::get_year_activity_impl(year, metric, settings).await
}
