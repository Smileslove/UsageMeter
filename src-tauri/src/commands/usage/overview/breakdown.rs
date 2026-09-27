use super::super::types::{OverviewBreakdown, OverviewBreakdownCapability, OverviewBreakdownItem};
use crate::commands::usage::accumulator::FactAccumulator;
use crate::models::{
    AppSettings, OFFICIAL_ANTHROPIC_CLAUDE_OAUTH_SOURCE_ID, OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID,
    OFFICIAL_OPENAI_OAUTH_SOURCE_ID,
};
use crate::proxy::compute_source_id;
use crate::unified_usage::{normalize_model_bucket, MergedRequestFact};
use std::collections::HashMap;

type BreakdownAccumulator = FactAccumulator;

struct BreakdownMeta {
    id: String,
    label: String,
    kind: String,
    color: Option<String>,
    icon: Option<String>,
}

#[derive(Clone, Copy)]
enum BreakdownPercentMetric {
    Cost,
    Tokens,
    Requests,
}

fn metric_for_percent(
    items: &HashMap<String, (BreakdownMeta, BreakdownAccumulator)>,
) -> (BreakdownPercentMetric, f64) {
    let cost_total: f64 = items.values().map(|(_, acc)| acc.cost).sum();
    if cost_total > 0.0 {
        return (BreakdownPercentMetric::Cost, cost_total);
    }

    let token_total: u64 = items.values().map(|(_, acc)| acc.total_tokens).sum();
    if token_total > 0 {
        return (BreakdownPercentMetric::Tokens, token_total as f64);
    }

    (
        BreakdownPercentMetric::Requests,
        items
            .values()
            .map(|(_, acc)| acc.request_count)
            .sum::<u64>() as f64,
    )
}

fn metric_value_for_percent(acc: &BreakdownAccumulator, metric: BreakdownPercentMetric) -> f64 {
    match metric {
        BreakdownPercentMetric::Cost => acc.cost,
        BreakdownPercentMetric::Tokens => acc.total_tokens as f64,
        BreakdownPercentMetric::Requests => acc.request_count as f64,
    }
}

fn overview_items_from_map(
    map: HashMap<String, (BreakdownMeta, BreakdownAccumulator)>,
) -> Vec<OverviewBreakdownItem> {
    let (percent_metric, denominator) = metric_for_percent(&map);
    let mut items: Vec<OverviewBreakdownItem> = map
        .into_values()
        .map(|(meta, acc)| {
            let error_requests = acc.client_error_requests + acc.server_error_requests;
            OverviewBreakdownItem {
                id: meta.id,
                label: meta.label,
                kind: meta.kind,
                color: meta.color,
                icon: meta.icon,
                request_count: acc.request_count,
                total_tokens: acc.total_tokens,
                input_tokens: acc.input_tokens,
                output_tokens: acc.output_tokens,
                cache_create_tokens: acc.cache_create_tokens,
                cache_read_tokens: acc.cache_read_tokens,
                cost: acc.cost,
                percent: if denominator > 0.0 {
                    (metric_value_for_percent(&acc, percent_metric) / denominator) * 100.0
                } else {
                    0.0
                },
                success_requests: acc.has_status.then_some(acc.success_requests),
                error_requests: acc.has_status.then_some(error_requests),
                avg_tokens_per_second: (acc.rate_count > 0)
                    .then_some(acc.rate_sum / acc.rate_count as f64),
                avg_ttft_ms: (acc.ttft_count > 0).then_some(acc.ttft_sum / acc.ttft_count as f64),
                last_seen_ms: (acc.last_seen_ms > 0).then_some(acc.last_seen_ms),
            }
        })
        .collect();
    items.sort_by(|a, b| {
        b.total_tokens
            .cmp(&a.total_tokens)
            .then_with(|| b.request_count.cmp(&a.request_count))
    });
    items
}

fn source_label_from_url(base_url: Option<&str>) -> String {
    match base_url {
        Some(url) if !url.trim().is_empty() => url
            .split("://")
            .nth(1)
            .unwrap_or(url)
            .split('/')
            .next()
            .unwrap_or(url)
            .to_string(),
        _ => "__official_api__".to_string(),
    }
}

