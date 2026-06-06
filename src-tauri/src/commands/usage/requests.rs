use super::types::ProxyState;
use crate::models::AppSettings;
use crate::unified_usage::CoverageOrigin;

const RECENT_REQUESTS_MAX_LIMIT: i64 = 30;
const RECENT_REQUESTS_MAX_OFFSET: i64 = 200;

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
#[tauri::command]
pub async fn get_recent_request_records(
    query: RequestRecordsQuery,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<Vec<RequestRecordItem>, String> {
    let include_errors = settings.proxy.include_error_requests;
    let limit = query.limit.clamp(1, RECENT_REQUESTS_MAX_LIMIT);
    let offset = query.offset.clamp(0, RECENT_REQUESTS_MAX_OFFSET);

    let (mut facts, _) =
        crate::unified_usage::get_merged_request_facts(&settings, None, None, include_errors)
            .await?;
    facts.sort_by_key(|fact| std::cmp::Reverse(fact.timestamp_ms));

    Ok(facts
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .map(|fact| RequestRecordItem {
            request_key: fact.canonical_request_key,
            session_id: fact.session_id,
            project_name: fact.project_name,
            project_path: fact.project_path,
            source_label: fact.source_label,
            api_key_prefix: fact.api_key_prefix,
            request_base_url: fact.request_base_url,
            tool: fact.tool,
            timestamp_sec: fact.timestamp_sec,
            timestamp_ms: fact.timestamp_ms,
            model: fact.model,
            input_tokens: fact.input_tokens,
            output_tokens: fact.output_tokens,
            cache_create_tokens: fact.cache_create_tokens,
            cache_read_tokens: fact.cache_read_tokens,
            total_tokens: fact.total_tokens,
            estimated_cost: fact.estimated_cost,
            coverage_origin: match fact.coverage_origin {
                CoverageOrigin::ProxyOnly => "proxy_only",
                CoverageOrigin::LocalOnly => "local_only",
                CoverageOrigin::MergedProxyPreferred => "merged_proxy_preferred",
            }
            .to_string(),
            status_code: fact.status_code,
            duration_ms: fact.duration_ms,
            output_tokens_per_second: fact.output_tokens_per_second,
            ttft_ms: fact.ttft_ms,
        })
        .collect())
}
