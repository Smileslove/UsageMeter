use super::helpers::{
    epoch_u64_to_i64_saturating, first_fact_index_in_range, perf_log, usage_window_cutoff_epoch,
};
use super::types::{
    OverviewBreakdown, OverviewBreakdownCapability, OverviewBreakdownItem,
    PreparedUsageRefreshData, ProxyState, UsageRefreshBundle, WindowPreparedFacts, MERGED_SOURCE,
    USAGE_WINDOWS,
};
use crate::models::{
    AppSettings, ModelRateStats, ModelTtftStats, OverallRateStats, TtftStats, UsageSnapshot,
    WindowRateSummary, WindowUsage,
};
use crate::proxy::compute_source_id;
use crate::unified_usage::MergedRequestFact;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

fn facts_slice_for_window<'a>(
    prepared: &'a PreparedUsageRefreshData,
    window: &str,
) -> &'a [MergedRequestFact] {
    prepared
        .windows
        .iter()
        .find(|entry| entry.window == window)
        .map(|entry| &prepared.facts[entry.start_index..])
        .unwrap_or(&prepared.facts[..0])
}

async fn prepare_usage_refresh_data(
    settings: &AppSettings,
    generated_at_epoch: u64,
) -> Result<PreparedUsageRefreshData, String> {
    let include_errors = settings.proxy.include_error_requests;
    let mut window_cutoffs: Vec<(String, i64)> = USAGE_WINDOWS
        .iter()
        .map(|window| ((*window).to_string(), usage_window_cutoff_epoch(window)))
        .collect();
    let summary_window = settings.summary_window.clone();
    if !window_cutoffs
        .iter()
        .any(|(window, _)| *window == summary_window)
    {
        window_cutoffs.push((
            summary_window.clone(),
            usage_window_cutoff_epoch(&summary_window),
        ));
    }

    let earliest_cutoff = window_cutoffs
        .iter()
        .map(|(_, cutoff)| *cutoff)
        .min()
        .unwrap_or(0);
    let end_epoch = generated_at_epoch.saturating_add(1);
    let (facts, _coverage) = crate::unified_usage::get_merged_request_facts(
        settings,
        Some(earliest_cutoff),
        Some(epoch_u64_to_i64_saturating(end_epoch)),
        include_errors,
    )
    .await?;

    let windows = window_cutoffs
        .into_iter()
        .map(|(window, cutoff_epoch)| WindowPreparedFacts {
            start_index: first_fact_index_in_range(&facts, cutoff_epoch),
            window,
        })
        .collect();

    Ok(PreparedUsageRefreshData {
        generated_at_epoch,
        facts,
        windows,
    })
}

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

