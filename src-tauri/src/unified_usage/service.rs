use super::types::{CoverageOrigin, MergedCoverage, MergedRequestFact};
use crate::models::{AppSettings, ToolFilter, UsageQueryFilter};
use crate::proxy::ProxyMergeCacheSignature;
use crate::proxy::{ProjectStats, ProjectToolStats, ProxyDatabase, SessionStats, UsageRecord};
use crate::session::{LocalRequestRecord, SessionMeta};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MergeCacheKey {
    start_epoch: i64,
    end_epoch: i64,
    include_errors: bool,
    source_filter: String,
    tool_filter: String,
    pricing_match_mode: String,
    pricing_fingerprint: u64,
    local_signature: crate::local_usage::LocalMergeCacheSignature,
    proxy_signature: Option<ProxyMergeCacheSignature>,
}

#[derive(Debug, Clone)]
struct MergeCacheEntry {
    key: MergeCacheKey,
    facts: Vec<MergedRequestFact>,
    coverage: MergedCoverage,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SessionDerivedCacheKey {
    merge_key: MergeCacheKey,
    limit: i64,
    offset: i64,
}

#[derive(Debug, Clone)]
struct SessionDerivedCacheEntry {
    key: SessionDerivedCacheKey,
    sessions: Vec<SessionStats>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProjectDerivedCacheKey {
    merge_key: MergeCacheKey,
}

#[derive(Debug, Clone)]
struct ProjectDerivedCacheEntry {
    key: ProjectDerivedCacheKey,
    projects: Vec<ProjectStats>,
}

static MERGED_REQUEST_FACTS_CACHE: OnceLock<Mutex<Vec<MergeCacheEntry>>> = OnceLock::new();
static MERGED_SESSIONS_CACHE: OnceLock<Mutex<Vec<SessionDerivedCacheEntry>>> = OnceLock::new();
static MERGED_PROJECTS_CACHE: OnceLock<Mutex<Vec<ProjectDerivedCacheEntry>>> = OnceLock::new();
const MERGED_REQUEST_FACTS_CACHE_CAPACITY: usize = 8;
const MERGED_SESSIONS_CACHE_CAPACITY: usize = 6;
const MERGED_PROJECTS_CACHE_CAPACITY: usize = 6;

fn perf_logging_enabled() -> bool {
    cfg!(debug_assertions) || matches!(std::env::var("USAGEMETER_DEBUG_PERF"), Ok(v) if v == "1")
}

fn perf_log(event: &str, message: impl AsRef<str>) {
    if perf_logging_enabled() {
        eprintln!("[UsageMeter][perf][{event}] {}", message.as_ref());
    }
}

fn merge_cache() -> &'static Mutex<Vec<MergeCacheEntry>> {
    MERGED_REQUEST_FACTS_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn merged_sessions_cache() -> &'static Mutex<Vec<SessionDerivedCacheEntry>> {
    MERGED_SESSIONS_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn merged_projects_cache() -> &'static Mutex<Vec<ProjectDerivedCacheEntry>> {
    MERGED_PROJECTS_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn cache_key_for_source_filter(filter: &crate::models::SourceFilter) -> String {
    match filter {
        crate::models::SourceFilter::All => "all".to_string(),
        crate::models::SourceFilter::Unknown { known_pairs } => {
            format!("unknown:{known_pairs:?}")
        }
        crate::models::SourceFilter::Source {
            api_key_prefixes,
            base_url,
        } => format!("source:{api_key_prefixes:?}:{base_url:?}"),
    }
}

fn cache_key_for_tool_filter(filter: &ToolFilter) -> String {
    match filter {
        ToolFilter::All => "all".to_string(),
        ToolFilter::Tool(tool) => format!("tool:{tool}"),
    }
}

fn fingerprint_pricings(pricings: &[crate::models::ModelPricingConfig]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    for pricing in pricings {
        pricing.model_id.hash(&mut hasher);
        pricing.display_name.hash(&mut hasher);
        pricing.input_price.to_bits().hash(&mut hasher);
        pricing.output_price.to_bits().hash(&mut hasher);
        pricing.cache_read_price.map(f64::to_bits).hash(&mut hasher);
        pricing
            .cache_write_price
            .map(f64::to_bits)
            .hash(&mut hasher);
        pricing.source.hash(&mut hasher);
        pricing.last_updated.hash(&mut hasher);
    }
    hasher.finish()
}

fn lookup_merge_cache(key: &MergeCacheKey) -> Option<(Vec<MergedRequestFact>, MergedCoverage)> {
    let cache = merge_cache();
    let mut guard = cache.lock().unwrap();
    let idx = guard.iter().position(|entry| entry.key == *key)?;
    let entry = guard.remove(idx);
    let result = (entry.facts.clone(), entry.coverage.clone());
    guard.insert(0, entry);
    Some(result)
}

fn store_merge_cache(key: MergeCacheKey, facts: &[MergedRequestFact], coverage: &MergedCoverage) {
    let cache = merge_cache();
    let mut guard = cache.lock().unwrap();
    if let Some(idx) = guard.iter().position(|entry| entry.key == key) {
        guard.remove(idx);
    }
    guard.insert(
        0,
        MergeCacheEntry {
            key,
            facts: facts.to_vec(),
            coverage: coverage.clone(),
        },
    );
    if guard.len() > MERGED_REQUEST_FACTS_CACHE_CAPACITY {
        guard.truncate(MERGED_REQUEST_FACTS_CACHE_CAPACITY);
    }
}

fn lookup_sessions_cache(key: &SessionDerivedCacheKey) -> Option<Vec<SessionStats>> {
    let cache = merged_sessions_cache();
    let mut guard = cache.lock().unwrap();
    let idx = guard.iter().position(|entry| entry.key == *key)?;
    let entry = guard.remove(idx);
    let result = entry.sessions.clone();
    guard.insert(0, entry);
    Some(result)
}

fn store_sessions_cache(key: SessionDerivedCacheKey, sessions: &[SessionStats]) {
    let cache = merged_sessions_cache();
    let mut guard = cache.lock().unwrap();
    if let Some(idx) = guard.iter().position(|entry| entry.key == key) {
        guard.remove(idx);
    }
    guard.insert(
        0,
        SessionDerivedCacheEntry {
            key,
            sessions: sessions.to_vec(),
        },
    );
    if guard.len() > MERGED_SESSIONS_CACHE_CAPACITY {
        guard.truncate(MERGED_SESSIONS_CACHE_CAPACITY);
    }
}

fn lookup_projects_cache(key: &ProjectDerivedCacheKey) -> Option<Vec<ProjectStats>> {
    let cache = merged_projects_cache();
    let mut guard = cache.lock().unwrap();
    let idx = guard.iter().position(|entry| entry.key == *key)?;
    let entry = guard.remove(idx);
    let result = entry.projects.clone();
    guard.insert(0, entry);
    Some(result)
}

fn store_projects_cache(key: ProjectDerivedCacheKey, projects: &[ProjectStats]) {
    let cache = merged_projects_cache();
    let mut guard = cache.lock().unwrap();
    if let Some(idx) = guard.iter().position(|entry| entry.key == key) {
        guard.remove(idx);
    }
    guard.insert(
        0,
        ProjectDerivedCacheEntry {
            key,
            projects: projects.to_vec(),
        },
    );
    if guard.len() > MERGED_PROJECTS_CACHE_CAPACITY {
        guard.truncate(MERGED_PROJECTS_CACHE_CAPACITY);
    }
}

fn request_key_for_local(record: &LocalRequestRecord) -> String {
    // 优先使用持久化的 request_key（v5 schema 之后由 local_usage 写入）；
    // 老数据或 scanner 直读路径没有这个字段时，落回与写入时一致的拼接规则。
    if let Some(key) = record.request_key.as_ref() {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if record.message_id.trim().is_empty() {
        format!(
            "{}:{}:{}:{}:{}:{}:{}:{}:{}",
            record.tool,
            record.session_id,
            record.timestamp,
            record.model,
            record.input_tokens,
            record.output_tokens,
            record.cache_create_tokens,
            record.cache_read_tokens,
            record.total_tokens
        )
    } else {
        format!("{}:{}", record.tool, record.message_id)
    }
}

fn request_key_for_proxy(record: &UsageRecord) -> String {
    if record.message_id.trim().is_empty() {
        format!(
            "{}:{}:{}:{}:{}:{}:{}:{}:{}",
            record.client_tool,
            record.session_id.clone().unwrap_or_default(),
            record.timestamp / 1000,
            record.model,
            record.input_tokens,
            record.output_tokens,
            record.cache_create_tokens,
            record.cache_read_tokens,
            record.total_tokens
        )
    } else {
        format!("{}:{}", record.client_tool, record.message_id)
    }
}

fn local_tool_matches(record: &LocalRequestRecord, tool_filter: &ToolFilter) -> bool {
    match tool_filter {
        ToolFilter::All => true,
        ToolFilter::Tool(tool) if tool.trim().is_empty() => true,
        ToolFilter::Tool(tool) => record.tool == *tool,
    }
}

fn session_meta_matches(meta: &SessionMeta, tool_filter: &ToolFilter) -> bool {
    match tool_filter {
        ToolFilter::All => true,
        ToolFilter::Tool(tool) if tool.trim().is_empty() => true,
        ToolFilter::Tool(tool) => meta.tool == *tool,
    }
}

fn build_local_meta_index(sessions: &[SessionMeta]) -> HashMap<String, SessionMeta> {
    sessions
        .iter()
        .cloned()
        .map(|meta| (meta.session_id.clone(), meta))
        .collect()
}

fn build_message_to_session_index(local_records: &[LocalRequestRecord]) -> HashMap<String, String> {
    let mut message_to_session = HashMap::new();
    for record in local_records {
        if !record.message_id.trim().is_empty() {
            message_to_session.insert(record.message_id.clone(), record.session_id.clone());
        }
    }
    message_to_session
}

fn attach_proxy_session_ids(
    proxy_records: &mut [UsageRecord],
    message_to_session: &HashMap<String, String>,
) {
    for record in proxy_records.iter_mut() {
        let needs_fill = record
            .session_id
            .as_ref()
            .map(|id| id.trim().is_empty())
            .unwrap_or(true);
        if needs_fill {
            if let Some(session_id) = message_to_session.get(&record.message_id) {
                record.session_id = Some(session_id.clone());
            }
        }
    }
}

fn compute_local_request_cost(
    record: &LocalRequestRecord,
    pricings: &[crate::models::ModelPricingConfig],
    match_mode: &str,
) -> f64 {
    crate::models::estimate_session_cost(
        record.input_tokens,
        record.output_tokens,
        record.cache_create_tokens,
        record.cache_read_tokens,
        &record.model,
        pricings,
        match_mode,
    )
}

#[derive(Default)]
struct ProjectAggregate {
    stats: ProjectStats,
    sessions: HashSet<String>,
    tool_sessions: HashMap<String, HashSet<String>>,
}

fn build_local_request_index(
    local_records: &[LocalRequestRecord],
) -> HashMap<String, LocalRequestRecord> {
    let mut map = HashMap::new();
    for record in local_records {
        map.insert(request_key_for_local(record), record.clone());
    }
    map
}

fn build_proxy_request_index(proxy_records: &[UsageRecord]) -> HashMap<String, UsageRecord> {
    let mut map = HashMap::new();
    for record in proxy_records {
        map.insert(request_key_for_proxy(record), record.clone());
    }
    map
}

async fn fetch_proxy_records(
    proxy_db: &ProxyDatabase,
    usage_filter: &UsageQueryFilter,
    start_epoch: Option<i64>,
    end_epoch: Option<i64>,
) -> Result<Vec<UsageRecord>, String> {
    let start_ms = start_epoch.unwrap_or(0).saturating_mul(1000);
    let end_ms = end_epoch.unwrap_or(i64::MAX / 1000).saturating_mul(1000);

    proxy_db
        .get_records_between_with_source(start_ms, end_ms, true, usage_filter)
        .await
}

fn normalize_range_bounds(start_epoch: Option<i64>, end_epoch: Option<i64>) -> (i64, i64) {
    let start = start_epoch.unwrap_or(0);
    let end = end_epoch.unwrap_or(i64::MAX);
    (start.max(0), end.max(start.max(0)))
}

struct MergeCacheKeyParts<'a> {
    settings: &'a AppSettings,
    range_start: i64,
    range_end: i64,
    include_errors: bool,
    source_filter: &'a crate::models::SourceFilter,
    tool_filter: &'a ToolFilter,
    local_signature: crate::local_usage::LocalMergeCacheSignature,
    proxy_signature: Option<ProxyMergeCacheSignature>,
    pricings: &'a [crate::models::ModelPricingConfig],
}

fn build_merge_cache_key(parts: MergeCacheKeyParts<'_>) -> MergeCacheKey {
    MergeCacheKey {
        start_epoch: parts.range_start,
        end_epoch: parts.range_end,
        include_errors: parts.include_errors,
        source_filter: cache_key_for_source_filter(parts.source_filter),
        tool_filter: cache_key_for_tool_filter(parts.tool_filter),
        pricing_match_mode: parts.settings.model_pricing.match_mode.clone(),
        pricing_fingerprint: fingerprint_pricings(parts.pricings),
        local_signature: parts.local_signature,
        proxy_signature: parts.proxy_signature,
    }
}

pub(crate) fn build_coverage(facts: &[MergedRequestFact]) -> MergedCoverage {
    let mut coverage = MergedCoverage::default();

    for fact in facts {
        match fact.coverage_origin {
            CoverageOrigin::ProxyOnly => coverage.proxy_backed_requests += 1,
            CoverageOrigin::LocalOnly => coverage.local_only_requests += 1,
            CoverageOrigin::MergedProxyPreferred => {
                coverage.proxy_backed_requests += 1;
                coverage.merged_overlap_requests += 1;
            }
        }
    }

    coverage.has_partial_status_coverage =
        coverage.proxy_backed_requests > 0 && coverage.local_only_requests > 0;
    coverage.has_partial_performance_coverage =
        coverage.proxy_backed_requests > 0 && coverage.local_only_requests > 0;
    coverage
}

pub async fn get_merged_request_facts(
    settings: &AppSettings,
    start_epoch: Option<i64>,
    end_epoch: Option<i64>,
    include_errors: bool,
) -> Result<(Vec<MergedRequestFact>, MergedCoverage), String> {
    let overall_started_at = Instant::now();
    let (range_start, range_end) = normalize_range_bounds(start_epoch, end_epoch);
    let tool_filter = settings.client_tools.build_filter();
    let usage_filter = UsageQueryFilter {
        source: settings.source_aware.build_filter(),
        tool: settings.client_tools.build_filter(),
    };
    // source_aware 启用过滤时，本地补全也要感知 source：
    // - 若一条 local 请求对应的 proxy 记录在「未过滤的代理集合」里能找到，
    //   说明它属于某个具体 source（可能不是当前 filter 选中的那个）——
    //   必须排除，避免「只看 source A」时混入 source B 的本地请求；
    // - 若在「未过滤代理集合」里也找不到，说明 proxy 真的漏掉了这条本地请求——
    //   归为 local-only 收录，source_label = None（未识别来源桶）。
    //
    // 这条无 source 过滤的副查询只在 source filter != All 时才需要；
    // All 模式下 unfiltered_proxy_records == filtered proxy_records，省一次 IO。
    let needs_unfiltered_proxy_lookup =
        !matches!(usage_filter.source, crate::models::SourceFilter::All);
    let unfiltered_usage_filter = if needs_unfiltered_proxy_lookup {
        Some(UsageQueryFilter {
            source: crate::models::SourceFilter::All,
            tool: settings.client_tools.build_filter(),
        })
    } else {
        None
    };
    let mut pricings = settings.model_pricing.pricings.clone();
    if let Ok(db) = crate::proxy::ProxyDatabase::new() {
        if let Ok(db_pricings) = db.get_all_model_pricings() {
            pricings.extend(db_pricings);
        }
    }
    let pricing_match_mode = settings.model_pricing.match_mode.clone();

    let local_db = crate::local_usage::ensure_local_usage_synced()?;
    let local_signature = local_db.get_merge_cache_signature()?;
    let proxy_signature = ProxyDatabase::get_global()
        .map(|db| db.get_merge_cache_signature())
        .transpose()?;
    let cache_key = build_merge_cache_key(MergeCacheKeyParts {
        settings,
        range_start,
        range_end,
        include_errors,
        source_filter: &usage_filter.source,
        tool_filter: &tool_filter,
        local_signature,
        proxy_signature,
        pricings: &pricings,
    });
    if let Some((facts, coverage)) = lookup_merge_cache(&cache_key) {
        perf_log(
            "merge_cache_hit",
            format!(
                "range={range_start}..{range_end} tool={} source={} include_errors={} facts={} elapsed_ms={}",
                cache_key.tool_filter,
                cache_key.source_filter,
                include_errors,
                facts.len(),
                overall_started_at.elapsed().as_millis(),
            ),
        );
        return Ok((facts, coverage));
    }
    perf_log(
        "merge_cache_miss",
        format!(
            "range={range_start}..{range_end} tool={} source={} include_errors={}",
            cache_key.tool_filter, cache_key.source_filter, include_errors,
        ),
    );

    let query_started_at = Instant::now();
    let mut local_sessions_all = local_db.get_all_sessions(&tool_filter)?;
    local_sessions_all.extend(local_db.get_remote_sessions(&tool_filter)?);
    let local_sessions: Vec<SessionMeta> = local_sessions_all
        .into_iter()
        .filter(|meta| session_meta_matches(meta, &tool_filter))
        .collect();
    let mut local_records =
        local_db.get_request_records_in_range(range_start, range_end, &tool_filter)?;
    let local_request_keys: HashSet<String> =
        local_records.iter().map(request_key_for_local).collect();
    let mut remote_records =
        local_db.get_remote_request_records_in_range(range_start, range_end, &tool_filter)?;
    remote_records.retain(|record| !local_request_keys.contains(&request_key_for_local(record)));
    local_records.extend(remote_records);
    local_records.retain(|record| local_tool_matches(record, &tool_filter));
    let mut seen_local_keys = HashSet::new();
    local_records.retain(|record| seen_local_keys.insert(request_key_for_local(record)));
    let session_meta_by_id = build_local_meta_index(&local_sessions);
    let message_to_session = build_message_to_session_index(&local_records);

    let (all_proxy_records, proxy_records) = if let Some(proxy_db) = ProxyDatabase::get_global() {
        let mut records =
            fetch_proxy_records(proxy_db.as_ref(), &usage_filter, start_epoch, end_epoch).await?;
        attach_proxy_session_ids(&mut records, &message_to_session);
        let visible_records: Vec<UsageRecord> = records
            .iter()
            .filter(|record| include_errors || (200..300).contains(&record.status_code))
            .cloned()
            .collect();
        // 当 source filter ≠ All 时，重新读一次无 source 过滤的代理集合作为 all_proxy_index。
        // 这条副查询不能省——否则当用户「只看 source A」时，被 source B 命中的 proxy 记录
        // 不会出现在 records 里，本地补全分支会误把它们当成「真本地补全」收录进来。
        let all_records: Vec<UsageRecord> = if let Some(filter) = unfiltered_usage_filter.as_ref() {
            let mut unfiltered =
                fetch_proxy_records(proxy_db.as_ref(), filter, start_epoch, end_epoch).await?;
            attach_proxy_session_ids(&mut unfiltered, &message_to_session);
            unfiltered
        } else {
            records
        };
        (all_records, visible_records)
    } else {
        (Vec::new(), Vec::new())
    };
    let query_elapsed_ms = query_started_at.elapsed().as_millis();

    let merge_started_at = Instant::now();
    let all_proxy_index = build_proxy_request_index(&all_proxy_records);
    let proxy_index = build_proxy_request_index(&proxy_records);
    let local_index = build_local_request_index(&local_records);

    let mut keys = HashSet::new();
    keys.extend(proxy_index.keys().cloned());
    keys.extend(local_index.keys().cloned());

    let mut merged = Vec::new();
    for key in keys {
        match (proxy_index.get(&key), local_index.get(&key)) {
            (Some(proxy), Some(local)) => {
                let meta = session_meta_by_id.get(&local.session_id);
                let fallback_cost =
                    compute_local_request_cost(local, &pricings, &pricing_match_mode);
                merged.push(MergedRequestFact::merge_proxy_preferred(
                    proxy,
                    local,
                    meta,
                    fallback_cost,
                ));
            }
            (Some(proxy), None) => {
                let meta = proxy
                    .session_id
                    .as_ref()
                    .and_then(|session_id| session_meta_by_id.get(session_id));
                merged.push(MergedRequestFact::from_proxy(proxy, meta));
            }
            (None, Some(local)) => {
                if all_proxy_index.contains_key(&key) {
                    continue;
                }
                let meta = session_meta_by_id.get(&local.session_id);
                let cost = compute_local_request_cost(local, &pricings, &pricing_match_mode);
                merged.push(MergedRequestFact::from_local(local, meta, cost));
            }
            (None, None) => {}
        }
    }

    merged.sort_by_key(|fact| fact.timestamp_ms);
    let coverage = build_coverage(&merged);
    store_merge_cache(cache_key, &merged, &coverage);
    perf_log(
        "merge_cache_store",
        format!(
            "range={range_start}..{range_end} local_records={} proxy_records={} all_proxy_records={} facts={} query_ms={} merge_ms={} total_ms={}",
            local_index.len(),
            proxy_index.len(),
            all_proxy_index.len(),
            merged.len(),
            query_elapsed_ms,
            merge_started_at.elapsed().as_millis(),
            overall_started_at.elapsed().as_millis(),
        ),
    );
    Ok((merged, coverage))
}

pub async fn get_merged_sessions(
    settings: &AppSettings,
    limit: i64,
    offset: i64,
) -> Result<Vec<SessionStats>, String> {
    let started_at = Instant::now();
    let include_errors = settings.proxy.include_error_requests;
    let tool_filter = settings.client_tools.build_filter();
    let source_filter = settings.source_aware.build_filter();
    let mut pricings = settings.model_pricing.pricings.clone();
    if let Ok(db) = crate::proxy::ProxyDatabase::new() {
        if let Ok(db_pricings) = db.get_all_model_pricings() {
            pricings.extend(db_pricings);
        }
    }
    let local_db = crate::local_usage::ensure_local_usage_synced()?;
    let local_signature = local_db.get_merge_cache_signature()?;
    let proxy_signature = ProxyDatabase::get_global()
        .map(|db| db.get_merge_cache_signature())
        .transpose()?;
    let merge_key = build_merge_cache_key(MergeCacheKeyParts {
        settings,
        range_start: 0,
        range_end: i64::MAX,
        include_errors,
        source_filter: &source_filter,
        tool_filter: &tool_filter,
        local_signature,
        proxy_signature,
        pricings: &pricings,
    });
    let session_cache_key = SessionDerivedCacheKey {
        merge_key,
        limit,
        offset,
    };
    if let Some(sessions) = lookup_sessions_cache(&session_cache_key) {
        perf_log(
            "merged_sessions_cache_hit",
            format!(
                "tool={} returned={} elapsed_ms={}",
                cache_key_for_tool_filter(&tool_filter),
                sessions.len(),
                started_at.elapsed().as_millis(),
            ),
        );
        return Ok(sessions);
    }

    let (facts, _) = get_merged_request_facts(settings, None, None, include_errors).await?;
    let facts_count = facts.len();
    let mut local_sessions = local_db.get_all_sessions(&tool_filter)?;
    local_sessions.extend(local_db.get_remote_sessions(&tool_filter)?);
    let meta_by_id: HashMap<String, SessionMeta> = local_sessions
        .into_iter()
        .map(|meta| (meta.session_id.clone(), meta))
        .collect();

    let mut by_session: HashMap<String, Vec<MergedRequestFact>> = HashMap::new();
    for fact in facts {
        if fact.session_id.trim().is_empty() {
            continue;
        }
        by_session
            .entry(fact.session_id.clone())
            .or_default()
            .push(fact);
    }

    let mut result = Vec::new();
    for (session_id, session_facts) in by_session {
        let meta = meta_by_id.get(&session_id);
        let mut models = BTreeSet::new();
        let mut total_input_tokens = 0_u64;
        let mut total_output_tokens = 0_u64;
        let mut total_cache_create_tokens = 0_u64;
        let mut total_cache_read_tokens = 0_u64;
        let mut estimated_cost = 0.0_f64;
        let mut first_request_time = i64::MAX;
        let mut last_request_time = 0_i64;
        let mut total_duration_ms = 0_u64;
        let mut rate_sum = 0.0_f64;
        let mut rate_count = 0_u64;
        let mut ttft_sum = 0.0_f64;
        let mut ttft_count = 0_u64;
        let mut success_requests = 0_u64;
        let mut error_requests = 0_u64;
        let mut has_partial_status_coverage = false;

        for fact in &session_facts {
            if !fact.model.trim().is_empty() {
                models.insert(fact.model.clone());
            }
            total_input_tokens += fact.input_tokens;
            total_output_tokens += fact.output_tokens;
            total_cache_create_tokens += fact.cache_create_tokens;
            total_cache_read_tokens += fact.cache_read_tokens;
            estimated_cost += fact.estimated_cost;
            first_request_time = first_request_time.min(fact.timestamp_sec);
            last_request_time = last_request_time.max(fact.timestamp_sec);

            if matches!(fact.coverage_origin, CoverageOrigin::LocalOnly) {
                has_partial_status_coverage = true;
            }

            if let Some(duration_ms) = fact.duration_ms {
                total_duration_ms += duration_ms;
            }
            if let Some(rate) = fact.output_tokens_per_second {
                if rate > 0.0 {
                    rate_sum += rate;
                    rate_count += 1;
                }
            }
            if let Some(ttft_ms) = fact.ttft_ms {
                if ttft_ms > 0 {
                    ttft_sum += ttft_ms as f64;
                    ttft_count += 1;
                }
            }
            if let Some(status_code) = fact.status_code {
                if status_code < 400 {
                    success_requests += 1;
                } else {
                    error_requests += 1;
                }
            }
        }

        result.push(SessionStats {
            session_id: session_id.clone(),
            tool: meta.map(|m| m.tool.clone()).unwrap_or_else(|| {
                settings
                    .client_tools
                    .active_tool_filter
                    .clone()
                    .unwrap_or_default()
            }),
            total_requests: session_facts.len() as u64,
            total_input_tokens,
            total_output_tokens,
            total_cache_create_tokens,
            total_cache_read_tokens,
            total_duration_ms,
            avg_output_tokens_per_second: if rate_count > 0 {
                rate_sum / rate_count as f64
            } else {
                0.0
            },
            first_request_time: if first_request_time == i64::MAX {
                0
            } else {
                first_request_time
            },
            last_request_time,
            models: models.into_iter().collect(),
            avg_ttft_ms: if ttft_count > 0 {
                ttft_sum / ttft_count as f64
            } else {
                0.0
            },
            success_requests: if has_partial_status_coverage {
                0
            } else {
                success_requests
            },
            error_requests: if has_partial_status_coverage {
                0
            } else {
                error_requests
            },
            estimated_cost,
            is_cost_estimated: true,
            cwd: meta.and_then(|m| m.cwd.clone()),
            project_name: meta.and_then(|m| m.project_name.clone()),
            topic: meta.and_then(|m| m.topic.clone()),
            last_prompt: meta.and_then(|m| m.last_prompt.clone()),
            session_name: meta.and_then(|m| m.session_name.clone()),
        });
    }

    result.sort_by_key(|session| std::cmp::Reverse(session.last_request_time));
    let result: Vec<SessionStats> = result
        .into_iter()
        .skip(offset.max(0) as usize)
        .take(limit.max(0) as usize)
        .collect();
    perf_log(
        "merged_sessions",
        format!(
            "tool={} facts={} returned={} elapsed_ms={}",
            cache_key_for_tool_filter(&tool_filter),
            facts_count,
            result.len(),
            started_at.elapsed().as_millis(),
        ),
    );
    store_sessions_cache(session_cache_key, &result);
    Ok(result)
}

pub async fn get_merged_session_detail(
    settings: &AppSettings,
    session_id: &str,
) -> Result<Option<SessionStats>, String> {
    let sessions = get_merged_sessions(settings, i64::MAX / 4, 0).await?;
    Ok(sessions
        .into_iter()
        .find(|session| session.session_id == session_id))
}

pub async fn get_merged_project_stats(settings: &AppSettings) -> Result<Vec<ProjectStats>, String> {
    let started_at = Instant::now();
    let include_errors = settings.proxy.include_error_requests;
    let tool_filter = settings.client_tools.build_filter();
    let source_filter = settings.source_aware.build_filter();
    let mut pricings = settings.model_pricing.pricings.clone();
    if let Ok(db) = crate::proxy::ProxyDatabase::new() {
        if let Ok(db_pricings) = db.get_all_model_pricings() {
            pricings.extend(db_pricings);
        }
    }
    let local_db = crate::local_usage::ensure_local_usage_synced()?;
    let local_signature = local_db.get_merge_cache_signature()?;
    let proxy_signature = ProxyDatabase::get_global()
        .map(|db| db.get_merge_cache_signature())
        .transpose()?;
    let project_cache_key = ProjectDerivedCacheKey {
        merge_key: build_merge_cache_key(MergeCacheKeyParts {
            settings,
            range_start: 0,
            range_end: i64::MAX,
            include_errors,
            source_filter: &source_filter,
            tool_filter: &tool_filter,
            local_signature,
            proxy_signature,
            pricings: &pricings,
        }),
    };
    if let Some(projects) = lookup_projects_cache(&project_cache_key) {
        perf_log(
            "merged_projects_cache_hit",
            format!(
                "tool={} projects={} elapsed_ms={}",
                cache_key_for_tool_filter(&tool_filter),
                projects.len(),
                started_at.elapsed().as_millis(),
            ),
        );
        return Ok(projects);
    }

    let (facts, _) = get_merged_request_facts(settings, None, None, include_errors).await?;
    let facts_count = facts.len();
    let mut map: HashMap<String, ProjectAggregate> = HashMap::new();

    for fact in facts {
        let project_name = fact
            .project_name
            .clone()
            .unwrap_or_else(|| "未命名项目".to_string());
        let project_path = fact.project_path.clone();
        let aggregate_key = project_path
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| project_name.clone());
        let entry = map
            .entry(aggregate_key)
            .or_insert_with(|| ProjectAggregate {
                stats: ProjectStats {
                    name: project_name.clone(),
                    project_path: project_path.clone(),
                    ..Default::default()
                },
                ..Default::default()
            });

        if entry.stats.project_path.is_none() {
            entry.stats.project_path = project_path.clone();
        }

        entry.stats.total_input_tokens += fact.input_tokens;
        entry.stats.total_output_tokens += fact.output_tokens;
        entry.stats.total_cache_create_tokens += fact.cache_create_tokens;
        entry.stats.total_cache_read_tokens += fact.cache_read_tokens;
        entry.stats.total_cost += fact.estimated_cost;
        entry.stats.request_count += 1;
        entry.stats.last_active = entry.stats.last_active.max(fact.timestamp_sec);
        if !fact.session_id.trim().is_empty() {
            entry.sessions.insert(fact.session_id.clone());
            entry
                .tool_sessions
                .entry(fact.tool.clone())
                .or_default()
                .insert(fact.session_id.clone());
        } else {
            entry.tool_sessions.entry(fact.tool.clone()).or_default();
        }

        let tool_stats = entry
            .stats
            .tool_breakdown
            .iter_mut()
            .find(|stats| stats.tool == fact.tool);
        let tool_stats = match tool_stats {
            Some(stats) => stats,
            None => {
                entry.stats.tool_breakdown.push(ProjectToolStats {
                    tool: fact.tool.clone(),
                    ..Default::default()
                });
                entry.stats.tool_breakdown.last_mut().unwrap()
            }
        };
        tool_stats.total_input_tokens += fact.input_tokens;
        tool_stats.total_output_tokens += fact.output_tokens;
        tool_stats.total_cache_create_tokens += fact.cache_create_tokens;
        tool_stats.total_cache_read_tokens += fact.cache_read_tokens;
        tool_stats.total_cost += fact.estimated_cost;
        tool_stats.request_count += 1;
        tool_stats.last_active = tool_stats.last_active.max(fact.timestamp_sec);
    }

    let mut projects: Vec<ProjectStats> = map
        .into_values()
        .map(|mut aggregate| {
            aggregate.stats.session_count = aggregate.sessions.len() as u64;
            for tool_stats in &mut aggregate.stats.tool_breakdown {
                tool_stats.session_count = aggregate
                    .tool_sessions
                    .get(&tool_stats.tool)
                    .map(|sessions| sessions.len() as u64)
                    .unwrap_or(0);
            }
            aggregate
                .stats
                .tool_breakdown
                .sort_by_key(|tool| std::cmp::Reverse(tool.last_active));
            aggregate.stats
        })
        .collect();
    projects.sort_by_key(|project| std::cmp::Reverse(project.last_active));
    perf_log(
        "merged_projects",
        format!(
            "tool={} facts={} projects={} elapsed_ms={}",
            cache_key_for_tool_filter(&tool_filter),
            facts_count,
            projects.len(),
            started_at.elapsed().as_millis(),
        ),
    );
    store_projects_cache(project_cache_key, &projects);
    Ok(projects)
}