fn source_meta_for_fact(settings: &AppSettings, fact: &MergedRequestFact) -> BreakdownMeta {
    if let Some(source_id) = fact.attribution_source_id.as_deref() {
        if matches!(
            source_id,
            OFFICIAL_OPENAI_OAUTH_SOURCE_ID
                | OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID
                | OFFICIAL_ANTHROPIC_CLAUDE_OAUTH_SOURCE_ID
        ) {
            let label = fact
                .source_label
                .clone()
                .unwrap_or_else(|| source_id.to_string());
            return BreakdownMeta {
                // Plans must remain distinct in rankings while the source filter remains stable.
                id: label.clone(),
                label,
                kind: "source".to_string(),
                color: Some(
                    if source_id == OFFICIAL_OPENAI_OAUTH_SOURCE_ID {
                        "#10A37F"
                    } else if source_id == OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID {
                        "#4285F4"
                    } else {
                        "#D97757"
                    }
                    .to_string(),
                ),
                icon: None,
            };
        }
        if let Some(source) = settings
            .source_aware
            .sources
            .iter()
            .find(|source| source.id == source_id)
        {
            return BreakdownMeta {
                id: source.id.clone(),
                label: source
                    .display_name
                    .clone()
                    .unwrap_or_else(|| source_label_from_url(source.base_url.as_deref())),
                kind: "source".to_string(),
                color: Some(source.color.clone()),
                icon: source.icon.clone(),
            };
        }
    }
    if fact.attribution_method == crate::unified_usage::AttributionMethod::Manual {
        return BreakdownMeta {
            id: "__unknown__".to_string(),
            label: "__unknown__".to_string(),
            kind: "source".to_string(),
            color: Some("#9CA3AF".to_string()),
            icon: None,
        };
    }
    let matched = settings.source_aware.sources.iter().find(|source| {
        let base_url_matches = source.base_url == fact.request_base_url;
        let key_matches = fact
            .api_key_prefix
            .as_ref()
            .map(|prefix| source.api_key_prefixes.contains(prefix))
            .unwrap_or(false);
        base_url_matches && key_matches
    });

    if let Some(source) = matched {
        return BreakdownMeta {
            id: source.id.clone(),
            label: source
                .display_name
                .clone()
                .unwrap_or_else(|| source_label_from_url(source.base_url.as_deref())),
            kind: "source".to_string(),
            color: Some(source.color.clone()),
            icon: source.icon.clone(),
        };
    }

    if let Some(prefix) = fact
        .api_key_prefix
        .as_ref()
        .filter(|prefix| !prefix.trim().is_empty())
    {
        let source_id = compute_source_id(prefix, fact.request_base_url.as_deref());
        return BreakdownMeta {
            id: source_id,
            label: source_label_from_url(fact.request_base_url.as_deref()),
            kind: "source".to_string(),
            color: Some("#9CA3AF".to_string()),
            icon: None,
        };
    }

    // Best-effort fallback for local-only records that have no api_key_prefix (never
    // reconstructable without the real Authorization header) but do carry a request_base_url
    // (e.g. Codex local-only facts filled from the currently-configured upstream — see
    // unified_usage::service::merge_realtime_range's codex_fallback_base_url). Only attribute
    // when exactly one registered source shares that base_url; 0 or 2+ matches stay ambiguous
    // and fall through to __unknown__ rather than guessing wrong. This is inherently a
    // heuristic based on Codex's *current* config, not necessarily what was active when a
    // historical local-only record was produced.
    if fact.api_key_prefix.is_none() {
        if let Some(base_url) = fact
            .request_base_url
            .as_deref()
            .filter(|url| !url.trim().is_empty())
        {
            let candidates: Vec<_> = settings
                .source_aware
                .sources
                .iter()
                .filter(|source| source.base_url.as_deref() == Some(base_url))
                .collect();
            if candidates.len() == 1 {
                let source = candidates[0];
                return BreakdownMeta {
                    id: source.id.clone(),
                    label: source
                        .display_name
                        .clone()
                        .unwrap_or_else(|| source_label_from_url(source.base_url.as_deref())),
                    kind: "source".to_string(),
                    color: Some(source.color.clone()),
                    icon: source.icon.clone(),
                };
            }
        }
    }

    BreakdownMeta {
        id: "__unknown__".to_string(),
        label: "__unknown__".to_string(),
        kind: "source".to_string(),
        color: Some("#9CA3AF".to_string()),
        icon: None,
    }
}

