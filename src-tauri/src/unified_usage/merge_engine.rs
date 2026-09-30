use super::match_support::{
    attach_proxy_session_ids, compute_local_request_cost_cached, request_key_for_local,
    request_key_for_proxy,
};
use super::types::{
    adapter_orphan_pools, codex_orphan_pools, find_adapter_fuzzy_matches, find_codex_fuzzy_matches,
    has_partial_coverage, session_meta_lookup_key_for_proxy, AdapterFuzzyOutcome,
    CodexFuzzyOutcome, CoverageOrigin, MergedCoverage, MergedRequestFact,
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
}

pub(crate) fn build_coverage(facts: &[MergedRequestFact]) -> MergedCoverage {
    let mut coverage = MergedCoverage::default();

    for fact in facts {
        if !fact.is_accounting_primary() {
            continue;
        }
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
    let mut grouped: HashMap<String, Vec<UsageRecord>> = HashMap::new();
    for record in proxy_records {
        grouped
            .entry(request_key_for_proxy(record))
            .or_default()
            .push(record.clone());
    }

    let mut index = HashMap::new();
    for (base_key, records) in grouped {
        if records.len() == 1 {
            let record = records.into_iter().next().expect("one grouped record");
            index.insert(base_key, record);
            continue;
        }

        // A provider message id is only unique within one observation stream. Keep
        // colliding Gateway/direct observations visible instead of letting HashMap
        // insertion silently drop all but the last record.
        for (ordinal, mut record) in records.into_iter().enumerate() {
            let observation_id = record
                .storage_dedupe_key
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .or(record.gateway_request_id.as_deref())
                .unwrap_or(record.message_id.as_str());
            let key = format!("{base_key}#proxy:{observation_id}:{ordinal}");
            record.canonical_request_key = Some(key.clone());
            index.insert(key, record);
        }
    }
    index
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
    let (adapter_local_orphans, adapter_proxy_orphans_visible, adapter_proxy_orphans_all_extra) =
        adapter_orphan_pools(&local_index, &proxy_index, &all_proxy_index);
    let adapter_fuzzy_outcomes = find_adapter_fuzzy_matches(
        &adapter_local_orphans,
        &adapter_proxy_orphans_visible,
        &adapter_proxy_orphans_all_extra,
    );

    let mut fuzzy_consumed_local_keys = HashSet::new();
    let mut fuzzy_consumed_proxy_keys = HashSet::new();
    let mut fuzzy_suppressed_local_keys = HashSet::new();
    let mut ambiguous_proxy_keys = HashSet::new();
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
                    fact.reconciliation = super::types::ReconciliationMetadata::fuzzy(
                        proxy,
                        local_key.clone(),
                        "codex_token_fingerprint",
                    );
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

    for outcome in adapter_fuzzy_outcomes {
        match outcome {
            AdapterFuzzyOutcome::MatchedVisible {
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
                    fact.reconciliation = super::types::ReconciliationMetadata::fuzzy(
                        proxy,
                        local_key.clone(),
                        "adapter_token_fingerprint",
                    );
                    merged.push(fact);
                    fuzzy_consumed_local_keys.insert(local_key);
                    fuzzy_consumed_proxy_keys.insert(proxy_key);
                }
            }
            AdapterFuzzyOutcome::SuppressedByAmbiguous {
                local_key,
                proxy_keys,
            } => {
                ambiguous_proxy_keys.extend(proxy_keys);
                fuzzy_suppressed_local_keys.insert(local_key);
            }
            AdapterFuzzyOutcome::SuppressedByFilteredProxy { local_key } => {
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
                if fuzzy_consumed_local_keys.contains(&key)
                    || fuzzy_consumed_proxy_keys.contains(&key)
                {
                    continue;
                }
                if super::types::proxy_reconciliation_tool(proxy) != Some(local.tool.as_str())
                    || !super::types::can_exact_reconcile(local, proxy)
                {
                    let proxy_meta = proxy.session_id.as_deref().and_then(|session_id| {
                        session_meta_by_id.get(&session_meta_lookup_key_for_proxy(
                            &proxy.client_tool,
                            session_id,
                        ))
                    });
                    merged.push(MergedRequestFact::from_proxy(proxy, proxy_meta));
                    let local_cost = compute_local_request_cost_cached(
                        local,
                        &pricings,
                        &pricing_match_mode,
                        &mut pricing_cache,
                    );
                    let local_meta = session_meta_by_id.get(&local.session_id);
                    merged.push(MergedRequestFact::from_local(local, local_meta, local_cost));
                    continue;
                }
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
                let mut fact = MergedRequestFact::from_proxy(proxy, meta);
                if ambiguous_proxy_keys.contains(&key) {
                    fact.reconciliation =
                        super::types::ReconciliationMetadata::ambiguous_proxy(proxy);
                }
                merged.push(fact);
            }
            (None, Some(local)) => {
                let has_supported_filtered_proxy = all_proxy_index.get(&key).is_some_and(|proxy| {
                    super::types::proxy_reconciliation_tool(proxy) == Some(local.tool.as_str())
                });
                if has_supported_filtered_proxy
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
                merged.push(MergedRequestFact::from_local(local, meta, cost));
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
    fn adapter_fuzzy_match_merges_claude_records_when_ids_differ() {
        let local = local("claude_code", "local-message-id", 1_700_000_000);
        let mut input = input(vec![local]);
        input.raw_proxy_records = vec![proxy(
            "claude_code",
            "provider-message-id",
            1_700_000_000_250,
        )];

        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].coverage_origin, CoverageOrigin::MergedFuzzyMatched);
        assert_eq!(facts[0].tool, "claude_code");
        assert_eq!(
            facts[0].reconciliation.status,
            super::super::types::ReconciliationStatus::Adapter
        );
    }

    #[test]
    fn adapter_fuzzy_match_merges_gemini_records_when_ids_differ() {
        let local = local("gemini", "local-message-id", 1_700_000_000);
        let mut input = input(vec![local]);
        input.raw_proxy_records = vec![proxy("gemini", "provider-response-id", 1_700_000_000_250)];

        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].coverage_origin, CoverageOrigin::MergedFuzzyMatched);
        assert_eq!(facts[0].tool, "gemini");
    }

    #[test]
    fn adapter_fuzzy_match_merges_opencode_records_when_ids_differ() {
        let local = local("opencode", "local-message-id", 1_700_000_000);
        let mut input = input(vec![local]);
        input.raw_proxy_records = vec![proxy("opencode", "provider-message-id", 1_700_000_000_250)];

        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].coverage_origin, CoverageOrigin::MergedFuzzyMatched);
        assert_eq!(facts[0].tool, "opencode");
    }

    #[test]
    fn gateway_adapter_fuzzy_match_merges_gemini_records() {
        let local = local("gemini", "local-message-id", 1_700_000_000);
        let mut gateway = proxy("api_gateway", "provider-response-id", 1_700_000_000_250);
        gateway.ingress_kind = "gateway".to_string();
        gateway.gateway_profile_id = Some("profile-1".to_string());
        gateway.gateway_caller_label = Some("Gemini CLI".to_string());
        gateway.canonical_request_key = Some("gateway:profile-1:gw-123-0".to_string());

        let mut input = input(vec![local]);
        input.raw_proxy_records = vec![gateway];
        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].coverage_origin, CoverageOrigin::MergedFuzzyMatched);
        assert_eq!(facts[0].tool, "gemini");
    }

    #[test]
    fn adapter_fuzzy_match_suppresses_ambiguous_local_duplicate() {
        let local = local("claude_code", "local-message-id", 1_700_000_000);
        let mut first = proxy("claude_code", "provider-message-1", 1_700_000_000_250);
        let mut second = proxy("claude_code", "provider-message-2", 1_700_000_001_250);
        first.model = "test-model".to_string();
        second.model = "test-model".to_string();

        let mut input = input(vec![local]);
        input.raw_proxy_records = vec![first, second];
        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 2);
        assert!(facts
            .iter()
            .all(|fact| fact.coverage_origin == CoverageOrigin::ProxyOnly));
        assert!(facts.iter().all(|fact| {
            fact.reconciliation.status == super::super::types::ReconciliationStatus::Ambiguous
        }));
    }

    #[test]
    fn filtered_adapter_proxy_suppresses_matching_local_duplicate() {
        let local = local("gemini", "local-message-id", 1_700_000_000);
        let mut input = input(vec![local]);
        input.raw_unfiltered_proxy_records = Some(vec![proxy(
            "gemini",
            "provider-response-id",
            1_700_000_000_250,
        )]);

        assert!(merge_realtime_facts(input).is_empty());
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
    fn codex_local_fact_has_no_request_base_url() {
        let local = local("codex", "local-only-id", 1_700_000_000);
        let facts = merge_realtime_facts(input(vec![local]));

        assert_eq!(facts.len(), 1);
        assert!(facts[0].request_base_url.is_none());
    }

    #[test]
    fn coverage_marks_mixed_sources_as_partial_performance_coverage() {
        let local =
            MergedRequestFact::from_local(&local("claude_code", "local", 1_700_000_000), None, 0.0);
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
    fn gateway_unsupported_local_tool_label_stays_isolated() {
        let local = local("qoder_cli", "chatcmpl-abc123", 1_700_000_000);
        let mut input = input(vec![local]);
        let mut gateway = proxy("api_gateway", "chatcmpl-abc123", 1_700_000_000_250);
        gateway.ingress_kind = "gateway".to_string();
        gateway.gateway_profile_id = Some("profile-1".to_string());
        gateway.gateway_caller_label = Some("Qoder CLI".to_string());
        gateway.canonical_request_key = Some("gateway:profile-1:gw-123-0".to_string());
        input.raw_proxy_records = vec![gateway];

        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 2);
        assert!(facts.iter().any(|fact| fact.tool == "qoder_cli"));
        assert!(facts.iter().any(|fact| fact.tool == "api_gateway"));
    }

    #[test]
    fn unsupported_direct_proxy_key_collision_stays_isolated() {
        let local = local("qoder_cli", "same-id", 1_700_000_000);
        let mut input = input(vec![local]);
        input.raw_proxy_records = vec![proxy("qoder_cli", "same-id", 1_700_000_000_250)];

        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 2);
        assert!(facts.iter().any(|fact| {
            fact.tool == "qoder_cli" && fact.coverage_origin == CoverageOrigin::LocalOnly
        }));
        assert!(facts.iter().any(|fact| {
            fact.tool == "qoder_cli" && fact.coverage_origin == CoverageOrigin::ProxyOnly
        }));
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

    #[test]
    fn fallback_ids_do_not_enter_exact_merge_when_fingerprint_does_not_match() {
        let mut local = local("claude_code", "", 1_700_000_000);
        local.request_key = None;
        let mut proxy = proxy("claude_code", "provider-fallback", 1_700_000_000_250);
        proxy.input_tokens = 999;
        // Force both observations onto the same composite key. The key collision
        // must still not be treated as proof of identity when the local side has
        // no stable observation id.
        proxy.canonical_request_key = Some(request_key_for_local(&local));

        let mut input = input(vec![local]);
        input.raw_proxy_records = vec![proxy];
        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 2);
        assert!(facts
            .iter()
            .any(|fact| fact.coverage_origin == CoverageOrigin::LocalOnly));
        assert!(facts
            .iter()
            .any(|fact| fact.coverage_origin == CoverageOrigin::ProxyOnly));
    }

    #[test]
    fn duplicate_proxy_observations_with_same_message_id_are_not_dropped() {
        let mut first = proxy("claude_code", "same-provider-id", 1_700_000_000_250);
        first.storage_dedupe_key = Some("direct:first".to_string());
        let mut second = first.clone();
        second.storage_dedupe_key = Some("direct:second".to_string());
        second.timestamp += 1_000;

        let mut input = input(Vec::new());
        input.raw_proxy_records = vec![first, second];
        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 2);
        assert!(facts
            .iter()
            .all(|fact| fact.coverage_origin == CoverageOrigin::ProxyOnly));
    }

    #[test]
    fn composite_local_request_keys_do_not_prove_exact_identity() {
        let mut local = local("opencode", "same-id", 1_700_000_000);
        local.request_key = Some("opencode:session-1|same-id".to_string());
        let mut proxy = proxy("opencode", "provider-id", 1_700_000_000_250);
        proxy.input_tokens = 999;
        proxy.canonical_request_key = local.request_key.clone();

        let mut input = input(vec![local]);
        input.raw_proxy_records = vec![proxy];
        let facts = merge_realtime_facts(input);

        assert_eq!(facts.len(), 2);
        assert!(facts
            .iter()
            .any(|fact| fact.coverage_origin == CoverageOrigin::LocalOnly));
        assert!(facts
            .iter()
            .any(|fact| fact.coverage_origin == CoverageOrigin::ProxyOnly));
    }
}
