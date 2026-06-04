use super::super::helpers::perf_log;
use super::super::types::{DayActivity, MonthActivity, StatisticsMetric, YearActivity};
use super::daily_summary::{
    can_use_unified_daily_summary, load_day_activity_from_summary_with_hot_overlay,
};
use super::shared::{
    collect_day_activity_from_facts, month_day_count, to_date_key, DayAccumulatorMap,
};
use crate::models::AppSettings;
use chrono::{Local, NaiveDate, TimeZone};
use std::collections::HashMap;

async fn load_activity_day_map(
    start_epoch: i64,
    end_epoch: i64,
    include_errors: bool,
    settings: &AppSettings,
) -> Result<(HashMap<String, DayActivity>, usize, &'static str), String> {
    if can_use_unified_daily_summary(settings) {
        crate::unified_usage::ensure_materialized_history(settings, start_epoch, end_epoch).await?;
        return Ok((
            load_day_activity_from_summary_with_hot_overlay(
                start_epoch,
                end_epoch,
                include_errors,
                settings,
            )
            .await?,
            0,
            "summary+hot",
        ));
    }

    let mut day_map: DayAccumulatorMap = HashMap::new();
    let (facts, _coverage) = crate::unified_usage::get_merged_request_facts(
        settings,
        Some(start_epoch),
        Some(end_epoch),
        include_errors,
    )
    .await?;
    let facts_count = facts.len();
    collect_day_activity_from_facts(facts, &mut day_map);
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

    Ok((days_by_date, facts_count, "facts"))
}

pub(super) async fn get_month_activity_impl(
    year: i32,
    month: u8,
    metric: StatisticsMetric,
    settings: AppSettings,
) -> Result<MonthActivity, String> {
    let started_at = std::time::Instant::now();
    let day_count = month_day_count(year, month);
    let month_start = Local
        .with_ymd_and_hms(year, month as u32, 1, 0, 0, 0)
        .single()
        .unwrap_or_else(Local::now)
        .timestamp();
    let next_month = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month as u32 + 1)
    };
    let month_end = Local
        .with_ymd_and_hms(next_month.0, next_month.1, 1, 0, 0, 0)
        .single()
        .unwrap_or_else(Local::now)
        .timestamp();

    let include_errors = settings.proxy.include_error_requests;
    let aggregate_started_at = std::time::Instant::now();
    let (days_by_date, facts_count, path_label) =
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
        timezone: settings.timezone,
        metric,
        days,
    };
    let today_key = to_date_key(Local::now().timestamp());
    let today_requests = activity
        .days
        .iter()
        .find(|day| day.date == today_key)
        .map(|day| day.request_count)
        .unwrap_or(0);
    perf_log(
        "get_month_activity",
        format!(
            "year={} month={} path={} facts={} days={} today_requests={} aggregate_ms={} total_ms={}",
            year,
            month,
            path_label,
            facts_count,
            activity.days.len(),
            today_requests,
            aggregate_started_at.elapsed().as_millis(),
            started_at.elapsed().as_millis(),
        ),
    );
    Ok(activity)
}

pub(super) async fn get_year_activity_impl(
    year: i32,
    metric: StatisticsMetric,
    settings: AppSettings,
) -> Result<YearActivity, String> {
    let started_at = std::time::Instant::now();
    let year_start = Local
        .with_ymd_and_hms(year, 1, 1, 0, 0, 0)
        .single()
        .unwrap_or_else(Local::now)
        .timestamp();
    let year_end = Local
        .with_ymd_and_hms(year + 1, 1, 1, 0, 0, 0)
        .single()
        .unwrap_or_else(Local::now)
        .timestamp();

    let include_errors = settings.proxy.include_error_requests;
    let aggregate_started_at = std::time::Instant::now();
    let (days_by_date, facts_count, path_label) =
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
        timezone: settings.timezone,
        metric,
        days,
    };
    let today_key = to_date_key(Local::now().timestamp());
    let today_requests = activity
        .days
        .iter()
        .find(|day| day.date == today_key)
        .map(|day| day.request_count)
        .unwrap_or(0);
    perf_log(
        "get_year_activity",
        format!(
            "year={} path={} facts={} days={} today_requests={} aggregate_ms={} total_ms={}",
            year,
            path_label,
            facts_count,
            activity.days.len(),
            today_requests,
            aggregate_started_at.elapsed().as_millis(),
            started_at.elapsed().as_millis(),
        ),
    );
    Ok(activity)
}
