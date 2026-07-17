use super::types::{
    canonical_request_key_for_local, canonical_request_key_for_proxy, codex_orphan_pools,
    find_codex_fuzzy_matches, has_partial_coverage, session_meta_lookup_key_for_proxy,
    CodexFuzzyOutcome, CoverageOrigin, MergedCoverage, MergedRequestFact,
};
use crate::models::{AppSettings, ToolFilter, UsageQueryFilter};
use crate::proxy::ProxyMergeCacheSignature;
use crate::proxy::{
    CodexConfigManager, CodexSourceRegistry, ProjectStats, ProjectToolStats, ProxyDatabase,
    SessionStats, UsageRecord,
};
use crate::session::{wsl_distro_from_path, LocalRequestRecord, SessionMeta};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MergeCacheKey {
    start_epoch: i64,
    end_epoch: i64,
    day_boundary_mode: String,
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
    /// Arc 包裹：命中时只克隆指针，避免大范围下几十 MB 的整向量深拷贝。
    facts: Arc<Vec<MergedRequestFact>>,
    coverage: MergedCoverage,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct HotMergeCacheKey {
    local_date: String,
    day_boundary_mode: String,
    include_errors: bool,
    source_filter: String,
    tool_filter: String,
    pricing_match_mode: String,
    pricing_fingerprint: u64,
    local_signature: crate::local_usage::LocalMergeCacheSignature,
    proxy_signature: Option<ProxyMergeCacheSignature>,
}

#[derive(Debug, Clone)]
struct HotMergeCacheEntry {
    key: HotMergeCacheKey,
    /// Arc 包裹：命中时只克隆指针，过滤方从 Arc 借用按需拷贝。
    facts: Arc<Vec<MergedRequestFact>>,
}

/// 冷段（历史日物化事实）缓存 key。
///
/// 与外层 MergeCacheKey 不同，这里刻意**不含** local/proxy 实时签名：今日新
/// 请求落库只抖动实时签名，不触碰历史日物化状态；冷段数据是否可复用完全由
/// "实际日期列表 + 物化状态指纹 (state_row_count, max_materialized_at)" 决定。
/// 任何历史日被重建/新增/删除都会改变指纹，从而自然失效。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ColdFactsCacheKey {
    /// 排序后日期列表的首日（空列表为空串）。
    first_date: String,
    /// 排序后日期列表的末日（空列表为空串）。
    last_date: String,
    /// 日期数量。
    dates_count: usize,
    /// 排序后逐日拼接的 hash，防止首末日相同但中间日期不同的碰撞。
    dates_fingerprint: u64,
    day_boundary_mode: String,
    tool_filter: String,
    /// unified_daily_materialization_state 中命中日期的行数。
    state_row_count: i64,
    /// 命中日期中最大的 materialized_at。
    max_materialized_at: i64,
}

