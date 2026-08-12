use super::match_support::{
    attach_proxy_session_ids, compute_local_request_cost_cached, request_key_for_local,
    request_key_for_proxy,
};
use super::types::{
    codex_orphan_pools, find_codex_fuzzy_matches, has_partial_coverage,
    session_meta_lookup_key_for_proxy, CodexFuzzyOutcome, CoverageOrigin, MergedCoverage,
    MergedRequestFact,
};
use crate::models::ModelPricingConfig;
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
    fn codex_gateway_record_fuzzy_merges_with_local() {
        // 网关 Codex 记录（client_tool=api_gateway、label=Codex、真实 resp id）与
        // 本地 Codex 扫描记录（合成 id）精确键永不相等：必须经 fuzzy 二次匹配
        // 合并为单条事实，消除网关 + 本地 Codex 双计。
        let local = local("codex", "local-generated-id", 1_700_000_000);
        let mut gateway = proxy("api_gateway", "resp_provider-1", 1_700_000_002_000);
        gateway.ingress_kind = "gateway".to_string();
        gateway.gateway_profile_id = Some("profile-1".to_string());
        gateway.gateway_caller_label = Some("Codex".to_string());

        let mut input = input(vec![local]);
        input.raw_proxy_records = vec![gateway];

        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].coverage_origin, CoverageOrigin::MergedFuzzyMatched);
        assert_eq!(facts[0].tool, "codex");
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

    #[test]
    fn gateway_record_merges_with_same_message_id_local_record() {
        // 回归用例：网关记录（ingress_kind=gateway、存储键 gateway:profile:gw-xxx）与
        // 本地 claude_code 扫描记录共享同一上游响应 message_id → 必须归一化为
        // `claude_code:{message_id}` 键并合并为单条 proxy_preferred fact，token/费用不再双计。
        let local = local("claude_code", "chatcmpl-abc123", 1_700_000_000);
        let mut input = input(vec![local]);
        let mut gateway = proxy("api_gateway", "chatcmpl-abc123", 1_700_000_000_250);
        gateway.ingress_kind = "gateway".to_string();
        gateway.gateway_profile_id = Some("profile-1".to_string());
        gateway.gateway_caller_label = Some("Claude Code".to_string());
        gateway.canonical_request_key = Some("gateway:profile-1:gw-123-0".to_string());
        input.raw_proxy_records = vec![gateway];

        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 1);
        assert_eq!(
            facts[0].coverage_origin,
            CoverageOrigin::MergedProxyPreferred
        );
        assert_eq!(facts[0].tool, "claude_code");
        assert_eq!(
            facts[0].canonical_request_key,
            "claude_code:chatcmpl-abc123"
        );
        let coverage = build_coverage(&facts);
        assert_eq!(coverage.proxy_backed_requests, 1);
        assert_eq!(coverage.merged_overlap_requests, 1);
        assert_eq!(coverage.local_only_requests, 0);
    }

    #[test]
    fn gateway_record_unrecognized_label_stays_isolated() {
        // label 归一化失败（Cursor 不在白名单）→ 网关记录回退存储层键，与本地记录
        // 各自独立成 fact，绝不误合并。
        let local = local("claude_code", "chatcmpl-abc123", 1_700_000_000);
        let mut input = input(vec![local]);
        let mut gateway = proxy("api_gateway", "chatcmpl-abc123", 1_700_000_000_250);
        gateway.ingress_kind = "gateway".to_string();
        gateway.gateway_profile_id = Some("profile-1".to_string());
        gateway.gateway_caller_label = Some("Cursor".to_string());
        gateway.canonical_request_key = Some("gateway:profile-1:gw-123-0".to_string());
        input.raw_proxy_records = vec![gateway];

        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 2);
        assert_eq!(
            facts
                .iter()
                .filter(|f| f.coverage_origin == CoverageOrigin::LocalOnly)
                .count(),
            1
        );
        assert_eq!(
            facts
                .iter()
                .filter(|f| f.coverage_origin == CoverageOrigin::ProxyOnly)
                .count(),
            1
        );
    }

    #[test]
    fn gateway_record_with_fallback_message_id_stays_isolated() {
        // fallback 合成 message_id → 不参与归一化合并，保持 api_gateway 孤立显示。
        let local = local("claude_code", "chatcmpl-abc123", 1_700_000_000);
        let mut input = input(vec![local]);
        let mut gateway = proxy(
            "api_gateway",
            "claude_usage_missing_1700000000_200",
            1_700_000_000_250,
        );
        gateway.ingress_kind = "gateway".to_string();
        gateway.gateway_profile_id = Some("profile-1".to_string());
        gateway.gateway_caller_label = Some("Claude Code".to_string());
        gateway.canonical_request_key = Some("gateway:profile-1:gw-123-0".to_string());
        input.raw_proxy_records = vec![gateway];

        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 2);
        assert_eq!(
            facts
                .iter()
                .filter(|f| f.coverage_origin == CoverageOrigin::ProxyOnly)
                .count(),
            1
        );
        assert_eq!(
            facts
                .iter()
                .filter(|f| f.coverage_origin == CoverageOrigin::LocalOnly)
                .count(),
            1
        );
    }
}
