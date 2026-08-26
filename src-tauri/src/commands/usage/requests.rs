use super::helpers::spawn_background_local_usage_sync;
use super::types::ProxyState;
use crate::models::AppSettings;
use crate::unified_usage::{CoverageOrigin, MergedRequestFact};

const RECENT_REQUESTS_MAX_LIMIT: i64 = 30;
const RECENT_REQUESTS_MAX_OFFSET: i64 = 200;

const PAGE_MAX_LIMIT: i64 = 100;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestRecordsQuery {
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestRecordItem {
    pub request_key: String,
    pub session_id: String,
    pub project_name: Option<String>,
    pub project_path: Option<String>,
    pub source_label: Option<String>,
    pub api_key_prefix: Option<String>,
    pub request_base_url: Option<String>,
    pub tool: String,
    pub timestamp_sec: i64,
    pub timestamp_ms: i64,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    pub estimated_cost: f64,
    pub coverage_origin: String,
    pub status_code: Option<u16>,
    pub duration_ms: Option<u64>,
    pub output_tokens_per_second: Option<f64>,
    pub ttft_ms: Option<u64>,
}

/// 获取最近请求记录流。
///
/// 这是菜单栏窄面板使用的轻量审计视图：不暴露日期筛选，只按时间倒序分页。
/// 快照优先：直接读当前数据，全盘扫描同步交给后台，完成且有新数据时
/// emit local_usage_synced 由前端静默刷新。
#[tauri::command]
pub async fn get_recent_request_records(
    app: tauri::AppHandle,
    query: RequestRecordsQuery,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<Vec<RequestRecordItem>, String> {
    let include_errors = settings.proxy.include_error_requests;
    let limit = query.limit.clamp(1, RECENT_REQUESTS_MAX_LIMIT);
    let offset = query.offset.clamp(0, RECENT_REQUESTS_MAX_OFFSET);

    spawn_background_local_usage_sync(app);
    let (facts, _) = crate::unified_usage::get_merged_request_facts_no_sync(
        &settings,
        None,
        None,
        include_errors,
    )
    .await?;
    // 事实向量来自共享 Arc，不能原地排序；先按引用做稳定排序（与原先对整表
    // sort_by_key 的顺序语义一致），再只克隆分页命中的至多 limit 条记录，
    // 避免为全量历史事实付一次深拷贝。
    let mut ordered: Vec<&MergedRequestFact> = facts.iter().collect();
    ordered.sort_by_key(|fact| std::cmp::Reverse(fact.timestamp_ms));

    Ok(ordered
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .map(map_fact_to_item)
        .collect())
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestRecordsPageQuery {
    pub limit: i64,
    pub offset: i64,
    pub search: Option<String>,
    pub status: Option<String>,
    pub coverage: Option<String>,
    pub performance: Option<String>,
    pub sort_field: Option<String>,
    pub sort_dir: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestRecordsPage {
    pub items: Vec<RequestRecordItem>,
    pub total: i64,
    pub has_more: bool,
}

fn map_fact_to_item(fact: &MergedRequestFact) -> RequestRecordItem {
    RequestRecordItem {
        request_key: fact.canonical_request_key.clone(),
        session_id: fact.session_id.clone(),
        project_name: fact.project_name.clone(),
        project_path: fact.project_path.clone(),
        source_label: fact.source_label.clone(),
        api_key_prefix: fact.api_key_prefix.clone(),
        request_base_url: fact.request_base_url.clone(),
        tool: fact.tool.clone(),
        timestamp_sec: fact.timestamp_sec,
        timestamp_ms: fact.timestamp_ms,
        model: fact.model.clone(),
        input_tokens: fact.input_tokens,
        output_tokens: fact.output_tokens,
        cache_create_tokens: fact.cache_create_tokens,
        cache_read_tokens: fact.cache_read_tokens,
        total_tokens: fact.total_tokens,
        estimated_cost: fact.estimated_cost,
        coverage_origin: coverage_origin_str(fact.coverage_origin).to_string(),
        status_code: fact.status_code,
        duration_ms: fact.duration_ms,
        output_tokens_per_second: fact.output_tokens_per_second,
        ttft_ms: fact.ttft_ms,
    }
}

fn coverage_origin_str(origin: CoverageOrigin) -> &'static str {
    match origin {
        CoverageOrigin::ProxyOnly => "proxy_only",
        CoverageOrigin::LocalOnly => "local_only",
        CoverageOrigin::MergedProxyPreferred => "merged_proxy_preferred",
        CoverageOrigin::MergedFuzzyMatched => "merged_fuzzy_matched",
    }
}

fn fact_matches_status(fact: &MergedRequestFact, status: &str) -> bool {
    match status {
        "local" => fact.coverage_origin == CoverageOrigin::LocalOnly,
        "success" => {
            fact.coverage_origin != CoverageOrigin::LocalOnly
                && fact.status_code.map(|c| c < 400).unwrap_or(true)
        }
        "error" => fact.status_code.map(|c| c >= 400).unwrap_or(false),
        _ => true,
    }
}

fn fact_matches_coverage(fact: &MergedRequestFact, coverage: &str) -> bool {
    match coverage {
        "proxy_only" => fact.coverage_origin == CoverageOrigin::ProxyOnly,
        "local_only" => fact.coverage_origin == CoverageOrigin::LocalOnly,
        "merged" => matches!(
            fact.coverage_origin,
            CoverageOrigin::MergedProxyPreferred | CoverageOrigin::MergedFuzzyMatched
        ),
        _ => true,
    }
}

fn fact_matches_performance(fact: &MergedRequestFact, perf: &str) -> bool {
    match perf {
        "has" => fact.duration_ms.is_some(),
        "none" => fact.duration_ms.is_none(),
        _ => true,
    }
}

fn fact_matches_search(fact: &MergedRequestFact, needle: &str) -> bool {
    let needle = needle.to_lowercase();
    if needle.is_empty() {
        return true;
    }
    let haystacks: [Option<&str>; 7] = [
        fact.project_name.as_deref(),
        Some(fact.model.as_str()),
        Some(fact.session_id.as_str()),
        fact.source_label.as_deref(),
        fact.request_base_url.as_deref(),
        Some(fact.tool.as_str()),
        Some(fact.canonical_request_key.as_str()),
    ];
    haystacks
        .iter()
        .flatten()
        .any(|h| h.to_lowercase().contains(&needle))
}

fn fact_sort_value(fact: &MergedRequestFact, field: &str) -> Option<f64> {
    match field {
        "input" => Some(fact.input_tokens as f64),
        "output" => Some(fact.output_tokens as f64),
        "totalTokens" => Some(fact.total_tokens as f64),
        "cost" => Some(fact.estimated_cost),
        "duration" => fact.duration_ms.map(|v| v as f64),
        "rate" => fact.output_tokens_per_second,
        "ttft" => fact.ttft_ms.map(|v| v as f64),
        _ => Some(fact.timestamp_ms as f64),
    }
}

fn compare_with_none_last(a: f64, b: f64, dir_asc: bool) -> std::cmp::Ordering {
    if dir_asc {
        a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
    } else {
        b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal)
    }
}

#[tauri::command]
pub async fn get_request_records_page(
    app: tauri::AppHandle,
    query: RequestRecordsPageQuery,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<RequestRecordsPage, String> {
    let include_errors = settings.proxy.include_error_requests;
    let limit = query.limit.clamp(1, PAGE_MAX_LIMIT);
    let offset = query.offset.max(0);

    spawn_background_local_usage_sync(app);
    let (facts, _) = crate::unified_usage::get_merged_request_facts_no_sync(
        &settings,
        None,
        None,
        include_errors,
    )
    .await?;

    let search = query.search.as_deref().unwrap_or("").trim();
    let status = query.status.as_deref().unwrap_or("all");
    let coverage = query.coverage.as_deref().unwrap_or("all");
    let performance = query.performance.as_deref().unwrap_or("all");
    let sort_field = query.sort_field.as_deref().unwrap_or("timestamp");
    let dir_asc = query.sort_dir.as_deref() == Some("asc");

    let mut filtered: Vec<&MergedRequestFact> = facts
        .iter()
        .filter(|fact| {
            fact_matches_status(fact, status)
                && fact_matches_coverage(fact, coverage)
                && fact_matches_performance(fact, performance)
                && fact_matches_search(fact, search)
        })
        .collect();

    filtered.sort_by(|a, b| {
        let av = fact_sort_value(a, sort_field);
        let bv = fact_sort_value(b, sort_field);
        match (av, bv) {
            (Some(av), Some(bv)) => compare_with_none_last(av, bv, dir_asc),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => {
                if dir_asc {
                    a.timestamp_ms.cmp(&b.timestamp_ms)
                } else {
                    b.timestamp_ms.cmp(&a.timestamp_ms)
                }
            }
        }
    });

    let total = filtered.len() as i64;
    let has_more = (offset as usize) + (limit as usize) < (total as usize);

    let items = filtered
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .map(map_fact_to_item)
        .collect();

    Ok(RequestRecordsPage {
        items,
        total,
        has_more,
    })
}
