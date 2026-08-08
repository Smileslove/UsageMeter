use super::match_support::{
    attach_proxy_session_ids, compute_local_request_cost_cached, request_key_for_local,
    request_key_for_proxy,
};
use super::reasonix_support::{
    build_reasonix_proxy_coverage_by_session, build_reasonix_telemetry_residual,
};
use super::types::{
    codex_orphan_pools, find_codex_fuzzy_matches, has_partial_coverage,
    session_meta_lookup_key_for_proxy, CodexFuzzyOutcome, CoverageOrigin, MergedCoverage,
    MergedRequestFact,
};
use crate::models::{CurrencySettings, ModelPricingConfig, SourceFilter};
use crate::proxy::UsageRecord;
use crate::session::{LocalRequestRecord, SessionMeta};
use std::collections::{HashMap, HashSet};

/// 合并引擎的完整输入。调用方负责读取存储、解析配置并构造这些内存快照。
pub(super) struct RealtimeMergeInput {
    pub(super) local_records: Vec<LocalRequestRecord>,
    pub(super) session_meta_by_id: HashMap<String, SessionMeta>,
    pub(super) message_to_session: HashMap<String, String>,
    pub(super) raw_proxy_records: Vec<UsageRecord>,
    pub(super) raw_unfiltered_proxy_records: Option<Vec<UsageRecord>>,
    pub(super) raw_reasonix_coverage_proxy_records: Vec<UsageRecord>,
    pub(super) source_filter: SourceFilter,
    pub(super) currency_settings: CurrencySettings,
    pub(super) range_start: i64,
    pub(super) range_end: i64,
    pub(super) include_errors: bool,
    pub(super) pricings: Vec<ModelPricingConfig>,
    pub(super) pricing_match_mode: String,
    pub(super) codex_fallback_base_url: Option<String>,
}

pub(crate) fn build_coverage(facts: &[MergedRequestFact]) -> MergedCoverage {
    let mut coverage = MergedCoverage::default();

    for fact in facts {
        match fact.coverage_origin {
            CoverageOrigin::ProxyOnly => coverage.proxy_backed_requests += 1,
            CoverageOrigin::LocalOnly => coverage.local_only_requests += 1,
            CoverageOrigin::MergedProxyPreferred | CoverageOrigin::MergedFuzzyMatched => {
                coverage.proxy_backed_requests += 1;
                coverage.merged_overlap_requests += 1;
            }
        }
    }

    let has_partial =
        has_partial_coverage(coverage.proxy_backed_requests, coverage.local_only_requests);
    // Local-only requests carry a synthetic Some(200); status suppression is no longer needed.
    coverage.has_partial_status_coverage = false;
    coverage.has_partial_performance_coverage = has_partial;
    coverage
}

fn build_local_request_index(
    local_records: &[LocalRequestRecord],
) -> HashMap<String, LocalRequestRecord> {
    local_records
        .iter()
        .cloned()
        .map(|record| (request_key_for_local(&record), record))
        .collect()
}

fn build_proxy_request_index(proxy_records: &[UsageRecord]) -> HashMap<String, UsageRecord> {
    proxy_records
        .iter()
        .cloned()
        .map(|record| (request_key_for_proxy(&record), record))
        .collect()
}

