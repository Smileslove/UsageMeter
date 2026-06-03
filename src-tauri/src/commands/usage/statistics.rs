use super::helpers::perf_log;
use super::types::{
    DayActivity, MonthActivity, ProxyState, StatisticsBucket, StatisticsCapability,
    StatisticsInsight, StatisticsMetric, StatisticsModelBreakdown, StatisticsPerformance,
    StatisticsQuery, StatisticsRange, StatisticsStatusBreakdown, StatisticsSummary,
    StatisticsTotals, StatisticsTrendPoint, YearActivity, MERGED_SOURCE, MODEL_TREND_LIMIT,
};
use crate::models::{AppSettings, StatusCodeCount};
use crate::unified_usage::{has_partial_coverage, CoverageOrigin, MergedRequestFact};
use chrono::{Local, NaiveDate, TimeZone};
use std::collections::{HashMap, HashSet};

fn merged_stat_capability_from_facts(facts: &[MergedRequestFact]) -> StatisticsCapability {
    let has_status_codes = facts.iter().any(|fact| fact.status_code.is_some());
    let has_performance = facts
        .iter()
        .any(|fact| fact.output_tokens_per_second.is_some() || fact.ttft_ms.is_some());

    StatisticsCapability {
        has_basic_usage: true,
        has_performance,
        has_status_codes,
    }
}

fn add_fact_to_stat_acc(acc: &mut StatAccumulator, fact: &MergedRequestFact) {
    acc.add_tokens(
        fact.input_tokens,
        fact.output_tokens,
        fact.cache_create_tokens,
        fact.cache_read_tokens,
        1,
        fact.estimated_cost,
    );

    if matches!(fact.coverage_origin, CoverageOrigin::LocalOnly) {
        acc.local_request_count += 1;
    } else {
        acc.proxy_request_count += 1;
    }

    if let Some(status_code) = fact.status_code {
        if (200..300).contains(&status_code) {
            acc.success_requests += 1;
        } else if (400..500).contains(&status_code) {
            acc.client_error_requests += 1;
        } else if status_code >= 500 {
            acc.server_error_requests += 1;
        }
        *acc.status_code_counts.entry(status_code).or_insert(0) += 1;
    }

    if let Some(rate) = fact.output_tokens_per_second {
        if rate > 0.0 {
            acc.rate_sum += rate;
            acc.rate_count += 1;
        }
    }
    if let Some(ttft) = fact.ttft_ms {
        if ttft > 0 {
            acc.ttft_sum += ttft as f64;
            acc.ttft_count += 1;
        }
    }
}

#[derive(Default, Clone)]
pub(super) struct StatAccumulator {
    pub(super) request_count: u64,
    pub(super) local_request_count: u64,
    pub(super) proxy_request_count: u64,
    pub(super) total_tokens: u64,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_create_tokens: u64,
    pub(super) cache_read_tokens: u64,
    pub(super) cost: f64,
    pub(super) success_requests: u64,
    pub(super) client_error_requests: u64,
    pub(super) server_error_requests: u64,
    pub(super) rate_sum: f64,
    pub(super) rate_count: u64,
    pub(super) ttft_sum: f64,
    pub(super) ttft_count: u64,
    pub(super) status_code_counts: HashMap<u16, u64>,
}

impl StatAccumulator {
    pub(super) fn add_tokens(
        &mut self,
        input: u64,
        output: u64,
        cache_create: u64,
        cache_read: u64,
        requests: u64,
        cost: f64,
    ) {
        self.request_count += requests;
        self.input_tokens += input;
        self.output_tokens += output;
        self.cache_create_tokens += cache_create;
        self.cache_read_tokens += cache_read;
        self.total_tokens += input + output + cache_create + cache_read;
        self.cost += cost;
    }
}

fn normalize_range(query: &StatisticsQuery) -> (i64, i64) {
    let start = query.start_epoch.max(0);
    let end = query.end_epoch.max(start + 1);
    (start, end)
}

fn bucket_step_seconds(bucket: &StatisticsBucket) -> i64 {
    match bucket {
        StatisticsBucket::Hour => 60 * 60,
        StatisticsBucket::Day => 24 * 60 * 60,
    }
}

fn bucket_name(bucket: &StatisticsBucket) -> String {
    match bucket {
        StatisticsBucket::Hour => "hour".to_string(),
        StatisticsBucket::Day => "day".to_string(),
    }
}

fn bucket_start(epoch: i64, bucket: &StatisticsBucket) -> i64 {
    let step = bucket_step_seconds(bucket);
    (epoch / step) * step
}

fn bucket_label(epoch: i64, bucket: &StatisticsBucket) -> String {
    let dt = Local
        .timestamp_opt(epoch, 0)
        .single()
        .unwrap_or_else(Local::now);
    match bucket {
        StatisticsBucket::Hour => dt.format("%m-%d %H:00").to_string(),
        StatisticsBucket::Day => dt.format("%m-%d").to_string(),
    }
}

fn make_empty_trend(
    start_epoch: i64,
    end_epoch: i64,
    bucket: &StatisticsBucket,
) -> Vec<StatisticsTrendPoint> {
    let step = bucket_step_seconds(bucket);
    let mut points = Vec::new();
    let mut cursor = bucket_start(start_epoch, bucket);
    while cursor < end_epoch {
        points.push(StatisticsTrendPoint {
            start_epoch: cursor,
            label: bucket_label(cursor, bucket),
            ..Default::default()
        });
        cursor += step;
    }
    points
}

