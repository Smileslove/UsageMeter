use super::super::types::{
    DayActivity, StatisticsBucket, StatisticsCapability, StatisticsModelBreakdown,
    StatisticsPerformance, StatisticsQuery, StatisticsRange, StatisticsStatusBreakdown,
    StatisticsSummary, StatisticsTotals, StatisticsTrendPoint, MERGED_SOURCE,
};
use super::aggregate::build_insights;
use super::shared::{
    bucket_name, collect_day_activity_from_facts, local_date_start_epoch, make_empty_trend,
    normalize_range, DayAccumulatorMap,
};
use crate::models::{AppSettings, StatusCodeCount};
use crate::unified_usage::{
    has_partial_coverage, normalize_model_bucket, CoverageOrigin, MergedRequestFact,
};
use std::collections::HashMap;

fn next_business_date(date: &str, settings: &AppSettings) -> Result<String, String> {
    let (_, end_epoch) = crate::utils::business_time::business_date_epoch_bounds(date, settings)?;
    Ok(crate::utils::business_time::business_date_for_timestamp(
        end_epoch, settings,
    ))
}

pub(super) fn can_use_unified_daily_summary(settings: &AppSettings) -> bool {
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

pub(super) async fn load_day_activity_from_summary_with_hot_overlay(
    start_epoch: i64,
    end_epoch: i64,
    include_errors: bool,
    settings: &AppSettings,
) -> Result<HashMap<String, DayActivity>, String> {
    crate::unified_usage::ensure_materialized_history_no_sync(settings, start_epoch, end_epoch)
        .await?;
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
            collect_day_activity_from_facts(facts, &mut day_map, settings);
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
    let mut models = std::collections::HashSet::new();
    let mut success_models = std::collections::HashSet::new();
    for fact in facts {
        let request_count = fact.request_count.max(1);
        summary.request_count += request_count;
        let visible = fact.status_code.map(|code| code < 300).unwrap_or(true);
        if visible {
            summary.visible_request_count += request_count;
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
            CoverageOrigin::ProxyOnly => summary.proxy_backed_requests += request_count,
            CoverageOrigin::LocalOnly => summary.local_only_requests += request_count,
            CoverageOrigin::MergedProxyPreferred => {
                summary.proxy_backed_requests += request_count;
                summary.merged_overlap_requests += request_count;
            }
        }
        if !fact.model.trim().is_empty() {
            models.insert(fact.model.clone());
        }
        if let Some(status_code) = fact.status_code {
            if status_code < 400 {
                summary.success_request_count += request_count;
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
                summary.client_error_requests += request_count;
            } else {
                summary.server_error_requests += request_count;
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
        let model_name = normalize_model_bucket(&fact.tool, &fact.model);
        let entry = by_model.entry(model_name.clone()).or_insert_with(|| {
            crate::local_usage::UnifiedDailyModelSummaryRow {
                local_date: local_date.to_string(),
                model_name: model_name.clone(),
                materialized_at,
                ..Default::default()
            }
        });
        let request_count = fact.request_count.max(1);
        entry.request_count += request_count;
        let visible = fact.status_code.map(|code| code < 300).unwrap_or(true);
        if visible {
            entry.visible_request_count += request_count;
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
            entry.local_only_requests += request_count;
        }
        if let Some(status_code) = fact.status_code {
            *entry.status_code_counts.entry(status_code).or_insert(0) += request_count;
            if status_code < 400 {
                entry.success_request_count += request_count;
                entry.success_total_tokens += fact.total_tokens;
                entry.success_input_tokens += fact.input_tokens;
                entry.success_output_tokens += fact.output_tokens;
                entry.success_cache_create_tokens += fact.cache_create_tokens;
                entry.success_cache_read_tokens += fact.cache_read_tokens;
                entry.success_cost += fact.estimated_cost;
            } else if status_code < 500 {
                entry.client_error_requests += request_count;
            } else {
                entry.server_error_requests += request_count;
            }
        }
    }
    let mut rows: Vec<_> = by_model.into_values().collect();
    rows.sort_by(|a, b| a.model_name.cmp(&b.model_name));
    rows
}

pub(super) async fn try_build_statistics_summary_from_daily_summary(
    query: &StatisticsQuery,
    settings: &AppSettings,
) -> Result<Option<StatisticsSummary>, String> {
    if !can_use_unified_daily_summary(settings) || !matches!(query.bucket, StatisticsBucket::Day) {
        return Ok(None);
    }

    let (start_epoch, end_epoch) = normalize_range(query);
    let include_errors = settings.proxy.include_error_requests;
    let local_db = crate::local_usage::get_local_usage_db()?;
    crate::unified_usage::ensure_materialized_history_no_sync(settings, start_epoch, end_epoch)
        .await?;
    let start_date =
        crate::utils::business_time::business_date_for_timestamp(start_epoch, settings);
    let end_date = crate::utils::business_time::business_date_for_timestamp(
        end_epoch.saturating_sub(1),
        settings,
    );
    let today_date =
        crate::local_usage::LocalUsageDatabase::today_local_date_with_settings(settings);

    let mut daily_rows = Vec::new();
    let mut model_rows = Vec::new();
    if start_date < today_date {
        let history_end = if end_date < today_date {
            next_business_date(&end_date, settings)?
        } else {
            today_date.clone()
        };
        daily_rows.extend(local_db.get_unified_daily_summaries_between(&start_date, &history_end)?);
        model_rows
            .extend(local_db.get_unified_daily_model_summaries_between(&start_date, &history_end)?);
    }

    let (today_start, _) =
        crate::local_usage::LocalUsageDatabase::local_date_epoch_bounds_with_settings(
            &today_date,
            settings,
        )?;
    if end_epoch > today_start {
        let hot_start = start_epoch.max(today_start);
        if end_epoch > hot_start {
            let (facts, _coverage) = crate::unified_usage::get_merged_request_facts_no_sync(
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
        let date_key =
            crate::utils::business_time::business_date_for_timestamp(point.start_epoch, settings);
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

        let day_start = local_date_start_epoch(&row.local_date, settings);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AppSettings;
    use crate::unified_usage::CoverageOrigin;

    fn test_fact(
        model: &str,
        status_code: Option<u16>,
        coverage_origin: CoverageOrigin,
        input_tokens: u64,
        output_tokens: u64,
        cache_create_tokens: u64,
        cache_read_tokens: u64,
        cost: f64,
    ) -> MergedRequestFact {
        MergedRequestFact {
            canonical_request_key: format!("{model}:{input_tokens}:{output_tokens}"),
            session_id: "session".to_string(),
            project_name: None,
            project_path: None,
            api_key_prefix: None,
            request_base_url: None,
            tool: "claude_code".to_string(),
            timestamp_sec: 1_700_000_000,
            timestamp_ms: 1_700_000_000_000,
            model: model.to_string(),
            input_tokens,
            output_tokens,
            cache_create_tokens,
            cache_read_tokens,
            total_tokens: input_tokens + output_tokens + cache_create_tokens + cache_read_tokens,
            request_count: 1,
            estimated_cost: cost,
            coverage_origin,
            status_code,
            duration_ms: None,
            output_tokens_per_second: Some(20.0),
            ttft_ms: Some(300),
            source_label: None,
        }
    }

    #[test]
    fn can_use_unified_daily_summary_requires_no_active_filters() {
        let mut settings = AppSettings::default();
        assert!(can_use_unified_daily_summary(&settings));

        settings.client_tools.active_tool_filter = Some("codex".to_string());
        assert!(!can_use_unified_daily_summary(&settings));

        settings.client_tools.active_tool_filter = None;
        settings.source_aware.active_source_filter = Some("source-1".to_string());
        assert!(!can_use_unified_daily_summary(&settings));
    }

    #[test]
    fn day_activity_from_summary_row_switches_between_visible_and_total_values() {
        let row = crate::local_usage::UnifiedDailySummaryRow {
            local_date: "2026-06-01".to_string(),
            request_count: 10,
            visible_request_count: 8,
            total_tokens: 1000,
            visible_total_tokens: 800,
            input_tokens: 600,
            visible_input_tokens: 480,
            output_tokens: 300,
            visible_output_tokens: 240,
            cache_create_tokens: 50,
            visible_cache_create_tokens: 40,
            cache_read_tokens: 50,
            visible_cache_read_tokens: 40,
            total_cost: 1.5,
            visible_cost: 1.2,
            success_request_count: 7,
            client_error_requests: 2,
            server_error_requests: 1,
            model_count: 3,
            ..Default::default()
        };

        let all = day_activity_from_summary_row(&row, true);
        let visible = day_activity_from_summary_row(&row, false);

        assert_eq!(all.request_count, 10);
        assert_eq!(all.total_tokens, 1000);
        assert_eq!(all.cost, 1.5);
        assert_eq!(visible.request_count, 8);
        assert_eq!(visible.total_tokens, 800);
        assert_eq!(visible.cost, 1.2);
        assert_eq!(visible.success_requests, Some(7));
        assert_eq!(visible.error_requests, Some(3));
    }

    #[test]
    fn build_daily_summary_from_facts_tracks_visibility_and_coverage() {
        let facts = vec![
            test_fact(
                "model-a",
                Some(200),
                CoverageOrigin::LocalOnly,
                100,
                20,
                10,
                0,
                1.0,
            ),
            test_fact(
                "model-b",
                Some(500),
                CoverageOrigin::ProxyOnly,
                40,
                10,
                0,
                0,
                0.5,
            ),
            test_fact(
                "model-a",
                None,
                CoverageOrigin::MergedProxyPreferred,
                30,
                30,
                0,
                5,
                0.25,
            ),
        ];

        let row = build_daily_summary_from_facts("2026-06-01", &facts, 1234);

        assert_eq!(row.request_count, 3);
        assert_eq!(row.visible_request_count, 2);
        assert_eq!(row.total_tokens, 245);
        assert_eq!(row.visible_total_tokens, 195);
        assert_eq!(row.success_request_count, 1);
        assert_eq!(row.client_error_requests, 0);
        assert_eq!(row.server_error_requests, 1);
        assert_eq!(row.model_count, 2);
        assert_eq!(row.success_model_count, 1);
        assert_eq!(row.local_only_requests, 1);
        assert_eq!(row.proxy_backed_requests, 2);
        assert_eq!(row.merged_overlap_requests, 1);
        assert!(row.has_partial_performance_coverage);
    }

    #[test]
    fn build_daily_model_summaries_from_facts_groups_unknown_and_status_counts() {
        let facts = vec![
            test_fact("", Some(200), CoverageOrigin::LocalOnly, 10, 20, 0, 0, 0.1),
            test_fact(
                "model-b",
                Some(404),
                CoverageOrigin::ProxyOnly,
                5,
                5,
                0,
                0,
                0.2,
            ),
            test_fact(
                "model-b",
                Some(200),
                CoverageOrigin::ProxyOnly,
                15,
                10,
                0,
                0,
                0.3,
            ),
        ];

        let rows = build_daily_model_summaries_from_facts("2026-06-01", &facts, 5678);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].model_name, "model-b");
        assert_eq!(rows[0].request_count, 2);
        assert_eq!(rows[0].success_request_count, 1);
        assert_eq!(rows[0].client_error_requests, 1);
        assert_eq!(rows[0].status_code_counts.get(&404), Some(&1));
        assert_eq!(rows[1].model_name, "unknown");
        assert_eq!(rows[1].local_only_requests, 1);
        assert_eq!(rows[1].success_request_count, 1);
    }
}