/// 合并本地事实与代理事实。该函数只操作调用方传入的内存快照，不访问数据库、
/// 文件系统、全局缓存或 Tauri runtime。
pub(super) fn merge_realtime_facts(input: RealtimeMergeInput) -> Vec<MergedRequestFact> {
    let RealtimeMergeInput {
        local_records,
        session_meta_by_id,
        message_to_session,
        raw_proxy_records,
        raw_unfiltered_proxy_records,
        raw_reasonix_coverage_proxy_records,
        source_filter,
        currency_settings,
        range_start,
        range_end,
        include_errors,
        pricings,
        pricing_match_mode,
        codex_fallback_base_url,
    } = input;
    let mut attached_proxy_records = raw_proxy_records;
    attach_proxy_session_ids(&mut attached_proxy_records, &message_to_session);
    let proxy_records: Vec<UsageRecord> = attached_proxy_records
        .iter()
        .filter(|record| include_errors || (200..300).contains(&record.status_code))
        .cloned()
        .collect();
    let all_proxy_records = if let Some(mut unfiltered) = raw_unfiltered_proxy_records {
        attach_proxy_session_ids(&mut unfiltered, &message_to_session);
        unfiltered
    } else {
        attached_proxy_records
    };

    let all_proxy_index = build_proxy_request_index(&all_proxy_records);
    let proxy_index = build_proxy_request_index(&proxy_records);
    let local_index = build_local_request_index(&local_records);

    let (codex_local_orphans, codex_proxy_orphans_visible, codex_proxy_orphans_all_extra) =
        codex_orphan_pools(&local_index, &proxy_index, &all_proxy_index);
    let codex_fuzzy_outcomes = find_codex_fuzzy_matches(
        &codex_local_orphans,
        &codex_proxy_orphans_visible,
        &codex_proxy_orphans_all_extra,
    );

    let mut fuzzy_consumed_local_keys = HashSet::new();
    let mut fuzzy_consumed_proxy_keys = HashSet::new();
    let mut fuzzy_suppressed_local_keys = HashSet::new();
    let mut pricing_cache = HashMap::new();
    let mut merged = Vec::new();

    for outcome in codex_fuzzy_outcomes {
        match outcome {
            CodexFuzzyOutcome::MatchedVisible {
                local_key,
                proxy_key,
            } => {
                if let (Some(local), Some(proxy)) =
                    (local_index.get(&local_key), proxy_index.get(&proxy_key))
                {
                    let meta = session_meta_by_id.get(&local.session_id);
                    let fallback_cost = compute_local_request_cost_cached(
                        local,
                        &pricings,
                        &pricing_match_mode,
                        &mut pricing_cache,
                    );
                    let mut fact =
                        MergedRequestFact::merge_proxy_preferred(proxy, local, meta, fallback_cost);
                    fact.coverage_origin = CoverageOrigin::MergedFuzzyMatched;
                    merged.push(fact);
                    fuzzy_consumed_local_keys.insert(local_key);
                    fuzzy_consumed_proxy_keys.insert(proxy_key);
                }
            }
            CodexFuzzyOutcome::SuppressedByFilteredProxy { local_key } => {
                fuzzy_suppressed_local_keys.insert(local_key);
            }
        }
    }

    let mut keys = HashSet::new();
    keys.extend(proxy_index.keys().cloned());
    keys.extend(local_index.keys().cloned());

    for key in keys {
        match (proxy_index.get(&key), local_index.get(&key)) {
            (Some(proxy), Some(local)) => {
                let meta = session_meta_by_id.get(&local.session_id);
                let fallback_cost = compute_local_request_cost_cached(
                    local,
                    &pricings,
                    &pricing_match_mode,
                    &mut pricing_cache,
                );
                merged.push(MergedRequestFact::merge_proxy_preferred(
                    proxy,
                    local,
                    meta,
                    fallback_cost,
                ));
            }
            (Some(proxy), None) => {
                if fuzzy_consumed_proxy_keys.contains(&key) {
                    continue;
                }
                let meta = proxy.session_id.as_deref().and_then(|session_id| {
                    session_meta_by_id.get(&session_meta_lookup_key_for_proxy(
                        &proxy.client_tool,
                        session_id,
                    ))
                });
                merged.push(MergedRequestFact::from_proxy(proxy, meta));
            }
            (None, Some(local)) => {
                if all_proxy_index.contains_key(&key)
                    || fuzzy_consumed_local_keys.contains(&key)
                    || fuzzy_suppressed_local_keys.contains(&key)
                {
                    continue;
                }
                let meta = session_meta_by_id.get(&local.session_id);
                let cost = compute_local_request_cost_cached(
                    local,
                    &pricings,
                    &pricing_match_mode,
                    &mut pricing_cache,
                );
                let fallback_base_url = (local.tool == "codex")
                    .then_some(codex_fallback_base_url.as_deref())
                    .flatten();
                merged.push(MergedRequestFact::from_local(
                    local,
                    meta,
                    cost,
                    fallback_base_url,
                ));
            }
            (None, None) => {}
        }
    }

    let mut reasonix_coverage_records = raw_reasonix_coverage_proxy_records;
    attach_proxy_session_ids(&mut reasonix_coverage_records, &message_to_session);
    let (reasonix_coverage_by_session, blocked_reasonix_sessions) =
        build_reasonix_proxy_coverage_by_session(&reasonix_coverage_records, &session_meta_by_id);
    for meta in session_meta_by_id.values() {
        if let Some(residual) = build_reasonix_telemetry_residual(
            meta,
            reasonix_coverage_by_session.get(&meta.session_id),
            blocked_reasonix_sessions.contains(&meta.session_id),
            &source_filter,
            &currency_settings,
            range_start,
            range_end,
        ) {
            merged.push(residual);
        }
    }

    merged.sort_by_key(|fact| fact.timestamp_ms);
    merged
}

