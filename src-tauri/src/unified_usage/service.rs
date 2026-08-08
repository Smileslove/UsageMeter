use super::aggregation_support::{
    build_metadata_only_session_stats, merge_metadata_only_project,
    reasonix_uncovered_request_count, session_has_reasonix_coverage_gap,
    session_usage_fully_covered, ProjectAggregate,
};
pub(crate) use super::cold_facts_support::ColdFactsShardCache;
use super::cold_facts_support::{
    ColdConcatMemo, ColdDayCacheEntry, ColdFactsLoad, COLD_CONCAT_MEMO_CAPACITY,
    COLD_FACTS_SHARD_CAPACITY,
};
use super::derived_support::{
    metadata_only_sessions_allowed, project_descriptor_for_fact, session_project_identity,
};
use super::inflight_support::acquire_inflight_key;
use super::match_support::{
    build_local_meta_index, build_message_to_session_index, local_tool_matches,
    request_key_for_local, session_meta_matches,
};
use super::materialization_support::{
    build_materialization_state, materialization_state_matches, MaterializationStateBuildContext,
};
use super::merge_engine::{
    build_coverage, merge_realtime_facts, request_key_for_fact, RealtimeMergeInput,
};
use super::query_support::{
    cache_key_for_source_filter, cache_key_for_tool_filter, fingerprint_pricings,
    normalize_open_ended_range_end, normalize_range_bounds, normalized_day_boundary_mode,
};
#[cfg(test)]
use super::reasonix_support::{
    build_reasonix_proxy_coverage_by_session, build_reasonix_telemetry_residual,
    reasonix_explicit_cost_usd,
};
use super::reasonix_support::{
    count_unresolved_reasonix_requests_by_session, reasonix_coverage_query_bounds,
};
use super::types::{has_partial_coverage, CoverageOrigin, MergedCoverage, MergedRequestFact};
use crate::models::{AppSettings, ToolFilter, UsageQueryFilter};
use crate::proxy::ProxyMergeCacheSignature;
use crate::proxy::{
    CodexConfigManager, CodexSourceRegistry, ProjectStats, ProjectToolStats, ProxyDatabase,
    SessionStats, UsageRecord,
};
use crate::session::{wsl_distro_from_path, SessionMeta};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

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
}

