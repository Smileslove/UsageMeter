use super::super::helpers::{
    epoch_u64_to_i64_saturating, first_fact_index_in_range, usage_window_cutoff_epoch,
};
use super::super::types::{
    PreparedUsageRefreshData, UsageRefreshBundle, UsageWindowPreparedSummary, WindowPreparedFacts,
    MERGED_SOURCE,
};
use crate::commands::usage::accumulator::FactAccumulator;
use crate::models::{AppSettings, UsageSnapshot, WindowUsage};
use crate::unified_usage::{has_partial_coverage, normalize_model_bucket, MergedRequestFact};
use std::collections::HashMap;

fn facts_slice_for_window<'a>(
    prepared: &'a PreparedUsageRefreshData,
    window: &str,
) -> &'a [MergedRequestFact] {
    prepared
        .windows
        .iter()
        .find(|entry| entry.window == window)
        .and_then(|entry| {
            entry
                .start_index
                .map(|start_index| &prepared.facts[start_index..])
        })
        .unwrap_or(&prepared.facts[..0])
}

fn can_use_unified_daily_summary(settings: &AppSettings) -> bool {
    settings.client_tools.active_tool_filter.is_none()
        && settings.source_aware.active_source_filter.is_none()
}

fn next_business_date(date: &str, settings: &AppSettings) -> Result<String, String> {
    let (_, end_epoch) = crate::utils::business_time::business_date_epoch_bounds(date, settings)?;
    Ok(crate::utils::business_time::business_date_for_timestamp(
        end_epoch, settings,
    ))
}

fn empty_window_usage(window: &str) -> WindowUsage {
    WindowUsage {
        window: window.to_string(),
        token_used: 0,
        input_tokens: 0,
        output_tokens: 0,
        cache_create_tokens: 0,
        cache_read_tokens: 0,
        request_used: 0,
        cost: 0.0,
        success_requests: 0,
        client_error_requests: 0,
        server_error_requests: 0,
    }
}

fn merge_window_usage(target: &mut WindowUsage, delta: &WindowUsage) {
    target.token_used = target.token_used.saturating_add(delta.token_used);
    target.input_tokens = target.input_tokens.saturating_add(delta.input_tokens);
    target.output_tokens = target.output_tokens.saturating_add(delta.output_tokens);
    target.cache_create_tokens = target
        .cache_create_tokens
        .saturating_add(delta.cache_create_tokens);
    target.cache_read_tokens = target
        .cache_read_tokens
        .saturating_add(delta.cache_read_tokens);
    target.request_used = target.request_used.saturating_add(delta.request_used);
    target.cost += delta.cost;
    target.success_requests = target
        .success_requests
        .saturating_add(delta.success_requests);
    target.client_error_requests = target
        .client_error_requests
        .saturating_add(delta.client_error_requests);
    target.server_error_requests = target
        .server_error_requests
        .saturating_add(delta.server_error_requests);
}

fn usage_from_daily_summary_row(
    window: &str,
    row: &crate::local_usage::UnifiedDailySummaryRow,
    include_errors: bool,
) -> WindowUsage {
    if include_errors {
        WindowUsage {
            window: window.to_string(),
            token_used: row.total_tokens,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_create_tokens: row.cache_create_tokens,
            cache_read_tokens: row.cache_read_tokens,
            request_used: row.request_count,
            cost: row.total_cost,
            success_requests: row.success_request_count,
            client_error_requests: row.client_error_requests,
            server_error_requests: row.server_error_requests,
        }
    } else {
        WindowUsage {
            window: window.to_string(),
            token_used: row.visible_total_tokens,
            input_tokens: row.visible_input_tokens,
            output_tokens: row.visible_output_tokens,
            cache_create_tokens: row.visible_cache_create_tokens,
            cache_read_tokens: row.visible_cache_read_tokens,
            request_used: row.visible_request_count,
            cost: row.visible_cost,
            success_requests: row.success_request_count,
            client_error_requests: 0,
            server_error_requests: 0,
        }
    }
}

#[derive(Default)]
struct CoverageAccumulator {
    proxy_backed_requests: u64,
    local_only_requests: u64,
}

impl CoverageAccumulator {
    fn add_fact(&mut self, fact: &MergedRequestFact) {
        let request_count = fact.request_count.max(1);
        match fact.coverage_origin {
            crate::unified_usage::CoverageOrigin::ProxyOnly => {
                self.proxy_backed_requests =
                    self.proxy_backed_requests.saturating_add(request_count);
            }
            crate::unified_usage::CoverageOrigin::LocalOnly => {
                self.local_only_requests = self.local_only_requests.saturating_add(request_count);
            }
            crate::unified_usage::CoverageOrigin::MergedProxyPreferred => {
                self.proxy_backed_requests =
                    self.proxy_backed_requests.saturating_add(request_count);
            }
        }
    }