#[derive(Debug, Clone)]
struct ColdFactsCacheEntry {
    key: ColdFactsCacheKey,
    /// 未做 retain 过滤前的完整冷段事实（range/source/include_errors 过滤条件
    /// 因查询而异，留在命中后每次执行——纯内存线性过滤远比 SQLite 重读便宜）。
    /// Arc 包裹：命中返回指针拷贝，过滤方从借用中 filter+cloned 只拷贝命中子集。
    facts: Arc<Vec<MergedRequestFact>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct HistoryMaterializationCacheKey {
    start_epoch: i64,
    end_epoch: i64,
    day_boundary_mode: String,
    pricing_match_mode: String,
    pricing_fingerprint: u64,
    local_signature: crate::local_usage::LocalMergeCacheSignature,
    proxy_signature: Option<ProxyMergeCacheSignature>,
}

#[derive(Debug, Clone)]
struct HistoryMaterializationCacheEntry {
    key: HistoryMaterializationCacheKey,
    ready_dates: Vec<String>,
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
static HOT_MERGED_REQUEST_FACTS_CACHE: OnceLock<Mutex<Vec<HotMergeCacheEntry>>> = OnceLock::new();
static COLD_MERGED_FACTS_CACHE: OnceLock<Mutex<Vec<ColdFactsCacheEntry>>> = OnceLock::new();
static HISTORY_MATERIALIZATION_CACHE: OnceLock<Mutex<Vec<HistoryMaterializationCacheEntry>>> =
    OnceLock::new();
static MERGED_SESSIONS_CACHE: OnceLock<Mutex<Vec<SessionDerivedCacheEntry>>> = OnceLock::new();
static MERGED_PROJECTS_CACHE: OnceLock<Mutex<Vec<ProjectDerivedCacheEntry>>> = OnceLock::new();
static UNIFIED_INFLIGHT_KEYS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
// 概览 6 个时间窗口 + 统计页 7 个预设可能同时驻留，8 条会互相挤出，提高到 16。
const MERGED_REQUEST_FACTS_CACHE_CAPACITY: usize = 16;
const HOT_MERGED_REQUEST_FACTS_CACHE_CAPACITY: usize = 4;
// 冷段缓存每条 value 是整段历史日的完整事实向量（30 天以上范围可达几十 MB），
// 容量刻意压到 3 条以控制常驻内存；典型命中场景是同一时间范围在今日签名抖动
// 下的反复重算 + 少量相邻范围切换，小容量已经足够。
const COLD_MERGED_FACTS_CACHE_CAPACITY: usize = 3;
const HISTORY_MATERIALIZATION_CACHE_CAPACITY: usize = 8;
const MERGED_SESSIONS_CACHE_CAPACITY: usize = 6;
const MERGED_PROJECTS_CACHE_CAPACITY: usize = 6;
const REASONIX_FULL_COVERAGE_GRACE_SECS: i64 = 30;

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

fn hot_merge_cache() -> &'static Mutex<Vec<HotMergeCacheEntry>> {
    HOT_MERGED_REQUEST_FACTS_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn history_materialization_cache() -> &'static Mutex<Vec<HistoryMaterializationCacheEntry>> {
    HISTORY_MATERIALIZATION_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn cold_merged_facts_cache() -> &'static Mutex<Vec<ColdFactsCacheEntry>> {
    COLD_MERGED_FACTS_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn inflight_keys() -> &'static Mutex<HashSet<String>> {
    UNIFIED_INFLIGHT_KEYS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn normalized_day_boundary_mode(settings: &AppSettings) -> String {
    crate::utils::business_time::normalize_day_boundary_mode(&settings.day_boundary_mode)
}

pub(crate) fn clear_runtime_caches() {
    merge_cache().lock().unwrap().clear();
    hot_merge_cache().lock().unwrap().clear();
    cold_merged_facts_cache().lock().unwrap().clear();
    history_materialization_cache().lock().unwrap().clear();
    merged_sessions_cache().lock().unwrap().clear();
    merged_projects_cache().lock().unwrap().clear();
}

#[cfg(test)]
pub(crate) fn seed_runtime_merge_cache_for_test() {
    clear_runtime_caches();
    let settings = AppSettings::default();
    let source_filter = settings.source_aware.build_filter();
    let tool_filter = settings.client_tools.build_filter();
    let local_signature = crate::local_usage::LocalMergeCacheSignature {
        local_request_count: 1,
        local_max_sync_version: 1,
        local_max_timestamp: 1,
        remote_request_count: 0,
        remote_max_export_seq: 0,
        remote_max_timestamp: 0,
        local_session_max_updated_at: 0,
        remote_session_max_imported_at: 0,
        unified_materialization_invalidation_version: 1,
    };
    let merge_key = build_merge_cache_key(MergeCacheKeyParts {
        settings: &settings,
        range_start: 10,
        range_end: 20,
        include_errors: true,
        source_filter: &source_filter,
        tool_filter: &tool_filter,
        local_signature,
        proxy_signature: None,
        pricings: &[],
    });
    store_merge_cache(merge_key, Arc::new(Vec::new()), &MergedCoverage::default());
}

#[cfg(test)]
pub(crate) fn runtime_merge_cache_len_for_test() -> usize {
    merge_cache().lock().unwrap().len()
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
        ToolFilter::AnyOf(tools) => {
            let mut sorted = tools.clone();
            sorted.sort();
            format!("anyof:{}", sorted.join(","))
        }
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

fn lookup_merge_cache(
    key: &MergeCacheKey,
) -> Option<(Arc<Vec<MergedRequestFact>>, MergedCoverage)> {
    let cache = merge_cache();
    let mut guard = cache.lock().unwrap();
    let idx = guard.iter().position(|entry| entry.key == *key)?;
    let entry = guard.remove(idx);
    // Arc clone 只是指针拷贝，命中路径不再整向量深拷贝。
    let result = (entry.facts.clone(), entry.coverage.clone());
    guard.insert(0, entry);
    Some(result)
}

fn store_merge_cache(
    key: MergeCacheKey,
    facts: Arc<Vec<MergedRequestFact>>,
    coverage: &MergedCoverage,
) {
    let cache = merge_cache();
    let mut guard = cache.lock().unwrap();
    if let Some(idx) = guard.iter().position(|entry| entry.key == key) {
        guard.remove(idx);
    }
    guard.insert(
        0,
        MergeCacheEntry {
            key,
            facts,
            coverage: coverage.clone(),
        },
    );
    if guard.len() > MERGED_REQUEST_FACTS_CACHE_CAPACITY {
        guard.truncate(MERGED_REQUEST_FACTS_CACHE_CAPACITY);
    }
}

fn lookup_hot_merge_cache(key: &HotMergeCacheKey) -> Option<Arc<Vec<MergedRequestFact>>> {
    let cache = hot_merge_cache();
    let mut guard = cache.lock().unwrap();
    let idx = guard.iter().position(|entry| entry.key == *key)?;
    let entry = guard.remove(idx);
    let result = entry.facts.clone();
    guard.insert(0, entry);
    Some(result)
}

fn store_hot_merge_cache(key: HotMergeCacheKey, facts: Arc<Vec<MergedRequestFact>>) {
    let cache = hot_merge_cache();
    let mut guard = cache.lock().unwrap();
    if let Some(idx) = guard.iter().position(|entry| entry.key == key) {
        guard.remove(idx);
    }
    guard.insert(0, HotMergeCacheEntry { key, facts });
    if guard.len() > HOT_MERGED_REQUEST_FACTS_CACHE_CAPACITY {
        guard.truncate(HOT_MERGED_REQUEST_FACTS_CACHE_CAPACITY);
    }
}

fn build_cold_facts_cache_key(
    local_dates: &[String],
    tool_filter: &ToolFilter,
    day_boundary_mode: &str,
    state_row_count: i64,
    max_materialized_at: i64,
) -> ColdFactsCacheKey {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut sorted_dates: Vec<&String> = local_dates.iter().collect();
    sorted_dates.sort();
    let mut hasher = DefaultHasher::new();
    for date in &sorted_dates {
        date.hash(&mut hasher);
    }
    ColdFactsCacheKey {
        first_date: sorted_dates
            .first()
            .map(|d| d.to_string())
            .unwrap_or_default(),
        last_date: sorted_dates
            .last()
            .map(|d| d.to_string())
            .unwrap_or_default(),
        dates_count: sorted_dates.len(),
        dates_fingerprint: hasher.finish(),
        day_boundary_mode: day_boundary_mode.to_string(),
        tool_filter: cache_key_for_tool_filter(tool_filter),
        state_row_count,
        max_materialized_at,
    }
}

fn lookup_cold_facts_cache(key: &ColdFactsCacheKey) -> Option<Arc<Vec<MergedRequestFact>>> {
    let cache = cold_merged_facts_cache();
    let mut guard = cache.lock().unwrap();
    let idx = guard.iter().position(|entry| entry.key == *key)?;
    let entry = guard.remove(idx);
    let result = entry.facts.clone();
    guard.insert(0, entry);
    Some(result)
}

fn store_cold_facts_cache(key: ColdFactsCacheKey, facts: Arc<Vec<MergedRequestFact>>) {
    let cache = cold_merged_facts_cache();
    let mut guard = cache.lock().unwrap();
    if let Some(idx) = guard.iter().position(|entry| entry.key == key) {
        guard.remove(idx);
    }
    guard.insert(0, ColdFactsCacheEntry { key, facts });
    if guard.len() > COLD_MERGED_FACTS_CACHE_CAPACITY {
        guard.truncate(COLD_MERGED_FACTS_CACHE_CAPACITY);
    }
}

fn lookup_history_materialization_cache(
    key: &HistoryMaterializationCacheKey,
) -> Option<Vec<String>> {
    let cache = history_materialization_cache();
    let mut guard = cache.lock().unwrap();
    let idx = guard.iter().position(|entry| entry.key == *key)?;
    let entry = guard.remove(idx);
    let result = entry.ready_dates.clone();
    guard.insert(0, entry);
    Some(result)
}

fn store_history_materialization_cache(
    key: HistoryMaterializationCacheKey,
    ready_dates: &[String],
) {
    let cache = history_materialization_cache();
    let mut guard = cache.lock().unwrap();
    if let Some(idx) = guard.iter().position(|entry| entry.key == key) {
        guard.remove(idx);
    }
    guard.insert(
        0,
        HistoryMaterializationCacheEntry {
            key,
            ready_dates: ready_dates.to_vec(),
        },
    );
    if guard.len() > HISTORY_MATERIALIZATION_CACHE_CAPACITY {
        guard.truncate(HISTORY_MATERIALIZATION_CACHE_CAPACITY);
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
    canonical_request_key_for_local(record)
}

fn request_key_for_proxy(record: &UsageRecord) -> String {
    canonical_request_key_for_proxy(record)
}

fn local_tool_matches(record: &LocalRequestRecord, tool_filter: &ToolFilter) -> bool {
    match tool_filter {
        ToolFilter::All => true,
        ToolFilter::Tool(tool) if tool.trim().is_empty() => true,
        ToolFilter::Tool(tool) => record.tool == *tool,
        ToolFilter::AnyOf(tools) => tools.contains(&record.tool),
    }
}

fn session_meta_matches(meta: &SessionMeta, tool_filter: &ToolFilter) -> bool {
    match tool_filter {
        ToolFilter::All => true,
        ToolFilter::Tool(tool) if tool.trim().is_empty() => true,
        ToolFilter::Tool(tool) => meta.tool == *tool,
        ToolFilter::AnyOf(tools) => tools.contains(&meta.tool),
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

fn compute_local_request_cost_cached(
    record: &LocalRequestRecord,
    pricings: &[crate::models::ModelPricingConfig],
    match_mode: &str,
    pricing_cache: &mut HashMap<String, crate::models::ModelPricing>,
) -> f64 {
    let pricing = pricing_cache
        .entry(record.model.clone())
        .or_insert_with(|| crate::models::get_pricing(&record.model, pricings, match_mode));

    let input_cost = (record.input_tokens as f64 / 1_000_000.0) * pricing.input;
    let output_cost = (record.output_tokens as f64 / 1_000_000.0) * pricing.output;
    let cache_read_cost = (record.cache_read_tokens as f64 / 1_000_000.0) * pricing.cache_read;
    let cache_create_cost =
        (record.cache_create_tokens as f64 / 1_000_000.0) * pricing.cache_write_1h;

    input_cost + output_cost + cache_read_cost + cache_create_cost
}

#[derive(Default)]
struct ProjectAggregate {
    stats: ProjectStats,
    sessions: HashSet<String>,
    tool_sessions: HashMap<String, HashSet<String>>,
}

#[derive(Debug, Clone)]
struct ProjectDescriptor {
    key: String,
    name: String,
    identity: String,
    path: Option<String>,
}

fn metadata_only_sessions_allowed(source_filter: &crate::models::SourceFilter) -> bool {
    matches!(
        source_filter,
        crate::models::SourceFilter::All | crate::models::SourceFilter::Unknown { .. }
    )
}

fn session_project_identity(project_name: Option<&str>, cwd: Option<&str>) -> &'static str {
    if project_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
        || cwd
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
    {
        "project"
    } else {
        "unknown"
    }
}

fn project_descriptor_for_session(meta: &SessionMeta) -> ProjectDescriptor {
    let project_name = meta
        .project_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let project_path = meta
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    if let Some(path) = project_path.clone() {
        return ProjectDescriptor {
            key: path.clone(),
            name: project_name.clone().unwrap_or_else(|| path.clone()),
            identity: "project".to_string(),
            path: Some(path),
        };
    }

    if let Some(name) = project_name {
        return ProjectDescriptor {
            key: format!("name::{name}"),
            name,
            identity: "project".to_string(),
            path: None,
        };
    }

    ProjectDescriptor {
        key: format!("unknown::{}", meta.session_id),
        name: meta.session_id.clone(),
        identity: "unknown".to_string(),
        path: None,
    }
}

fn project_descriptor_for_fact(fact: &MergedRequestFact) -> ProjectDescriptor {
    let project_name = fact
        .project_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let project_path = fact
        .project_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    if let Some(path) = project_path.clone() {
        return ProjectDescriptor {
            key: path.clone(),
            name: project_name.clone().unwrap_or_else(|| path.clone()),
            identity: "project".to_string(),
            path: Some(path),
        };
    }

    if let Some(name) = project_name {
        return ProjectDescriptor {
            key: format!("name::{name}"),
            name,
            identity: "project".to_string(),
            path: None,
        };
    }

    let fallback = if !fact.session_id.trim().is_empty() {
        format!("unknown::{}", fact.session_id)
    } else {
        format!("unknown::fact::{}", fact.canonical_request_key)
    };
    ProjectDescriptor {
        key: fallback,
        name: fact.session_id.clone(),
        identity: "unknown".to_string(),
        path: None,
    }
}

fn session_usage_fully_covered(
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

fn reasonix_uncovered_request_count(
    local_only_requests: u64,
    unresolved_proxy_requests: u64,
) -> u64 {
    local_only_requests.saturating_add(unresolved_proxy_requests)
}

fn session_has_reasonix_coverage_gap(
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

fn build_metadata_only_session_stats(meta: &SessionMeta, now_sec: i64) -> SessionStats {
    SessionStats {
        session_id: meta.session_id.clone(),
        tool: meta.tool.clone(),
        total_requests: meta.message_count,
        total_input_tokens: meta.total_input_tokens,
        total_output_tokens: meta.total_output_tokens,
        total_cache_create_tokens: meta.total_cache_create_tokens,
        total_cache_read_tokens: meta.total_cache_read_tokens,
        total_duration_ms: 0,
        avg_output_tokens_per_second: 0.0,
        first_request_time: meta.start_time,
        last_request_time: meta.end_time.max(meta.last_modified),
        models: meta.models.clone(),
        avg_ttft_ms: 0.0,
        success_requests: 0,
        error_requests: 0,
        estimated_cost: meta.explicit_estimated_cost.unwrap_or(0.0),
        is_cost_estimated: meta.explicit_estimated_cost.is_none(),
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

fn merge_metadata_only_project(map: &mut HashMap<String, ProjectAggregate>, meta: &SessionMeta) {
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
        entry.sessions.insert(meta.session_id.clone());
        entry
            .tool_sessions
            .entry(meta.tool.clone())
            .or_default()
            .insert(meta.session_id.clone());
    } else {
        entry.tool_sessions.entry(meta.tool.clone()).or_default();
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

fn build_local_request_index(
    local_records: &[LocalRequestRecord],
) -> HashMap<String, LocalRequestRecord> {
    let mut map = HashMap::new();
    for record in local_records {
        map.insert(request_key_for_local(record), record.clone());
    }
    map
}

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

fn count_unresolved_reasonix_requests_by_session(
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

fn build_proxy_request_index(proxy_records: &[UsageRecord]) -> HashMap<String, UsageRecord> {
    let mut map = HashMap::new();
    for record in proxy_records {
        map.insert(request_key_for_proxy(record), record.clone());
    }
    map
}

fn enumerate_local_dates(start_epoch: i64, end_epoch: i64, settings: &AppSettings) -> Vec<String> {
    crate::utils::business_time::enumerate_business_dates(start_epoch, end_epoch, settings)
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

/// 归一化"查询到此刻"的开放式 range_end，用于稳定合并缓存 key。
///
/// 调用方（概览传 end=now+1、统计页传 end=当前秒）每次查询的 end 都是流动的
/// "此刻"，若直接进入缓存 key，则 key 每秒都不同，LRU 永远 miss。
///
/// 归一化规则：当 range_end >= now（开放式查询）时，把有效 range_end 上调到
/// 下一个整分钟边界 ((now / 60) + 1) * 60；range_end 严格小于 now 的历史精确
/// 范围（用户自定义历史区间、单个历史日等）保持原值不动，语义不变。
///
/// 正确性论证：
/// 1. now 之后的事实尚不存在，把过滤上界扩到下一分钟不会引入额外数据；
/// 2. 新数据到达会改变 local/proxy signature（缓存 key 的一部分），缓存自动失效；
/// 3. 因此同一分钟内、签名不变时的重复查询可以安全命中同一条缓存。
fn normalize_open_ended_range_end(range_end: i64, now: i64) -> i64 {
    if range_end < now {
        // 历史精确范围：保持原值，不改变查询语义。
        return range_end;
    }
    // 开放式查询：上调到下一个整分钟边界，让同一分钟内的 key 保持稳定。
    ((now / 60) + 1) * 60
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

struct HistoryMaterializationCacheKeyParts<'a> {
    settings: &'a AppSettings,
    range_start: i64,
    range_end: i64,
    pricing_match_mode: &'a str,
    pricings: &'a [crate::models::ModelPricingConfig],
    local_signature: crate::local_usage::LocalMergeCacheSignature,
    proxy_signature: Option<ProxyMergeCacheSignature>,
}

struct RealtimeMergeResult {
    facts: Vec<MergedRequestFact>,
    local_record_count: usize,
    remote_record_count: usize,
    proxy_record_count: usize,
    proxy_all_record_count: usize,
}

struct MergeRealtimeParams<'a> {
    settings: &'a AppSettings,
    start_epoch: Option<i64>,
    end_epoch: Option<i64>,
    include_errors: bool,
    pricings: &'a [crate::models::ModelPricingConfig],
    pricing_match_mode: &'a str,
    phase_label: &'a str,
}

fn build_merge_cache_key(parts: MergeCacheKeyParts<'_>) -> MergeCacheKey {
    MergeCacheKey {
        start_epoch: parts.range_start,
        end_epoch: parts.range_end,
        day_boundary_mode: normalized_day_boundary_mode(parts.settings),
        include_errors: parts.include_errors,
        source_filter: cache_key_for_source_filter(parts.source_filter),
        tool_filter: cache_key_for_tool_filter(parts.tool_filter),
        pricing_match_mode: parts.settings.model_pricing.match_mode.clone(),
        pricing_fingerprint: fingerprint_pricings(parts.pricings),
        local_signature: parts.local_signature,
        proxy_signature: parts.proxy_signature,
    }
}

fn build_hot_merge_cache_key(
    parts: MergeCacheKeyParts<'_>,
    local_date: String,
) -> HotMergeCacheKey {
    HotMergeCacheKey {
        local_date,
        day_boundary_mode: normalized_day_boundary_mode(parts.settings),
        include_errors: parts.include_errors,
        source_filter: cache_key_for_source_filter(parts.source_filter),
        tool_filter: cache_key_for_tool_filter(parts.tool_filter),
        pricing_match_mode: parts.settings.model_pricing.match_mode.clone(),
        pricing_fingerprint: fingerprint_pricings(parts.pricings),
        local_signature: parts.local_signature,
        proxy_signature: parts.proxy_signature,
    }
}

fn build_history_materialization_cache_key(
    parts: HistoryMaterializationCacheKeyParts<'_>,
) -> HistoryMaterializationCacheKey {
    HistoryMaterializationCacheKey {
        start_epoch: parts.range_start,
        end_epoch: parts.range_end,
        day_boundary_mode: normalized_day_boundary_mode(parts.settings),
        pricing_match_mode: parts.pricing_match_mode.to_string(),
        pricing_fingerprint: fingerprint_pricings(parts.pricings),
        local_signature: parts.local_signature,
        proxy_signature: parts.proxy_signature,
    }
}

fn merge_cache_inflight_key(key: &MergeCacheKey) -> String {
    format!(
        "merge:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        key.start_epoch,
        key.end_epoch,
        key.day_boundary_mode,
        key.include_errors,
        key.source_filter,
        key.tool_filter,
        key.pricing_match_mode,
        key.pricing_fingerprint,
        key.local_signature.local_request_count,
        key.local_signature.local_max_sync_version,
        key.local_signature.remote_request_count,
        key.local_signature
            .unified_materialization_invalidation_version,
        key.proxy_signature
            .map(|sig| format!(
                "{}:{}:{}",
                sig.usage_record_count, sig.max_timestamp, sig.max_updated_at
            ))
            .unwrap_or_else(|| "none".to_string()),
    )
}

fn history_materialization_inflight_key(key: &HistoryMaterializationCacheKey) -> String {
    format!(
        "history:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        key.start_epoch,
        key.end_epoch,
        key.day_boundary_mode,
        key.pricing_match_mode,
        key.pricing_fingerprint,
        key.local_signature.local_max_sync_version,
        key.local_signature.remote_max_export_seq,
        key.local_signature
            .unified_materialization_invalidation_version,
        key.proxy_signature
            .map(|sig| format!(
                "{}:{}:{}:{}",
                sig.usage_record_count,
                sig.max_timestamp,
                sig.max_updated_at,
                sig.session_stats_max_updated_at
            ))
            .unwrap_or_else(|| "none".to_string()),
    )
}

struct InflightKeyGuard {
    key: String,
}

impl Drop for InflightKeyGuard {
    fn drop(&mut self) {
        inflight_keys().lock().unwrap().remove(&self.key);
    }
}

async fn acquire_inflight_key(key: &str) -> InflightKeyGuard {
    loop {
        let acquired = {
            let mut guard = inflight_keys().lock().unwrap();
            if guard.contains(key) {
                false
            } else {
                guard.insert(key.to_string());
                true
            }
        };
        if acquired {
            return InflightKeyGuard {
                key: key.to_string(),
            };
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn canonical_history_settings(settings: &AppSettings) -> AppSettings {
    let mut canonical = settings.clone();
    canonical.client_tools.active_tool_filter = None;
    canonical.source_aware.active_source_filter = None;
    canonical
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

async fn merge_realtime_range(
    local_db: Arc<crate::local_usage::LocalUsageDatabase>,
    params: MergeRealtimeParams<'_>,
) -> Result<RealtimeMergeResult, String> {
    let overall_started_at = Instant::now();
    let MergeRealtimeParams {
        settings,
        start_epoch,
        end_epoch,
        include_errors,
        pricings,
        pricing_match_mode,
        phase_label,
    } = params;
    let tool_filter = settings.client_tools.build_filter();
    let usage_filter = UsageQueryFilter {
        source: settings.source_aware.build_filter(),
        tool: settings.client_tools.build_filter(),
    };
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

    let (range_start, range_end) = normalize_range_bounds(start_epoch, end_epoch);

    // 前置同步段：本地/远端会话与请求记录的 SQLite 全行读取、去重与索引构建
    // 是合并路径上的同步重活，移入阻塞线程池，避免占住 tauri async runtime 的
    // 工作线程；local_ms 计时随逻辑一起进入闭包，字段含义不变。
    let (
        local_records,
        local_record_count,
        remote_record_count,
        session_meta_by_id,
        message_to_session,
        local_query_elapsed_ms,
    ) = tauri::async_runtime::spawn_blocking(move || {
        let local_query_started_at = Instant::now();
        let mut local_sessions_all = local_db.get_all_sessions(&tool_filter)?;
        local_sessions_all.extend(local_db.get_remote_sessions(&tool_filter)?);
        let local_sessions: Vec<SessionMeta> = local_sessions_all
            .into_iter()
            .filter(|meta| session_meta_matches(meta, &tool_filter))
            .collect();
        let mut local_records =
            local_db.get_request_records_in_range(range_start, range_end, &tool_filter)?;
        let local_record_count = local_records.len();
        let local_request_keys: HashSet<String> =
            local_records.iter().map(request_key_for_local).collect();
        let mut remote_records =
            local_db.get_remote_request_records_in_range(range_start, range_end, &tool_filter)?;
        let remote_record_count = remote_records.len();
        remote_records
            .retain(|record| !local_request_keys.contains(&request_key_for_local(record)));
        local_records.extend(remote_records);
        local_records.retain(|record| local_tool_matches(record, &tool_filter));
        let mut seen_local_keys = HashSet::new();
        local_records.retain(|record| seen_local_keys.insert(request_key_for_local(record)));
        let session_meta_by_id = build_local_meta_index(&local_sessions);
        let message_to_session = build_message_to_session_index(&local_records);
        Ok::<_, String>((
            local_records,
            local_record_count,
            remote_record_count,
            session_meta_by_id,
            message_to_session,
            local_query_started_at.elapsed().as_millis(),
        ))
    })
    .await
    .map_err(|e| format!("Task error: {}", e))??;

    // 异步取数段：proxy 记录的两次取数是真正的 await 点，保持在 async 上下文；
    // 取回后的纯内存加工（session 归属回填、可见性过滤）挪入后置同步段一并
    // 下沉阻塞线程池，proxy_ms 仍计到 proxy 记录整理就绪为止，含义不变。
    let proxy_query_started_at = Instant::now();
    let (raw_proxy_records, raw_unfiltered_proxy_records) = if let Some(proxy_db) =
        ProxyDatabase::get_global()
    {
        let records =
            fetch_proxy_records(proxy_db.as_ref(), &usage_filter, start_epoch, end_epoch).await?;
        let unfiltered = if let Some(filter) = unfiltered_usage_filter.as_ref() {
            Some(fetch_proxy_records(proxy_db.as_ref(), filter, start_epoch, end_epoch).await?)
        } else {
            None
        };
        (records, unfiltered)
    } else {
        (Vec::new(), None)
    };

    // 后置同步合并段：索引构建、模糊匹配、合并主循环与排序都是大向量上的纯
    // CPU 工作（codex 回退 base_url 还含一次阻塞的 config.toml 读取），整体移
    // 入阻塞线程池。Instant 是 Copy，跨线程后各段 elapsed 计时含义保持不变。
    let inputs = RealtimeMergeComputeInputs {
        local_records,
        local_record_count,
        remote_record_count,
        session_meta_by_id,
        message_to_session,
        raw_proxy_records,
        raw_unfiltered_proxy_records,
        include_errors,
        pricings: pricings.to_vec(),
        pricing_match_mode: pricing_match_mode.to_string(),
        phase_label: phase_label.to_string(),
        range_start,
        range_end,
        local_query_elapsed_ms,
        proxy_query_started_at,
        overall_started_at,
    };
    tauri::async_runtime::spawn_blocking(move || merge_realtime_compute_sync(inputs))
        .await
        .map_err(|e| format!("Task error: {}", e))?
}

/// merge_realtime_range 后置同步合并段的输入（跨线程移交所需的全部所有权数据）。
struct RealtimeMergeComputeInputs {
    local_records: Vec<LocalRequestRecord>,
    local_record_count: usize,
    remote_record_count: usize,
    session_meta_by_id: HashMap<String, SessionMeta>,
    message_to_session: HashMap<String, String>,
    raw_proxy_records: Vec<UsageRecord>,
    raw_unfiltered_proxy_records: Option<Vec<UsageRecord>>,
    include_errors: bool,
    pricings: Vec<crate::models::ModelPricingConfig>,
    pricing_match_mode: String,
    phase_label: String,
    range_start: i64,
    range_end: i64,
    local_query_elapsed_ms: u128,
    proxy_query_started_at: Instant,
    overall_started_at: Instant,
}

/// merge_realtime_range 的后置同步合并段，经 spawn_blocking 在阻塞线程池执行。
fn merge_realtime_compute_sync(
    inputs: RealtimeMergeComputeInputs,
) -> Result<RealtimeMergeResult, String> {
    let RealtimeMergeComputeInputs {
        local_records,
        local_record_count,
        remote_record_count,
        session_meta_by_id,
        message_to_session,
        raw_proxy_records,
        raw_unfiltered_proxy_records,
        include_errors,
        pricings,
        pricing_match_mode,
        phase_label,
        range_start,
        range_end,
        local_query_elapsed_ms,
        proxy_query_started_at,
        overall_started_at,
    } = inputs;
    let pricings: &[crate::models::ModelPricingConfig] = &pricings;
    let pricing_match_mode: &str = &pricing_match_mode;
    let mut attached_proxy_records = raw_proxy_records;
    attach_proxy_session_ids(&mut attached_proxy_records, &message_to_session);
    let proxy_records: Vec<UsageRecord> = attached_proxy_records
        .iter()
        .filter(|record| include_errors || (200..300).contains(&record.status_code))
        .cloned()
        .collect();
    let all_proxy_records: Vec<UsageRecord> =
        if let Some(mut unfiltered) = raw_unfiltered_proxy_records {
            attach_proxy_session_ids(&mut unfiltered, &message_to_session);
            unfiltered
        } else {
            attached_proxy_records
        };
    let proxy_query_elapsed_ms = proxy_query_started_at.elapsed().as_millis();

    let index_build_started_at = Instant::now();
    let all_proxy_index = build_proxy_request_index(&all_proxy_records);
    let proxy_index = build_proxy_request_index(&proxy_records);
    let local_index = build_local_request_index(&local_records);
    let index_build_elapsed_ms = index_build_started_at.elapsed().as_millis();

    // Codex's local JSONL scanner fabricates a per-request message_id (see codex_reader.rs),
    // so it can never exact-key-match its real proxy counterpart. Reconcile the leftover
    // Codex-only orphans on both sides via a bounded fuzzy match (same session/model/
    // total_tokens, close timestamp) before the exact-match loop runs, so the same physical
    // request doesn't surface twice (once unattributed via from_local, once via from_proxy).
    let (codex_local_orphans, codex_proxy_orphans_visible, codex_proxy_orphans_all_extra) =
        codex_orphan_pools(&local_index, &proxy_index, &all_proxy_index);
    let codex_fuzzy_outcomes = find_codex_fuzzy_matches(
        &codex_local_orphans,
        &codex_proxy_orphans_visible,
        &codex_proxy_orphans_all_extra,
    );

    let mut fuzzy_consumed_local_keys: HashSet<String> = HashSet::new();
    let mut fuzzy_consumed_proxy_keys: HashSet<String> = HashSet::new();
    let mut fuzzy_suppressed_local_keys: HashSet<String> = HashSet::new();

    let merge_loop_started_at = Instant::now();
    let mut pricing_cache: HashMap<String, crate::models::ModelPricing> = HashMap::new();
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
                        pricings,
                        pricing_match_mode,
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

    // Fix B: best-effort attribution for local Codex records that remain genuinely
    // local-only after fuzzy matching (proxy wasn't running for that request). Resolved once
    // per merge call — this is a blocking config.toml read — and only when there's any Codex
    // local material in range, to avoid the FS read for non-Codex users.
    let codex_fallback_base_url: Option<String> = if local_records.iter().any(|r| r.tool == "codex")
    {
        CodexConfigManager::new()
            .active_source_id()
            .and_then(|id| CodexSourceRegistry::new().get(&id))
            .map(|handle| handle.real_base_url)
    } else {
        None
    };

    let mut keys = HashSet::new();
    keys.extend(proxy_index.keys().cloned());
    keys.extend(local_index.keys().cloned());

    for key in keys {
        match (proxy_index.get(&key), local_index.get(&key)) {
            (Some(proxy), Some(local)) => {
                let meta = session_meta_by_id.get(&local.session_id);
                let fallback_cost = compute_local_request_cost_cached(
                    local,
                    pricings,
                    pricing_match_mode,
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
                    // Already emitted as a fuzzy-merged fact above — avoid double counting.
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
                if all_proxy_index.contains_key(&key) {
                    continue;
                }
                if fuzzy_consumed_local_keys.contains(&key)
                    || fuzzy_suppressed_local_keys.contains(&key)
                {
                    // Either already emitted as a fuzzy-merged fact, or intentionally dropped
                    // because its only candidate was filtered out on the proxy side.
                    continue;
                }
                let meta = session_meta_by_id.get(&local.session_id);
                let cost = compute_local_request_cost_cached(
                    local,
                    pricings,
                    pricing_match_mode,
                    &mut pricing_cache,
                );
                let fallback_base_url = if local.tool == "codex" {
                    codex_fallback_base_url.as_deref()
                } else {
                    None
                };
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
    let merge_loop_elapsed_ms = merge_loop_started_at.elapsed().as_millis();

    let sort_started_at = Instant::now();
    merged.sort_by_key(|fact| fact.timestamp_ms);
    let sort_elapsed_ms = sort_started_at.elapsed().as_millis();
    perf_log(
        "merge_realtime_breakdown",
        format!(
            "phase={} range={}..{} local_ms={} proxy_ms={} index_ms={} merge_ms={} sort_ms={} local_records={} remote_records={} proxy_records={} all_proxy_records={} facts={} total_ms={}",
            phase_label,
            range_start,
            range_end,
            local_query_elapsed_ms,
            proxy_query_elapsed_ms,
            index_build_elapsed_ms,
            merge_loop_elapsed_ms,
            sort_elapsed_ms,
            local_record_count,
            remote_record_count,
            proxy_index.len(),
            all_proxy_index.len(),
            merged.len(),
            overall_started_at.elapsed().as_millis(),
        ),
    );
    Ok(RealtimeMergeResult {
        facts: merged,
        local_record_count,
        remote_record_count,
        proxy_record_count: proxy_index.len(),
        proxy_all_record_count: all_proxy_index.len(),
    })
}

fn request_key_for_fact(fact: &MergedRequestFact) -> String {
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

fn materialization_state_matches(
    state: &crate::local_usage::UnifiedDayMaterializationState,
    snapshot: &crate::local_usage::UnifiedDayLocalSnapshot,
    proxy_snapshot: crate::proxy::ProxyDayDependencySnapshot,
    pricing_fingerprint: u64,
    settings: &AppSettings,
) -> bool {
    state.is_finalized
        && state.day_boundary_mode
            == crate::utils::business_time::normalize_day_boundary_mode(&settings.day_boundary_mode)
        && state.pricing_fingerprint == pricing_fingerprint
        && state.local_request_count == snapshot.local_request_count
        && state.local_max_sync_version == snapshot.local_max_sync_version
        && state.local_max_timestamp == snapshot.local_max_timestamp
        && state.remote_request_count == snapshot.remote_request_count
        && state.remote_max_export_seq == snapshot.remote_max_export_seq
        && state.remote_max_timestamp == snapshot.remote_max_timestamp
        && state.proxy_record_count == proxy_snapshot.record_count
        && state.proxy_max_timestamp_ms == proxy_snapshot.max_timestamp_ms
        && state.proxy_max_updated_at == proxy_snapshot.max_updated_at
}

struct MaterializationStateBuildContext<'a> {
    local_date: &'a str,
    fact_count: usize,
    local_snapshot: &'a crate::local_usage::UnifiedDayLocalSnapshot,
    proxy_snapshot: crate::proxy::ProxyDayDependencySnapshot,
    pricing_fingerprint: u64,
    max_fact_timestamp_ms: i64,
    materialized_at: i64,
    settings: &'a AppSettings,
}

fn build_materialization_state(
    ctx: MaterializationStateBuildContext<'_>,
) -> crate::local_usage::UnifiedDayMaterializationState {
    let MaterializationStateBuildContext {
        local_date,
        fact_count,
        local_snapshot,
        proxy_snapshot,
        pricing_fingerprint,
        max_fact_timestamp_ms,
        materialized_at,
        settings,
    } = ctx;
    crate::local_usage::UnifiedDayMaterializationState {
        local_date: local_date.to_string(),
        day_boundary_mode: crate::utils::business_time::normalize_day_boundary_mode(
            &settings.day_boundary_mode,
        ),
        fact_count: fact_count as u64,
        local_request_count: local_snapshot.local_request_count,
        local_max_sync_version: local_snapshot.local_max_sync_version,
        local_max_timestamp: local_snapshot.local_max_timestamp,
        remote_request_count: local_snapshot.remote_request_count,
        remote_max_export_seq: local_snapshot.remote_max_export_seq,
        remote_max_timestamp: local_snapshot.remote_max_timestamp,
        proxy_record_count: proxy_snapshot.record_count,
        proxy_all_record_count: proxy_snapshot.record_count,
        proxy_max_timestamp_ms: proxy_snapshot.max_timestamp_ms,
        proxy_max_updated_at: proxy_snapshot.max_updated_at,
        max_fact_timestamp_ms,
        pricing_fingerprint,
        is_finalized: true,
        finalized_at: Some(materialized_at),
        materialized_at,
    }
}

#[allow(clippy::too_many_arguments)]
async fn ensure_materialized_history_for_range(
    local_db: Arc<crate::local_usage::LocalUsageDatabase>,
    settings: &AppSettings,
    range_start: i64,
    range_end: i64,
    pricings: &[crate::models::ModelPricingConfig],
    pricing_match_mode: &str,
    local_signature: crate::local_usage::LocalMergeCacheSignature,
    proxy_signature: Option<ProxyMergeCacheSignature>,
) -> Result<Vec<String>, String> {
    let today = crate::local_usage::LocalUsageDatabase::today_local_date_with_settings(settings);
    let canonical_settings = canonical_history_settings(settings);
    let cache_key = build_history_materialization_cache_key(HistoryMaterializationCacheKeyParts {
        settings,
        range_start,
        range_end,
        pricing_match_mode,
        pricings,
        local_signature,
        proxy_signature,
    });
    if let Some(ready_dates) = lookup_history_materialization_cache(&cache_key) {
        perf_log(
            "merge_history_materialize_cache_hit",
            format!(
                "range={}..{} dates={}",
                range_start,
                range_end,
                ready_dates.len(),
            ),
        );
        return Ok(ready_dates);
    }

    let materializable_dates: Vec<String> = enumerate_local_dates(range_start, range_end, settings)
        .into_iter()
        .filter(|date| date < &today)
        .collect();

    let mut ready_dates = Vec::new();
    let pricing_fingerprint = fingerprint_pricings(pricings);
    for local_date in materializable_dates {
        let day_started_at = Instant::now();
        let (day_start, day_end) =
            crate::local_usage::LocalUsageDatabase::local_date_epoch_bounds_with_settings(
                &local_date,
                settings,
            )?;
        let local_snapshot =
            local_db.get_unified_day_local_snapshot_with_settings(&local_date, settings)?;
        let proxy_snapshot = ProxyDatabase::get_global()
            .map(|db| {
                db.get_day_dependency_snapshot(
                    day_start.saturating_mul(1000),
                    day_end.saturating_mul(1000),
                )
            })
            .transpose()?
            .unwrap_or_default();
        let state = local_db.get_unified_day_materialization_state(&local_date)?;
        let needs_rebuild = match state {
            Some(ref state) => !materialization_state_matches(
                state,
                &local_snapshot,
                proxy_snapshot,
                pricing_fingerprint,
                settings,
            ),
            None => true,
        };

        if needs_rebuild {
            let inflight_key = format!("materialize:{local_date}");
            let _inflight_guard = acquire_inflight_key(&inflight_key).await;
            let latest_state = local_db.get_unified_day_materialization_state(&local_date)?;
            let latest_local_snapshot =
                local_db.get_unified_day_local_snapshot_with_settings(&local_date, settings)?;
            let latest_proxy_snapshot = ProxyDatabase::get_global()
                .map(|db| {
                    db.get_day_dependency_snapshot(
                        day_start.saturating_mul(1000),
                        day_end.saturating_mul(1000),
                    )
                })
                .transpose()?
                .unwrap_or_default();
            let still_needs_rebuild = match latest_state {
                Some(ref state) => !materialization_state_matches(
                    state,
                    &latest_local_snapshot,
                    latest_proxy_snapshot,
                    pricing_fingerprint,
                    settings,
                ),
                None => true,
            };
            let materialize_result = async {
                if still_needs_rebuild {
                    let merge = merge_realtime_range(
                        local_db.clone(),
                        MergeRealtimeParams {
                            settings: &canonical_settings,
                            start_epoch: Some(day_start),
                            end_epoch: Some(day_end),
                            include_errors: true,
                            pricings,
                            pricing_match_mode,
                            phase_label: "materialize_day",
                        },
                    )
                    .await?;
                    let fact_entries: Vec<(String, MergedRequestFact)> = merge
                        .facts
                        .iter()
                        .map(|fact| (request_key_for_fact(fact), fact.clone()))
                        .collect();
                    let max_fact_timestamp_ms = merge
                        .facts
                        .iter()
                        .map(|fact| fact.timestamp_ms)
                        .max()
                        .unwrap_or(0);
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    local_db.replace_unified_day_materialization(
                        &local_date,
                        &fact_entries,
                        &build_materialization_state(MaterializationStateBuildContext {
                            local_date: &local_date,
                            fact_count: merge.facts.len(),
                            local_snapshot: &latest_local_snapshot,
                            proxy_snapshot: latest_proxy_snapshot,
                            pricing_fingerprint,
                            max_fact_timestamp_ms,
                            materialized_at: now_ms,
                            settings,
                        }),
                    )?;
                    perf_log(
                        "merge_materialize_day",
                        format!(
                            "date={} facts={} local_records={} remote_records={} proxy_records={} all_proxy_records={} elapsed_ms={}",
                            local_date,
                            merge.facts.len(),
                            merge.local_record_count,
                            merge.remote_record_count,
                            merge.proxy_record_count,
                            merge.proxy_all_record_count,
                            day_started_at.elapsed().as_millis(),
                        ),
                    );
                }
                Ok::<(), String>(())
            }
            .await;
            materialize_result?;
        }
        ready_dates.push(local_date);
    }

    store_history_materialization_cache(cache_key, &ready_dates);
    Ok(ready_dates)
}

fn combined_data_time_bounds(
    local_db: &crate::local_usage::LocalUsageDatabase,
) -> Result<Option<(i64, i64)>, String> {
    let local_bounds = local_db.get_request_time_bounds()?;
    let proxy_bounds = ProxyDatabase::get_global()
        .map(|db| db.get_request_time_bounds())
        .transpose()?
        .flatten();

    Ok(match (local_bounds, proxy_bounds) {
        (Some((local_start, local_end)), Some((proxy_start, proxy_end))) => {
            Some((local_start.min(proxy_start), local_end.max(proxy_end)))
        }
        (Some(bounds), None) | (None, Some(bounds)) => Some(bounds),
        (None, None) => None,
    })
}

async fn ensure_materialized_history_with_db(
    local_db: Arc<crate::local_usage::LocalUsageDatabase>,
    settings: &AppSettings,
    start_epoch: i64,
    end_epoch: i64,
) -> Result<(), String> {
    let local_signature = local_db.get_merge_cache_signature()?;
    let proxy_signature = ProxyDatabase::get_global()
        .map(|db| db.get_merge_cache_signature())
        .transpose()?;
    let mut pricings = settings.model_pricing.pricings.clone();
    if let Ok(db) = crate::proxy::ProxyDatabase::new() {
        if let Ok(db_pricings) = db.get_all_model_pricings() {
            pricings.extend(db_pricings);
        }
    }
    let pricing_match_mode = settings.model_pricing.match_mode.clone();
    let effective_range =
        if let Some((data_start, data_end)) = combined_data_time_bounds(&local_db)? {
            let effective_start = start_epoch.max(data_start);
            let effective_end = end_epoch.min(data_end.max(start_epoch.saturating_add(1)));
            (effective_start, effective_end)
        } else {
            (start_epoch, start_epoch)
        };
    if effective_range.1 <= effective_range.0 {
        return Ok(());
    }
    let cache_key = build_history_materialization_cache_key(HistoryMaterializationCacheKeyParts {
        settings,
        range_start: effective_range.0,
        range_end: effective_range.1,
        pricing_match_mode: &pricing_match_mode,
        pricings: &pricings,
        local_signature,
        proxy_signature,
    });
    if lookup_history_materialization_cache(&cache_key).is_some() {
        return Ok(());
    }

    let inflight_key = history_materialization_inflight_key(&cache_key);
    let _inflight_guard = acquire_inflight_key(&inflight_key).await;
    if lookup_history_materialization_cache(&cache_key).is_some() {
        return Ok(());
    }
    let result = ensure_materialized_history_for_range(
        local_db,
        settings,
        effective_range.0,
        effective_range.1,
        &pricings,
        &pricing_match_mode,
        local_signature,
        proxy_signature,
    )
    .await;
    let _ = result?;
    Ok(())
}

pub(crate) async fn ensure_materialized_history_no_sync(
    settings: &AppSettings,
    start_epoch: i64,
    end_epoch: i64,
) -> Result<(), String> {
    let local_db = crate::local_usage::get_local_usage_db()?;
    ensure_materialized_history_with_db(local_db, settings, start_epoch, end_epoch).await
}

async fn get_hot_merge_facts(
    local_db: Arc<crate::local_usage::LocalUsageDatabase>,
    settings: &AppSettings,
    include_errors: bool,
    pricings: &[crate::models::ModelPricingConfig],
    pricing_match_mode: &str,
    local_signature: crate::local_usage::LocalMergeCacheSignature,
    proxy_signature: Option<ProxyMergeCacheSignature>,
) -> Result<Arc<Vec<MergedRequestFact>>, String> {
    let today = crate::local_usage::LocalUsageDatabase::today_local_date_with_settings(settings);
    let today_start =
        crate::local_usage::LocalUsageDatabase::local_date_epoch_bounds_with_settings(
            &today, settings,
        )
        .map(|(start, _)| start)?;
    let tool_filter = settings.client_tools.build_filter();
    let source_filter = settings.source_aware.build_filter();
    let cache_key = build_hot_merge_cache_key(
        MergeCacheKeyParts {
            settings,
            range_start: today_start,
            range_end: i64::MAX,
            include_errors,
            source_filter: &source_filter,
            tool_filter: &tool_filter,
            local_signature,
            proxy_signature,
            pricings,
        },
        today.clone(),
    );
    if let Some(facts) = lookup_hot_merge_cache(&cache_key) {
        perf_log(
            "merge_hot_cache_hit",
            format!("date={} facts={}", today, facts.len()),
        );
        return Ok(facts);
    }

    let inflight_key = format!(
        "hot:{}:{}:{}:{}:{}:{}",
        cache_key.local_date,
        cache_key.include_errors,
        cache_key.tool_filter,
        cache_key.source_filter,
        cache_key.pricing_match_mode,
        cache_key.pricing_fingerprint
    );
    let _inflight_guard = acquire_inflight_key(&inflight_key).await;
    if let Some(facts) = lookup_hot_merge_cache(&cache_key) {
        perf_log(
            "merge_hot_cache_hit_after_wait",
            format!("date={} facts={}", today, facts.len()),
        );
        return Ok(facts);
    }

    let compute_result = async {
        let merge = merge_realtime_range(
            local_db,
            MergeRealtimeParams {
                settings,
                start_epoch: Some(today_start),
                end_epoch: None,
                include_errors,
                pricings,
                pricing_match_mode,
                phase_label: "hot_day",
            },
        )
        .await?;
        // 缓存与调用方共享同一个 Arc，写缓存不再整向量深拷贝。
        let facts = Arc::new(merge.facts);
        store_hot_merge_cache(cache_key.clone(), facts.clone());
        perf_log(
            "merge_hot_cache_store",
            format!(
                "date={} facts={} local_records={} remote_records={} proxy_records={} all_proxy_records={}",
                today,
                facts.len(),
                merge.local_record_count,
                merge.remote_record_count,
                merge.proxy_record_count,
                merge.proxy_all_record_count,
            ),
        );
        Ok::<Arc<Vec<MergedRequestFact>>, String>(facts)
    }
    .await;
    compute_result
}

async fn get_merged_request_facts_with_db(
    local_db: Arc<crate::local_usage::LocalUsageDatabase>,
    settings: &AppSettings,
    start_epoch: Option<i64>,
    end_epoch: Option<i64>,
    include_errors: bool,
) -> Result<(Arc<Vec<MergedRequestFact>>, MergedCoverage), String> {
    let overall_started_at = Instant::now();
    let (range_start, raw_range_end) = normalize_range_bounds(start_epoch, end_epoch);
    // 对开放式 range_end 做整分钟归一化（详见 normalize_open_ended_range_end），
    // 归一化后的 end 同时作为缓存 key 与事实过滤上界；再 max(range_start) 保持
    // end >= start 的不变量（未来起点的空区间仍返回空结果）。
    let range_end = normalize_open_ended_range_end(raw_range_end, chrono::Utc::now().timestamp())
        .max(range_start);
    if range_end != raw_range_end {
        perf_log(
            "merge_range_end_normalized",
            format!("raw_end={raw_range_end} normalized_end={range_end}"),
        );
    }
    let tool_filter = settings.client_tools.build_filter();
    let source_filter = settings.source_aware.build_filter();
    let mut pricings = settings.model_pricing.pricings.clone();
    if let Ok(db) = crate::proxy::ProxyDatabase::new() {
        if let Ok(db_pricings) = db.get_all_model_pricings() {
            pricings.extend(db_pricings);
        }
    }
    let pricing_match_mode = settings.model_pricing.match_mode.clone();

    let local_signature = local_db.get_merge_cache_signature()?;
    let proxy_signature = ProxyDatabase::get_global()
        .map(|db| db.get_merge_cache_signature())
        .transpose()?;
    let cache_key = build_merge_cache_key(MergeCacheKeyParts {
        settings,
        range_start,
        range_end,
        include_errors,
        source_filter: &source_filter,
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
    let inflight_key = merge_cache_inflight_key(&cache_key);
    let _inflight_guard = acquire_inflight_key(&inflight_key).await;
    if let Some((facts, coverage)) = lookup_merge_cache(&cache_key) {
        perf_log(
            "merge_cache_hit_after_wait",
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
    let query_started_at = Instant::now();
    let compute_result = async {
        let history_materialize_started_at = Instant::now();
        let history_ready_dates = if let Some((data_start, data_end)) = combined_data_time_bounds(&local_db)? {
            let effective_start = range_start.max(data_start);
            let effective_end = range_end.min(data_end.max(range_start + 1));
            if effective_end > effective_start {
                ensure_materialized_history_for_range(
                    local_db.clone(),
                    settings,
                    effective_start,
                    effective_end,
                    &pricings,
                    &pricing_match_mode,
                    local_signature,
                    proxy_signature,
                )
                .await?
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let history_materialize_elapsed_ms = history_materialize_started_at.elapsed().as_millis();
        let history_dates_count = history_ready_dates.len();
        perf_log(
            "merge_history_materialize",
            format!(
                "range={range_start}..{range_end} dates={} elapsed_ms={}",
                history_dates_count, history_materialize_elapsed_ms,
            ),
        );

        // 冷读是 SQLite 全行读取 + 反序列化，随后的范围过滤是大向量线性扫描，
        // 两者是合并路径上最重的同步阻塞段；一并移入阻塞线程池，避免占住
        // tauri async runtime 的工作线程拖慢并发命令。perf 埋点随逻辑一起
        // 进入闭包，elapsed 计时含义保持不变。
        let cold_db = local_db.clone();
        let cold_dates = history_ready_dates;
        let cold_tool_filter = tool_filter.clone();
        let cold_source_filter = source_filter.clone();
        let cold_day_boundary_mode = normalized_day_boundary_mode(settings);
        let (mut merged, filtered_cold_facts_count) =
            tauri::async_runtime::spawn_blocking(move || {
                let cold_read_started_at = Instant::now();
                // 冷段缓存：今日新请求只抖动 local/proxy 实时签名（外层合并缓存
                // 因此失效），但不触碰历史日的物化状态。这里以 ensure_materialized
                // 校验后的实际日期列表 + 物化状态指纹为 key，命中即跳过对全部
                // 历史日事实行的 SQLite 全量重读。指纹查询是一次轻量点查，必须
                // 与冷读同处本阻塞闭包内执行。
                let cold_fingerprint = if cold_dates.is_empty() {
                    None
                } else {
                    match cold_db.get_unified_days_materialization_fingerprint(&cold_dates) {
                        // 状态行数必须与就绪日期数一致才可信；缺行说明某日状态
                        // 异常（如并发失效清理），此时跳过缓存直读数据库。
                        Ok((count, max_at)) if count == cold_dates.len() as i64 => {
                            Some((count, max_at))
                        }
                        Ok((count, _)) => {
                            perf_log(
                                "merge_cold_cache_skip",
                                format!(
                                    "range={range_start}..{range_end} dates={} state_rows={count} reason=state_row_mismatch",
                                    cold_dates.len(),
                                ),
                            );
                            None
                        }
                        Err(err) => {
                            perf_log(
                                "merge_cold_cache_skip",
                                format!(
                                    "range={range_start}..{range_end} dates={} reason=fingerprint_error error={err}",
                                    cold_dates.len(),
                                ),
                            );
                            None
                        }
                    }
                };
                let cold_cache_key = cold_fingerprint.map(|(count, max_at)| {
                    build_cold_facts_cache_key(
                        &cold_dates,
                        &cold_tool_filter,
                        &cold_day_boundary_mode,
                        count,
                        max_at,
                    )
                });
                let cached_cold_facts = cold_cache_key.as_ref().and_then(lookup_cold_facts_cache);
                let cold_facts: Arc<Vec<MergedRequestFact>> = match cached_cold_facts {
                    Some(facts) => {
                        perf_log(
                            "merge_cold_cache_hit",
                            format!(
                                "range={range_start}..{range_end} dates={} facts={} elapsed_ms={}",
                                cold_dates.len(),
                                facts.len(),
                                cold_read_started_at.elapsed().as_millis(),
                            ),
                        );
                        facts
                    }
                    None => {
                        let facts =
                            cold_db.get_unified_facts_for_dates(&cold_dates, &cold_tool_filter)?;
                        perf_log(
                            "merge_cold_read",
                            format!(
                                "range={range_start}..{range_end} dates={} facts={} elapsed_ms={}",
                                cold_dates.len(),
                                facts.len(),
                                cold_read_started_at.elapsed().as_millis(),
                            ),
                        );
                        let facts = Arc::new(facts);
                        if let Some(key) = cold_cache_key {
                            // 写缓存前复核指纹：若读事实期间恰有历史日被重建，
                            // 本次结果与 key 已不对应，放弃写入避免污染缓存。
                            match cold_db
                                .get_unified_days_materialization_fingerprint(&cold_dates)
                            {
                                Ok((count, max_at))
                                    if count == key.state_row_count
                                        && max_at == key.max_materialized_at =>
                                {
                                    perf_log(
                                        "merge_cold_cache_store",
                                        format!(
                                            "range={range_start}..{range_end} dates={} facts={}",
                                            cold_dates.len(),
                                            facts.len(),
                                        ),
                                    );
                                    store_cold_facts_cache(key, facts.clone());
                                }
                                _ => {
                                    perf_log(
                                        "merge_cold_cache_skip",
                                        format!(
                                            "range={range_start}..{range_end} dates={} reason=fingerprint_changed_during_read",
                                            cold_dates.len(),
                                        ),
                                    );
                                }
                            }
                        }
                        facts
                    }
                };
                let cold_facts_count = cold_facts.len();

                let filter_started_at = Instant::now();
                // 缓存数据经 Arc 共享，不能原地 retain。这里选 filter+cloned
                // collect 而非先整体 Vec::clone 再 retain：前者只克隆命中范围
                // 内的事实，任何保留率下克隆元素数都 ≤ 后者（后者恒克隆全段再
                // 丢弃落选项），无需按保留率分支。
                let merged: Vec<MergedRequestFact> = cold_facts
                    .iter()
                    .filter(|fact| {
                        fact.timestamp_sec >= range_start
                            && fact.timestamp_sec < range_end
                            && crate::unified_usage::matches_source_filter(
                                fact,
                                &cold_source_filter,
                            )
                            && (include_errors
                                || fact.status_code.map(|code| code < 300).unwrap_or(true))
                    })
                    .cloned()
                    .collect();
                let filtered_cold_facts_count = merged.len();
                perf_log(
                    "merge_cold_filter",
                    format!(
                        "range={range_start}..{range_end} before={} after={} elapsed_ms={}",
                        cold_facts_count,
                        filtered_cold_facts_count,
                        filter_started_at.elapsed().as_millis(),
                    ),
                );
                Ok::<(Vec<MergedRequestFact>, usize), String>((merged, filtered_cold_facts_count))
            })
            .await
            .map_err(|e| format!("Task error: {}", e))??;

        let today_start = {
            let today = crate::local_usage::LocalUsageDatabase::today_local_date_with_settings(settings);
            crate::local_usage::LocalUsageDatabase::local_date_epoch_bounds_with_settings(
                &today,
                settings,
            )
                .map(|(start, _)| start)?
        };
        let hot_start = range_start.max(today_start);
        let hot_merge_started_at = Instant::now();
        let hot_facts = if range_end > hot_start {
            Some(
                get_hot_merge_facts(
                    local_db.clone(),
                    settings,
                    include_errors,
                    &pricings,
                    &pricing_match_mode,
                    local_signature,
                    proxy_signature,
                )
                .await?,
            )
        } else {
            None
        };
        let hot_full_facts_count = hot_facts.as_ref().map(|facts| facts.len()).unwrap_or(0);
        // 热缓存同样经 Arc 共享，过滤时从借用中按需克隆命中子集。
        let filtered_hot_facts: Vec<MergedRequestFact> = hot_facts
            .as_deref()
            .map(|facts| {
                facts
                    .iter()
                    .filter(|fact| {
                        fact.timestamp_sec >= hot_start
                            && fact.timestamp_sec < range_end
                            && (include_errors
                                || fact.status_code.map(|code| code < 300).unwrap_or(true))
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let hot_facts_count = filtered_hot_facts.len();
        perf_log(
            "merge_hot_read",
            format!(
                "range={range_start}..{range_end} hot_start={} full_day_facts={} filtered_facts={} elapsed_ms={}",
                hot_start,
                hot_full_facts_count,
                hot_facts_count,
                hot_merge_started_at.elapsed().as_millis(),
            ),
        );
        merged.extend(filtered_hot_facts);

        // 大向量排序与覆盖率统计同为 CPU 重活，同样移入阻塞线程池；
        // Instant 是 Copy，计时语义与原实现一致。缓存与返回值共享同一个
        // Arc，写缓存已不再整表克隆。
        let (merged, coverage) = tauri::async_runtime::spawn_blocking(move || {
            merged.sort_by_key(|fact| fact.timestamp_ms);
            let coverage = build_coverage(&merged);
            let query_elapsed_ms = query_started_at.elapsed().as_millis();
            let merged = Arc::new(merged);
            store_merge_cache(cache_key, merged.clone(), &coverage);
            perf_log(
                "merge_cache_store",
                format!(
                    "range={range_start}..{range_end} cold_days={} cold_facts={} hot_facts={} facts={} query_ms={} merge_ms={} total_ms={}",
                    history_dates_count,
                    filtered_cold_facts_count,
                    hot_facts_count,
                    merged.len(),
                    query_elapsed_ms,
                    history_materialize_elapsed_ms,
                    overall_started_at.elapsed().as_millis(),
                ),
            );
            (merged, coverage)
        })
        .await
        .map_err(|e| format!("Task error: {}", e))?;
        Ok::<(Arc<Vec<MergedRequestFact>>, MergedCoverage), String>((merged, coverage))
    }
    .await;
    compute_result
}

/// 返回 Arc 包裹的事实向量：命中缓存时只做指针拷贝，调用方将 facts 当只读
/// 切片使用（&facts 经 Deref 即 &Vec / &[..]），确需所有权时显式 clone。
pub async fn get_merged_request_facts(
    settings: &AppSettings,
    start_epoch: Option<i64>,
    end_epoch: Option<i64>,
    include_errors: bool,
) -> Result<(Arc<Vec<MergedRequestFact>>, MergedCoverage), String> {
    // ensure_local_usage_synced 内部有 condvar 等待与全盘会话文件扫描，
    // 是最典型的同步阻塞源；经 spawn_blocking 执行避免占住 async 工作线程。
    let local_db =
        tauri::async_runtime::spawn_blocking(crate::local_usage::ensure_local_usage_synced)
            .await
            .map_err(|e| format!("Task error: {}", e))??;
    get_merged_request_facts_with_db(local_db, settings, start_epoch, end_epoch, include_errors)
        .await
}

pub async fn get_merged_request_facts_no_sync(
    settings: &AppSettings,
    start_epoch: Option<i64>,
    end_epoch: Option<i64>,
    include_errors: bool,
) -> Result<(Arc<Vec<MergedRequestFact>>, MergedCoverage), String> {
    let local_db = crate::local_usage::get_local_usage_db()?;
    get_merged_request_facts_with_db(local_db, settings, start_epoch, end_epoch, include_errors)
        .await
}

pub async fn get_merged_sessions(
    settings: &AppSettings,
    limit: i64,
    offset: i64,
) -> Result<Vec<SessionStats>, String> {
    let started_at = Instant::now();
    let now_sec = chrono::Utc::now().timestamp();
    let include_errors = settings.proxy.include_error_requests;
    let tool_filter = settings.client_tools.build_filter();
    let source_filter = settings.source_aware.build_filter();
    let mut pricings = settings.model_pricing.pricings.clone();
    if let Ok(db) = crate::proxy::ProxyDatabase::new() {
        if let Ok(db_pricings) = db.get_all_model_pricings() {
            pricings.extend(db_pricings);
        }
    }
    // 同上：全盘扫描同步放阻塞线程池执行，避免拖慢并发命令。
    let local_db =
        tauri::async_runtime::spawn_blocking(crate::local_usage::ensure_local_usage_synced)
            .await
            .map_err(|e| format!("Task error: {}", e))??;
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
    let unresolved_reasonix_requests_by_session =
        count_unresolved_reasonix_requests_by_session(&facts, &local_sessions);
    let meta_by_id: HashMap<String, SessionMeta> = local_sessions
        .into_iter()
        .map(|meta| (meta.session_id.clone(), meta))
        .collect();

    // 事实向量来自共享 Arc（只读），会话分桶只借用引用，避免整表深拷贝。
    let mut by_session: HashMap<String, Vec<&MergedRequestFact>> = HashMap::new();
    for fact in facts.iter() {
        if fact.session_id.trim().is_empty() {
            continue;
        }
        by_session
            .entry(fact.session_id.clone())
            .or_default()
            .push(fact);
    }

    let fact_backed_session_ids: HashSet<String> = by_session.keys().cloned().collect();
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
        let mut proxy_backed_requests = 0_u64;
        let mut local_only_requests = 0_u64;

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

            match fact.coverage_origin {
                CoverageOrigin::LocalOnly => local_only_requests += 1,
                CoverageOrigin::ProxyOnly
                | CoverageOrigin::MergedProxyPreferred
                | CoverageOrigin::MergedFuzzyMatched => {
                    proxy_backed_requests += 1;
                }
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
        let has_partial_status_coverage =
            has_partial_coverage(proxy_backed_requests, local_only_requests);

        let session_tool = meta.map(|m| m.tool.clone()).unwrap_or_else(|| {
            settings
                .client_tools
                .active_tool_filter
                .clone()
                .unwrap_or_default()
        });
        let unresolved_proxy_requests = unresolved_reasonix_requests_by_session
            .get(&session_id)
            .copied()
            .unwrap_or(0);
        let usage_fully_covered = session_usage_fully_covered(
            meta,
            &session_tool,
            proxy_backed_requests,
            unresolved_proxy_requests,
            now_sec,
        );

        result.push(SessionStats {
            session_id: session_id.clone(),
            tool: session_tool,
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
            usage_fully_covered,
            covered_requests: proxy_backed_requests,
            uncovered_requests: reasonix_uncovered_request_count(
                local_only_requests,
                unresolved_proxy_requests,
            ),
            cwd: meta.and_then(|m| m.cwd.clone()),
            project_name: meta.and_then(|m| m.project_name.clone()),
            project_identity: Some(
                meta.map(|m| {
                    session_project_identity(m.project_name.as_deref(), m.cwd.as_deref())
                        .to_string()
                })
                .unwrap_or_else(|| "unknown".to_string()),
            ),
            topic: meta.and_then(|m| m.topic.clone()),
            last_prompt: meta.and_then(|m| m.last_prompt.clone()),
            session_name: meta.and_then(|m| m.session_name.clone()),
            scope: meta.and_then(|m| m.scope.clone()),
            wsl_distro: meta.and_then(|m| wsl_distro_from_path(&m.file_path)),
        });
    }

    if metadata_only_sessions_allowed(&source_filter) {
        for meta in meta_by_id.values() {
            if fact_backed_session_ids.contains(&meta.session_id) {
                continue;
            }
            result.push(build_metadata_only_session_stats(meta, now_sec));
        }
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
    let now_sec = chrono::Utc::now().timestamp();
    let include_errors = settings.proxy.include_error_requests;
    let tool_filter = settings.client_tools.build_filter();
    let source_filter = settings.source_aware.build_filter();
    let mut pricings = settings.model_pricing.pricings.clone();
    if let Ok(db) = crate::proxy::ProxyDatabase::new() {
        if let Ok(db_pricings) = db.get_all_model_pricings() {
            pricings.extend(db_pricings);
        }
    }
    // 同上：全盘扫描同步放阻塞线程池执行，避免拖慢并发命令。
    let local_db =
        tauri::async_runtime::spawn_blocking(crate::local_usage::ensure_local_usage_synced)
            .await
            .map_err(|e| format!("Task error: {}", e))??;
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

    let mut local_sessions = local_db.get_all_sessions(&tool_filter)?;
    local_sessions.extend(local_db.get_remote_sessions(&tool_filter)?);
    let local_sessions_by_id: HashMap<String, SessionMeta> = local_sessions
        .iter()
        .cloned()
        .map(|meta| (meta.session_id.clone(), meta))
        .collect();

    let (facts, _) = get_merged_request_facts(settings, None, None, include_errors).await?;
    let facts_count = facts.len();
    let unresolved_reasonix_requests_by_session =
        count_unresolved_reasonix_requests_by_session(&facts, &local_sessions);
    let mut map: HashMap<String, ProjectAggregate> = HashMap::new();
    let mut fact_backed_project_session_ids: HashSet<String> = HashSet::new();
    let mut proxy_backed_requests_by_session: HashMap<String, u64> = HashMap::new();

    // 事实向量来自共享 Arc（只读），聚合循环仅读取字段并按需 clone 字符串，
    // 借用遍历即可，无需事实所有权。
    for fact in facts.iter() {
        let descriptor = project_descriptor_for_fact(fact);
        let entry = map
            .entry(descriptor.key.clone())
            .or_insert_with(|| ProjectAggregate {
                stats: ProjectStats {
                    name: descriptor.name.clone(),
                    project_key: Some(descriptor.key.clone()),
                    project_identity: Some(descriptor.identity.clone()),
                    project_path: descriptor.path.clone(),
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
            entry.stats.project_identity = Some(descriptor.identity.clone());
        }

        let request_count = fact.request_count.max(1);
        entry.stats.total_input_tokens += fact.input_tokens;
        entry.stats.total_output_tokens += fact.output_tokens;
        entry.stats.total_cache_create_tokens += fact.cache_create_tokens;
        entry.stats.total_cache_read_tokens += fact.cache_read_tokens;
        entry.stats.total_cost += fact.estimated_cost;
        entry.stats.request_count += request_count;
        entry.stats.last_active = entry.stats.last_active.max(fact.timestamp_sec);
        if !fact.session_id.trim().is_empty() {
            fact_backed_project_session_ids.insert(fact.session_id.clone());
            entry.sessions.insert(fact.session_id.clone());
            entry
                .tool_sessions
                .entry(fact.tool.clone())
                .or_default()
                .insert(fact.session_id.clone());
            if fact.tool == "reasonix"
                && matches!(
                    fact.coverage_origin,
                    CoverageOrigin::ProxyOnly | CoverageOrigin::MergedProxyPreferred
                )
            {
                *proxy_backed_requests_by_session
                    .entry(fact.session_id.clone())
                    .or_default() += request_count;
            }
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
                    covered_requests: 0,
                    uncovered_requests: 0,
                    ..Default::default()
                });
                entry.stats.tool_breakdown.last_mut().unwrap()
            }
        };
        entry.stats.covered_requests += request_count;
        tool_stats.total_input_tokens += fact.input_tokens;
        tool_stats.total_output_tokens += fact.output_tokens;
        tool_stats.total_cache_create_tokens += fact.cache_create_tokens;
        tool_stats.total_cache_read_tokens += fact.cache_read_tokens;
        tool_stats.total_cost += fact.estimated_cost;
        tool_stats.request_count += request_count;
        tool_stats.covered_requests += request_count;
        tool_stats.last_active = tool_stats.last_active.max(fact.timestamp_sec);
    }

    if metadata_only_sessions_allowed(&source_filter) {
        for meta in &local_sessions {
            if fact_backed_project_session_ids.contains(&meta.session_id) {
                continue;
            }
            merge_metadata_only_project(&mut map, meta);
        }
    }

    let mut projects: Vec<ProjectStats> = map
        .into_values()
        .map(|mut aggregate| {
            aggregate.stats.session_count = aggregate.sessions.len() as u64;
            let mut session_ids: Vec<&String> = aggregate.sessions.iter().collect();
            session_ids.sort();
            aggregate.stats.wsl_distros = session_ids
                .into_iter()
                .filter_map(|session_id| local_sessions_by_id.get(session_id))
                .filter_map(|meta| wsl_distro_from_path(&meta.file_path))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            aggregate.stats.wsl_distro = aggregate.stats.wsl_distros.first().cloned();
            let has_unresolved_reasonix_tool_rows =
                aggregate.stats.tool_breakdown.iter().any(|tool| {
                    tool.tool == "reasonix"
                        && tool.request_count > 0
                        && aggregate
                            .tool_sessions
                            .get(&tool.tool)
                            .map(|sessions| sessions.is_empty())
                            .unwrap_or(true)
                });
            let reasonix_session_ids: Vec<&String> = aggregate
                .sessions
                .iter()
                .filter(|session_id| {
                    local_sessions_by_id
                        .get(*session_id)
                        .map(|meta| meta.tool == "reasonix")
                        .unwrap_or(false)
                })
                .collect();
            let has_reasonix_sessions =
                !reasonix_session_ids.is_empty() || has_unresolved_reasonix_tool_rows;
            aggregate.stats.usage_fully_covered = if has_reasonix_sessions {
                !has_unresolved_reasonix_tool_rows
                    && reasonix_session_ids.iter().all(|session_id| {
                        local_sessions_by_id
                            .get(*session_id)
                            .map(|meta| {
                                !session_has_reasonix_coverage_gap(
                                    meta,
                                    proxy_backed_requests_by_session
                                        .get(*session_id)
                                        .copied()
                                        .unwrap_or(0),
                                    unresolved_reasonix_requests_by_session
                                        .get(*session_id)
                                        .copied()
                                        .unwrap_or(0),
                                    now_sec,
                                )
                            })
                            .unwrap_or(true)
                    })
            } else {
                true
            };
            aggregate.stats.uncovered_requests = aggregate
                .sessions
                .iter()
                .filter(|session_id| {
                    local_sessions_by_id
                        .get(*session_id)
                        .map(|meta| meta.tool == "reasonix")
                        .unwrap_or(false)
                })
                .map(|session_id| {
                    unresolved_reasonix_requests_by_session
                        .get(session_id)
                        .copied()
                        .unwrap_or(0)
                })
                .sum();
            for tool_stats in &mut aggregate.stats.tool_breakdown {
                tool_stats.session_count = aggregate
                    .tool_sessions
                    .get(&tool_stats.tool)
                    .map(|sessions| sessions.len() as u64)
                    .unwrap_or(0);
                let sessions_for_tool = aggregate.tool_sessions.get(&tool_stats.tool);
                tool_stats.usage_fully_covered = if tool_stats.tool == "reasonix"
                    && tool_stats.request_count > 0
                    && sessions_for_tool
                        .map(|sessions| sessions.is_empty())
                        .unwrap_or(true)
                {
                    false
                } else {
                    sessions_for_tool
                        .map(|sessions| {
                            sessions.iter().all(|session_id| {
                                local_sessions_by_id
                                    .get(session_id)
                                    .map(|meta| {
                                        !session_has_reasonix_coverage_gap(
                                            meta,
                                            proxy_backed_requests_by_session
                                                .get(session_id)
                                                .copied()
                                                .unwrap_or(0),
                                            unresolved_reasonix_requests_by_session
                                                .get(session_id)
                                                .copied()
                                                .unwrap_or(0),
                                            now_sec,
                                        )
                                    })
                                    .unwrap_or(true)
                            })
                        })
                        .unwrap_or(true)
                };
                tool_stats.uncovered_requests = sessions_for_tool
                    .map(|sessions| {
                        sessions
                            .iter()
                            .filter(|session_id| {
                                local_sessions_by_id
                                    .get(*session_id)
                                    .map(|meta| meta.tool == "reasonix")
                                    .unwrap_or(false)
                            })
                            .map(|session_id| {
                                unresolved_reasonix_requests_by_session
                                    .get(session_id)
                                    .copied()
                                    .unwrap_or(0)
                            })
                            .sum()
                    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_local_signature() -> crate::local_usage::LocalMergeCacheSignature {
        crate::local_usage::LocalMergeCacheSignature {
            local_request_count: 1,
            local_max_sync_version: 1,
            local_max_timestamp: 1,
            remote_request_count: 0,
            remote_max_export_seq: 0,
            remote_max_timestamp: 0,
            local_session_max_updated_at: 0,
            remote_session_max_imported_at: 0,
            unified_materialization_invalidation_version: 1,
        }
    }

    fn sample_settings(mode: &str) -> AppSettings {
        let mut settings = AppSettings::default();
        settings.day_boundary_mode = mode.to_string();
        settings
    }

    #[test]
    fn open_ended_range_end_aligns_to_next_minute_boundary() {
        let now = 1_750_000_123; // 非整分钟时刻
        let next_minute = ((now / 60) + 1) * 60;

        // end == now（统计页传当前秒）与 end == now + 1（概览传 now+1）
        // 都应归一化到同一个下一分钟边界，保证同一分钟内缓存 key 稳定。
        assert_eq!(normalize_open_ended_range_end(now, now), next_minute);
        assert_eq!(normalize_open_ended_range_end(now + 1, now), next_minute);
        // 远未来上界（如 i64::MAX 开放区间）同样收敛到下一分钟边界。
        assert_eq!(normalize_open_ended_range_end(i64::MAX, now), next_minute);
        // 归一化结果严格大于 now：now 之后的事实尚不存在，不会引入额外数据。
        assert!(next_minute > now);

        // now 恰好落在整分钟边界时，仍上调到"下一个"边界而非原地保留。
        let aligned_now = 1_750_000_200; // 整分钟
        assert_eq!(
            normalize_open_ended_range_end(aligned_now, aligned_now),
            aligned_now + 60
        );
    }

    #[test]
    fn historical_range_end_is_left_untouched() {
        let now = 1_750_000_123;
        // 历史精确范围（end < now）必须原样返回，不能改变查询语义。
        assert_eq!(normalize_open_ended_range_end(now - 1, now), now - 1);
        assert_eq!(normalize_open_ended_range_end(0, now), 0);
        assert_eq!(
            normalize_open_ended_range_end(1_700_000_000, now),
            1_700_000_000
        );
    }

    #[test]
    fn merge_cache_key_includes_day_boundary_mode() {
        let standard = sample_settings("standard");
        let night_owl = sample_settings("night_owl");
        let source_filter = standard.source_aware.build_filter();
        let tool_filter = standard.client_tools.build_filter();
        let local_signature = sample_local_signature();

        let standard_key = build_merge_cache_key(MergeCacheKeyParts {
            settings: &standard,
            range_start: 100,
            range_end: 200,
            include_errors: true,
            source_filter: &source_filter,
            tool_filter: &tool_filter,
            local_signature,
            proxy_signature: None,
            pricings: &[],
        });
        let night_owl_key = build_merge_cache_key(MergeCacheKeyParts {
            settings: &night_owl,
            range_start: 100,
            range_end: 200,
            include_errors: true,
            source_filter: &source_filter,
            tool_filter: &tool_filter,
            local_signature,
            proxy_signature: None,
            pricings: &[],
        });

        assert_ne!(standard_key, night_owl_key);
        assert_eq!(standard_key.day_boundary_mode, "standard");
        assert_eq!(night_owl_key.day_boundary_mode, "night_owl");
    }

    #[test]
    fn clear_runtime_caches_removes_all_cached_entries() {
        clear_runtime_caches();
        let settings = sample_settings("standard");
        let source_filter = settings.source_aware.build_filter();
        let tool_filter = settings.client_tools.build_filter();
        let local_signature = sample_local_signature();
        let merge_key = build_merge_cache_key(MergeCacheKeyParts {
            settings: &settings,
            range_start: 10,
            range_end: 20,
            include_errors: true,
            source_filter: &source_filter,
            tool_filter: &tool_filter,
            local_signature,
            proxy_signature: None,
            pricings: &[],
        });
        let hot_key = build_hot_merge_cache_key(
            MergeCacheKeyParts {
                settings: &settings,
                range_start: 10,
                range_end: 20,
                include_errors: true,
                source_filter: &source_filter,
                tool_filter: &tool_filter,
                local_signature,
                proxy_signature: None,
                pricings: &[],
            },
            "2026-06-11".to_string(),
        );
        let history_key =
            build_history_materialization_cache_key(HistoryMaterializationCacheKeyParts {
                settings: &settings,
                range_start: 10,
                range_end: 20,
                pricing_match_mode: &settings.model_pricing.match_mode,
                pricings: &[],
                local_signature,
                proxy_signature: None,
            });
        let session_key = SessionDerivedCacheKey {
            merge_key: merge_key.clone(),
            limit: 10,
            offset: 0,
        };
        let project_key = ProjectDerivedCacheKey {
            merge_key: merge_key.clone(),
        };

        store_merge_cache(
            merge_key.clone(),
            Arc::new(Vec::new()),
            &MergedCoverage::default(),
        );
        store_hot_merge_cache(hot_key.clone(), Arc::new(Vec::new()));
        store_history_materialization_cache(history_key.clone(), &["2026-06-11".to_string()]);
        store_sessions_cache(session_key.clone(), &[]);
        store_projects_cache(project_key.clone(), &[]);

        assert!(lookup_merge_cache(&merge_key).is_some());
        assert!(lookup_hot_merge_cache(&hot_key).is_some());
        assert!(lookup_history_materialization_cache(&history_key).is_some());
        assert!(lookup_sessions_cache(&session_key).is_some());
        assert!(lookup_projects_cache(&project_key).is_some());

        clear_runtime_caches();

        assert!(lookup_merge_cache(&merge_key).is_none());
        assert!(lookup_hot_merge_cache(&hot_key).is_none());
        assert!(lookup_history_materialization_cache(&history_key).is_none());
        assert!(lookup_sessions_cache(&session_key).is_none());
        assert!(lookup_projects_cache(&project_key).is_none());
    }

    #[test]
    fn cold_facts_cache_key_ignores_date_order_but_tracks_content() {
        let dates = vec![
            "2026-06-01".to_string(),
            "2026-06-02".to_string(),
            "2026-06-03".to_string(),
        ];
        let shuffled = vec![
            "2026-06-03".to_string(),
            "2026-06-01".to_string(),
            "2026-06-02".to_string(),
        ];
        let base = build_cold_facts_cache_key(&dates, &ToolFilter::All, "standard", 3, 1_000);

        // 同一批日期不同顺序 → key 相同（缓存可复用）。
        assert_eq!(
            base,
            build_cold_facts_cache_key(&shuffled, &ToolFilter::All, "standard", 3, 1_000)
        );

        // 首末日与数量相同但中间日期不同 → dates_fingerprint 区分。
        let different_middle = vec![
            "2026-06-01".to_string(),
            "2026-06-02T".to_string(),
            "2026-06-03".to_string(),
        ];
        assert_ne!(
            base,
            build_cold_facts_cache_key(&different_middle, &ToolFilter::All, "standard", 3, 1_000)
        );

        // 任一历史日重建（max_materialized_at 变化）或状态行数变化 → key 不同。
        assert_ne!(
            base,
            build_cold_facts_cache_key(&dates, &ToolFilter::All, "standard", 3, 1_001)
        );
        assert_ne!(
            base,
            build_cold_facts_cache_key(&dates, &ToolFilter::All, "standard", 2, 1_000)
        );

        // tool_filter 与 day_boundary_mode 参与 key。
        assert_ne!(
            base,
            build_cold_facts_cache_key(
                &dates,
                &ToolFilter::Tool("claude_code".to_string()),
                "standard",
                3,
                1_000
            )
        );
        assert_ne!(
            base,
            build_cold_facts_cache_key(&dates, &ToolFilter::All, "night_owl", 3, 1_000)
        );
    }

    #[test]
    fn cold_facts_cache_round_trips_and_respects_capacity() {
        clear_runtime_caches();
        let dates = vec!["2026-06-01".to_string()];
        let key = build_cold_facts_cache_key(&dates, &ToolFilter::All, "standard", 1, 500);
        assert!(lookup_cold_facts_cache(&key).is_none());
        store_cold_facts_cache(key.clone(), Arc::new(Vec::new()));
        assert!(matches!(
            lookup_cold_facts_cache(&key),
            Some(facts) if facts.is_empty()
        ));

        // 超出容量后最旧条目被淘汰。
        for offset in 0..COLD_MERGED_FACTS_CACHE_CAPACITY as i64 {
            let evicting_key =
                build_cold_facts_cache_key(&dates, &ToolFilter::All, "standard", 1, 600 + offset);
            store_cold_facts_cache(evicting_key, Arc::new(Vec::new()));
        }
        assert!(lookup_cold_facts_cache(&key).is_none());
        clear_runtime_caches();
    }

    #[test]
    fn history_materialization_cache_key_tracks_signatures() {
        clear_runtime_caches();
        let settings = sample_settings("standard");
        let key = build_history_materialization_cache_key(HistoryMaterializationCacheKeyParts {
            settings: &settings,
            range_start: 10,
            range_end: 20,
            pricing_match_mode: &settings.model_pricing.match_mode,
            pricings: &[],
            local_signature: sample_local_signature(),
            proxy_signature: None,
        });
        store_history_materialization_cache(key.clone(), &["2026-06-11".to_string()]);

        assert_eq!(
            lookup_history_materialization_cache(&key),
            Some(vec!["2026-06-11".to_string()])
        );

        let changed_signature = crate::local_usage::LocalMergeCacheSignature {
            unified_materialization_invalidation_version: 2,
            ..sample_local_signature()
        };
        let changed_key =
            build_history_materialization_cache_key(HistoryMaterializationCacheKeyParts {
                settings: &settings,
                range_start: 10,
                range_end: 20,
                pricing_match_mode: &settings.model_pricing.match_mode,
                pricings: &[],
                local_signature: changed_signature,
                proxy_signature: None,
            });
        assert!(lookup_history_materialization_cache(&changed_key).is_none());
    }
}
