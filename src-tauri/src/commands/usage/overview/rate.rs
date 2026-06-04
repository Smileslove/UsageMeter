use crate::commands::usage::accumulator::FactAccumulator;
use crate::models::{
    ModelRateStats, ModelTtftStats, OverallRateStats, TtftStats, WindowRateSummary,
};
use crate::unified_usage::MergedRequestFact;
use std::collections::HashMap;

fn empty_window_rate_summary(window: String) -> WindowRateSummary {
    WindowRateSummary {
        window,
        overall: OverallRateStats {
            request_count: 0,
            total_output_tokens: 0,
            total_duration_ms: 0,
            avg_tokens_per_second: 0.0,
        },
        by_model: Vec::new(),
        ttft: TtftStats::default(),
        ttft_by_model: Vec::new(),
    }
}

pub(super) fn build_window_rate_summary_from_facts(
    window: String,
    facts: Vec<MergedRequestFact>,
) -> WindowRateSummary {
    let mut overall = FactAccumulator::default();
    let mut by_model: HashMap<String, FactAccumulator> = HashMap::new();

    for fact in &facts {
        overall.add_fact(fact);
        if !fact.model.trim().is_empty() {
            by_model
                .entry(fact.model.clone())
                .or_default()
                .add_fact(fact);
        }
    }

    if overall.rate_count == 0 {
        return empty_window_rate_summary(window);
    }

    let mut by_model_stats: Vec<ModelRateStats> = by_model
        .into_iter()
        .filter_map(|(model_name, acc)| {
            (acc.rate_count > 0).then_some(ModelRateStats {
                model_name,
                request_count: acc.rate_count,
                total_output_tokens: acc.rate_output_tokens,
                total_duration_ms: acc.rate_duration_ms,
                avg_tokens_per_second: if acc.rate_count > 0 {
                    acc.rate_sum / acc.rate_count as f64
                } else {
                    0.0
                },
                min_tokens_per_second: acc.min_rate.unwrap_or(0.0),
                max_tokens_per_second: acc.max_rate.unwrap_or(0.0),
            })
        })
        .collect();
    by_model_stats.sort_by(|a, b| {
        b.avg_tokens_per_second
            .partial_cmp(&a.avg_tokens_per_second)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut ttft_by_model: Vec<ModelTtftStats> = facts
        .iter()
        .fold(
            HashMap::<String, FactAccumulator>::new(),
            |mut acc, fact| {
                if !fact.model.trim().is_empty() {
                    acc.entry(fact.model.clone()).or_default().add_fact(fact);
                }
                acc
            },
        )
        .into_iter()
        .filter_map(|(model_name, acc)| {
            (acc.ttft_count > 0).then_some(ModelTtftStats {
                model_name,
                request_count: acc.ttft_count,
                avg_ttft_ms: acc.ttft_sum / acc.ttft_count as f64,
                min_ttft_ms: acc.min_ttft_ms.unwrap_or(0),
                max_ttft_ms: acc.max_ttft_ms.unwrap_or(0),
            })
        })
        .collect();
    ttft_by_model.sort_by(|a, b| {
        a.avg_ttft_ms
            .partial_cmp(&b.avg_ttft_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    WindowRateSummary {
        window,
        overall: OverallRateStats {
            request_count: overall.rate_count,
            total_output_tokens: overall.rate_output_tokens,
            total_duration_ms: overall.rate_duration_ms,
            avg_tokens_per_second: if overall.rate_count > 0 {
                overall.rate_sum / overall.rate_count as f64
            } else {
                0.0
            },
        },
        by_model: by_model_stats,
        ttft: TtftStats {
            request_count: overall.ttft_count,
            avg_ttft_ms: if overall.ttft_count > 0 {
                overall.ttft_sum / overall.ttft_count as f64
            } else {
                0.0
            },
            min_ttft_ms: overall.min_ttft_ms.unwrap_or(0),
            max_ttft_ms: overall.max_ttft_ms.unwrap_or(0),
        },
        ttft_by_model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unified_usage::CoverageOrigin;

    fn test_fact(
        model: &str,
        output_tokens: u64,
        duration_ms: Option<u64>,
        rate: Option<f64>,
        ttft_ms: Option<u64>,
    ) -> MergedRequestFact {
        MergedRequestFact {
            canonical_request_key: format!("{model}:{output_tokens}"),
            session_id: "session".to_string(),
            project_name: None,
            project_path: None,
            api_key_prefix: None,
            request_base_url: None,
            tool: "claude_code".to_string(),
            timestamp_sec: 1,
            timestamp_ms: 1000,
            model: model.to_string(),
            input_tokens: 0,
            output_tokens,
            cache_create_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: output_tokens,
            estimated_cost: 0.0,
            coverage_origin: CoverageOrigin::ProxyOnly,
            status_code: Some(200),
            duration_ms,
            output_tokens_per_second: rate,
            ttft_ms,
            source_label: None,
        }
    }

    #[test]
    fn build_window_rate_summary_returns_empty_when_no_valid_rate_samples() {
        let summary = build_window_rate_summary_from_facts(
            "5h".to_string(),
            vec![test_fact("model-a", 10, None, None, None)],
        );

        assert_eq!(summary.window, "5h");
        assert_eq!(summary.overall.request_count, 0);
        assert!(summary.by_model.is_empty());
        assert_eq!(summary.ttft.request_count, 0);
    }

    #[test]
    fn build_window_rate_summary_aggregates_rate_and_ttft_by_model() {
        let summary = build_window_rate_summary_from_facts(
            "24h".to_string(),
            vec![
                test_fact("model-a", 100, Some(2000), Some(50.0), Some(300)),
                test_fact("model-a", 80, Some(2000), Some(40.0), Some(500)),
                test_fact("model-b", 60, Some(3000), Some(20.0), Some(700)),
            ],
        );

        assert_eq!(summary.overall.request_count, 3);
        assert_eq!(summary.overall.total_output_tokens, 240);
        assert_eq!(summary.overall.total_duration_ms, 7000);
        assert!((summary.overall.avg_tokens_per_second - 110.0 / 3.0).abs() < f64::EPSILON);
        assert_eq!(summary.by_model.len(), 2);
        assert_eq!(summary.by_model[0].model_name, "model-a");
        assert_eq!(summary.by_model[0].request_count, 2);
        assert_eq!(summary.by_model[0].min_tokens_per_second, 40.0);
        assert_eq!(summary.by_model[0].max_tokens_per_second, 50.0);
        assert_eq!(summary.ttft.request_count, 3);
        assert_eq!(summary.ttft.min_ttft_ms, 300);
        assert_eq!(summary.ttft.max_ttft_ms, 700);
        assert_eq!(summary.ttft_by_model[0].model_name, "model-a");
        assert_eq!(summary.ttft_by_model[0].request_count, 2);
    }
}