    fn add_row(&mut self, row: &crate::local_usage::UnifiedDailySummaryRow) {
        self.proxy_backed_requests = self
            .proxy_backed_requests
            .saturating_add(row.proxy_backed_requests);
        self.local_only_requests = self
            .local_only_requests
            .saturating_add(row.local_only_requests);
    }

    fn has_partial_coverage(&self) -> bool {
        has_partial_coverage(self.proxy_backed_requests, self.local_only_requests)
    }
}

fn include_fact_for_window(fact: &MergedRequestFact, start_epoch: i64, end_epoch: i64) -> bool {
    fact.timestamp_sec >= start_epoch && fact.timestamp_sec < end_epoch
}

fn include_fact_for_usage(
    fact: &MergedRequestFact,
    start_epoch: i64,
    end_epoch: i64,
    include_errors: bool,
) -> bool {
    include_fact_for_window(fact, start_epoch, end_epoch)
        && (include_errors || fact.status_code.map(|code| code < 300).unwrap_or(true))
}

async fn precompute_window_usage_from_summary(
    settings: &AppSettings,
    window: &str,
    start_epoch: i64,
    end_epoch: i64,
    include_errors: bool,
    fetched_facts: &[MergedRequestFact],
) -> Result<UsageWindowPreparedSummary, String> {
    crate::unified_usage::ensure_materialized_history_no_sync(settings, start_epoch, end_epoch)
        .await?;
    let local_db = crate::local_usage::get_local_usage_db()?;
    let start_date =
        crate::utils::business_time::business_date_for_timestamp(start_epoch, settings);
    let today_date =
        crate::local_usage::LocalUsageDatabase::today_local_date_with_settings(settings);
    let (start_day_start, start_day_end) =
        crate::local_usage::LocalUsageDatabase::local_date_epoch_bounds_with_settings(
            &start_date,
            settings,
        )?;
    let (today_start, _) =
        crate::local_usage::LocalUsageDatabase::local_date_epoch_bounds_with_settings(
            &today_date,
            settings,
        )?;

    let mut usage = empty_window_usage(window);
    let mut coverage = CoverageAccumulator::default();
    let mut has_partial_status_coverage = false;
    let mut has_partial_performance_coverage = false;

    if start_date < today_date {
        let summary_start = if start_epoch == start_day_start {
            start_date.clone()
        } else {
            next_business_date(&start_date, settings)?
        };
        if summary_start < today_date {
            let rows = local_db.get_unified_daily_summaries_between(&summary_start, &today_date)?;
            for row in rows {
                merge_window_usage(
                    &mut usage,
                    &usage_from_daily_summary_row(window, &row, include_errors),
                );
                coverage.add_row(&row);
                has_partial_status_coverage |= row.has_partial_status_coverage;
                has_partial_performance_coverage |= row.has_partial_performance_coverage
                    || has_partial_coverage(row.proxy_backed_requests, row.local_only_requests);
            }
        }

        if start_epoch > start_day_start {
            let mut start_day_facts = local_db.get_unified_facts_for_dates(
                std::slice::from_ref(&start_date),
                &settings.client_tools.build_filter(),
            )?;
            start_day_facts.retain(|fact| {
                include_fact_for_usage(
                    fact,
                    start_epoch,
                    end_epoch.min(start_day_end),
                    include_errors,
                )
            });
            let (boundary_usage, _) = build_window_usage_from_facts(window, &start_day_facts);
            merge_window_usage(&mut usage, &boundary_usage);
            for fact in &start_day_facts {
                coverage.add_fact(fact);
            }
            has_partial_performance_coverage |= coverage.has_partial_coverage();
        }
    }

    if end_epoch > today_start {
        let hot_start = start_epoch.max(today_start);
        if end_epoch > hot_start {
            let (hot_usage, _) = build_window_usage_from_facts(
                window,
                &fetched_facts
                    .iter()
                    .filter(|fact| {
                        include_fact_for_usage(fact, hot_start, end_epoch, include_errors)
                    })
                    .cloned()
                    .collect::<Vec<_>>(),
            );
            merge_window_usage(&mut usage, &hot_usage);
            for fact in fetched_facts {
                if include_fact_for_window(fact, hot_start, end_epoch) {
                    coverage.add_fact(fact);
                }
            }
            has_partial_performance_coverage |= coverage.has_partial_coverage();
        }
    }

    Ok(UsageWindowPreparedSummary {
        usage,
        has_partial_status_coverage,
        has_partial_performance_coverage,
    })
}