fn apply_acc_to_trend_point(point: &mut StatisticsTrendPoint, acc: &StatAccumulator) {
    point.request_count = acc.request_count;
    point.total_tokens = acc.total_tokens;
    point.input_tokens = acc.input_tokens;
    point.output_tokens = acc.output_tokens;
    point.cache_create_tokens = acc.cache_create_tokens;
    point.cache_read_tokens = acc.cache_read_tokens;
    point.cost = acc.cost;
    point.avg_tokens_per_second =
        (acc.rate_count > 0).then_some(acc.rate_sum / acc.rate_count as f64);
}

fn trend_from_map(
    trend_map: &HashMap<i64, StatAccumulator>,
    start_epoch: i64,
    end_epoch: i64,
    bucket: &StatisticsBucket,
) -> Vec<StatisticsTrendPoint> {
    let mut trend = make_empty_trend(start_epoch, end_epoch, bucket);
    for point in &mut trend {
        if let Some(acc) = trend_map.get(&point.start_epoch) {
            apply_acc_to_trend_point(point, acc);
        }
    }
    trend
}

fn value_for_metric(point: &StatisticsTrendPoint, metric: &StatisticsMetric) -> f64 {
    match metric {
        StatisticsMetric::Cost => point.cost,
        StatisticsMetric::Requests => point.request_count as f64,
        StatisticsMetric::Tokens => point.total_tokens as f64,
    }
}

fn totals_from_acc(acc: &StatAccumulator, model_count: u64, with_status: bool) -> StatisticsTotals {
    let error_requests = acc.client_error_requests + acc.server_error_requests;
    StatisticsTotals {
        request_count: acc.request_count,
        total_tokens: acc.total_tokens,
        input_tokens: acc.input_tokens,
        output_tokens: acc.output_tokens,
        cache_create_tokens: acc.cache_create_tokens,
        cache_read_tokens: acc.cache_read_tokens,
        cost: acc.cost,
        model_count,
        local_request_count: acc.local_request_count,
        proxy_request_count: acc.proxy_request_count,
        success_requests: with_status.then_some(acc.success_requests),
        error_requests: with_status.then_some(error_requests),
    }
}

fn build_insights(
    totals: &StatisticsTotals,
    trend: &[StatisticsTrendPoint],
    models: &[StatisticsModelBreakdown],
    metric: &StatisticsMetric,
    performance: Option<&StatisticsPerformance>,
) -> Vec<StatisticsInsight> {
    let mut insights = Vec::new();

    if let Some(peak) = trend.iter().max_by(|a, b| {
        value_for_metric(a, metric)
            .partial_cmp(&value_for_metric(b, metric))
            .unwrap_or(std::cmp::Ordering::Equal)
    }) {
        if value_for_metric(peak, metric) > 0.0 {
            insights.push(StatisticsInsight {
                kind: "peak".to_string(),
                level: "info".to_string(),
                value: match metric {
                    StatisticsMetric::Cost => format!("{:.4}", peak.cost),
                    StatisticsMetric::Requests => peak.request_count.to_string(),
                    StatisticsMetric::Tokens => peak.total_tokens.to_string(),
                },
                model_name: None,
                date: Some(peak.label.clone()),
            });
        }
    }

    if let Some(model) = models.first() {
        insights.push(StatisticsInsight {
            kind: "topModel".to_string(),
            level: "info".to_string(),
            value: format!("{:.1}", model.percent),
            model_name: Some(model.model_name.clone()),
            date: None,
        });
    }

    if let Some(error_requests) = totals.error_requests {
        if error_requests > 0 {
            insights.push(StatisticsInsight {
                kind: "errors".to_string(),
                level: "warning".to_string(),
                value: error_requests.to_string(),
                model_name: None,
                date: None,
            });
        }
    }

    if let Some(perf) = performance {
        if let Some(model) = &perf.slowest_model {
            insights.push(StatisticsInsight {
                kind: "slowestModel".to_string(),
                level: "info".to_string(),
                value: format!("{:.0}", perf.avg_ttft_ms),
                model_name: Some(model.clone()),
                date: None,
            });
        }
    }

    insights.truncate(4);
    insights
}