fn tool_meta_for_fact(settings: &AppSettings, fact: &MergedRequestFact) -> BreakdownMeta {
    let profile = settings
        .client_tools
        .profiles
        .iter()
        .find(|profile| profile.tool == fact.tool);
    BreakdownMeta {
        id: fact.tool.clone(),
        label: profile
            .and_then(|profile| profile.display_name.clone())
            .unwrap_or_else(|| {
                if fact.tool.trim().is_empty() {
                    "__unknown__".to_string()
                } else {
                    fact.tool.clone()
                }
            }),
        kind: "tool".to_string(),
        color: None,
        icon: profile.and_then(|profile| profile.icon.clone()),
    }
}

fn model_meta_for_fact(fact: &MergedRequestFact) -> BreakdownMeta {
    // 与统计聚合保持一致的分桶口径：ReasonX 的 `provider/model`（本地 telemetry）
    // 需剥离 provider 前缀，才能与代理捕获的 `model`（无前缀）归入同一桶。
    let label = if fact.model.trim().is_empty() {
        "__unknown__".to_string()
    } else {
        normalize_model_bucket(&fact.tool, &fact.model)
    };
    BreakdownMeta {
        id: label.clone(),
        label,
        kind: "model".to_string(),
        color: None,
        icon: None,
    }
}

fn add_breakdown_fact(
    map: &mut HashMap<String, (BreakdownMeta, BreakdownAccumulator)>,
    meta: BreakdownMeta,
    fact: &MergedRequestFact,
) {
    let entry = map
        .entry(meta.id.clone())
        .or_insert_with(|| (meta, BreakdownAccumulator::default()));
    entry.1.add_fact(fact);
}