pub(super) fn request_key_for_fact(fact: &MergedRequestFact) -> String {
    if !fact.canonical_request_key.trim().is_empty() {
        return fact.canonical_request_key.clone();
    }
    format!(
        "{}:{}:{}:{}:{}:{}:{}:{}:{}",
        fact.tool,
        fact.session_id,
        fact.timestamp_ms,
        fact.model,
        fact.input_tokens,
        fact.output_tokens,
        fact.cache_create_tokens,
        fact.cache_read_tokens,
        fact.total_tokens
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(tool: &str, message_id: &str, timestamp: i64) -> LocalRequestRecord {
        LocalRequestRecord {
            session_id: format!("{tool}::session-1"),
            tool: tool.to_string(),
            timestamp,
            message_id: message_id.to_string(),
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            model: "test-model".to_string(),
            ..Default::default()
        }
    }

    fn proxy(tool: &str, message_id: &str, timestamp_ms: i64) -> UsageRecord {
        UsageRecord {
            timestamp: timestamp_ms,
            message_id: message_id.to_string(),
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            model: "test-model".to_string(),
            status_code: 200,
            client_tool: tool.to_string(),
            ..Default::default()
        }
    }

    fn input(local_records: Vec<LocalRequestRecord>) -> RealtimeMergeInput {
        RealtimeMergeInput {
            local_records,
            session_meta_by_id: HashMap::new(),
            message_to_session: HashMap::new(),
            raw_proxy_records: Vec::new(),
            raw_unfiltered_proxy_records: None,
            raw_reasonix_coverage_proxy_records: Vec::new(),
            source_filter: SourceFilter::All,
            currency_settings: CurrencySettings::default(),
            range_start: 0,
            range_end: i64::MAX,
            include_errors: true,
            pricings: Vec::new(),
            pricing_match_mode: "fuzzy".to_string(),
            codex_fallback_base_url: None,
        }
    }

    #[test]
    fn exact_match_emits_one_proxy_preferred_fact() {
        let local = local("claude_code", "msg-1", 1_700_000_000);
        let mut input = input(vec![local]);
        input.raw_proxy_records = vec![proxy("claude_code", "msg-1", 1_700_000_000_250)];

        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 1);
        assert_eq!(
            facts[0].coverage_origin,
            CoverageOrigin::MergedProxyPreferred
        );
        let coverage = build_coverage(&facts);
        assert_eq!(coverage.proxy_backed_requests, 1);
        assert_eq!(coverage.merged_overlap_requests, 1);
        assert_eq!(coverage.local_only_requests, 0);
    }

    #[test]
    fn codex_fuzzy_match_emits_one_fact_for_different_message_ids() {
        let local = local("codex", "local-generated-id", 1_700_000_000);
        let mut input = input(vec![local]);
        input.raw_proxy_records = vec![proxy("codex", "provider-response-id", 1_700_000_002_000)];

        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].coverage_origin, CoverageOrigin::MergedFuzzyMatched);
    }

    #[test]
    fn filtered_codex_proxy_suppresses_matching_local_duplicate() {
        let local = local("codex", "local-generated-id", 1_700_000_000);
        let mut input = input(vec![local]);
        input.raw_unfiltered_proxy_records = Some(vec![proxy(
            "codex",
            "provider-response-id",
            1_700_000_002_000,
        )]);

        assert!(merge_realtime_facts(input).is_empty());
    }

    #[test]
    fn codex_local_fact_uses_prepared_fallback_base_url() {
        let local = local("codex", "local-only-id", 1_700_000_000);
        let mut input = input(vec![local]);
        input.codex_fallback_base_url = Some("https://codex.example.test".to_string());

        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 1);
        assert_eq!(
            facts[0].request_base_url.as_deref(),
            Some("https://codex.example.test")
        );
    }

    #[test]
    fn coverage_marks_mixed_sources_as_partial_performance_coverage() {
        let local = MergedRequestFact::from_local(
            &local("claude_code", "local", 1_700_000_000),
            None,
            0.0,
            None,
        );
        let proxy =
            MergedRequestFact::from_proxy(&proxy("claude_code", "proxy", 1_700_000_001_000), None);

        let coverage = build_coverage(&[local, proxy]);

        assert!(coverage.has_partial_performance_coverage);
        assert!(!coverage.has_partial_status_coverage);
    }
}