fn build_merged_statistics(
    facts: Vec<MergedRequestFact>,
    query: &StatisticsQuery,
) -> StatisticsSummary {
    let started_at = std::time::Instant::now();
    let (start_epoch, end_epoch) = normalize_range(query);
    let mut total = StatAccumulator::default();
    let mut trend_map: HashMap<i64, StatAccumulator> = HashMap::new();
    let mut model_map: HashMap<String, StatAccumulator> = HashMap::new();

    for fact in &facts {
        let model_name = if fact.model.is_empty() {
            "unknown".to_string()
        } else {
            fact.model.clone()
        };
        let bucket = bucket_start(fact.timestamp_sec, &query.bucket);
        add_fact_to_stat_acc(&mut total, fact);
        add_fact_to_stat_acc(trend_map.entry(bucket).or_default(), fact);
        add_fact_to_stat_acc(model_map.entry(model_name.clone()).or_default(), fact);
    }

    let mut trend = trend_from_map(&trend_map, start_epoch, end_epoch, &query.bucket);

    let mut models: Vec<StatisticsModelBreakdown> = model_map
        .into_iter()
        .map(|(model_name, acc)| {
            let mut status_codes: Vec<StatusCodeCount> = acc
                .status_code_counts
                .iter()
                .map(|(status_code, count)| StatusCodeCount {
                    status_code: *status_code,
                    count: *count,
                })
                .collect();
            status_codes.sort_by(|a, b| a.status_code.cmp(&b.status_code));

            let has_status = !status_codes.is_empty();
            let has_perf = acc.rate_count > 0 || acc.ttft_count > 0;

            StatisticsModelBreakdown {
                model_name: model_name.clone(),
                request_count: acc.request_count,
                local_request_count: acc.local_request_count,
                total_tokens: acc.total_tokens,
                input_tokens: acc.input_tokens,
                output_tokens: acc.output_tokens,
                cache_create_tokens: acc.cache_create_tokens,
                cache_read_tokens: acc.cache_read_tokens,
                cost: acc.cost,
                percent: if total.total_tokens > 0 {
                    (acc.total_tokens as f64 / total.total_tokens as f64) * 100.0
                } else {
                    0.0
                },
                avg_tokens_per_second: has_perf
                    .then_some(acc.rate_sum / acc.rate_count.max(1) as f64),
                avg_ttft_ms: (acc.ttft_count > 0).then_some(acc.ttft_sum / acc.ttft_count as f64),
                error_requests: has_status
                    .then_some(acc.client_error_requests + acc.server_error_requests),
                success_requests: has_status.then_some(acc.success_requests),
                client_error_requests: has_status.then_some(acc.client_error_requests),
                server_error_requests: has_status.then_some(acc.server_error_requests),
                status_codes,
                trend: Vec::new(),
            }
        })
        .collect();
    models.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens));

    let top_model_names: HashSet<String> = models
        .iter()
        .take(MODEL_TREND_LIMIT)
        .map(|model| model.model_name.clone())
        .collect();
    let mut model_trend_map: HashMap<String, HashMap<i64, StatAccumulator>> = HashMap::new();

    for fact in &facts {
        let model_name = if fact.model.is_empty() {
            "unknown"
        } else {
            fact.model.as_str()
        };
        if !top_model_names.contains(model_name) {
            continue;
        }
        let bucket = bucket_start(fact.timestamp_sec, &query.bucket);
        add_fact_to_stat_acc(
            model_trend_map
                .entry(model_name.to_string())
                .or_default()
                .entry(bucket)
                .or_default(),
            fact,
        );
    }

    for model in models.iter_mut().take(MODEL_TREND_LIMIT) {
        model.trend = model_trend_map
            .get(&model.model_name)
            .map(|trend_map| trend_from_map(trend_map, start_epoch, end_epoch, &query.bucket))
            .unwrap_or_else(|| make_empty_trend(start_epoch, end_epoch, &query.bucket));
    }

    let capability = merged_stat_capability_from_facts(&facts);
    if !capability.has_performance {
        for point in &mut trend {
            point.avg_tokens_per_second = None;
        }
        for model in &mut models {
            model.avg_tokens_per_second = None;
            model.avg_ttft_ms = None;
            for point in &mut model.trend {
                point.avg_tokens_per_second = None;
            }
        }
    }
    if !capability.has_status_codes {
        for model in &mut models {
            model.error_requests = None;
            model.success_requests = None;
            model.client_error_requests = None;
            model.server_error_requests = None;
            model.status_codes.clear();
        }
    }
    let performance = if capability.has_performance {
        let fastest_model = models
            .iter()
            .filter_map(|m| m.avg_tokens_per_second.map(|v| (m.model_name.clone(), v)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|m| m.0);
        let slowest_model = models
            .iter()
            .filter_map(|m| m.avg_ttft_ms.map(|v| (m.model_name.clone(), v)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|m| m.0);

        Some(StatisticsPerformance {
            request_count: total.rate_count.max(total.ttft_count),
            avg_tokens_per_second: if total.rate_count > 0 {
                total.rate_sum / total.rate_count as f64
            } else {
                0.0
            },
            avg_ttft_ms: if total.ttft_count > 0 {
                total.ttft_sum / total.ttft_count as f64
            } else {
                0.0
            },
            slowest_model,
            fastest_model,
        })
    } else {
        None
    };

    let status = if capability.has_status_codes {
        let status_total =
            total.success_requests + total.client_error_requests + total.server_error_requests;
        Some(StatisticsStatusBreakdown {
            success_requests: total.success_requests,
            client_error_requests: total.client_error_requests,
            server_error_requests: total.server_error_requests,
            success_rate: if status_total > 0 {
                (total.success_requests as f64 / status_total as f64) * 100.0
            } else {
                0.0
            },
        })
    } else {
        None
    };

    let totals = totals_from_acc(
        &total,
        models.len() as u64,
        total.success_requests + total.client_error_requests + total.server_error_requests > 0,
    );
    let insights = build_insights(
        &totals,
        &trend,
        &models,
        &query.metric,
        performance.as_ref(),
    );

    let summary = StatisticsSummary {
        generated_at_epoch: chrono::Utc::now().timestamp(),
        source: MERGED_SOURCE.to_string(),
        capability,
        range: StatisticsRange {
            start_epoch,
            end_epoch,
            timezone: query.timezone.clone(),
            bucket: bucket_name(&query.bucket),
        },
        totals,
        trend,
        models,
        performance,
        status,
        insights,
    };
    perf_log(
        "statistics_memory_aggregate",
        format!(
            "range={}..{} bucket={} facts={} models={} trend_points={} elapsed_ms={}",
            start_epoch,
            end_epoch,
            bucket_name(&query.bucket),
            facts.len(),
            summary.models.len(),
            summary.trend.len(),
            started_at.elapsed().as_millis(),
        ),
    );
    summary
}

fn collect_day_activity_from_facts(
    facts: Vec<MergedRequestFact>,
    day_map: &mut HashMap<String, (StatAccumulator, std::collections::HashSet<String>)>,
) {
    for fact in facts {
        let date = Local
            .timestamp_opt(fact.timestamp_sec, 0)
            .single()
            .unwrap_or_else(Local::now)
            .format("%Y-%m-%d")
            .to_string();
        let entry = day_map.entry(date).or_default();
        add_fact_to_stat_acc(&mut entry.0, &fact);
        if !fact.model.is_empty() {
            entry.1.insert(fact.model);
        }
    }
}

fn to_date_key(timestamp_sec: i64) -> String {
    Local
        .timestamp_opt(timestamp_sec, 0)
        .single()
        .unwrap_or_else(Local::now)
        .format("%Y-%m-%d")
        .to_string()
}

fn can_use_unified_daily_summary(settings: &AppSettings) -> bool {
    settings.client_tools.active_tool_filter.is_none()
        && settings.source_aware.active_source_filter.is_none()
}

fn day_activity_from_summary_row(
    row: &crate::local_usage::UnifiedDailySummaryRow,
    include_errors: bool,
) -> DayActivity {
    if include_errors {
        DayActivity {
            date: row.local_date.clone(),
            request_count: row.request_count,
            total_tokens: row.total_tokens,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_create_tokens: row.cache_create_tokens,
            cache_read_tokens: row.cache_read_tokens,
            cost: row.total_cost,
            model_count: row.model_count,
            success_requests: Some(row.success_request_count),
            error_requests: Some(row.client_error_requests + row.server_error_requests),
        }
    } else {
        DayActivity {
            date: row.local_date.clone(),
            request_count: row.visible_request_count,
            total_tokens: row.visible_total_tokens,
            input_tokens: row.visible_input_tokens,
            output_tokens: row.visible_output_tokens,
            cache_create_tokens: row.visible_cache_create_tokens,
            cache_read_tokens: row.visible_cache_read_tokens,
            cost: row.visible_cost,
            model_count: row.model_count,
            success_requests: Some(row.success_request_count),
            error_requests: Some(row.client_error_requests + row.server_error_requests),
        }
    }
}

async fn load_day_activity_from_summary_with_hot_overlay(
    start_epoch: i64,
    end_epoch: i64,
    include_errors: bool,
    settings: &AppSettings,
) -> Result<HashMap<String, DayActivity>, String> {
    crate::unified_usage::ensure_materialized_history(settings, start_epoch, end_epoch).await?;
    let local_db = crate::local_usage::ensure_local_usage_synced()?;
    let start_date = Local
        .timestamp_opt(start_epoch, 0)
        .single()
        .unwrap_or_else(Local::now)
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    let end_date = Local
        .timestamp_opt(end_epoch.saturating_sub(1), 0)
        .single()
        .unwrap_or_else(Local::now)
        .date_naive()
        .succ_opt()
        .unwrap_or_else(|| Local::now().date_naive())
        .format("%Y-%m-%d")
        .to_string();
    let today_date = crate::local_usage::LocalUsageDatabase::today_local_date();
    let mut by_date = HashMap::new();

    if start_date < today_date {
        let summary_end = if end_date < today_date {
            end_date.clone()
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
        crate::local_usage::LocalUsageDatabase::local_date_epoch_bounds(&today_date)?;
    if end_epoch > today_start && start_epoch < end_epoch {
        let hot_start = start_epoch.max(today_start);
        if end_epoch > hot_start {
            let (facts, _coverage) = crate::unified_usage::get_merged_request_facts(
                settings,
                Some(hot_start),
                Some(end_epoch),
                include_errors,
            )
            .await?;
            let mut day_map: HashMap<String, (StatAccumulator, std::collections::HashSet<String>)> =
                HashMap::new();
            collect_day_activity_from_facts(facts, &mut day_map);
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

fn build_daily_summary_from_facts(
    local_date: &str,
    facts: &[MergedRequestFact],
    materialized_at: i64,
) -> crate::local_usage::UnifiedDailySummaryRow {
    let mut summary = crate::local_usage::UnifiedDailySummaryRow {
        local_date: local_date.to_string(),
        materialized_at,
        ..Default::default()
    };
    let mut models = HashSet::new();
    let mut success_models = HashSet::new();
    for fact in facts {
        summary.request_count += 1;
        let visible = fact.status_code.map(|code| code < 300).unwrap_or(true);
        if visible {
            summary.visible_request_count += 1;
            summary.visible_total_tokens += fact.total_tokens;
            summary.visible_input_tokens += fact.input_tokens;
            summary.visible_output_tokens += fact.output_tokens;
            summary.visible_cache_create_tokens += fact.cache_create_tokens;
            summary.visible_cache_read_tokens += fact.cache_read_tokens;
            summary.visible_cost += fact.estimated_cost;
        }
        summary.total_tokens += fact.total_tokens;
        summary.input_tokens += fact.input_tokens;
        summary.output_tokens += fact.output_tokens;
        summary.cache_create_tokens += fact.cache_create_tokens;
        summary.cache_read_tokens += fact.cache_read_tokens;
        summary.total_cost += fact.estimated_cost;
        match fact.coverage_origin {
            CoverageOrigin::ProxyOnly => summary.proxy_backed_requests += 1,
            CoverageOrigin::LocalOnly => summary.local_only_requests += 1,
            CoverageOrigin::MergedProxyPreferred => {
                summary.proxy_backed_requests += 1;
                summary.merged_overlap_requests += 1;
            }
        }
        if !fact.model.trim().is_empty() {
            models.insert(fact.model.clone());
        }
        if let Some(status_code) = fact.status_code {
            if status_code < 400 {
                summary.success_request_count += 1;
                summary.success_total_tokens += fact.total_tokens;
                summary.success_input_tokens += fact.input_tokens;
                summary.success_output_tokens += fact.output_tokens;
                summary.success_cache_create_tokens += fact.cache_create_tokens;
                summary.success_cache_read_tokens += fact.cache_read_tokens;
                summary.success_cost += fact.estimated_cost;
                if !fact.model.trim().is_empty() {
                    success_models.insert(fact.model.clone());
                }
            } else if status_code < 500 {
                summary.client_error_requests += 1;
            } else {
                summary.server_error_requests += 1;
            }
        }
    }
    summary.model_count = models.len() as u64;
    summary.success_model_count = success_models.len() as u64;
    let has_partial =
        has_partial_coverage(summary.proxy_backed_requests, summary.local_only_requests);
    summary.has_partial_status_coverage = false;
    summary.has_partial_performance_coverage = has_partial;
    summary
}

fn build_daily_model_summaries_from_facts(
    local_date: &str,
    facts: &[MergedRequestFact],
    materialized_at: i64,
) -> Vec<crate::local_usage::UnifiedDailyModelSummaryRow> {
    let mut by_model: HashMap<String, crate::local_usage::UnifiedDailyModelSummaryRow> =
        HashMap::new();
    for fact in facts {
        let model_name = if fact.model.trim().is_empty() {
            "unknown".to_string()
        } else {
            fact.model.clone()
        };
        let entry = by_model.entry(model_name.clone()).or_insert_with(|| {
            crate::local_usage::UnifiedDailyModelSummaryRow {
                local_date: local_date.to_string(),
                model_name: model_name.clone(),
                materialized_at,
                ..Default::default()
            }
        });
        entry.request_count += 1;
        let visible = fact.status_code.map(|code| code < 300).unwrap_or(true);
        if visible {
            entry.visible_request_count += 1;
            entry.visible_total_tokens += fact.total_tokens;
            entry.visible_input_tokens += fact.input_tokens;
            entry.visible_output_tokens += fact.output_tokens;
            entry.visible_cache_create_tokens += fact.cache_create_tokens;
            entry.visible_cache_read_tokens += fact.cache_read_tokens;
            entry.visible_cost += fact.estimated_cost;
        }
        entry.total_tokens += fact.total_tokens;
        entry.input_tokens += fact.input_tokens;
        entry.output_tokens += fact.output_tokens;
        entry.cache_create_tokens += fact.cache_create_tokens;
        entry.cache_read_tokens += fact.cache_read_tokens;
        entry.total_cost += fact.estimated_cost;
        if let Some(rate) = fact.output_tokens_per_second {
            if rate > 0.0 {
                entry.rate_sum += rate;
                entry.rate_count += 1;
            }
        }
        if let Some(ttft_ms) = fact.ttft_ms {
            if ttft_ms > 0 {
                entry.ttft_sum += ttft_ms as f64;
                entry.ttft_count += 1;
            }
        }
        if matches!(fact.coverage_origin, CoverageOrigin::LocalOnly) {
            entry.local_only_requests += 1;
        }
        if let Some(status_code) = fact.status_code {
            *entry.status_code_counts.entry(status_code).or_insert(0) += 1;
            if status_code < 400 {
                entry.success_request_count += 1;
                entry.success_total_tokens += fact.total_tokens;
                entry.success_input_tokens += fact.input_tokens;
                entry.success_output_tokens += fact.output_tokens;
                entry.success_cache_create_tokens += fact.cache_create_tokens;
                entry.success_cache_read_tokens += fact.cache_read_tokens;
                entry.success_cost += fact.estimated_cost;
            } else if status_code < 500 {
                entry.client_error_requests += 1;
            } else {
                entry.server_error_requests += 1;
            }
        }
    }
    let mut rows: Vec<_> = by_model.into_values().collect();
    rows.sort_by(|a, b| a.model_name.cmp(&b.model_name));
    rows
}

async fn try_build_statistics_summary_from_daily_summary(
    query: &StatisticsQuery,
    settings: &AppSettings,
) -> Result<Option<StatisticsSummary>, String> {
    if !can_use_unified_daily_summary(settings) || !matches!(query.bucket, StatisticsBucket::Day) {
        return Ok(None);
    }

    let (start_epoch, end_epoch) = normalize_range(query);
    let include_errors = settings.proxy.include_error_requests;
    let local_db = crate::local_usage::ensure_local_usage_synced()?;
    crate::unified_usage::ensure_materialized_history(settings, start_epoch, end_epoch).await?;
    let start_date = Local
        .timestamp_opt(start_epoch, 0)
        .single()
        .unwrap_or_else(Local::now)
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    let end_date = Local
        .timestamp_opt(end_epoch.saturating_sub(1), 0)
        .single()
        .unwrap_or_else(Local::now)
        .date_naive()
        .succ_opt()
        .unwrap_or_else(|| Local::now().date_naive())
        .format("%Y-%m-%d")
        .to_string();
    let today_date = crate::local_usage::LocalUsageDatabase::today_local_date();

    let mut daily_rows = Vec::new();
    let mut model_rows = Vec::new();
    if start_date < today_date {
        let history_end = if end_date < today_date {
            end_date.clone()
        } else {
            today_date.clone()
        };
        daily_rows.extend(local_db.get_unified_daily_summaries_between(&start_date, &history_end)?);
        model_rows
            .extend(local_db.get_unified_daily_model_summaries_between(&start_date, &history_end)?);
    }

    let (today_start, _) =
        crate::local_usage::LocalUsageDatabase::local_date_epoch_bounds(&today_date)?;
    if end_epoch > today_start {
        let hot_start = start_epoch.max(today_start);
        if end_epoch > hot_start {
            let (facts, _coverage) = crate::unified_usage::get_merged_request_facts(
                settings,
                Some(hot_start),
                Some(end_epoch),
                include_errors,
            )
            .await?;
            let now_ms = chrono::Utc::now().timestamp_millis();
            daily_rows.push(build_daily_summary_from_facts(&today_date, &facts, now_ms));
            model_rows.extend(build_daily_model_summaries_from_facts(
                &today_date,
                &facts,
                now_ms,
            ));
        }
    }

    let mut row_by_date = HashMap::new();
    for row in daily_rows {
        row_by_date.insert(row.local_date.clone(), row);
    }

    let mut trend = make_empty_trend(start_epoch, end_epoch, &query.bucket);
    let mut totals = StatisticsTotals::default();
    let mut total_success_requests = 0_u64;
    let mut total_client_error_requests = 0_u64;
    let mut total_server_error_requests = 0_u64;
    for point in &mut trend {
        let date_key = Local
            .timestamp_opt(point.start_epoch, 0)
            .single()
            .unwrap_or_else(Local::now)
            .date_naive()
            .format("%Y-%m-%d")
            .to_string();
        let Some(row) = row_by_date.get(&date_key) else {
            continue;
        };
        let use_visible_only = !include_errors;
        point.request_count = if use_visible_only {
            row.visible_request_count
        } else {
            row.request_count
        };
        point.total_tokens = if use_visible_only {
            row.visible_total_tokens
        } else {
            row.total_tokens
        };
        point.input_tokens = if use_visible_only {
            row.visible_input_tokens
        } else {
            row.input_tokens
        };
        point.output_tokens = if use_visible_only {
            row.visible_output_tokens
        } else {
            row.output_tokens
        };
        point.cache_create_tokens = if use_visible_only {
            row.visible_cache_create_tokens
        } else {
            row.cache_create_tokens
        };
        point.cache_read_tokens = if use_visible_only {
            row.visible_cache_read_tokens
        } else {
            row.cache_read_tokens
        };
        point.cost = if use_visible_only {
            row.visible_cost
        } else {
            row.total_cost
        };
        point.avg_tokens_per_second = None;

        totals.request_count += point.request_count;
        totals.total_tokens += point.total_tokens;
        totals.input_tokens += point.input_tokens;
        totals.output_tokens += point.output_tokens;
        totals.cache_create_tokens += point.cache_create_tokens;
        totals.cache_read_tokens += point.cache_read_tokens;
        totals.cost += point.cost;
        totals.local_request_count += row.local_only_requests;
        totals.proxy_request_count += row.proxy_backed_requests;
        total_success_requests += row.success_request_count;
        total_client_error_requests += row.client_error_requests;
        total_server_error_requests += row.server_error_requests;
    }

    #[derive(Default)]
    struct ModelAgg {
        request_count: u64,
        local_request_count: u64,
        total_tokens: u64,
        input_tokens: u64,
        output_tokens: u64,
        cache_create_tokens: u64,
        cache_read_tokens: u64,
        cost: f64,
        success_requests: u64,
        client_error_requests: u64,
        server_error_requests: u64,
        rate_sum: f64,
        rate_count: u64,
        ttft_sum: f64,
        ttft_count: u64,
        status_code_counts: HashMap<u16, u64>,
    }

    let mut model_totals: HashMap<String, ModelAgg> = HashMap::new();
    let mut model_day_map: HashMap<String, HashMap<i64, StatisticsTrendPoint>> = HashMap::new();
    for row in model_rows {
        let use_visible_only = !include_errors;
        let agg = model_totals.entry(row.model_name.clone()).or_default();
        agg.request_count += if use_visible_only {
            row.visible_request_count
        } else {
            row.request_count
        };
        agg.local_request_count += row.local_only_requests;
        agg.total_tokens += if use_visible_only {
            row.visible_total_tokens
        } else {
            row.total_tokens
        };
        agg.input_tokens += if use_visible_only {
            row.visible_input_tokens
        } else {
            row.input_tokens
        };
        agg.output_tokens += if use_visible_only {
            row.visible_output_tokens
        } else {
            row.output_tokens
        };
        agg.cache_create_tokens += if use_visible_only {
            row.visible_cache_create_tokens
        } else {
            row.cache_create_tokens
        };
        agg.cache_read_tokens += if use_visible_only {
            row.visible_cache_read_tokens
        } else {
            row.cache_read_tokens
        };
        agg.cost += if use_visible_only {
            row.visible_cost
        } else {
            row.total_cost
        };
        agg.success_requests += row.success_request_count;
        agg.client_error_requests += row.client_error_requests;
        agg.server_error_requests += row.server_error_requests;
        agg.rate_sum += row.rate_sum;
        agg.rate_count += row.rate_count;
        agg.ttft_sum += row.ttft_sum;
        agg.ttft_count += row.ttft_count;
        for (status_code, count) in row.status_code_counts {
            *agg.status_code_counts.entry(status_code).or_insert(0) += count;
        }

        let day_start = Local
            .from_local_datetime(
                &NaiveDate::parse_from_str(&row.local_date, "%Y-%m-%d")
                    .ok()
                    .and_then(|date| date.and_hms_opt(0, 0, 0))
                    .unwrap_or_else(|| Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap()),
            )
            .single()
            .unwrap_or_else(Local::now)
            .timestamp();
        let point = model_day_map
            .entry(row.model_name.clone())
            .or_default()
            .entry(day_start)
            .or_insert_with(|| StatisticsTrendPoint {
                start_epoch: day_start,
                label: row.local_date.clone(),
                ..Default::default()
            });
        point.request_count += if use_visible_only {
            row.visible_request_count
        } else {
            row.request_count
        };
        point.total_tokens += if use_visible_only {
            row.visible_total_tokens
        } else {
            row.total_tokens
        };
        point.input_tokens += if use_visible_only {
            row.visible_input_tokens
        } else {
            row.input_tokens
        };
        point.output_tokens += if use_visible_only {
            row.visible_output_tokens
        } else {
            row.output_tokens
        };
        point.cache_create_tokens += if use_visible_only {
            row.visible_cache_create_tokens
        } else {
            row.cache_create_tokens
        };
        point.cache_read_tokens += if use_visible_only {
            row.visible_cache_read_tokens
        } else {
            row.cache_read_tokens
        };
        point.cost += if use_visible_only {
            row.visible_cost
        } else {
            row.total_cost
        };
    }

    let mut models: Vec<StatisticsModelBreakdown> = model_totals
        .into_iter()
        .map(|(model_name, agg)| {
            let mut status_codes: Vec<StatusCodeCount> = agg
                .status_code_counts
                .into_iter()
                .map(|(status_code, count)| StatusCodeCount { status_code, count })
                .collect();
            status_codes.sort_by(|a, b| a.status_code.cmp(&b.status_code));
            let mut trend_points = make_empty_trend(start_epoch, end_epoch, &query.bucket);
            if let Some(points) = model_day_map.get(&model_name) {
                for point in &mut trend_points {
                    if let Some(saved) = points.get(&point.start_epoch) {
                        *point = saved.clone();
                    }
                }
            }
            StatisticsModelBreakdown {
                model_name,
                request_count: agg.request_count,
                local_request_count: agg.local_request_count,
                total_tokens: agg.total_tokens,
                input_tokens: agg.input_tokens,
                output_tokens: agg.output_tokens,
                cache_create_tokens: agg.cache_create_tokens,
                cache_read_tokens: agg.cache_read_tokens,
                cost: agg.cost,
                percent: if totals.total_tokens > 0 {
                    (agg.total_tokens as f64 / totals.total_tokens as f64) * 100.0
                } else {
                    0.0
                },
                avg_tokens_per_second: (agg.rate_count > 0)
                    .then_some(agg.rate_sum / agg.rate_count as f64),
                avg_ttft_ms: (agg.ttft_count > 0).then_some(agg.ttft_sum / agg.ttft_count as f64),
                error_requests: Some(agg.client_error_requests + agg.server_error_requests),
                success_requests: Some(agg.success_requests),
                client_error_requests: Some(agg.client_error_requests),
                server_error_requests: Some(agg.server_error_requests),
                status_codes,
                trend: trend_points,
            }
        })
        .collect();
    models.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens));

    totals.model_count = models.len() as u64;
    let capability = StatisticsCapability {
        has_basic_usage: true,
        has_performance: models
            .iter()
            .any(|model| model.avg_tokens_per_second.is_some() || model.avg_ttft_ms.is_some()),
        has_status_codes: total_success_requests
            + total_client_error_requests
            + total_server_error_requests
            > 0,
    };
    if !capability.has_status_codes {
        for model in &mut models {
            model.error_requests = None;
            model.success_requests = None;
            model.client_error_requests = None;
            model.server_error_requests = None;
            model.status_codes.clear();
        }
    } else {
        totals.success_requests = Some(total_success_requests);
        totals.error_requests = Some(total_client_error_requests + total_server_error_requests);
    }
    if !capability.has_performance {
        for model in &mut models {
            model.avg_tokens_per_second = None;
            model.avg_ttft_ms = None;
        }
    }

    let performance = if capability.has_performance {
        let fastest_model = models
            .iter()
            .filter_map(|m| m.avg_tokens_per_second.map(|v| (m.model_name.clone(), v)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|m| m.0);
        let slowest_model = models
            .iter()
            .filter_map(|m| m.avg_ttft_ms.map(|v| (m.model_name.clone(), v)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|m| m.0);
        let rate_values: Vec<f64> = models
            .iter()
            .filter_map(|m| m.avg_tokens_per_second)
            .collect();
        let ttft_values: Vec<f64> = models.iter().filter_map(|m| m.avg_ttft_ms).collect();
        Some(StatisticsPerformance {
            request_count: models.iter().map(|m| m.request_count).sum(),
            avg_tokens_per_second: if rate_values.is_empty() {
                0.0
            } else {
                rate_values.iter().sum::<f64>() / rate_values.len() as f64
            },
            avg_ttft_ms: if ttft_values.is_empty() {
                0.0
            } else {
                ttft_values.iter().sum::<f64>() / ttft_values.len() as f64
            },
            slowest_model,
            fastest_model,
        })
    } else {
        None
    };

    let status = if capability.has_status_codes {
        let status_total =
            total_success_requests + total_client_error_requests + total_server_error_requests;
        Some(StatisticsStatusBreakdown {
            success_requests: total_success_requests,
            client_error_requests: total_client_error_requests,
            server_error_requests: total_server_error_requests,
            success_rate: if status_total > 0 {
                (total_success_requests as f64 / status_total as f64) * 100.0
            } else {
                0.0
            },
        })
    } else {
        None
    };

    let insights = build_insights(
        &totals,
        &trend,
        &models,
        &query.metric,
        performance.as_ref(),
    );
    Ok(Some(StatisticsSummary {
        generated_at_epoch: chrono::Utc::now().timestamp(),
        source: MERGED_SOURCE.to_string(),
        capability,
        range: StatisticsRange {
            start_epoch,
            end_epoch,
            timezone: query.timezone.clone(),
            bucket: bucket_name(&query.bucket),
        },
        totals,
        trend,
        models,
        performance,
        status,
        insights,
    }))
}

fn month_day_count(year: i32, month: u8) -> u32 {
    for day in (28..=31).rev() {
        if NaiveDate::from_ymd_opt(year, month as u32, day).is_some() {
            return day;
        }
    }
    30
}

// ── Commands ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_statistics_summary(
    query: StatisticsQuery,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<StatisticsSummary, String> {
    let started_at = std::time::Instant::now();
    if let Some(summary) =
        try_build_statistics_summary_from_daily_summary(&query, &settings).await?
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
    let (start_epoch, end_epoch) = normalize_range(&query);
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
    let summary = build_merged_statistics(facts, &query);
    perf_log(
        "get_statistics_summary",
        format!(
            "range={}..{} bucket={} facts={} build_ms={} total_ms={}",
            start_epoch,
            end_epoch,
            bucket_name(&query.bucket),
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
    let (days_by_date, facts_count, path_label) = if can_use_unified_daily_summary(&settings) {
        crate::unified_usage::ensure_materialized_history(&settings, month_start, month_end)
            .await?;
        (
            load_day_activity_from_summary_with_hot_overlay(
                month_start,
                month_end,
                include_errors,
                &settings,
            )
            .await?,
            0,
            "summary+hot",
        )
    } else {
        let mut day_map: HashMap<String, (StatAccumulator, std::collections::HashSet<String>)> =
            HashMap::new();
        let (facts, _coverage) = crate::unified_usage::get_merged_request_facts(
            &settings,
            Some(month_start),
            Some(month_end),
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
        (days_by_date, facts_count, "facts")
    };

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

#[tauri::command]
pub async fn get_year_activity(
    year: i32,
    metric: StatisticsMetric,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
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
    let (days_by_date, facts_count, path_label) = if can_use_unified_daily_summary(&settings) {
        crate::unified_usage::ensure_materialized_history(&settings, year_start, year_end).await?;
        (
            load_day_activity_from_summary_with_hot_overlay(
                year_start,
                year_end,
                include_errors,
                &settings,
            )
            .await?,
            0,
            "summary+hot",
        )
    } else {
        let mut day_map: HashMap<String, (StatAccumulator, std::collections::HashSet<String>)> =
            HashMap::new();
        let (facts, _coverage) = crate::unified_usage::get_merged_request_facts(
            &settings,
            Some(year_start),
            Some(year_end),
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
        (days_by_date, facts_count, "facts")
    };

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
