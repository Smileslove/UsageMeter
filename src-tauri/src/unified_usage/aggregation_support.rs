use super::derived_support::{project_descriptor_for_session, session_project_identity};
use super::reasonix_support::reasonix_explicit_cost_usd;
use crate::models::CurrencySettings;
use crate::proxy::{ProjectStats, ProjectToolStats};
use crate::session::{wsl_distro_from_path, SessionMeta};
use std::collections::{HashMap, HashSet};

pub(super) const REASONIX_FULL_COVERAGE_GRACE_SECS: i64 = 30;

#[derive(Default)]
pub(super) struct ProjectAggregate<'a> {
    pub(super) stats: ProjectStats,
    // 聚合生命周期内 facts 与 session metadata 均保持存活；借用稳定标识，避免
    // 为同一 session/tool 的每条事实反复分配 String。
    pub(super) sessions: HashSet<&'a str>,
    pub(super) tool_sessions: HashMap<&'a str, HashSet<&'a str>>,
}

pub(super) fn session_usage_fully_covered(
    meta: Option<&SessionMeta>,
    tool: &str,
    proxy_backed_requests: u64,
    unresolved_proxy_requests: u64,
    now_sec: i64,
) -> bool {
    if tool != "reasonix" {
        return true;
    }
    let Some(meta) = meta else {
        return false;
    };
    if meta.message_count == 0 {
        return false;
    }
    let last_activity = meta.end_time.max(meta.last_modified);
    if last_activity <= 0
        || now_sec.saturating_sub(last_activity) <= REASONIX_FULL_COVERAGE_GRACE_SECS
    {
        return false;
    }
    unresolved_proxy_requests == 0 && proxy_backed_requests == meta.message_count
}

pub(super) fn reasonix_uncovered_request_count(
    local_only_requests: u64,
    unresolved_proxy_requests: u64,
) -> u64 {
    local_only_requests.saturating_add(unresolved_proxy_requests)
}

pub(super) fn session_has_reasonix_coverage_gap(
    meta: &SessionMeta,
    proxy_backed_requests: u64,
    unresolved_proxy_requests: u64,
    now_sec: i64,
) -> bool {
    meta.tool == "reasonix"
        && !session_usage_fully_covered(
            Some(meta),
            &meta.tool,
            proxy_backed_requests,
            unresolved_proxy_requests,
            now_sec,
        )
}

pub(super) fn build_metadata_only_session_stats(
    meta: &SessionMeta,
    currency_settings: &CurrencySettings,
    now_sec: i64,
) -> crate::proxy::SessionStats {
    let explicit_cost_usd = reasonix_explicit_cost_usd(meta, currency_settings)
        .or(meta.explicit_estimated_cost)
        .filter(|cost| cost.is_finite() && *cost >= 0.0);
    crate::proxy::SessionStats {
        session_id: meta.session_id.clone(),
        tool: meta.tool.clone(),
        total_requests: meta.message_count,
        total_input_tokens: meta.total_input_tokens,
        total_output_tokens: meta.total_output_tokens,
        total_cache_create_tokens: meta.total_cache_create_tokens,
        total_cache_read_tokens: meta.total_cache_read_tokens,
        total_duration_ms: meta.total_elapsed_ms,
        avg_output_tokens_per_second: 0.0,
        first_request_time: meta.start_time,
        last_request_time: meta.end_time.max(meta.last_modified),
        models: meta.models.clone(),
        avg_ttft_ms: 0.0,
        success_requests: 0,
        error_requests: 0,
        estimated_cost: explicit_cost_usd.unwrap_or(0.0),
        is_cost_estimated: explicit_cost_usd.is_none(),
        usage_fully_covered: session_usage_fully_covered(Some(meta), &meta.tool, 0, 0, now_sec),
        covered_requests: 0,
        uncovered_requests: meta.message_count,
        cwd: meta.cwd.clone(),
        project_name: meta.project_name.clone(),
        project_identity: Some(
            session_project_identity(meta.project_name.as_deref(), meta.cwd.as_deref()).to_string(),
        ),
        topic: meta.topic.clone(),
        last_prompt: meta.last_prompt.clone(),
        session_name: meta.session_name.clone(),
        scope: meta.scope.clone(),
        wsl_distro: wsl_distro_from_path(&meta.file_path),
    }
}

pub(super) fn merge_metadata_only_project<'a>(
    map: &mut HashMap<String, ProjectAggregate<'a>>,
    meta: &'a SessionMeta,
) {
    let descriptor = project_descriptor_for_session(meta);
    let entry = map
        .entry(descriptor.key.clone())
        .or_insert_with(|| ProjectAggregate {
            stats: ProjectStats {
                name: descriptor.name.clone(),
                project_key: Some(descriptor.key.clone()),
                project_identity: Some(descriptor.identity.clone()),
                project_path: descriptor.path.clone(),
                usage_fully_covered: true,
                covered_requests: 0,
                uncovered_requests: 0,
                ..Default::default()
            },
            ..Default::default()
        });

    if entry.stats.project_path.is_none() {
        entry.stats.project_path = descriptor.path.clone();
    }
    if entry.stats.project_key.is_none() {
        entry.stats.project_key = Some(descriptor.key.clone());
    }
    if entry.stats.project_identity.is_none() {
        entry.stats.project_identity = Some(descriptor.identity);
    }
    entry.stats.request_count += meta.message_count;
    entry.stats.uncovered_requests += meta.message_count;
    entry.stats.total_input_tokens += meta.total_input_tokens;
    entry.stats.total_output_tokens += meta.total_output_tokens;
    entry.stats.total_cache_create_tokens += meta.total_cache_create_tokens;
    entry.stats.total_cache_read_tokens += meta.total_cache_read_tokens;
    entry.stats.last_active = entry
        .stats
        .last_active
        .max(meta.end_time.max(meta.last_modified));
    if !meta.session_id.trim().is_empty() {
        entry.sessions.insert(meta.session_id.as_str());
        entry
            .tool_sessions
            .entry(meta.tool.as_str())
            .or_default()
            .insert(meta.session_id.as_str());
    } else {
        entry.tool_sessions.entry(meta.tool.as_str()).or_default();
    }

    let tool_stats = entry
        .stats
        .tool_breakdown
        .iter_mut()
        .find(|stats| stats.tool == meta.tool);
    let tool_stats = match tool_stats {
        Some(stats) => stats,
        None => {
            entry.stats.tool_breakdown.push(ProjectToolStats {
                tool: meta.tool.clone(),
                usage_fully_covered: true,
                covered_requests: 0,
                uncovered_requests: 0,
                ..Default::default()
            });
            entry.stats.tool_breakdown.last_mut().unwrap()
        }
    };
    tool_stats.request_count += meta.message_count;
    tool_stats.uncovered_requests += meta.message_count;
    tool_stats.total_input_tokens += meta.total_input_tokens;
    tool_stats.total_output_tokens += meta.total_output_tokens;
    tool_stats.total_cache_create_tokens += meta.total_cache_create_tokens;
    tool_stats.total_cache_read_tokens += meta.total_cache_read_tokens;
    tool_stats.last_active = tool_stats
        .last_active
        .max(meta.end_time.max(meta.last_modified));
}
