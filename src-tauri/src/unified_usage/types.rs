use crate::proxy::UsageRecord;
use crate::session::{LocalRequestRecord, SessionMeta};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMode {
    LocalOnly,
    ProxyOnly,
    ProxyWithLocalFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageOrigin {
    ProxyOnly,
    LocalOnly,
    MergedProxyPreferred,
}

#[derive(Debug, Clone, Default)]
pub struct MergedCoverage {
    pub proxy_backed_requests: u64,
    pub local_only_requests: u64,
    pub merged_overlap_requests: u64,
    pub has_partial_status_coverage: bool,
    pub has_partial_performance_coverage: bool,
}

#[derive(Debug, Clone)]
pub struct MergedRequestFact {
    pub session_id: String,
    pub project_name: Option<String>,
    pub project_path: Option<String>,
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
    pub coverage_origin: CoverageOrigin,
    pub status_code: Option<u16>,
    pub duration_ms: Option<u64>,
    pub output_tokens_per_second: Option<f64>,
    pub ttft_ms: Option<u64>,
}

impl MergedRequestFact {
    pub fn from_local(record: &LocalRequestRecord, meta: Option<&SessionMeta>, cost: f64) -> Self {
        let project_name = meta.and_then(|m| m.project_name.clone());
        let project_path = meta.and_then(|m| m.cwd.clone());

        Self {
            session_id: record.session_id.clone(),
            project_name,
            project_path,
            api_key_prefix: None,
            request_base_url: None,
            tool: record.tool.clone(),
            timestamp_sec: record.timestamp,
            timestamp_ms: record.timestamp.saturating_mul(1000),
            model: record.model.clone(),
            input_tokens: record.input_tokens,
            output_tokens: record.output_tokens,
            cache_create_tokens: record.cache_create_tokens,
            cache_read_tokens: record.cache_read_tokens,
            total_tokens: record.total_tokens,
            estimated_cost: cost,
            coverage_origin: CoverageOrigin::LocalOnly,
            status_code: None,
            duration_ms: None,
            output_tokens_per_second: None,
            ttft_ms: None,
        }
    }

    pub fn from_proxy(record: &UsageRecord, meta: Option<&SessionMeta>) -> Self {
        let project_name = meta.and_then(|m| m.project_name.clone());
        let project_path = meta.and_then(|m| m.cwd.clone());

        Self {
            session_id: record.session_id.clone().unwrap_or_default(),
            project_name,
            project_path,
            api_key_prefix: record.api_key_prefix.clone(),
            request_base_url: record.request_base_url.clone(),
            tool: record.client_tool.clone(),
            timestamp_sec: record.timestamp / 1000,
            timestamp_ms: record.timestamp,
            model: record.model.clone(),
            input_tokens: record.input_tokens,
            output_tokens: record.output_tokens,
            cache_create_tokens: record.cache_create_tokens,
            cache_read_tokens: record.cache_read_tokens,
            total_tokens: record.total_tokens,
            estimated_cost: record.estimated_cost,
            coverage_origin: CoverageOrigin::ProxyOnly,
            status_code: Some(record.status_code),
            duration_ms: Some(record.duration_ms),
            output_tokens_per_second: record.output_tokens_per_second,
            ttft_ms: record.ttft_ms,
        }
    }

    pub fn merge_proxy_preferred(
        proxy: &UsageRecord,
        local: &LocalRequestRecord,
        meta: Option<&SessionMeta>,
        fallback_cost: f64,
    ) -> Self {
        let project_name = meta.and_then(|m| m.project_name.clone());
        let project_path = meta.and_then(|m| m.cwd.clone());

        Self {
            session_id: if !local.session_id.trim().is_empty() {
                local.session_id.clone()
            } else {
                proxy.session_id.clone().unwrap_or_default()
            },
            project_name,
            project_path,
            api_key_prefix: proxy.api_key_prefix.clone(),
            request_base_url: proxy.request_base_url.clone(),
            tool: if !local.tool.trim().is_empty() {
                local.tool.clone()
            } else {
                proxy.client_tool.clone()
            },
            timestamp_sec: local.timestamp,
            timestamp_ms: proxy.timestamp,
            model: if !proxy.model.trim().is_empty() {
                proxy.model.clone()
            } else {
                local.model.clone()
            },
            input_tokens: if proxy.input_tokens > 0 {
                proxy.input_tokens
            } else {
                local.input_tokens
            },
            output_tokens: if proxy.output_tokens > 0 {
                proxy.output_tokens
            } else {
                local.output_tokens
            },
            cache_create_tokens: if proxy.cache_create_tokens > 0 {
                proxy.cache_create_tokens
            } else {
                local.cache_create_tokens
            },
            cache_read_tokens: if proxy.cache_read_tokens > 0 {
                proxy.cache_read_tokens
            } else {
                local.cache_read_tokens
            },
            total_tokens: if proxy.total_tokens > 0 {
                proxy.total_tokens
            } else {
                local.total_tokens
            },
            estimated_cost: if proxy.estimated_cost > 0.0 {
                proxy.estimated_cost
            } else {
                fallback_cost
            },
            coverage_origin: CoverageOrigin::MergedProxyPreferred,
            status_code: Some(proxy.status_code),
            duration_ms: Some(proxy.duration_ms),
            output_tokens_per_second: proxy.output_tokens_per_second,
            ttft_ms: proxy.ttft_ms,
        }
    }
}