fn build_window_rate_summary_from_facts(
    window: String,
    facts: Vec<MergedRequestFact>,
) -> WindowRateSummary {
    #[derive(Default)]
    struct PerfAccumulator {
        request_count: u64,
        total_output_tokens: u64,
        total_duration_ms: u64,
        rate_sum: f64,
        rate_count: u64,
        min_rate: Option<f64>,
        max_rate: Option<f64>,
        ttft_sum: f64,
        ttft_count: u64,
        min_ttft_ms: Option<u64>,
        max_ttft_ms: Option<u64>,
    }

    impl PerfAccumulator {
        fn add(&mut self, fact: &MergedRequestFact) {
            if let (Some(duration_ms), Some(rate)) =
                (fact.duration_ms, fact.output_tokens_per_second)
            {
                if duration_ms > 0 && rate > 0.0 {
                    self.request_count += 1;
                    self.total_output_tokens += fact.output_tokens;
                    self.total_duration_ms += duration_ms;
                    self.rate_sum += rate;
                    self.rate_count += 1;
                    self.min_rate = Some(self.min_rate.map_or(rate, |current| current.min(rate)));
                    self.max_rate = Some(self.max_rate.map_or(rate, |current| current.max(rate)));
                }
            }

            if let Some(ttft_ms) = fact.ttft_ms {
                if ttft_ms > 0 {
                    self.ttft_sum += ttft_ms as f64;
                    self.ttft_count += 1;
                    self.min_ttft_ms = Some(
                        self.min_ttft_ms
                            .map_or(ttft_ms, |current| current.min(ttft_ms)),
                    );
                    self.max_ttft_ms = Some(
                        self.max_ttft_ms
                            .map_or(ttft_ms, |current| current.max(ttft_ms)),
                    );
                }
            }
        }
    }

    let mut overall = PerfAccumulator::default();
    let mut by_model: HashMap<String, PerfAccumulator> = HashMap::new();

    for fact in &facts {
        overall.add(fact);
        if !fact.model.trim().is_empty() {
            by_model.entry(fact.model.clone()).or_default().add(fact);
        }
    }

    if overall.request_count == 0 {
        return empty_window_rate_summary(window);
    }

    let mut by_model_stats: Vec<ModelRateStats> = by_model
        .into_iter()
        .filter_map(|(model_name, acc)| {
            (acc.request_count > 0).then_some(ModelRateStats {
                model_name,
                request_count: acc.request_count,
                total_output_tokens: acc.total_output_tokens,
                total_duration_ms: acc.total_duration_ms,
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
            HashMap::<String, PerfAccumulator>::new(),
            |mut acc, fact| {
                if !fact.model.trim().is_empty() {
                    acc.entry(fact.model.clone()).or_default().add(fact);
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
            request_count: overall.request_count,
            total_output_tokens: overall.total_output_tokens,
            total_duration_ms: overall.total_duration_ms,
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

fn build_window_usage_from_facts(
    window: &str,
    facts: &[MergedRequestFact],
) -> (WindowUsage, HashMap<String, ModelTokenTotals>) {
    let mut model_stats: HashMap<String, ModelTokenTotals> = HashMap::new();
    let mut token_used = 0_u64;
    let mut input_tokens = 0_u64;
    let mut output_tokens = 0_u64;
    let mut cache_create_tokens = 0_u64;
    let mut cache_read_tokens = 0_u64;
    let request_used = facts.len() as u64;
    let mut success_requests = 0_u64;
    let mut client_error_requests = 0_u64;
    let mut server_error_requests = 0_u64;

    for fact in facts {
        token_used += fact.total_tokens;
        input_tokens += fact.input_tokens;
        output_tokens += fact.output_tokens;
        cache_create_tokens += fact.cache_create_tokens;
        cache_read_tokens += fact.cache_read_tokens;

        if let Some(status_code) = fact.status_code {
            if (200..300).contains(&status_code) {
                success_requests += 1;
            } else if (400..500).contains(&status_code) {
                client_error_requests += 1;
            } else if status_code >= 500 {
                server_error_requests += 1;
            }
        }

        if !fact.model.is_empty() {
            let entry = model_stats.entry(fact.model.clone()).or_default();
            entry.input_tokens += fact.input_tokens;
            entry.output_tokens += fact.output_tokens;
            entry.cache_create_tokens += fact.cache_create_tokens;
            entry.cache_read_tokens += fact.cache_read_tokens;
            entry.request_count += 1;
        }
    }

    let cost: f64 = facts.iter().map(|fact| fact.estimated_cost).sum();
    (
        WindowUsage {
            window: window.to_string(),
            token_used,
            input_tokens,
            output_tokens,
            cache_create_tokens,
            cache_read_tokens,
            request_used,
            cost,
            success_requests,
            client_error_requests,
            server_error_requests,
        },
        model_stats,
    )
}

#[derive(Default, Clone)]
struct ModelTokenTotals {
    input_tokens: u64,
    output_tokens: u64,
    cache_create_tokens: u64,
    cache_read_tokens: u64,
    request_count: u64,
}

fn build_model_distribution_from_window_stats(
    window_stats: Option<&HashMap<String, ModelTokenTotals>>,
) -> Vec<crate::models::ModelUsage> {
    let total_model_tokens: u64 = window_stats
        .into_iter()
        .flat_map(|stats| stats.values())
        .map(|totals| {
            totals.input_tokens
                + totals.output_tokens
                + totals.cache_create_tokens
                + totals.cache_read_tokens
        })
        .sum();

    let mut model_distribution: Vec<crate::models::ModelUsage> = window_stats
        .into_iter()
        .flat_map(|stats| stats.iter())
        .map(|(model_name, totals)| {
            let tokens = totals.input_tokens
                + totals.output_tokens
                + totals.cache_create_tokens
                + totals.cache_read_tokens;
            let percent = if total_model_tokens > 0 {
                (tokens as f64 / total_model_tokens as f64) * 100.0
            } else {
                0.0
            };

            crate::models::ModelUsage {
                model_name: model_name.clone(),
                token_used: tokens,
                input_tokens: totals.input_tokens,
                output_tokens: totals.output_tokens,
                cache_create_tokens: totals.cache_create_tokens,
                cache_read_tokens: totals.cache_read_tokens,
                request_count: totals.request_count,
                percent,
                status_codes: Vec::new(),
            }
        })
        .collect();

    model_distribution.sort_by_key(|entry| std::cmp::Reverse(entry.token_used));
    model_distribution.truncate(5);
    model_distribution
}

fn build_model_token_totals_from_facts(
    facts: &[MergedRequestFact],
) -> HashMap<String, ModelTokenTotals> {
    let mut model_stats: HashMap<String, ModelTokenTotals> = HashMap::new();

    for fact in facts {
        if fact.model.is_empty() {
            continue;
        }

        let entry = model_stats.entry(fact.model.clone()).or_default();
        entry.input_tokens += fact.input_tokens;
        entry.output_tokens += fact.output_tokens;
        entry.cache_create_tokens += fact.cache_create_tokens;
        entry.cache_read_tokens += fact.cache_read_tokens;
        entry.request_count += 1;
    }

    model_stats
}

fn build_usage_summary_from_usage(
    usage: &WindowUsage,
    total_success_requests: u64,
    total_client_error_requests: u64,
    total_server_error_requests: u64,
) -> crate::models::UsageSummary {
    crate::models::UsageSummary {
        total_tokens: usage.token_used,
        total_requests: usage.request_used,
        total_input_tokens: usage.input_tokens,
        total_output_tokens: usage.output_tokens,
        total_cache_create_tokens: usage.cache_create_tokens,
        total_cache_read_tokens: usage.cache_read_tokens,
        total_cost: usage.cost,
        total_success_requests,
        total_client_error_requests,
        total_server_error_requests,
    }
}

fn build_usage_refresh_bundle_from_prepared(
    settings: &AppSettings,
    prepared: &PreparedUsageRefreshData,
) -> UsageRefreshBundle {
    let mut windows = Vec::new();
    let mut has_partial_snapshot_coverage = false;
    let mut summary_model_stats: Option<HashMap<String, ModelTokenTotals>> = None;

    for window_name in USAGE_WINDOWS {
        let facts = facts_slice_for_window(prepared, window_name);
        let coverage = crate::unified_usage::build_coverage(facts);
        has_partial_snapshot_coverage |=
            coverage.has_partial_status_coverage || coverage.has_partial_performance_coverage;
        let (window_usage, model_stats) = build_window_usage_from_facts(window_name, facts);
        if *window_name == settings.summary_window {
            summary_model_stats = Some(model_stats.clone());
        }
        windows.push(window_usage);
    }

    let summary_facts = facts_slice_for_window(prepared, &settings.summary_window);
    let summary_coverage = crate::unified_usage::build_coverage(summary_facts);
    let (summary_window_usage, summary_window_model_stats) =
        build_window_usage_from_facts(&settings.summary_window, summary_facts);
    if summary_model_stats.is_none() {
        summary_model_stats = Some(summary_window_model_stats);
    }
    let derived_summary_model_stats = summary_model_stats
        .is_none()
        .then(|| build_model_token_totals_from_facts(summary_facts));
    let summary_model_distribution = build_model_distribution_from_window_stats(
        summary_model_stats
            .as_ref()
            .or(derived_summary_model_stats.as_ref()),
    );
    let mut summary_success_requests = 0_u64;
    let mut summary_client_error_requests = 0_u64;
    let mut summary_server_error_requests = 0_u64;

    for fact in summary_facts {
        if let Some(status_code) = fact.status_code {
            if (200..300).contains(&status_code) {
                summary_success_requests += 1;
            } else if (400..500).contains(&status_code) {
                summary_client_error_requests += 1;
            } else if status_code >= 500 {
                summary_server_error_requests += 1;
            }
        }
    }

    let summary = build_usage_summary_from_usage(
        &summary_window_usage,
        summary_success_requests,
        summary_client_error_requests,
        summary_server_error_requests,
    );
    let snapshot = UsageSnapshot {
        generated_at_epoch: prepared.generated_at_epoch,
        windows,
        source: MERGED_SOURCE.to_string(),
        note: (has_partial_snapshot_coverage || summary_coverage.has_partial_performance_coverage)
            .then_some("NOTE_PARTIAL_PROXY_COVERAGE".to_string()),
        summary,
        model_distribution: summary_model_distribution,
    };

    let rate_summary = build_window_rate_summary_from_facts(
        settings.summary_window.clone(),
        summary_facts.to_vec(),
    );
    let overview_breakdown = build_overview_breakdown_from_facts(
        settings,
        settings.summary_window.clone(),
        epoch_u64_to_i64_saturating(prepared.generated_at_epoch),
        summary_facts,
    );

    UsageRefreshBundle {
        generated_at_epoch: prepared.generated_at_epoch,
        snapshot,
        rate_summary,
        overview_breakdown,
    }
}

#[derive(Default)]
struct BreakdownAccumulator {
    request_count: u64,
    total_tokens: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_create_tokens: u64,
    cache_read_tokens: u64,
    cost: f64,
    success_requests: u64,
    client_error_requests: u64,
    server_error_requests: u64,
    has_status: bool,
    rate_sum: f64,
    rate_count: u64,
    ttft_sum: f64,
    ttft_count: u64,
    last_seen_ms: i64,
}

impl BreakdownAccumulator {
    fn add(&mut self, fact: &MergedRequestFact) {
        self.request_count += 1;
        self.total_tokens += fact.total_tokens;
        self.input_tokens += fact.input_tokens;
        self.output_tokens += fact.output_tokens;
        self.cache_create_tokens += fact.cache_create_tokens;
        self.cache_read_tokens += fact.cache_read_tokens;
        self.cost += fact.estimated_cost;
        self.last_seen_ms = self.last_seen_ms.max(fact.timestamp_ms);

        if let Some(status_code) = fact.status_code {
            self.has_status = true;
            if (200..300).contains(&status_code) {
                self.success_requests += 1;
            } else if (400..500).contains(&status_code) {
                self.client_error_requests += 1;
            } else if status_code >= 500 {
                self.server_error_requests += 1;
            }
        }

        if let Some(rate) = fact.output_tokens_per_second {
            if rate > 0.0 {
                self.rate_sum += rate;
                self.rate_count += 1;
            }
        }

        if let Some(ttft_ms) = fact.ttft_ms {
            if ttft_ms > 0 {
                self.ttft_sum += ttft_ms as f64;
                self.ttft_count += 1;
            }
        }
    }
}

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
    let label = if fact.model.trim().is_empty() {
        "__unknown__".to_string()
    } else {
        fact.model.clone()
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
    entry.1.add(fact);
}

fn build_overview_breakdown_from_facts(
    settings: &AppSettings,
    window: String,
    generated_at_epoch: i64,
    facts: &[MergedRequestFact],
) -> OverviewBreakdown {
    let mut source_map: HashMap<String, (BreakdownMeta, BreakdownAccumulator)> = HashMap::new();
    let mut tool_map: HashMap<String, (BreakdownMeta, BreakdownAccumulator)> = HashMap::new();
    let mut model_map: HashMap<String, (BreakdownMeta, BreakdownAccumulator)> = HashMap::new();

    for fact in facts {
        add_breakdown_fact(&mut source_map, source_meta_for_fact(settings, fact), fact);
        add_breakdown_fact(&mut tool_map, tool_meta_for_fact(settings, fact), fact);
        add_breakdown_fact(&mut model_map, model_meta_for_fact(fact), fact);
    }

    let capability = OverviewBreakdownCapability {
        has_source: !source_map.is_empty(),
        has_tool: !tool_map.is_empty(),
        has_cost: facts.iter().any(|fact| fact.estimated_cost > 0.0),
        has_status: facts.iter().any(|fact| fact.status_code.is_some()),
        has_performance: facts
            .iter()
            .any(|fact| fact.output_tokens_per_second.is_some() || fact.ttft_ms.is_some()),
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

// ── Commands ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn refresh_usage_bundle(
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<UsageRefreshBundle, String> {
    let started_at = std::time::Instant::now();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let prepared = prepare_usage_refresh_data(&settings, now).await?;
    let build_started_at = std::time::Instant::now();
    let bundle = build_usage_refresh_bundle_from_prepared(&settings, &prepared);
    perf_log(
        "refresh_usage_bundle",
        format!(
            "facts={} build_ms={} total_ms={}",
            prepared.facts.len(),
            build_started_at.elapsed().as_millis(),
            started_at.elapsed().as_millis(),
        ),
    );
    Ok(bundle)
}

#[tauri::command]
pub async fn get_overview_breakdown(
    window: String,
    settings: AppSettings,
) -> Result<OverviewBreakdown, String> {
    let now = chrono::Utc::now().timestamp();
    let include_errors = settings.proxy.include_error_requests;
    let cutoff_ms = crate::proxy::UsageCollector::calculate_window_cutoff_public(&window);
    let (facts, _) = crate::unified_usage::get_merged_request_facts(
        &settings,
        Some(cutoff_ms / 1000),
        Some(now + 1),
        include_errors,
    )
    .await?;
    Ok(build_overview_breakdown_from_facts(
        &settings, window, now, &facts,
    ))
}

#[tauri::command]
pub async fn get_window_rate_summary(
    window: String,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<WindowRateSummary, String> {
    let settings = crate::commands::load_settings()?;
    let cutoff_ms = crate::proxy::UsageCollector::calculate_window_cutoff_public(&window);
    let include_errors = settings.proxy.include_error_requests;
    let (facts, _) = crate::unified_usage::get_merged_request_facts(
        &settings,
        Some(cutoff_ms / 1000),
        None,
        include_errors,
    )
    .await?;

    Ok(build_window_rate_summary_from_facts(window, facts))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unified_usage::CoverageOrigin;

    fn local_record_age_seconds(created_at: i64, now_ts: i64) -> Option<i64> {
        if created_at > now_ts {
            None
        } else {
            Some(now_ts - created_at)
        }
    }

    #[test]
    fn local_record_age_seconds_rejects_future_timestamps() {
        assert_eq!(local_record_age_seconds(100, 100), Some(0));
        assert_eq!(local_record_age_seconds(80, 100), Some(20));
        assert_eq!(local_record_age_seconds(101, 100), None);
    }

    #[test]
    fn build_usage_summary_from_usage_keeps_cost_in_same_window() {
        let usage = WindowUsage {
            window: "5h".to_string(),
            token_used: 120,
            input_tokens: 70,
            output_tokens: 40,
            cache_create_tokens: 5,
            cache_read_tokens: 5,
            request_used: 3,
            cost: 1.25,
            success_requests: 0,
            client_error_requests: 0,
            server_error_requests: 0,
        };

        let summary = build_usage_summary_from_usage(&usage, 0, 0, 0);

        assert_eq!(summary.total_tokens, 120);
        assert_eq!(summary.total_requests, 3);
        assert_eq!(summary.total_input_tokens, 70);
        assert_eq!(summary.total_output_tokens, 40);
        assert_eq!(summary.total_cache_create_tokens, 5);
        assert_eq!(summary.total_cache_read_tokens, 5);
        assert_eq!(summary.total_cost, 1.25);
    }

    #[test]
    fn build_model_distribution_from_window_stats_uses_selected_window_only() {
        let mut five_hour = HashMap::new();
        five_hour.insert(
            "model-a".to_string(),
            ModelTokenTotals {
                input_tokens: 70,
                output_tokens: 30,
                cache_create_tokens: 0,
                cache_read_tokens: 0,
                request_count: 2,
            },
        );

        let mut thirty_day = HashMap::new();
        thirty_day.insert(
            "model-b".to_string(),
            ModelTokenTotals {
                input_tokens: 700,
                output_tokens: 300,
                cache_create_tokens: 0,
                cache_read_tokens: 0,
                request_count: 20,
            },
        );

        let five_hour_distribution = build_model_distribution_from_window_stats(Some(&five_hour));
        let thirty_day_distribution = build_model_distribution_from_window_stats(Some(&thirty_day));

        assert_eq!(five_hour_distribution.len(), 1);
        assert_eq!(five_hour_distribution[0].model_name, "model-a");
        assert_eq!(five_hour_distribution[0].token_used, 100);
        assert_eq!(five_hour_distribution[0].request_count, 2);
        assert_eq!(five_hour_distribution[0].percent, 100.0);

        assert_eq!(thirty_day_distribution.len(), 1);
        assert_eq!(thirty_day_distribution[0].model_name, "model-b");
        assert_eq!(thirty_day_distribution[0].token_used, 1000);
        assert_eq!(thirty_day_distribution[0].request_count, 20);
    }

    fn test_fact(
        session_id: &str,
        timestamp_sec: i64,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cache_create_tokens: u64,
        cache_read_tokens: u64,
        coverage_origin: CoverageOrigin,
        status_code: Option<u16>,
    ) -> MergedRequestFact {
        MergedRequestFact {
            canonical_request_key: format!("claude_code:{}:{timestamp_sec}:{model}", session_id),
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
            estimated_cost: 0.0,
            coverage_origin,
            status_code,
            duration_ms: None,
            output_tokens_per_second: None,
            ttft_ms: None,
            source_label: None,
        }
    }

    #[test]
    fn build_model_distribution_from_facts_supports_custom_summary_window() {
        let facts = vec![
            test_fact(
                "session-1",
                1,
                "model-a",
                100,
                50,
                10,
                0,
                CoverageOrigin::LocalOnly,
                None,
            ),
            test_fact(
                "session-2",
                2,
                "model-b",
                20,
                20,
                0,
                0,
                CoverageOrigin::ProxyOnly,
                Some(200),
            ),
        ];

        let distribution = build_model_distribution_from_window_stats(Some(
            &build_model_token_totals_from_facts(&facts),
        ));

        assert_eq!(distribution.len(), 2);
        assert_eq!(distribution[0].model_name, "model-a");
        assert_eq!(distribution[0].token_used, 160);
        assert_eq!(distribution[0].request_count, 1);
        assert!((distribution[0].percent - 80.0).abs() < f64::EPSILON);
        assert_eq!(distribution[1].model_name, "model-b");
        assert_eq!(distribution[1].token_used, 40);
        assert_eq!(distribution[1].request_count, 1);
        assert!((distribution[1].percent - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn build_usage_summary_from_usage_supports_custom_summary_window() {
        let facts = vec![
            test_fact(
                "session-1",
                1,
                "model-a",
                100,
                50,
                10,
                0,
                CoverageOrigin::LocalOnly,
                None,
            ),
            test_fact(
                "session-2",
                2,
                "model-b",
                20,
                20,
                0,
                0,
                CoverageOrigin::ProxyOnly,
                Some(200),
            ),
        ];
        let (summary_usage, _) = build_window_usage_from_facts("custom", &facts);

        let summary = build_usage_summary_from_usage(&summary_usage, 1, 0, 0);

        assert_eq!(summary.total_tokens, 200);
        assert_eq!(summary.total_requests, 2);
        assert_eq!(summary.total_input_tokens, 120);
        assert_eq!(summary.total_output_tokens, 70);
        assert_eq!(summary.total_cache_create_tokens, 10);
        assert_eq!(summary.total_cache_read_tokens, 0);
        assert_eq!(summary.total_success_requests, 1);
    }

    #[test]
    fn build_window_usage_local_synthetic_200_counts_as_success() {
        let facts = vec![
            test_fact(
                "session-1",
                1,
                "model-a",
                100,
                50,
                10,
                0,
                CoverageOrigin::LocalOnly,
                Some(200),
            ),
            test_fact(
                "session-2",
                2,
                "model-b",
                20,
                20,
                0,
                0,
                CoverageOrigin::ProxyOnly,
                Some(200),
            ),
        ];
        let (summary_usage, _) = build_window_usage_from_facts("custom", &facts);

        assert_eq!(summary_usage.request_used, 2);
        assert_eq!(summary_usage.success_requests, 2);
        assert_eq!(summary_usage.client_error_requests, 0);
        assert_eq!(summary_usage.server_error_requests, 0);
    }
}
