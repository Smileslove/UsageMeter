use super::types::{canonical_request_key_for_proxy, CoverageOrigin, MergedRequestFact};
use crate::models::{CurrencySettings, SourceFilter};
use crate::proxy::UsageRecord;
use crate::session::SessionMeta;
use std::collections::{HashMap, HashSet};

fn reasonix_session_matches_proxy_fact(meta: &SessionMeta, fact: &MergedRequestFact) -> bool {
    if meta.tool != "reasonix" || fact.tool != "reasonix" {
        return false;
    }
    if !fact.session_id.trim().is_empty() {
        return false;
    }

    let proxy_model_key = crate::models::normalize_model_id(&fact.model);
    if !meta.models.is_empty()
        && !meta
            .models
            .iter()
            .any(|model| crate::models::normalize_model_id(model) == proxy_model_key)
    {
        return false;
    }

    let start = if meta.start_time > 0 {
        meta.start_time
    } else {
        meta.last_modified.saturating_sub(15)
    };
    let end = meta.end_time.max(meta.last_modified);
    let effective_end = if end > 0 {
        end
    } else {
        start.saturating_add(15)
    };
    let grace = 15;
    fact.timestamp_sec >= start.saturating_sub(grace)
        && fact.timestamp_sec <= effective_end.saturating_add(grace)
}

pub(super) fn count_unresolved_reasonix_requests_by_session(
    facts: &[MergedRequestFact],
    local_sessions: &[SessionMeta],
) -> HashMap<String, u64> {
    let reasonix_sessions: Vec<&SessionMeta> = local_sessions
        .iter()
        .filter(|meta| meta.tool == "reasonix" && !meta.session_id.trim().is_empty())
        .collect();
    let mut unresolved_by_session: HashMap<String, u64> = HashMap::new();

    for fact in facts
        .iter()
        .filter(|fact| fact.tool == "reasonix" && fact.session_id.trim().is_empty())
    {
        for meta in reasonix_sessions
            .iter()
            .copied()
            .filter(|meta| reasonix_session_matches_proxy_fact(meta, fact))
        {
            *unresolved_by_session
                .entry(meta.session_id.clone())
                .or_default() += 1;
        }
    }

    unresolved_by_session
}

fn reasonix_telemetry_timestamp(meta: &SessionMeta) -> i64 {
    if meta.end_time > 0 {
        meta.end_time
    } else {
        meta.last_modified
    }
}

fn has_reasonix_telemetry_usage(meta: &SessionMeta) -> bool {
    meta.tool == "reasonix"
        && meta.message_count > 0
        && (meta.total_input_tokens > 0
            || meta.total_output_tokens > 0
            || meta.total_cache_create_tokens > 0
            || meta.total_cache_read_tokens > 0
            || meta.total_elapsed_ms > 0
            || meta.explicit_cost.is_some()
            || !meta.usage_sources.is_empty())
}

fn reasonix_residual_allowed_for_source_filter(source_filter: &SourceFilter) -> bool {
    matches!(
        source_filter,
        SourceFilter::All | SourceFilter::Unknown { .. }
    )
}

pub(super) fn reasonix_coverage_query_bounds<'a>(
    sessions: impl Iterator<Item = &'a SessionMeta>,
    source_filter: &SourceFilter,
    range_start: i64,
    range_end: i64,
) -> Option<(i64, i64)> {
    if !reasonix_residual_allowed_for_source_filter(source_filter) {
        return None;
    }

    let mut min_start = i64::MAX;
    let mut max_end = 0_i64;
    let mut found = false;
    for meta in sessions.filter(|meta| has_reasonix_telemetry_usage(meta)) {
        let telemetry_ts = reasonix_telemetry_timestamp(meta);
        if telemetry_ts < range_start || telemetry_ts >= range_end {
            continue;
        }
        let session_start = if meta.start_time > 0 {
            meta.start_time
        } else {
            telemetry_ts.saturating_sub(15)
        };
        min_start = min_start.min(session_start.saturating_sub(15).max(0));
        // fetch_proxy_records 使用半开区间，额外加 16 秒同时覆盖 15 秒归属宽限。
        max_end = max_end.max(telemetry_ts.saturating_add(16));
        found = true;
    }

    found.then_some((min_start, max_end))
}

#[derive(Debug, Clone, Default)]
pub(super) struct ReasonixProxyCoverage {
    pub(super) request_count: u64,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_create_tokens: u64,
    pub(super) cache_read_tokens: u64,
    pub(super) estimated_cost_usd: f64,
    pub(super) duration_ms: u64,
}

fn reasonix_proxy_timestamp_sec(record: &UsageRecord) -> i64 {
    if record.request_end_time > 0 {
        record.request_end_time / 1000
    } else {
        record.timestamp / 1000
    }
}