async fn prepare_usage_refresh_data_internal(
    settings: &AppSettings,
    generated_at_epoch: u64,
) -> Result<PreparedUsageRefreshData, String> {
    let include_errors = settings.proxy.include_error_requests;
    let summary_window = settings.summary_window.clone();
    let summary_cutoff = usage_window_cutoff_epoch(&summary_window, settings);
    let survival_cutoff = usage_window_cutoff_epoch("7d", settings);
    let facts_cutoff = summary_cutoff.min(survival_cutoff);
    let end_epoch = generated_at_epoch.saturating_add(1);
    let (facts, _coverage) = crate::unified_usage::get_merged_request_facts_no_sync(
        settings,
        Some(facts_cutoff),
        Some(epoch_u64_to_i64_saturating(end_epoch)),
        include_errors,
    )
    .await?;

    let mut windows = Vec::new();
    let can_use_summary = can_use_unified_daily_summary(settings);
    let cutoff_epoch = usage_window_cutoff_epoch(&summary_window, settings);
    if cutoff_epoch >= facts_cutoff || !can_use_summary {
        windows.push(WindowPreparedFacts {
            start_index: Some(first_fact_index_in_range(&facts, cutoff_epoch)),
            precomputed_usage: None,
            window: summary_window.clone(),
        });
    } else {
        windows.push(WindowPreparedFacts {
            start_index: None,
            precomputed_usage: Some(
                precompute_window_usage_from_summary(
                    settings,
                    &summary_window,
                    cutoff_epoch,
                    epoch_u64_to_i64_saturating(end_epoch),
                    include_errors,
                    &facts,
                )
                .await?,
            ),
            window: summary_window.clone(),
        });
    }

    Ok(PreparedUsageRefreshData {
        generated_at_epoch,
        facts,
        windows,
    })
}

pub(super) async fn prepare_usage_refresh_data_no_sync(
    settings: &AppSettings,
    generated_at_epoch: u64,
) -> Result<PreparedUsageRefreshData, String> {
    prepare_usage_refresh_data_internal(settings, generated_at_epoch).await
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

        let model_name = normalize_model_bucket(&fact.tool, &fact.model);
        if model_name != "unknown" {
            let entry = model_stats.entry(model_name).or_default();
            entry.input_tokens += fact.input_tokens;
            entry.output_tokens += fact.output_tokens;
            entry.cache_create_tokens += fact.cache_create_tokens;
            entry.cache_read_tokens += fact.cache_read_tokens;
            entry.request_count += fact.request_count.max(1);
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
        let model_name = normalize_model_bucket(&fact.tool, &fact.model);
        if model_name == "unknown" {
            continue;
        }

        let entry = model_stats.entry(model_name).or_default();
        entry.input_tokens += fact.input_tokens;
        entry.output_tokens += fact.output_tokens;
        entry.cache_create_tokens += fact.cache_create_tokens;
        entry.cache_read_tokens += fact.cache_read_tokens;
        entry.request_count += fact.request_count.max(1);
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

pub(super) fn summarize_status_counts(facts: &[MergedRequestFact]) -> (u64, u64, u64) {
    let mut success_requests = 0_u64;
    let mut client_error_requests = 0_u64;
    let mut server_error_requests = 0_u64;

    for fact in facts {
        let request_count = fact.request_count.max(1);
        if let Some(status_code) = fact.status_code {
            if (200..300).contains(&status_code) {
                success_requests += request_count;
            } else if (400..500).contains(&status_code) {
                client_error_requests += request_count;
            } else if status_code >= 500 {
                server_error_requests += request_count;
            }
        }
    }

    (
        success_requests,
        client_error_requests,
        server_error_requests,
    )
}

pub(super) fn build_usage_refresh_bundle_from_prepared(
    settings: &AppSettings,
    prepared: &PreparedUsageRefreshData,
) -> UsageRefreshBundle {
    let mut windows = Vec::new();
    let mut has_partial_snapshot_coverage = false;
    let mut summary_model_stats: Option<HashMap<String, ModelTokenTotals>> = None;

    for entry in &prepared.windows {
        if let Some(precomputed) = &entry.precomputed_usage {
            has_partial_snapshot_coverage |= precomputed.has_partial_status_coverage
                || precomputed.has_partial_performance_coverage;
            windows.push(precomputed.usage.clone());
            continue;
        }

        let facts = facts_slice_for_window(prepared, &entry.window);
        let coverage = crate::unified_usage::build_coverage(facts);
        has_partial_snapshot_coverage |=
            coverage.has_partial_status_coverage || coverage.has_partial_performance_coverage;
        let (window_usage, model_stats) = build_window_usage_from_facts(&entry.window, facts);
        if entry.window == settings.summary_window {
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
    let (summary_success_requests, summary_client_error_requests, summary_server_error_requests) =
        summarize_status_counts(summary_facts);

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

    let limit_survival = crate::commands::usage::survival::build_limit_survival(
        &prepared.facts,
        epoch_u64_to_i64_saturating(prepared.generated_at_epoch),
    );

    UsageRefreshBundle {
        generated_at_epoch: prepared.generated_at_epoch,
        snapshot,
        limit_survival,
    }
}
