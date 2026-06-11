use super::super::helpers::{
    epoch_u64_to_i64_saturating, first_fact_index_in_range, usage_window_cutoff_epoch,
};
use super::super::types::{
    PreparedUsageRefreshData, UsageRefreshBundle, WindowPreparedFacts, MERGED_SOURCE, USAGE_WINDOWS,
};
use super::breakdown::build_overview_breakdown_from_facts;
use super::rate::build_window_rate_summary_from_facts;
use crate::commands::usage::accumulator::FactAccumulator;
use crate::models::{AppSettings, UsageSnapshot, WindowUsage};
use crate::unified_usage::MergedRequestFact;
use std::collections::HashMap;

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

pub(super) async fn prepare_usage_refresh_data(
    settings: &AppSettings,
    generated_at_epoch: u64,
) -> Result<PreparedUsageRefreshData, String> {
    let include_errors = settings.proxy.include_error_requests;
    let mut window_cutoffs: Vec<(String, i64)> = USAGE_WINDOWS
        .iter()
        .map(|window| {
            (
                (*window).to_string(),
                usage_window_cutoff_epoch(window, settings),
            )
        })
        .collect();
    let summary_window = settings.summary_window.clone();
    if !window_cutoffs
        .iter()
        .any(|(window, _)| *window == summary_window)
    {
        window_cutoffs.push((
            summary_window.clone(),
            usage_window_cutoff_epoch(&summary_window, settings),
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

#[derive(Default, Clone)]
pub(super) struct ModelTokenTotals {
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_create_tokens: u64,
    pub(super) cache_read_tokens: u64,
    pub(super) request_count: u64,
}

pub(super) fn build_window_usage_from_facts(
    window: &str,
    facts: &[MergedRequestFact],
) -> (WindowUsage, HashMap<String, ModelTokenTotals>) {
    let mut overall = FactAccumulator::default();
    let mut model_stats: HashMap<String, ModelTokenTotals> = HashMap::new();

    for fact in facts {
        overall.add_fact(fact);

        if !fact.model.is_empty() {
            let entry = model_stats.entry(fact.model.clone()).or_default();
            entry.input_tokens += fact.input_tokens;
            entry.output_tokens += fact.output_tokens;
            entry.cache_create_tokens += fact.cache_create_tokens;
            entry.cache_read_tokens += fact.cache_read_tokens;
            entry.request_count += 1;
        }
    }

    (
        WindowUsage {
            window: window.to_string(),
            token_used: overall.total_tokens,
            input_tokens: overall.input_tokens,
            output_tokens: overall.output_tokens,
            cache_create_tokens: overall.cache_create_tokens,
            cache_read_tokens: overall.cache_read_tokens,
            request_used: overall.request_count,
            cost: overall.cost,
            success_requests: overall.success_requests,
            client_error_requests: overall.client_error_requests,
            server_error_requests: overall.server_error_requests,
        },
        model_stats,
    )
}

pub(super) fn build_model_distribution_from_window_stats(
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

pub(super) fn build_model_token_totals_from_facts(
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

pub(super) fn build_usage_summary_from_usage(
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

pub(super) fn build_usage_refresh_bundle_from_prepared(
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

    let limit_survival = crate::commands::usage::survival::build_limit_survival(
        &prepared.facts,
        epoch_u64_to_i64_saturating(prepared.generated_at_epoch),
    );

    UsageRefreshBundle {
        generated_at_epoch: prepared.generated_at_epoch,
        snapshot,
        rate_summary,
        overview_breakdown,
        limit_survival,
    }
}
