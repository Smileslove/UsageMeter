use super::super::helpers::perf_log;
use super::super::types::{
    StatisticsCapability, StatisticsInsight, StatisticsMetric, StatisticsModelBreakdown,
    StatisticsPerformance, StatisticsQuery, StatisticsRange, StatisticsStatusBreakdown,
    StatisticsSummary, StatisticsTotals, StatisticsTrendPoint, MERGED_SOURCE, MODEL_TREND_LIMIT,
};
use super::shared::{
    add_fact_to_stat_acc, bucket_name, bucket_start, make_empty_trend, normalize_range,
    trend_from_map, value_for_metric, StatAccumulator,
};
use crate::models::StatusCodeCount;
use crate::unified_usage::MergedRequestFact;
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

pub(super) fn build_insights(
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

pub(super) fn build_merged_statistics(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::usage::types::{StatisticsBucket, StatisticsMetric};
    use crate::unified_usage::CoverageOrigin;

    fn test_fact(
        session_id: &str,
        timestamp_sec: i64,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cache_create_tokens: u64,
        cache_read_tokens: u64,
        cost: f64,
        coverage_origin: CoverageOrigin,
        status_code: Option<u16>,
        rate: Option<f64>,
        ttft_ms: Option<u64>,
    ) -> MergedRequestFact {
        MergedRequestFact {
            canonical_request_key: format!("{session_id}:{timestamp_sec}:{model}"),
            session_id: session_id.to_string(),
            project_name: None,
            project_path: None,
            api_key_prefix: None,
            request_base_url: None,
            tool: "claude_code".to_string(),
            timestamp_sec,
            timestamp_ms: timestamp_sec.saturating_mul(1000),
            model: model.to_string(),
            input_tokens,
            output_tokens,
            cache_create_tokens,
            cache_read_tokens,
            total_tokens: input_tokens + output_tokens + cache_create_tokens + cache_read_tokens,
            estimated_cost: cost,
            coverage_origin,
            status_code,
            duration_ms: Some(1000),
            output_tokens_per_second: rate,
            ttft_ms,
            source_label: None,
        }
    }

    fn test_query() -> StatisticsQuery {
        StatisticsQuery {
            start_epoch: 0,
            end_epoch: 7200,
            timezone: "Asia/Shanghai".to_string(),
            bucket: StatisticsBucket::Hour,
            metric: StatisticsMetric::Tokens,
        }
    }

    #[test]
    fn build_merged_statistics_aggregates_totals_models_and_capabilities() {
        let facts = vec![
            test_fact(
                "s1",
                1200,
                "model-a",
                100,
                50,
                10,
                0,
                1.25,
                CoverageOrigin::LocalOnly,
                Some(200),
                Some(25.0),
                Some(400),
            ),
            test_fact(
                "s2",
                1800,
                "model-a",
                20,
                30,
                0,
                0,
                0.75,
                CoverageOrigin::ProxyOnly,
                Some(500),
                Some(15.0),
                Some(800),
            ),
            test_fact(
                "s3",
                4200,
                "model-b",
                40,
                10,
                0,
                5,
                0.5,
                CoverageOrigin::MergedProxyPreferred,
                Some(404),
                Some(10.0),
                Some(900),
            ),
        ];

        let summary = build_merged_statistics(facts, &test_query());

        assert_eq!(summary.totals.request_count, 3);
        assert_eq!(summary.totals.total_tokens, 265);
        assert_eq!(summary.totals.input_tokens, 160);
        assert_eq!(summary.totals.output_tokens, 90);
        assert_eq!(summary.totals.cache_create_tokens, 10);
        assert_eq!(summary.totals.cache_read_tokens, 5);
        assert_eq!(summary.totals.local_request_count, 1);
        assert_eq!(summary.totals.proxy_request_count, 2);
        assert_eq!(summary.totals.success_requests, Some(1));
        assert_eq!(summary.totals.error_requests, Some(2));
        assert_eq!(summary.models.len(), 2);
        assert_eq!(summary.models[0].model_name, "model-a");
        assert_eq!(summary.models[0].request_count, 2);
        assert_eq!(summary.models[0].error_requests, Some(1));
        assert_eq!(summary.models[0].status_codes.len(), 2);
        assert_eq!(summary.models[0].trend.len(), 2);
        assert_eq!(summary.models[1].model_name, "model-b");
        assert!(summary.capability.has_basic_usage);
        assert!(summary.capability.has_performance);
        assert!(summary.capability.has_status_codes);
        assert_eq!(summary.status.as_ref().map(|s| s.success_requests), Some(1));
        assert_eq!(
            summary
                .performance
                .as_ref()
                .and_then(|p| p.fastest_model.as_deref()),
            Some("model-a")
        );
        assert_eq!(
            summary
                .performance
                .as_ref()
                .and_then(|p| p.slowest_model.as_deref()),
            Some("model-b")
        );
        assert!(!summary.insights.is_empty());
    }

    #[test]
    fn build_merged_statistics_hides_optional_fields_without_status_or_performance() {
        let facts = vec![
            test_fact(
                "s1",
                1200,
                "",
                10,
                20,
                0,
                0,
                0.0,
                CoverageOrigin::LocalOnly,
                None,
                None,
                None,
            ),
            test_fact(
                "s2",
                3600,
                "model-b",
                5,
                5,
                0,
                0,
                0.0,
                CoverageOrigin::ProxyOnly,
                None,
                None,
                None,
            ),
        ];

        let summary = build_merged_statistics(facts, &test_query());

        assert!(!summary.capability.has_performance);
        assert!(!summary.capability.has_status_codes);
        assert!(summary.performance.is_none());
        assert!(summary.status.is_none());
        assert_eq!(summary.totals.success_requests, None);
        assert_eq!(summary.totals.error_requests, None);
        assert!(summary
            .models
            .iter()
            .all(|model| model.avg_tokens_per_second.is_none()));
        assert!(summary
            .models
            .iter()
            .all(|model| model.avg_ttft_ms.is_none()));
        assert!(summary
            .models
            .iter()
            .all(|model| model.error_requests.is_none()));
        assert!(summary
            .models
            .iter()
            .all(|model| model.status_codes.is_empty()));
        assert_eq!(summary.models[0].model_name, "unknown");
    }
}
