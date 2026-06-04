use super::helpers::perf_log;
use super::types::{
    MonthActivity, ProxyState, StatisticsMetric, StatisticsQuery, StatisticsSummary, YearActivity,
};
use crate::models::AppSettings;

mod activity;
mod aggregate;
mod daily_summary;
mod shared;

#[tauri::command]
pub async fn get_statistics_summary(
    query: StatisticsQuery,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<StatisticsSummary, String> {
    let started_at = std::time::Instant::now();
    if let Some(summary) =
        daily_summary::try_build_statistics_summary_from_daily_summary(&query, &settings).await?
    {
        perf_log(
            "get_statistics_summary",
            format!(
                "range={}..{} bucket={} path=summary+hot models={} trend_points={} total_ms={}",
                summary.range.start_epoch,
                summary.range.end_epoch,
                summary.range.bucket,
                summary.models.len(),
                summary.trend.len(),
                started_at.elapsed().as_millis(),
            ),
        );
        return Ok(summary);
    }

    let (start_epoch, end_epoch) = shared::normalize_range(&query);
    let include_errors = settings.proxy.include_error_requests;
    let (facts, _) = crate::unified_usage::get_merged_request_facts(
        &settings,
        Some(start_epoch),
        Some(end_epoch),
        include_errors,
    )
    .await?;
    let facts_count = facts.len();
    let build_started_at = std::time::Instant::now();
    let summary = aggregate::build_merged_statistics(facts, &query);
    perf_log(
        "get_statistics_summary",
        format!(
            "range={}..{} bucket={} facts={} build_ms={} total_ms={}",
            start_epoch,
            end_epoch,
            shared::bucket_name(&query.bucket),
            facts_count,
            build_started_at.elapsed().as_millis(),
            started_at.elapsed().as_millis(),
        ),
    );
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