fn reasonix_session_matches_proxy_record(
    meta: &SessionMeta,
    record: &UsageRecord,
    require_model_match: bool,
) -> bool {
    if meta.tool != "reasonix" || record.client_tool != "reasonix" {
        return false;
    }
    if require_model_match {
        let proxy_model_key = crate::models::normalize_model_id(&record.model);
        if !meta.models.is_empty()
            && !meta
                .models
                .iter()
                .any(|model| crate::models::normalize_model_id(model) == proxy_model_key)
        {
            return false;
        }
    }

    let start = if meta.start_time > 0 {
        meta.start_time
    } else {
        meta.last_modified.saturating_sub(15)
    };
    let end = reasonix_telemetry_timestamp(meta);
    let effective_end = if end > 0 {
        end
    } else {
        start.saturating_add(15)
    };
    let timestamp = reasonix_proxy_timestamp_sec(record);
    timestamp >= start.saturating_sub(15) && timestamp <= effective_end.saturating_add(15)
}

fn add_reasonix_proxy_coverage(coverage: &mut ReasonixProxyCoverage, record: &UsageRecord) {
    coverage.request_count = coverage.request_count.saturating_add(1);
    coverage.input_tokens = coverage.input_tokens.saturating_add(record.input_tokens);
    coverage.output_tokens = coverage.output_tokens.saturating_add(record.output_tokens);
    coverage.cache_create_tokens = coverage
        .cache_create_tokens
        .saturating_add(record.cache_create_tokens);
    coverage.cache_read_tokens = coverage
        .cache_read_tokens
        .saturating_add(record.cache_read_tokens);
    if record.estimated_cost.is_finite() && record.estimated_cost > 0.0 {
        coverage.estimated_cost_usd += record.estimated_cost;
    }
    coverage.duration_ms = coverage.duration_ms.saturating_add(record.duration_ms);
}

pub(super) fn build_reasonix_proxy_coverage_by_session(
    records: &[UsageRecord],
    sessions: &HashMap<String, SessionMeta>,
) -> (HashMap<String, ReasonixProxyCoverage>, HashSet<String>) {
    let target_sessions: Vec<&SessionMeta> = sessions
        .values()
        .filter(|meta| has_reasonix_telemetry_usage(meta))
        .collect();
    let mut coverage_by_session: HashMap<String, ReasonixProxyCoverage> = HashMap::new();
    let mut blocked_sessions = HashSet::new();
    let mut seen_keys = HashSet::new();

    for record in records.iter().filter(|record| {
        record.client_tool == "reasonix"
            // Reasonix 只在 provider 返回有效 Usage 时增加 telemetry.requestCount。
            // coverage 因而必须独立于查询的 include_errors，只接受成功且确有 Token
            // 事实的代理记录；错误、599 和历史全零记录都不能消耗 telemetry 覆盖量。
            && (200..300).contains(&record.status_code)
            && (record.input_tokens > 0
                || record.output_tokens > 0
                || record.cache_create_tokens > 0
                || record.cache_read_tokens > 0
                || record.reasoning_tokens > 0
                || record.total_tokens > 0)
    }) {
        if !seen_keys.insert(canonical_request_key_for_proxy(record)) {
            continue;
        }

        if record.message_id_conflicted {
            if let Some(session_id) = record.session_id.as_deref() {
                if sessions.contains_key(session_id) {
                    blocked_sessions.insert(session_id.to_string());
                }
            }
            continue;
        }

        if let Some(session_id) = record
            .session_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if sessions.contains_key(session_id) {
                add_reasonix_proxy_coverage(
                    coverage_by_session
                        .entry(session_id.to_string())
                        .or_default(),
                    record,
                );
            }
            continue;
        }

        let strict_candidates: Vec<&SessionMeta> = target_sessions
            .iter()
            .copied()
            .filter(|meta| reasonix_session_matches_proxy_record(meta, record, true))
            .collect();
        match strict_candidates.as_slice() {
            [meta] => add_reasonix_proxy_coverage(
                coverage_by_session
                    .entry(meta.session_id.clone())
                    .or_default(),
                record,
            ),
            [] => {
                // 模型不匹配仍可能是新模型名尚未写入会话元数据。只按时间找出可能
                // 受影响的会话并阻止补齐，宁可少算也不把同一请求重复计入。
                for meta in target_sessions
                    .iter()
                    .copied()
                    .filter(|meta| reasonix_session_matches_proxy_record(meta, record, false))
                {
                    blocked_sessions.insert(meta.session_id.clone());
                }
            }
            candidates => {
                for meta in candidates {
                    blocked_sessions.insert(meta.session_id.clone());
                }
            }
        }
    }

    (coverage_by_session, blocked_sessions)
}