#[derive(Debug, Clone)]
struct SessionDerivedCacheEntry {
    key: SessionDerivedCacheKey,
    // 完整排序后的会话列表按 merge_key 缓存，分页只在切片范围内 clone。
    sessions: Arc<Vec<SessionStats>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SessionDetailCacheKey {
    merge_key: MergeCacheKey,
    session_id: String,
}

#[derive(Debug, Clone)]
struct SessionDetailCacheEntry {
    key: SessionDetailCacheKey,
    detail: Option<SessionStats>,
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
static COLD_FACTS_SHARD_CACHE: OnceLock<Mutex<ColdFactsShardCache>> = OnceLock::new();
static HISTORY_MATERIALIZATION_CACHE: OnceLock<Mutex<Vec<HistoryMaterializationCacheEntry>>> =
    OnceLock::new();
static MERGED_SESSIONS_CACHE: OnceLock<Mutex<Vec<SessionDerivedCacheEntry>>> = OnceLock::new();
static MERGED_SESSION_DETAILS_CACHE: OnceLock<Mutex<Vec<SessionDetailCacheEntry>>> =
    OnceLock::new();
static MERGED_PROJECTS_CACHE: OnceLock<Mutex<Vec<ProjectDerivedCacheEntry>>> = OnceLock::new();
// 概览 6 个时间窗口 + 统计页 7 个预设可能同时驻留，8 条会互相挤出，提高到 16。
const MERGED_REQUEST_FACTS_CACHE_CAPACITY: usize = 16;
const HOT_MERGED_REQUEST_FACTS_CACHE_CAPACITY: usize = 4;
const HISTORY_MATERIALIZATION_CACHE_CAPACITY: usize = 8;
const MERGED_SESSIONS_CACHE_CAPACITY: usize = 6;
const MERGED_SESSION_DETAILS_CACHE_CAPACITY: usize = 32;
const MERGED_PROJECTS_CACHE_CAPACITY: usize = 6;

fn merge_cache() -> &'static Mutex<Vec<MergeCacheEntry>> {
    MERGED_REQUEST_FACTS_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn merged_sessions_cache() -> &'static Mutex<Vec<SessionDerivedCacheEntry>> {
    MERGED_SESSIONS_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn merged_session_details_cache() -> &'static Mutex<Vec<SessionDetailCacheEntry>> {
    MERGED_SESSION_DETAILS_CACHE.get_or_init(|| Mutex::new(Vec::new()))
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

fn cold_facts_shard_cache() -> &'static Mutex<ColdFactsShardCache> {
    COLD_FACTS_SHARD_CACHE.get_or_init(|| Mutex::new(ColdFactsShardCache::new()))
}

pub(crate) fn clear_runtime_caches() {
    merge_cache().lock().unwrap().clear();
    hot_merge_cache().lock().unwrap().clear();
    {
        let mut guard = cold_facts_shard_cache().lock().unwrap();
        *guard = ColdFactsShardCache::new();
    }
    history_materialization_cache().lock().unwrap().clear();
    merged_sessions_cache().lock().unwrap().clear();
    merged_session_details_cache().lock().unwrap().clear();
    merged_projects_cache().lock().unwrap().clear();
}

#[cfg(test)]
pub(crate) fn seed_runtime_merge_cache_for_test() {
    clear_runtime_caches();
    let settings = AppSettings::default();
    let source_filter = settings.source_aware.build_filter();
    let tool_filter = settings.client_tools.build_filter();
    let local_signature = crate::local_usage::LocalMergeCacheSignature {
        merge_cache_generation: 1,
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

/// 冷段拼接 memo 指纹：日期集合 + 各日 materialized_at（+ 日界模式）。
///
/// stamps 需按日期升序传入；任何一天被重物化、日期集合增删（换日/范围变化）
/// 都会改变指纹，从而自然失效对应 memo。
fn cold_concat_fingerprint(day_boundary_mode: &str, stamps: &[(String, i64)]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    day_boundary_mode.hash(&mut hasher);
    for (date, materialized_at) in stamps {
        date.hash(&mut hasher);
        materialized_at.hash(&mut hasher);
    }
    hasher.finish()
}

/// MergedRequestFact 的 tool 过滤，语义与 get_unified_facts_for_dates 的 SQL
/// tool 子句一致（空 Tool 视同不过滤、空 AnyOf 不匹配任何行）。冷段分片统一
/// 存全量数据，tool 过滤下放到拼接后的内存线性过滤。
fn fact_tool_matches(fact: &MergedRequestFact, tool_filter: &ToolFilter) -> bool {
    match tool_filter {
        ToolFilter::All => true,
        ToolFilter::Tool(tool) if tool.trim().is_empty() => true,
        ToolFilter::Tool(tool) => fact.tool == *tool,
        ToolFilter::AnyOf(tools) => tools.contains(&fact.tool),
    }
}

/// 冷段按日分片读取：逐日核对 materialized_at，未变的天直接复用分片，变化/
/// 缺失的天一次批量重读后写回分片，最终按日期升序拼接为平铺向量。
///
/// cache 以参数注入而非直接取全局单例，便于测试用独立实例隔离并发干扰；
/// 生产路径统一传 cold_facts_shard_cache()。
pub(crate) fn load_cold_facts_via_shards(
    cache: &Mutex<ColdFactsShardCache>,
    db: &crate::local_usage::LocalUsageDatabase,
    local_dates: &[String],
    day_boundary_mode: &str,
) -> Result<ColdFactsLoad, String> {
    if local_dates.is_empty() {
        return Ok(ColdFactsLoad {
            facts: Arc::new(Vec::new()),
            days_cached: 0,
            days_fetched: 0,
            memo_hit: false,
            fallback_uncached: false,
            stale_days_skipped: 0,
        });
    }

    // a. 取逐日物化时间戳（state 表按 local_date 主键的一次轻量点查）。
    //    状态行数与请求日期数不一致说明某日状态异常（如并发失效清理），
    //    此时跳过分片缓存整段直读数据库，结果不写回任何缓存。
    let mut stamps = db.get_unified_days_materialization_stamps(local_dates)?;
    if stamps.len() != local_dates.len() {
        let facts = db.get_unified_facts_for_dates(local_dates, &ToolFilter::All)?;
        return Ok(ColdFactsLoad {
            facts: Arc::new(facts),
            days_cached: 0,
            days_fetched: local_dates.len(),
            memo_hit: false,
            fallback_uncached: true,
            stale_days_skipped: 0,
        });
    }
    stamps.sort_by(|a, b| a.0.cmp(&b.0));
    let concat_fingerprint = cold_concat_fingerprint(day_boundary_mode, &stamps);
    let stamp_by_date: HashMap<&str, i64> = stamps
        .iter()
        .map(|(date, materialized_at)| (date.as_str(), *materialized_at))
        .collect();

    // b. 逐日核对分片：代次（日界模式）不匹配先整体清空重建；分片存在且
    //    materialized_at 一致 → 复用，否则记入 missing 待批量重读。
    let mut cached: HashMap<String, Arc<Vec<MergedRequestFact>>> = HashMap::new();
    let mut missing: Vec<String> = Vec::new();
    {
        let mut guard = cache.lock().unwrap();
        if guard.day_boundary_mode != day_boundary_mode {
            *guard = ColdFactsShardCache::new();
            guard.day_boundary_mode = day_boundary_mode.to_string();
        }
        // memo 命中：同一日期集合、各日 materialized_at 均未变时直接复用
        // 最近一次拼接结果，连 concat 也省掉。
        if let Some(idx) = guard
            .concat_memos
            .iter()
            .position(|memo| memo.fingerprint == concat_fingerprint)
        {
            let memo = guard.concat_memos.remove(idx);
            let facts = memo.facts.clone();
            guard.concat_memos.insert(0, memo);
            return Ok(ColdFactsLoad {
                facts,
                days_cached: stamps.len(),
                days_fetched: 0,
                memo_hit: true,
                fallback_uncached: false,
                stale_days_skipped: 0,
            });
        }
        for (date, materialized_at) in &stamps {
            match guard.shards.get(date) {
                Some(entry) if entry.materialized_at == *materialized_at => {
                    cached.insert(date.clone(), entry.facts.clone());
                }
                _ => missing.push(date.clone()),
            }
        }
    }

    // c. 一次批量读缺失日（按日分组返回）；SQLite 读取在锁外执行，避免
    //    阻塞其他并发查询路径对分片缓存的命中。
    let mut fetched: HashMap<String, Arc<Vec<MergedRequestFact>>> = HashMap::new();
    let mut stale_days_skipped = 0usize;
    if !missing.is_empty() {
        let mut grouped = db.get_unified_facts_by_date(&missing)?;
        // 写回前复核时间戳：若读事实期间恰有历史日被重建，本次结果与读取
        // 前的 stamp 已不对应，跳过该日分片写入避免污染缓存（返回值仍用
        // 本次读取结果，与旧实现语义一致）。
        let recheck: HashMap<String, i64> = db
            .get_unified_days_materialization_stamps(&missing)?
            .into_iter()
            .collect();
        let mut guard = cache.lock().unwrap();
        // 读取期间可能有设置变更切换代次；代次已变时放弃全部写回。
        let generation_ok = guard.day_boundary_mode == day_boundary_mode;
        for date in &missing {
            let facts = Arc::new(grouped.remove(date).unwrap_or_default());
            let expected_at = stamp_by_date.get(date.as_str()).copied();
            let still_valid = expected_at.is_some() && recheck.get(date).copied() == expected_at;
            if generation_ok && still_valid {
                guard.shards.insert(
                    date.clone(),
                    ColdDayCacheEntry {
                        materialized_at: expected_at.unwrap_or(0),
                        facts: facts.clone(),
                    },
                );
            } else {
                stale_days_skipped += 1;
            }
            fetched.insert(date.clone(), facts);
        }
        // 分片总量裁剪：超出上限按日期最旧淘汰，防无界增长。
        if guard.shards.len() > COLD_FACTS_SHARD_CAPACITY {
            let mut shard_dates: Vec<String> = guard.shards.keys().cloned().collect();
            shard_dates.sort();
            let excess = guard.shards.len() - COLD_FACTS_SHARD_CAPACITY;
            for stale_date in shard_dates.into_iter().take(excess) {
                guard.shards.remove(&stale_date);
            }
        }
    }

    // d. 按日期升序把各分片 concat 成平铺向量（各日内部已按 timestamp_ms
    //    排序；调用方随后与热段合并时还会整体重排，这里保持近有序即可）。
    let shard_for_date = |date: &String| cached.get(date).or_else(|| fetched.get(date));
    let total: usize = stamps
        .iter()
        .filter_map(|(date, _)| shard_for_date(date).map(|facts| facts.len()))
        .sum();
    let mut concat: Vec<MergedRequestFact> = Vec::with_capacity(total);
    for (date, _) in &stamps {
        if let Some(facts) = shard_for_date(date) {
            concat.extend(facts.iter().cloned());
        }
    }
    let facts = Arc::new(concat);

    // memo 只在全部分片数据与读取前 stamp 一致时写入；概览/统计/会话页会
    // 以相同范围高频调用，命中时避免对几万行事实的重复 concat。
    if stale_days_skipped == 0 {
        let mut guard = cache.lock().unwrap();
        if guard.day_boundary_mode == day_boundary_mode {
            guard
                .concat_memos
                .retain(|memo| memo.fingerprint != concat_fingerprint);
            guard.concat_memos.insert(
                0,
                ColdConcatMemo {
                    fingerprint: concat_fingerprint,
                    facts: facts.clone(),
                },
            );
            if guard.concat_memos.len() > COLD_CONCAT_MEMO_CAPACITY {
                guard.concat_memos.truncate(COLD_CONCAT_MEMO_CAPACITY);
            }
        }
    }

    Ok(ColdFactsLoad {
        facts,
        days_cached: cached.len(),
        days_fetched: missing.len(),
        memo_hit: false,
        fallback_uncached: false,
        stale_days_skipped,
    })
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

fn lookup_sessions_cache(key: &SessionDerivedCacheKey) -> Option<Arc<Vec<SessionStats>>> {
    let cache = merged_sessions_cache();
    let mut guard = cache.lock().unwrap();
    let idx = guard.iter().position(|entry| entry.key == *key)?;
    let entry = guard.remove(idx);
    let result = Arc::clone(&entry.sessions);
    guard.insert(0, entry);
    Some(result)
}

fn store_sessions_cache(key: SessionDerivedCacheKey, sessions: Arc<Vec<SessionStats>>) {
    let cache = merged_sessions_cache();
    let mut guard = cache.lock().unwrap();
    if let Some(idx) = guard.iter().position(|entry| entry.key == key) {
        guard.remove(idx);
    }
    guard.insert(0, SessionDerivedCacheEntry { key, sessions });
    if guard.len() > MERGED_SESSIONS_CACHE_CAPACITY {
        guard.truncate(MERGED_SESSIONS_CACHE_CAPACITY);
    }
}

fn lookup_session_detail_cache(key: &SessionDetailCacheKey) -> Option<Option<SessionStats>> {
    let cache = merged_session_details_cache();
    let mut guard = cache.lock().unwrap();
    let idx = guard.iter().position(|entry| entry.key == *key)?;
    let entry = guard.remove(idx);
    let result = entry.detail.clone();
    guard.insert(0, entry);
    Some(result)
}

fn store_session_detail_cache(key: SessionDetailCacheKey, detail: Option<SessionStats>) {
    let cache = merged_session_details_cache();
    let mut guard = cache.lock().unwrap();
    if let Some(idx) = guard.iter().position(|entry| entry.key == key) {
        guard.remove(idx);
    }
    guard.insert(0, SessionDetailCacheEntry { key, detail });
    if guard.len() > MERGED_SESSION_DETAILS_CACHE_CAPACITY {
        guard.truncate(MERGED_SESSION_DETAILS_CACHE_CAPACITY);
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

struct MergeRealtimeParams<'a> {
    settings: &'a AppSettings,
    start_epoch: Option<i64>,
    end_epoch: Option<i64>,
    include_errors: bool,
    pricings: &'a [crate::models::ModelPricingConfig],
    pricing_match_mode: &'a str,
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
        "merge:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        key.start_epoch,
        key.end_epoch,
        key.day_boundary_mode,
        key.include_errors,
        key.source_filter,
        key.tool_filter,
        key.pricing_match_mode,
        key.pricing_fingerprint,
        key.local_signature.merge_cache_generation,
        key.local_signature
            .unified_materialization_invalidation_version,
        key.proxy_signature
            .map(|sig| sig.merge_cache_generation.to_string())
            .unwrap_or_else(|| "none".to_string()),
    )
}

fn history_materialization_inflight_key(key: &HistoryMaterializationCacheKey) -> String {
    format!(
        "history:{}:{}:{}:{}:{}:{}:{}:{}",
        key.start_epoch,
        key.end_epoch,
        key.day_boundary_mode,
        key.pricing_match_mode,
        key.pricing_fingerprint,
        key.local_signature.merge_cache_generation,
        key.local_signature
            .unified_materialization_invalidation_version,
        key.proxy_signature
            .map(|sig| sig.merge_cache_generation.to_string())
            .unwrap_or_else(|| "none".to_string()),
    )
}

fn canonical_history_settings(settings: &AppSettings) -> AppSettings {
    let mut canonical = settings.clone();
    canonical.client_tools.active_tool_filter = None;
    canonical.source_aware.active_source_filter = None;
    canonical
}

async fn merge_realtime_range(
    local_db: Arc<crate::local_usage::LocalUsageDatabase>,
    params: MergeRealtimeParams<'_>,
) -> Result<Vec<MergedRequestFact>, String> {
    let MergeRealtimeParams {
        settings,
        start_epoch,
        end_epoch,
        include_errors,
        pricings,
        pricing_match_mode,
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
    // 工作线程。
    let (local_records, session_meta_by_id, message_to_session, codex_fallback_base_url) =
        tauri::async_runtime::spawn_blocking(move || {
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
            let mut remote_records = local_db.get_remote_request_records_in_range(
                range_start,
                range_end,
                &tool_filter,
            )?;
            remote_records
                .retain(|record| !local_request_keys.contains(&request_key_for_local(record)));
            local_records.extend(remote_records);
            local_records.retain(|record| local_tool_matches(record, &tool_filter));
            let mut seen_local_keys = HashSet::new();
            local_records.retain(|record| seen_local_keys.insert(request_key_for_local(record)));
            let session_meta_by_id = build_local_meta_index(&local_sessions);
            let message_to_session = build_message_to_session_index(&local_records);
            let codex_fallback_base_url =
                if local_records.iter().any(|record| record.tool == "codex") {
                    CodexConfigManager::new()
                        .active_source_id()
                        .and_then(|id| CodexSourceRegistry::new().get(&id))
                        .map(|handle| handle.real_base_url)
                } else {
                    None
                };
            Ok::<_, String>((
                local_records,
                session_meta_by_id,
                message_to_session,
                codex_fallback_base_url,
            ))
        })
        .await
        .map_err(|e| format!("Task error: {}", e))??;

    // 异步取数段：proxy 记录的两次取数是真正的 await 点，保持在 async 上下文；
    // 取回后的纯内存加工（session 归属回填、可见性过滤）挪入后置同步段一并
    // 下沉阻塞线程池。
    let reasonix_coverage_bounds = reasonix_coverage_query_bounds(
        session_meta_by_id.values(),
        &usage_filter.source,
        range_start,
        range_end,
    );
    let (raw_proxy_records, raw_unfiltered_proxy_records, raw_reasonix_coverage_proxy_records) =
        if let Some(proxy_db) = ProxyDatabase::get_global() {
            let records =
                fetch_proxy_records(proxy_db.as_ref(), &usage_filter, start_epoch, end_epoch)
                    .await?;
            let unfiltered = if let Some(filter) = unfiltered_usage_filter.as_ref() {
                Some(fetch_proxy_records(proxy_db.as_ref(), filter, start_epoch, end_epoch).await?)
            } else {
                None
            };
            // telemetry 是会话累计值。为了在会话结束窗口生成正确残差，覆盖基线必须
            // 包含这些会话的完整生命周期，而不只是当前查询窗口内的代理事实。查询
            // 边界由本窗口内结束的 Reasonix 会话推导，避免每次合并无界扫描代理全表。
            let coverage = if let Some((coverage_start, coverage_end)) = reasonix_coverage_bounds {
                let reasonix_filter = UsageQueryFilter {
                    source: crate::models::SourceFilter::All,
                    tool: ToolFilter::Tool("reasonix".to_string()),
                };
                fetch_proxy_records(
                    proxy_db.as_ref(),
                    &reasonix_filter,
                    Some(coverage_start),
                    Some(coverage_end),
                )
                .await?
            } else {
                Vec::new()
            };
            (records, unfiltered, coverage)
        } else {
            (Vec::new(), None, Vec::new())
        };

    // 后置同步合并段只操作内存快照，在阻塞线程池完成索引、去重与排序。
    let input = RealtimeMergeInput {
        local_records,
        session_meta_by_id,
        message_to_session,
        raw_proxy_records,
        raw_unfiltered_proxy_records,
        raw_reasonix_coverage_proxy_records,
        source_filter: usage_filter.source,
        currency_settings: settings.currency.clone(),
        range_start,
        range_end,
        include_errors,
        pricings: pricings.to_vec(),
        pricing_match_mode: pricing_match_mode.to_string(),
        codex_fallback_base_url,
    };
    let facts = tauri::async_runtime::spawn_blocking(move || merge_realtime_facts(input))
        .await
        .map_err(|e| format!("Task error: {}", e))?;
    Ok(facts)
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
        return Ok(ready_dates);
    }

    let materializable_dates: Vec<String> = enumerate_local_dates(range_start, range_end, settings)
        .into_iter()
        .filter(|date| date < &today)
        .collect();

    let mut ready_dates = Vec::with_capacity(materializable_dates.len());
    let pricing_fingerprint = fingerprint_pricings(pricings);
    let states = local_db.get_unified_days_materialization_states(&materializable_dates)?;

    for local_date in materializable_dates {
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
        let needs_rebuild = states
            .get(&local_date)
            .map(|state| {
                !materialization_state_matches(
                    state,
                    &local_snapshot,
                    proxy_snapshot,
                    pricing_fingerprint,
                    settings,
                ) || !local_db
                    .is_unified_day_fact_cache_complete(&local_date)
                    .unwrap_or(false)
            })
            .unwrap_or(true);

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
            let still_needs_rebuild = latest_state
                .as_ref()
                .map(|state| {
                    !materialization_state_matches(
                        state,
                        &latest_local_snapshot,
                        latest_proxy_snapshot,
                        pricing_fingerprint,
                        settings,
                    ) || !local_db
                        .is_unified_day_fact_cache_complete(&local_date)
                        .unwrap_or(false)
                })
                .unwrap_or(true);
            let materialize_result = async {
                if still_needs_rebuild {
                    let facts = merge_realtime_range(
                        local_db.clone(),
                        MergeRealtimeParams {
                            settings: &canonical_settings,
                            start_epoch: Some(day_start),
                            end_epoch: Some(day_end),
                            include_errors: true,
                            pricings,
                            pricing_match_mode,
                        },
                    )
                    .await?;
                    let fact_entries: Vec<(String, MergedRequestFact)> = facts
                        .iter()
                        .map(|fact| (request_key_for_fact(fact), fact.clone()))
                        .collect();
                    let max_fact_timestamp_ms = facts
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
                            fact_count: facts.len(),
                            local_snapshot: &latest_local_snapshot,
                            proxy_snapshot: latest_proxy_snapshot,
                            pricing_fingerprint,
                            max_fact_timestamp_ms,
                            materialized_at: now_ms,
                            settings,
                        }),
                    )?;
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
    let pricings = crate::proxy::ProxyDatabase::get_global()
        .and_then(|db| db.get_all_model_pricings().ok())
        .unwrap_or_default();
    let pricing_match_mode = settings.model_pricing.match_mode.clone();
    let effective_range =
        if let Some((data_start, data_end)) = combined_data_time_bounds(&local_db)? {
            let effective_start = start_epoch.max(data_start);
            // 数据库 time bounds 返回的是已存在事实的最大时间戳，而下游查询使用
            // 半开区间 [start, end)。end 必须向后扩一秒，否则最后一条事实会被排除；
            // 当唯一数据是 telemetry-only session 时还会错误形成空区间。
            let effective_end = end_epoch.min(data_end.saturating_add(1));
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
        return Ok(facts);
    }

    let facts = merge_realtime_range(
        local_db,
        MergeRealtimeParams {
            settings,
            start_epoch: Some(today_start),
            end_epoch: None,
            include_errors,
            pricings,
            pricing_match_mode,
        },
    )
    .await?;
    // 缓存与调用方共享同一个 Arc，写缓存不再整向量深拷贝。
    let facts = Arc::new(facts);
    store_hot_merge_cache(cache_key.clone(), facts.clone());
    Ok(facts)
}

async fn get_merged_request_facts_with_db(
    local_db: Arc<crate::local_usage::LocalUsageDatabase>,
    settings: &AppSettings,
    start_epoch: Option<i64>,
    end_epoch: Option<i64>,
    include_errors: bool,
) -> Result<(Arc<Vec<MergedRequestFact>>, MergedCoverage), String> {
    // Unbounded list/prewarm callers use the request-level cache retention window.
    // Explicit historical statistics ranges remain exact and can materialize older days on demand.
    let effective_start_epoch = match start_epoch {
        Some(value) => Some(value),
        None => Some(
            crate::local_usage::LocalUsageDatabase::materialized_fact_retention_cutoff_epoch(
                settings,
            )?,
        ),
    };
    let (range_start, raw_range_end) = normalize_range_bounds(effective_start_epoch, end_epoch);
    // 对开放式 range_end 做整分钟归一化（详见 normalize_open_ended_range_end），
    // 归一化后的 end 同时作为缓存 key 与事实过滤上界；再 max(range_start) 保持
    // end >= start 的不变量（未来起点的空区间仍返回空结果）。
    let range_end = normalize_open_ended_range_end(raw_range_end, chrono::Utc::now().timestamp())
        .max(range_start);
    let tool_filter = settings.client_tools.build_filter();
    let source_filter = settings.source_aware.build_filter();
    let pricings = crate::proxy::ProxyDatabase::get_global()
        .and_then(|db| db.get_all_model_pricings().ok())
        .unwrap_or_default();
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
        return Ok((facts, coverage));
    }
    let inflight_key = merge_cache_inflight_key(&cache_key);
    let _inflight_guard = acquire_inflight_key(&inflight_key).await;
    if let Some((facts, coverage)) = lookup_merge_cache(&cache_key) {
        return Ok((facts, coverage));
    }
    let compute_result = async {
        let history_ready_dates =
            if let Some((data_start, data_end)) = combined_data_time_bounds(&local_db)? {
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

        // 冷读是 SQLite 全行读取 + 反序列化，随后的范围过滤是大向量线性扫描，
        // 两者是合并路径上最重的同步阻塞段；一并移入阻塞线程池，避免占住
        // tauri async runtime 的工作线程拖慢并发命令。
        let cold_db = local_db.clone();
        let cold_dates = history_ready_dates;
        let cold_tool_filter = tool_filter.clone();
        let cold_source_filter = source_filter.clone();
        let cold_day_boundary_mode = normalized_day_boundary_mode(settings);
        let mut merged = tauri::async_runtime::spawn_blocking(move || {
            // 冷段按日分片缓存：今日新请求只抖动 local/proxy 实时签名（外层
            // 合并缓存因此失效），但不触碰历史日的物化状态。这里逐日核对
            // materialized_at，未变的天直接复用分片，单日重物化或换日时只
            // 重读发生变化/缺失的那几天；分片存全量数据，tool 过滤与
            // range/source/include_errors 一并留在命中后的线性过滤。
            let cold_load = load_cold_facts_via_shards(
                cold_facts_shard_cache(),
                &cold_db,
                &cold_dates,
                &cold_day_boundary_mode,
            )?;
            let cold_facts = cold_load.facts;

            // 缓存数据经 Arc 共享，不能原地 retain。这里选 filter+cloned
            // collect 而非先整体 Vec::clone 再 retain：前者只克隆命中范围
            // 内的事实，任何保留率下克隆元素数都 ≤ 后者（后者恒克隆全段再
            // 丢弃落选项），无需按保留率分支。
            let merged: Vec<MergedRequestFact> = cold_facts
                .iter()
                .filter(|fact| {
                    fact.timestamp_sec >= range_start
                        && fact.timestamp_sec < range_end
                        && fact_tool_matches(fact, &cold_tool_filter)
                        && crate::unified_usage::matches_source_filter(fact, &cold_source_filter)
                        && (include_errors
                            || fact.status_code.map(|code| code < 300).unwrap_or(true))
                })
                .cloned()
                .collect();
            Ok::<Vec<MergedRequestFact>, String>(merged)
        })
        .await
        .map_err(|e| format!("Task error: {}", e))??;

        let today_start = {
            let today =
                crate::local_usage::LocalUsageDatabase::today_local_date_with_settings(settings);
            crate::local_usage::LocalUsageDatabase::local_date_epoch_bounds_with_settings(
                &today, settings,
            )
            .map(|(start, _)| start)?
        };
        let hot_start = range_start.max(today_start);
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
        merged.extend(filtered_hot_facts);

        // 大向量排序与覆盖率统计同为 CPU 重活，同样移入阻塞线程池；
        // 缓存与返回值共享同一个 Arc，写缓存已不再整表克隆。
        let (merged, coverage) = tauri::async_runtime::spawn_blocking(move || {
            merged.sort_by_key(|fact| fact.timestamp_ms);
            let coverage = build_coverage(&merged);
            let merged = Arc::new(merged);
            store_merge_cache(cache_key, merged.clone(), &coverage);
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
///
/// 快照优先：不触发 ensure_local_usage_synced 全盘扫描（那会 condvar 等待
/// 整个同步），直接以当前 SQLite 数据回答；扫描由命令层丢到后台执行。
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

fn build_fact_backed_session_stats(
    session_id: &str,
    session_facts: &[&MergedRequestFact],
    meta: Option<&SessionMeta>,
    unresolved_proxy_requests: u64,
    settings: &AppSettings,
    now_sec: i64,
) -> SessionStats {
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
    let mut total_requests = 0_u64;

    for fact in session_facts {
        let request_count = fact.request_count.max(1);
        total_requests = total_requests.saturating_add(request_count);
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
            CoverageOrigin::LocalOnly => {
                local_only_requests = local_only_requests.saturating_add(request_count)
            }
            CoverageOrigin::ProxyOnly
            | CoverageOrigin::MergedProxyPreferred
            | CoverageOrigin::MergedFuzzyMatched => {
                proxy_backed_requests = proxy_backed_requests.saturating_add(request_count);
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
    let usage_fully_covered = session_usage_fully_covered(
        meta,
        &session_tool,
        proxy_backed_requests,
        unresolved_proxy_requests,
        now_sec,
    );

    SessionStats {
        session_id: session_id.to_string(),
        tool: session_tool,
        total_requests,
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
                session_project_identity(m.project_name.as_deref(), m.cwd.as_deref()).to_string()
            })
            .unwrap_or_else(|| "unknown".to_string()),
        ),
        topic: meta.and_then(|m| m.topic.clone()),
        last_prompt: meta.and_then(|m| m.last_prompt.clone()),
        session_name: meta.and_then(|m| m.session_name.clone()),
        scope: meta.and_then(|m| m.scope.clone()),
        wsl_distro: meta.and_then(|m| wsl_distro_from_path(&m.file_path)),
    }
}

/// 快照优先读取：跳过 ensure_local_usage_synced 的全盘扫描，直接以当前
/// SQLite 数据回答。供会话面板首屏使用（后台同步完成后由事件驱动二次刷新）。
pub async fn get_merged_sessions_no_sync(
    settings: &AppSettings,
    limit: i64,
    offset: i64,
) -> Result<Vec<SessionStats>, String> {
    let local_db = crate::local_usage::get_local_usage_db()?;
    get_merged_sessions_with_db(local_db, settings, limit, offset).await
}

async fn get_merged_sessions_with_db(
    local_db: Arc<crate::local_usage::LocalUsageDatabase>,
    settings: &AppSettings,
    limit: i64,
    offset: i64,
) -> Result<Vec<SessionStats>, String> {
    let now_sec = chrono::Utc::now().timestamp();
    let include_errors = settings.proxy.include_error_requests;
    let tool_filter = settings.client_tools.build_filter();
    let source_filter = settings.source_aware.build_filter();
    let pricings = crate::proxy::ProxyDatabase::get_global()
        .and_then(|db| db.get_all_model_pricings().ok())
        .unwrap_or_default();
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
    let session_cache_key = SessionDerivedCacheKey { merge_key };
    if let Some(sessions) = lookup_sessions_cache(&session_cache_key) {
        // 缓存保存完整排序列表，clone 只发生在分页切片范围内。
        let page: Vec<SessionStats> = sessions
            .iter()
            .skip(offset.max(0) as usize)
            .take(limit.max(0) as usize)
            .cloned()
            .collect();
        return Ok(page);
    }

    let (facts, _) =
        get_merged_request_facts_with_db(local_db.clone(), settings, None, None, include_errors)
            .await?;
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
        let unresolved_proxy_requests = unresolved_reasonix_requests_by_session
            .get(&session_id)
            .copied()
            .unwrap_or(0);
        result.push(build_fact_backed_session_stats(
            &session_id,
            &session_facts,
            meta,
            unresolved_proxy_requests,
            settings,
            now_sec,
        ));
    }

    if metadata_only_sessions_allowed(&source_filter) {
        for meta in meta_by_id.values() {
            if fact_backed_session_ids.contains(&meta.session_id) {
                continue;
            }
            result.push(build_metadata_only_session_stats(
                meta,
                &settings.currency,
                now_sec,
            ));
        }
    }

    result.sort_by_key(|session| std::cmp::Reverse(session.last_request_time));
    // 先按 merge_key 缓存完整排序列表，再切片返回，避免翻页时重复全量聚合。
    let full = Arc::new(result);
    store_sessions_cache(session_cache_key, Arc::clone(&full));
    let page: Vec<SessionStats> = full
        .iter()
        .skip(offset.max(0) as usize)
        .take(limit.max(0) as usize)
        .cloned()
        .collect();
    Ok(page)
}

#[allow(clippy::too_many_arguments)]
async fn get_targeted_session_facts_with_db(
    local_db: Arc<crate::local_usage::LocalUsageDatabase>,
    settings: &AppSettings,
    session_id: &str,
    include_errors: bool,
    source_filter: &crate::models::SourceFilter,
    tool_filter: &ToolFilter,
    pricings: &[crate::models::ModelPricingConfig],
    local_signature: crate::local_usage::LocalMergeCacheSignature,
    proxy_signature: Option<ProxyMergeCacheSignature>,
) -> Result<Vec<MergedRequestFact>, String> {
    let pricing_match_mode = settings.model_pricing.match_mode.clone();
    let today = crate::local_usage::LocalUsageDatabase::today_local_date_with_settings(settings);
    let today_start =
        crate::local_usage::LocalUsageDatabase::local_date_epoch_bounds_with_settings(
            &today, settings,
        )
        .map(|(start, _)| start)?;
    let history_dates = if let Some((data_start, data_end)) = combined_data_time_bounds(&local_db)?
    {
        let history_end = data_end.min(today_start);
        if history_end > data_start {
            ensure_materialized_history_for_range(
                local_db.clone(),
                settings,
                data_start,
                history_end,
                pricings,
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

    let cold_db = local_db.clone();
    let cold_session_id = session_id.to_string();
    let cold_tool_filter = tool_filter.clone();
    let cold_source_filter = source_filter.clone();
    let mut facts = tauri::async_runtime::spawn_blocking(move || {
        let facts = cold_db.get_unified_facts_for_session_dates(
            &history_dates,
            &cold_session_id,
            &cold_tool_filter,
        )?;
        Ok::<Vec<MergedRequestFact>, String>(
            facts
                .into_iter()
                .filter(|fact| {
                    crate::unified_usage::matches_source_filter(fact, &cold_source_filter)
                        && (include_errors
                            || fact.status_code.map(|code| code < 300).unwrap_or(true))
                })
                .collect(),
        )
    })
    .await
    .map_err(|error| format!("Task error: {error}"))??;

    let hot_facts = get_hot_merge_facts(
        local_db,
        settings,
        include_errors,
        pricings,
        &pricing_match_mode,
        local_signature,
        proxy_signature,
    )
    .await?;
    facts.extend(
        hot_facts
            .iter()
            .filter(|fact| fact.session_id == session_id)
            .cloned(),
    );
    Ok(facts)
}

pub async fn get_merged_session_detail(
    settings: &AppSettings,
    session_id: &str,
) -> Result<Option<SessionStats>, String> {
    if session_id.trim().is_empty() {
        return Ok(None);
    }

    // 详情通常从会话列表点入：先按与列表完全相同的签名查派生缓存，命中时只克隆
    // 一个 SessionStats。未命中时仍复用统一事实合并（保留 Local/Remote/Proxy 去重、
    // Codex fuzzy 和 Source/Tool Filter 语义），但只聚合目标会话，不再构建并排序全量列表。
    let local_db = crate::local_usage::get_local_usage_db()?;
    let now_sec = chrono::Utc::now().timestamp();
    let include_errors = settings.proxy.include_error_requests;
    let tool_filter = settings.client_tools.build_filter();
    let source_filter = settings.source_aware.build_filter();
    let pricings = crate::proxy::ProxyDatabase::get_global()
        .and_then(|db| db.get_all_model_pricings().ok())
        .unwrap_or_default();
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
        merge_key: merge_key.clone(),
    };
    if let Some(sessions) = lookup_sessions_cache(&session_cache_key) {
        return Ok(sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .cloned());
    }
    let detail_cache_key = SessionDetailCacheKey {
        merge_key,
        session_id: session_id.to_string(),
    };
    if let Some(detail) = lookup_session_detail_cache(&detail_cache_key) {
        return Ok(detail);
    }

    let mut local_sessions = local_db.get_all_sessions(&tool_filter)?;
    local_sessions.extend(local_db.get_remote_sessions(&tool_filter)?);
    let meta_by_id: HashMap<String, SessionMeta> = local_sessions
        .iter()
        .cloned()
        .map(|meta| (meta.session_id.clone(), meta))
        .collect();
    let meta = meta_by_id.get(session_id);
    let facts = if meta.map(|value| value.tool == "reasonix").unwrap_or(false) {
        // Reasonix 覆盖率需要观察 session_id 为空的未归属 Proxy 事实；该特殊语义
        // 不能靠 session_id 定向 SQL 表达，因此保留全量统一事实路径。
        get_merged_request_facts_with_db(local_db.clone(), settings, None, None, include_errors)
            .await?
            .0
            .as_ref()
            .clone()
    } else {
        get_targeted_session_facts_with_db(
            local_db.clone(),
            settings,
            session_id,
            include_errors,
            &source_filter,
            &tool_filter,
            &pricings,
            local_signature,
            proxy_signature,
        )
        .await?
    };
    let session_facts: Vec<&MergedRequestFact> = facts
        .iter()
        .filter(|fact| fact.session_id == session_id)
        .collect();

    if session_facts.is_empty() {
        let detail = if metadata_only_sessions_allowed(&source_filter) {
            meta.map(|value| build_metadata_only_session_stats(value, &settings.currency, now_sec))
        } else {
            None
        };
        store_session_detail_cache(detail_cache_key, detail.clone());
        return Ok(detail);
    }

    let unresolved_proxy_requests = meta
        .filter(|value| value.tool == "reasonix")
        .map(|value| {
            count_unresolved_reasonix_requests_by_session(&facts, std::slice::from_ref(value))
                .get(session_id)
                .copied()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    let detail = Some(build_fact_backed_session_stats(
        session_id,
        &session_facts,
        meta,
        unresolved_proxy_requests,
        settings,
        now_sec,
    ));
    store_session_detail_cache(detail_cache_key, detail.clone());
    Ok(detail)
}

pub async fn get_merged_project_stats_no_sync(
    settings: &AppSettings,
) -> Result<Vec<ProjectStats>, String> {
    let local_db = crate::local_usage::get_local_usage_db()?;
    get_merged_project_stats_with_db(local_db, settings).await
}

async fn get_merged_project_stats_with_db(
    local_db: Arc<crate::local_usage::LocalUsageDatabase>,
    settings: &AppSettings,
) -> Result<Vec<ProjectStats>, String> {
    let now_sec = chrono::Utc::now().timestamp();
    let include_errors = settings.proxy.include_error_requests;
    let tool_filter = settings.client_tools.build_filter();
    let source_filter = settings.source_aware.build_filter();
    let pricings = crate::proxy::ProxyDatabase::get_global()
        .and_then(|db| db.get_all_model_pricings().ok())
        .unwrap_or_default();
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
        return Ok(projects);
    }

    let mut local_sessions = local_db.get_all_sessions(&tool_filter)?;
    local_sessions.extend(local_db.get_remote_sessions(&tool_filter)?);
    let local_sessions_by_id: HashMap<String, SessionMeta> = local_sessions
        .iter()
        .cloned()
        .map(|meta| (meta.session_id.clone(), meta))
        .collect();

    let (facts, _) =
        get_merged_request_facts_with_db(local_db.clone(), settings, None, None, include_errors)
            .await?;
    let unresolved_reasonix_requests_by_session =
        count_unresolved_reasonix_requests_by_session(&facts, &local_sessions);
    let mut map: HashMap<String, ProjectAggregate<'_>> = HashMap::new();
    let mut fact_backed_project_session_ids: HashSet<&str> = HashSet::new();
    let mut proxy_backed_requests_by_session: HashMap<&str, u64> = HashMap::new();

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
            let session_id = fact.session_id.as_str();
            fact_backed_project_session_ids.insert(session_id);
            entry.sessions.insert(session_id);
            entry
                .tool_sessions
                .entry(fact.tool.as_str())
                .or_default()
                .insert(session_id);
            if fact.tool == "reasonix"
                && matches!(
                    fact.coverage_origin,
                    CoverageOrigin::ProxyOnly | CoverageOrigin::MergedProxyPreferred
                )
            {
                *proxy_backed_requests_by_session
                    .entry(session_id)
                    .or_default() += request_count;
            }
        } else {
            entry.tool_sessions.entry(fact.tool.as_str()).or_default();
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
            if fact_backed_project_session_ids.contains(meta.session_id.as_str()) {
                continue;
            }
            merge_metadata_only_project(&mut map, meta);
        }
    }

    let mut projects: Vec<ProjectStats> = map
        .into_values()
        .map(|mut aggregate| {
            aggregate.stats.session_count = aggregate.sessions.len() as u64;
            let mut session_ids: Vec<&str> = aggregate.sessions.iter().copied().collect();
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
                            .get(tool.tool.as_str())
                            .map(|sessions| sessions.is_empty())
                            .unwrap_or(true)
                });
            let reasonix_session_ids: Vec<&str> = aggregate
                .sessions
                .iter()
                .copied()
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
                    && reasonix_session_ids.iter().copied().all(|session_id| {
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
            } else {
                true
            };
            aggregate.stats.uncovered_requests = aggregate
                .sessions
                .iter()
                .copied()
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
                    .get(tool_stats.tool.as_str())
                    .map(|sessions| sessions.len() as u64)
                    .unwrap_or(0);
                let sessions_for_tool = aggregate.tool_sessions.get(tool_stats.tool.as_str());
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
                            sessions.iter().copied().all(|session_id| {
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
                            .copied()
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
    store_projects_cache(project_cache_key, &projects);
    Ok(projects)
}

#[cfg(test)]
mod tests {
    use super::super::reasonix_support::ReasonixProxyCoverage;
    use super::*;

    fn sample_local_signature() -> crate::local_usage::LocalMergeCacheSignature {
        crate::local_usage::LocalMergeCacheSignature {
            merge_cache_generation: 1,
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
        store_sessions_cache(session_key.clone(), Arc::new(Vec::new()));
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
    fn cold_concat_fingerprint_tracks_dates_and_stamps() {
        let stamps = vec![
            ("2026-06-01".to_string(), 100i64),
            ("2026-06-02".to_string(), 200i64),
        ];
        let base = cold_concat_fingerprint("standard", &stamps);

        // 完全相同的输入 → 指纹一致（memo 可复用）。
        assert_eq!(base, cold_concat_fingerprint("standard", &stamps));

        // 任一日重物化（materialized_at 变化）→ 指纹变化。
        let rebuilt = vec![
            ("2026-06-01".to_string(), 100i64),
            ("2026-06-02".to_string(), 201i64),
        ];
        assert_ne!(base, cold_concat_fingerprint("standard", &rebuilt));

        // 日期集合增删（换日/范围变化）→ 指纹变化。
        let extended = vec![
            ("2026-06-01".to_string(), 100i64),
            ("2026-06-02".to_string(), 200i64),
            ("2026-06-03".to_string(), 300i64),
        ];
        assert_ne!(base, cold_concat_fingerprint("standard", &extended));

        // 日界模式（全局代次维度）参与指纹。
        assert_ne!(base, cold_concat_fingerprint("night_owl", &stamps));
    }

    #[test]
    fn fact_tool_matches_mirrors_sql_tool_clause_semantics() {
        let mut fact = MergedRequestFact {
            canonical_request_key: "claude_code:msg-1".to_string(),
            session_id: "sess-1".to_string(),
            project_name: None,
            project_path: None,
            api_key_prefix: None,
            request_base_url: None,
            tool: "claude_code".to_string(),
            timestamp_sec: 0,
            timestamp_ms: 0,
            model: "claude-sonnet-4".to_string(),
            input_tokens: 0,
            output_tokens: 0,
            cache_create_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: 0,
            request_count: 1,
            estimated_cost: 0.0,
            coverage_origin: CoverageOrigin::LocalOnly,
            status_code: Some(200),
            duration_ms: None,
            output_tokens_per_second: None,
            ttft_ms: None,
            source_label: None,
        };

        assert!(fact_tool_matches(&fact, &ToolFilter::All));
        // 空 Tool 视同不过滤（与 SQL 侧 trim 后为空不加子句一致）。
        assert!(fact_tool_matches(&fact, &ToolFilter::Tool(" ".to_string())));
        assert!(fact_tool_matches(
            &fact,
            &ToolFilter::Tool("claude_code".to_string())
        ));
        assert!(!fact_tool_matches(
            &fact,
            &ToolFilter::Tool("codex".to_string())
        ));
        assert!(fact_tool_matches(
            &fact,
            &ToolFilter::AnyOf(vec!["codex".to_string(), "claude_code".to_string()])
        ));
        // 空 AnyOf 不匹配任何行（与 SQL 侧空 IN 列表提前返回空一致）。
        assert!(!fact_tool_matches(&fact, &ToolFilter::AnyOf(Vec::new())));
        fact.tool = "codex".to_string();
        assert!(fact_tool_matches(
            &fact,
            &ToolFilter::Tool("codex".to_string())
        ));
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

    fn reasonix_meta(session_id: &str, start_time: i64, end_time: i64) -> SessionMeta {
        SessionMeta {
            session_id: session_id.to_string(),
            tool: "reasonix".to_string(),
            cwd: Some("/tmp/reasonix-project".to_string()),
            project_name: Some("reasonix-project".to_string()),
            file_path: format!("/tmp/{session_id}.jsonl"),
            last_modified: end_time,
            total_input_tokens: 600,
            total_output_tokens: 300,
            total_cache_create_tokens: 100,
            total_cache_read_tokens: 500,
            models: vec!["deepseek-v4-pro".to_string()],
            message_count: 3,
            start_time,
            end_time,
            total_elapsed_ms: 4_000,
            explicit_cost: Some(7.2),
            explicit_cost_currency: Some("CNY".to_string()),
            ..Default::default()
        }
    }

    fn reasonix_proxy(
        message_id: &str,
        timestamp_sec: i64,
        session_id: Option<&str>,
    ) -> UsageRecord {
        UsageRecord {
            timestamp: timestamp_sec * 1000,
            request_end_time: timestamp_sec * 1000,
            request_start_time: timestamp_sec * 1000 - 100,
            message_id: message_id.to_string(),
            canonical_request_key: Some(format!("reasonix:{message_id}")),
            input_tokens: 200,
            output_tokens: 100,
            cache_create_tokens: 50,
            cache_read_tokens: 100,
            total_tokens: 450,
            model: "deepseek-v4-pro".to_string(),
            session_id: session_id.map(str::to_string),
            session_resolution_state: session_id.map(|_| "known".to_string()),
            duration_ms: 1_000,
            estimated_cost: 0.25,
            cost_locked: true,
            client_tool: "reasonix".to_string(),
            status_code: 200,
            ..Default::default()
        }
    }

    fn cny_currency_settings() -> crate::models::CurrencySettings {
        let mut settings = crate::models::CurrencySettings::default();
        settings.exchange_rates.insert("CNY".to_string(), 7.2);
        settings
    }

    #[test]
    fn reasonix_telemetry_only_builds_weighted_residual() {
        let meta = reasonix_meta("reasonix::sess-1", 100, 200);
        let fact = build_reasonix_telemetry_residual(
            &meta,
            None,
            false,
            &crate::models::SourceFilter::All,
            &cny_currency_settings(),
            150,
            250,
        )
        .expect("telemetry residual");

        assert_eq!(fact.session_id, meta.session_id);
        assert_eq!(fact.timestamp_sec, 200);
        assert_eq!(fact.request_count, 3);
        assert_eq!(fact.input_tokens, 600);
        assert_eq!(fact.output_tokens, 300);
        assert_eq!(fact.cache_create_tokens, 100);
        assert_eq!(fact.cache_read_tokens, 500);
        assert_eq!(fact.total_tokens, 1_500);
        assert_eq!(fact.duration_ms, Some(4_000));
        assert!((fact.estimated_cost - 1.0).abs() < 1e-9);
        assert_eq!(fact.coverage_origin, CoverageOrigin::LocalOnly);
    }

    #[test]
    fn reasonix_partial_proxy_coverage_builds_only_non_negative_residual() {
        let meta = reasonix_meta("reasonix::sess-1", 100, 200);
        let covered = ReasonixProxyCoverage {
            request_count: 1,
            input_tokens: 700,
            output_tokens: 100,
            cache_create_tokens: 50,
            cache_read_tokens: 100,
            estimated_cost_usd: 0.25,
            duration_ms: 1_000,
        };
        let fact = build_reasonix_telemetry_residual(
            &meta,
            Some(&covered),
            false,
            &crate::models::SourceFilter::All,
            &cny_currency_settings(),
            0,
            300,
        )
        .expect("partial residual");

        assert_eq!(fact.request_count, 2);
        assert_eq!(fact.input_tokens, 0);
        assert_eq!(fact.output_tokens, 200);
        assert_eq!(fact.cache_create_tokens, 50);
        assert_eq!(fact.cache_read_tokens, 400);
        assert_eq!(fact.total_tokens, 650);
        assert_eq!(fact.duration_ms, Some(3_000));
        assert!((fact.estimated_cost - 0.75).abs() < 1e-9);
    }

    #[test]
    fn reasonix_full_request_coverage_does_not_emit_field_residual() {
        let meta = reasonix_meta("reasonix::sess-1", 100, 200);
        let covered = ReasonixProxyCoverage {
            request_count: 3,
            input_tokens: 100,
            ..Default::default()
        };
        assert!(build_reasonix_telemetry_residual(
            &meta,
            Some(&covered),
            false,
            &crate::models::SourceFilter::All,
            &cny_currency_settings(),
            0,
            300,
        )
        .is_none());
    }

    #[test]
    fn reasonix_residual_respects_blocked_range_and_source_filters() {
        let meta = reasonix_meta("reasonix::sess-1", 100, 200);
        let currency = cny_currency_settings();
        let unknown = crate::models::SourceFilter::Unknown {
            known_pairs: Vec::new(),
        };
        assert!(
            build_reasonix_telemetry_residual(&meta, None, false, &unknown, &currency, 0, 300,)
                .is_some()
        );
        assert!(build_reasonix_telemetry_residual(
            &meta,
            None,
            true,
            &crate::models::SourceFilter::All,
            &currency,
            0,
            300,
        )
        .is_none());
        assert!(build_reasonix_telemetry_residual(
            &meta,
            None,
            false,
            &crate::models::SourceFilter::All,
            &currency,
            0,
            200,
        )
        .is_none());
        assert!(build_reasonix_telemetry_residual(
            &meta,
            None,
            false,
            &crate::models::SourceFilter::Source {
                api_key_prefixes: vec!["sk-test".to_string()],
                base_url: None,
            },
            &currency,
            0,
            300,
        )
        .is_none());
    }

    #[test]
    fn reasonix_unknown_or_missing_currency_does_not_leak_into_usd_cost() {
        for currency in [Some("XYZ"), None] {
            let mut meta = reasonix_meta("reasonix::sess-1", 100, 200);
            meta.explicit_cost_currency = currency.map(str::to_string);
            let fact = build_reasonix_telemetry_residual(
                &meta,
                None,
                false,
                &crate::models::SourceFilter::All,
                &crate::models::CurrencySettings::default(),
                0,
                300,
            )
            .expect("token residual remains valid");
            assert_eq!(fact.estimated_cost, 0.0);
        }
    }

    #[test]
    fn reasonix_persisted_currency_symbols_are_normalized_during_conversion() {
        let mut settings = crate::models::CurrencySettings::default();
        settings.exchange_rates.insert("CNY".to_string(), 7.2);
        settings.exchange_rates.insert("EUR".to_string(), 0.9);
        settings.exchange_rates.insert("GBP".to_string(), 0.8);

        for (currency, cost, expected_usd) in [
            ("¥", 7.2, 1.0),
            ("￥", 7.2, 1.0),
            ("RMB", 7.2, 1.0),
            ("$", 7.2, 7.2),
            ("€", 0.9, 1.0),
            ("EUR", 0.9, 1.0),
            ("£", 0.8, 1.0),
            ("GBP", 0.8, 1.0),
        ] {
            let mut meta = reasonix_meta("reasonix::sess-1", 100, 200);
            meta.explicit_cost = Some(cost);
            meta.explicit_cost_currency = Some(currency.to_string());
            let converted = reasonix_explicit_cost_usd(&meta, &settings).expect("known currency");
            assert!(
                (converted - expected_usd).abs() < 1e-9,
                "currency {currency} converted to {converted}, expected {expected_usd}"
            );
        }
    }

    #[test]
    fn reasonix_proxy_coverage_uses_unique_candidate_and_blocks_ambiguous_candidates() {
        let first = reasonix_meta("reasonix::sess-1", 100, 160);
        let second = reasonix_meta("reasonix::sess-2", 140, 200);
        let sessions = HashMap::from([
            (first.session_id.clone(), first),
            (second.session_id.clone(), second),
        ]);
        let unique = reasonix_proxy("unique", 110, None);
        let ambiguous = reasonix_proxy("ambiguous", 150, None);
        let (coverage, blocked) =
            build_reasonix_proxy_coverage_by_session(&[unique, ambiguous], &sessions);

        assert_eq!(coverage["reasonix::sess-1"].request_count, 1);
        assert!(blocked.contains("reasonix::sess-1"));
        assert!(blocked.contains("reasonix::sess-2"));
    }

    #[test]
    fn reasonix_proxy_coverage_ignores_errors_and_zero_token_records() {
        let meta = reasonix_meta("reasonix::sess-1", 100, 200);
        let sessions = HashMap::from([(meta.session_id.clone(), meta)]);
        let valid = reasonix_proxy("valid", 120, Some("reasonix::sess-1"));
        let mut error = reasonix_proxy("error", 130, Some("reasonix::sess-1"));
        error.status_code = 500;
        let mut zero = reasonix_proxy("zero", 140, Some("reasonix::sess-1"));
        zero.input_tokens = 0;
        zero.output_tokens = 0;
        zero.cache_create_tokens = 0;
        zero.cache_read_tokens = 0;
        zero.reasoning_tokens = 0;
        zero.total_tokens = 0;

        let (coverage, blocked) =
            build_reasonix_proxy_coverage_by_session(&[valid, error, zero], &sessions);
        assert_eq!(coverage["reasonix::sess-1"].request_count, 1);
        assert!(blocked.is_empty());
    }

    #[test]
    fn reasonix_coverage_query_is_bounded_to_sessions_ending_in_range() {
        let in_range = reasonix_meta("reasonix::sess-1", 100, 200);
        let out_of_range = reasonix_meta("reasonix::sess-2", 1_000, 1_100);
        let sessions = vec![in_range, out_of_range];
        assert_eq!(
            reasonix_coverage_query_bounds(
                sessions.iter(),
                &crate::models::SourceFilter::All,
                150,
                250,
            ),
            Some((85, 216))
        );
        assert!(reasonix_coverage_query_bounds(
            sessions.iter(),
            &crate::models::SourceFilter::Source {
                api_key_prefixes: Vec::new(),
                base_url: None,
            },
            0,
            2_000,
        )
        .is_none());
    }

    #[test]
    fn session_stats_honor_weighted_reasonix_residual_request_count() {
        let meta = reasonix_meta("reasonix::sess-1", 100, 200);
        let fact = build_reasonix_telemetry_residual(
            &meta,
            None,
            false,
            &crate::models::SourceFilter::All,
            &cny_currency_settings(),
            0,
            300,
        )
        .unwrap();
        let stats = build_fact_backed_session_stats(
            &meta.session_id,
            &[&fact],
            Some(&meta),
            0,
            &AppSettings::default(),
            1_000,
        );
        assert_eq!(stats.total_requests, 3);
        assert_eq!(stats.uncovered_requests, 3);
    }
}