pub(super) fn build_overview_breakdown_from_facts(
    settings: &AppSettings,
    window: String,
    generated_at_epoch: i64,
    facts: &[MergedRequestFact],
) -> OverviewBreakdown {
    let mut source_map: HashMap<String, (BreakdownMeta, BreakdownAccumulator)> = HashMap::new();
    let mut tool_map: HashMap<String, (BreakdownMeta, BreakdownAccumulator)> = HashMap::new();
    let mut model_map: HashMap<String, (BreakdownMeta, BreakdownAccumulator)> = HashMap::new();
    let mut has_cost = false;
    let mut has_status = false;
    let mut has_performance = false;

    for fact in facts {
        add_breakdown_fact(&mut source_map, source_meta_for_fact(settings, fact), fact);
        add_breakdown_fact(&mut tool_map, tool_meta_for_fact(settings, fact), fact);
        add_breakdown_fact(&mut model_map, model_meta_for_fact(fact), fact);
        has_cost |= fact.estimated_cost > 0.0;
        has_status |= fact.status_code.is_some();
        has_performance |= fact.output_tokens_per_second.is_some() || fact.ttft_ms.is_some();
    }

    let capability = OverviewBreakdownCapability {
        has_source: !source_map.is_empty(),
        has_tool: !tool_map.is_empty(),
        has_cost,
        has_status,
        has_performance,
    };

    OverviewBreakdown {
        window,
        generated_at_epoch,
        source_ranking: overview_items_from_map(source_map),
        tool_ranking: overview_items_from_map(tool_map),
        model_ranking: overview_items_from_map(model_map),
        capability,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ApiSource, AppSettings, ClientToolProfile};
    use crate::unified_usage::CoverageOrigin;
    use std::collections::HashMap;

    fn test_fact(
        tool: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cost: f64,
        api_key_prefix: Option<&str>,
        request_base_url: Option<&str>,
        status_code: Option<u16>,
        rate: Option<f64>,
        ttft_ms: Option<u64>,
    ) -> MergedRequestFact {
        MergedRequestFact {
            canonical_request_key: format!("{tool}:{model}:{input_tokens}:{output_tokens}"),
            session_id: "session".to_string(),
            project_name: None,
            project_path: None,
            api_key_prefix: api_key_prefix.map(|value| value.to_string()),
            request_base_url: request_base_url.map(|value| value.to_string()),
            tool: tool.to_string(),
            timestamp_sec: 1_700_000_000,
            timestamp_ms: 1_700_000_100,
            model: model.to_string(),
            input_tokens,
            output_tokens,
            cache_create_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: input_tokens + output_tokens,
            request_count: 1,
            estimated_cost: cost,
            estimated: false,
            coverage_origin: CoverageOrigin::ProxyOnly,
            status_code,
            duration_ms: Some(1000),
            output_tokens_per_second: rate,
            ttft_ms,
            source_label: None,
            attribution_source_id: None,
            attribution_method: crate::unified_usage::AttributionMethod::Unattributed,
        }
    }

    #[test]
    fn build_overview_breakdown_prefers_cost_for_percent_and_uses_configured_labels() {
        let mut settings = AppSettings::default();
        settings.source_aware.sources = vec![ApiSource {
            id: "src-1".to_string(),
            display_name: Some("Primary".to_string()),
            base_url: Some("https://api.example.com/v1".to_string()),
            api_key_prefixes: vec!["sk-test".to_string()],
            api_key_notes: HashMap::new(),
            color: "#112233".to_string(),
            icon: Some("bot".to_string()),
            auto_detected: false,
            quota_query: None,
            first_seen_ms: 1,
            last_seen_ms: 1,
        }];
        settings.client_tools.profiles.push(ClientToolProfile {
            id: "custom-tool".to_string(),
            tool: "custom-tool".to_string(),
            display_name: Some("Custom Tool".to_string()),
            path_prefix: "custom".to_string(),
            target_base_url: None,
            enabled: true,
            auto_detected: false,
            first_seen_ms: 1,
            last_seen_ms: 1,
            icon: Some("wrench".to_string()),
        });

        let breakdown = build_overview_breakdown_from_facts(
            &settings,
            "30d".to_string(),
            1234,
            &[
                test_fact(
                    "custom-tool",
                    "model-a",
                    10,
                    30,
                    3.0,
                    Some("sk-test"),
                    Some("https://api.example.com/v1"),
                    Some(200),
                    Some(15.0),
                    Some(300),
                ),
                test_fact(
                    "custom-tool",
                    "model-b",
                    30,
                    30,
                    1.0,
                    Some("sk-test"),
                    Some("https://api.example.com/v1"),
                    Some(500),
                    Some(10.0),
                    Some(600),
                ),
            ],
        );

        assert!(breakdown.capability.has_source);
        assert!(breakdown.capability.has_tool);
        assert!(breakdown.capability.has_cost);
        assert!(breakdown.capability.has_status);
        assert!(breakdown.capability.has_performance);
        assert_eq!(breakdown.source_ranking.len(), 1);
        assert_eq!(breakdown.source_ranking[0].label, "Primary");
        assert_eq!(
            breakdown.source_ranking[0].color.as_deref(),
            Some("#112233")
        );
        assert_eq!(breakdown.tool_ranking[0].label, "Custom Tool");
        assert_eq!(breakdown.tool_ranking[0].icon.as_deref(), Some("wrench"));
        assert_eq!(breakdown.model_ranking[0].label, "model-b");
        assert!((breakdown.source_ranking[0].percent - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn build_overview_breakdown_falls_back_to_token_percent_and_unknown_labels() {
        let settings = AppSettings::default();
        let breakdown = build_overview_breakdown_from_facts(
            &settings,
            "5h".to_string(),
            1234,
            &[
                test_fact("", "", 10, 30, 0.0, None, None, None, None, None),
                test_fact(
                    "claude_code",
                    "model-b",
                    10,
                    10,
                    0.0,
                    None,
                    Some("https://proxy.example.com/v1"),
                    None,
                    None,
                    None,
                ),
            ],
        );

        assert!(!breakdown.capability.has_cost);
        assert!(!breakdown.capability.has_status);
        assert!(!breakdown.capability.has_performance);
        assert_eq!(breakdown.source_ranking.len(), 1);
        assert_eq!(breakdown.source_ranking[0].label, "__unknown__");
        assert_eq!(breakdown.tool_ranking[0].label, "__unknown__");
        assert_eq!(breakdown.model_ranking[0].label, "__unknown__");
        assert!((breakdown.model_ranking[0].percent - (40.0 / 60.0 * 100.0)).abs() < 0.0001);
    }

    fn test_source(id: &str, base_url: &str, display_name: &str) -> ApiSource {
        ApiSource {
            id: id.to_string(),
            display_name: Some(display_name.to_string()),
            base_url: Some(base_url.to_string()),
            api_key_prefixes: vec!["sk-unrelated".to_string()],
            api_key_notes: HashMap::new(),
            color: "#445566".to_string(),
            icon: None,
            auto_detected: true,
            quota_query: None,
            first_seen_ms: 1,
            last_seen_ms: 1,
        }
    }

    #[test]
    fn source_meta_falls_back_to_single_base_url_match_without_api_key_prefix() {
        // Codex local-only fact: no api_key_prefix (not reconstructable), but request_base_url
        // was best-effort filled from Codex's currently-configured upstream.
        let mut settings = AppSettings::default();
        settings.source_aware.sources = vec![test_source(
            "src-codex",
            "https://sui-xiang.com",
            "sui-xiang.com",
        )];

        let fact = test_fact(
            "codex",
            "gpt-5",
            10,
            20,
            0.0,
            None,
            Some("https://sui-xiang.com"),
            Some(200),
            None,
            None,
        );

        let meta = source_meta_for_fact(&settings, &fact);
        assert_eq!(meta.id, "src-codex");
        assert_eq!(meta.label, "sui-xiang.com");
    }

    #[test]
    fn source_meta_stays_unknown_when_base_url_is_ambiguous() {
        let mut settings = AppSettings::default();
        settings.source_aware.sources = vec![
            test_source("src-a", "https://shared.example.com", "Account A"),
            test_source("src-b", "https://shared.example.com", "Account B"),
        ];

        let fact = test_fact(
            "codex",
            "gpt-5",
            10,
            20,
            0.0,
            None,
            Some("https://shared.example.com"),
            Some(200),
            None,
            None,
        );

        let meta = source_meta_for_fact(&settings, &fact);
        assert_eq!(meta.id, "__unknown__");
    }

    #[test]
    fn reasonix_provider_model_merges_with_proxy_model_into_single_bucket() {
        // 回归：本地 telemetry 的模型是 `provider/model`（基元律动/deepseek-v4-flash-0731），
        // 代理捕获的是无前缀 `deepseek-v4-flash-0731`。两者必须剥离 provider 前缀
        // 归入同一模型桶，否则 Overview 模型排行出现两个同模型桶。
        let settings = AppSettings::default();
        let breakdown = build_overview_breakdown_from_facts(
            &settings,
            "24h".to_string(),
            1234,
            &[
                test_fact(
                    "reasonix",
                    "基元律动/deepseek-v4-flash-0731",
                    100,
                    50,
                    1.0,
                    None,
                    None,
                    Some(200),
                    None,
                    None,
                ),
                test_fact(
                    "api_gateway",
                    "deepseek-v4-flash-0731",
                    200,
                    100,
                    2.0,
                    None,
                    None,
                    Some(200),
                    Some(10.0),
                    None,
                ),
            ],
        );
        assert_eq!(
            breakdown.model_ranking.len(),
            1,
            "本地与代理的同一模型必须合并为一个桶"
        );
        assert_eq!(breakdown.model_ranking[0].label, "deepseek-v4-flash-0731");
        assert_eq!(breakdown.model_ranking[0].request_count, 2);
    }
}