/// Normalize Reasonix's persisted currency symbol/code to an ISO 4217 code.
fn normalize_reasonix_currency_code(value: &str) -> String {
    let upper = value.trim().to_ascii_uppercase();
    match upper.as_str() {
        "CNY" | "RMB" | "CNH" | "¥" | "￥" => "CNY".to_string(),
        "USD" | "$" | "US$" => "USD".to_string(),
        "EUR" | "€" => "EUR".to_string(),
        "GBP" | "£" => "GBP".to_string(),
        _ => upper,
    }
}

pub(super) fn reasonix_explicit_cost_usd(
    meta: &SessionMeta,
    currency_settings: &CurrencySettings,
) -> Option<f64> {
    // explicit_cost 只有与明确币种配对时才可换算。Reasonix 的历史
    // sessionCostUsd 只是兼容别名，不保证实际为 USD，不能因币种缺失而默认成 USD。
    if let (Some(cost), Some(currency)) =
        (meta.explicit_cost, meta.explicit_cost_currency.as_deref())
    {
        if !cost.is_finite() || cost < 0.0 {
            return None;
        }
        let currency = normalize_reasonix_currency_code(currency);
        if currency == "USD" {
            return Some(cost);
        }
        let rate = currency_settings.exchange_rates.get(&currency).copied()?;
        return (rate.is_finite() && rate > 0.0).then_some(cost / rate);
    }

    // 旧 reader 的 explicit_estimated_cost 字段语义始终是已确认的 USD。
    meta.explicit_estimated_cost
        .filter(|cost| cost.is_finite() && *cost >= 0.0)
}

pub(super) fn build_reasonix_telemetry_residual(
    meta: &SessionMeta,
    covered: Option<&ReasonixProxyCoverage>,
    is_blocked: bool,
    source_filter: &SourceFilter,
    currency_settings: &CurrencySettings,
    range_start: i64,
    range_end: i64,
) -> Option<MergedRequestFact> {
    if !has_reasonix_telemetry_usage(meta)
        || is_blocked
        || !reasonix_residual_allowed_for_source_filter(source_filter)
    {
        return None;
    }
    let timestamp_sec = reasonix_telemetry_timestamp(meta);
    if timestamp_sec < range_start || timestamp_sec >= range_end {
        return None;
    }

    let covered = covered.cloned().unwrap_or_default();
    let request_count = meta.message_count.saturating_sub(covered.request_count);
    // 请求数已被完整生命周期代理事实覆盖时，不因 provider 字段口径差异再次补
    // Token；否则同一个请求会以“字段残差”形式被重复计成第二条请求。
    if request_count == 0 {
        return None;
    }

    let input_tokens = meta.total_input_tokens.saturating_sub(covered.input_tokens);
    let output_tokens = meta
        .total_output_tokens
        .saturating_sub(covered.output_tokens);
    let cache_create_tokens = meta
        .total_cache_create_tokens
        .saturating_sub(covered.cache_create_tokens);
    let cache_read_tokens = meta
        .total_cache_read_tokens
        .saturating_sub(covered.cache_read_tokens);
    let total_tokens = input_tokens
        .saturating_add(output_tokens)
        .saturating_add(cache_create_tokens)
        .saturating_add(cache_read_tokens);
    let estimated_cost = reasonix_explicit_cost_usd(meta, currency_settings)
        .map(|total| (total - covered.estimated_cost_usd).max(0.0))
        .unwrap_or(0.0);
    let duration_ms = meta.total_elapsed_ms.saturating_sub(covered.duration_ms);
    let model = if meta.models.len() == 1 {
        meta.models[0].clone()
    } else {
        "reasonix-telemetry".to_string()
    };

    Some(MergedRequestFact {
        canonical_request_key: format!(
            "reasonix:telemetry-residual:{}:{}",
            meta.session_id, timestamp_sec
        ),
        session_id: meta.session_id.clone(),
        project_name: meta.project_name.clone(),
        project_path: meta.cwd.clone(),
        api_key_prefix: None,
        request_base_url: None,
        tool: meta.tool.clone(),
        timestamp_sec,
        timestamp_ms: timestamp_sec.saturating_mul(1000),
        model,
        input_tokens,
        output_tokens,
        cache_create_tokens,
        cache_read_tokens,
        total_tokens,
        request_count,
        estimated_cost,
        coverage_origin: CoverageOrigin::LocalOnly,
        status_code: Some(200),
        duration_ms: (duration_ms > 0).then_some(duration_ms),
        output_tokens_per_second: None,
        ttft_ms: None,
        source_label: None,
    })
}

#[cfg(test)]
mod tests {
    use super::normalize_reasonix_currency_code;

    #[test]
    fn currency_symbols_normalize_to_iso_codes() {
        assert_eq!(normalize_reasonix_currency_code(" ¥ "), "CNY");
        assert_eq!(normalize_reasonix_currency_code("us$"), "USD");
        assert_eq!(normalize_reasonix_currency_code("EUR"), "EUR");
    }
}
