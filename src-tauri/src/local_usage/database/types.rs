use std::collections::HashMap;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncExportSession {
    pub session_id: String,
    pub tool: String,
    pub project_key: Option<String>,
    pub project_name: Option<String>,
    pub start_time: i64,
    pub end_time: i64,
    pub request_count: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_create_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_tokens: u64,
    pub model_list: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncExportRequest {
    pub request_key: String,
    pub session_id: String,
    pub tool: String,
    pub project_key: Option<String>,
    pub timestamp: i64,
    pub message_id: Option<String>,
    pub dedupe_key: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    pub is_subagent: bool,
    pub source_kind: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncExportData {
    pub sessions: Vec<SyncExportSession>,
    pub requests: Vec<SyncExportRequest>,
}

#[derive(Debug, Clone)]
pub struct SyncOutboxBatch {
    pub request_events: Vec<SyncExportRequest>,
    pub session_events: Vec<SyncExportSession>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSyncDevice {
    pub device_id: String,
    pub last_seen_at: Option<i64>,
    pub last_export_seq: i64,
    pub sync_status: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalMergeCacheSignature {
    pub local_request_count: u64,
    pub local_max_sync_version: i64,
    pub local_max_timestamp: i64,
    pub remote_request_count: u64,
    pub remote_max_export_seq: i64,
    pub remote_max_timestamp: i64,
    pub local_session_max_updated_at: i64,
    pub remote_session_max_imported_at: i64,
    pub unified_materialization_invalidation_version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnifiedDayMaterializationState {
    pub local_date: String,
    pub day_boundary_mode: String,
    pub fact_count: u64,
    pub local_request_count: u64,
    pub local_max_sync_version: i64,
    pub local_max_timestamp: i64,
    pub remote_request_count: u64,
    pub remote_max_export_seq: i64,
    pub remote_max_timestamp: i64,
    pub proxy_record_count: u64,
    pub proxy_all_record_count: u64,
    pub proxy_max_timestamp_ms: i64,
    pub proxy_max_updated_at: i64,
    pub max_fact_timestamp_ms: i64,
    pub pricing_fingerprint: u64,
    pub is_finalized: bool,
    pub finalized_at: Option<i64>,
    pub materialized_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnifiedDayLocalSnapshot {
    pub local_request_count: u64,
    pub local_max_sync_version: i64,
    pub local_max_timestamp: i64,
    pub remote_request_count: u64,
    pub remote_max_export_seq: i64,
    pub remote_max_timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UnifiedDailySummaryRow {
    pub local_date: String,
    pub request_count: u64,
    pub visible_request_count: u64,
    pub total_tokens: u64,
    pub visible_total_tokens: u64,
    pub input_tokens: u64,
    pub visible_input_tokens: u64,
    pub output_tokens: u64,
    pub visible_output_tokens: u64,
    pub cache_create_tokens: u64,
    pub visible_cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub visible_cache_read_tokens: u64,
    pub total_cost: f64,
    pub visible_cost: f64,
    pub success_request_count: u64,
    pub success_total_tokens: u64,
    pub success_input_tokens: u64,
    pub success_output_tokens: u64,
    pub success_cache_create_tokens: u64,
    pub success_cache_read_tokens: u64,
    pub success_cost: f64,
    pub client_error_requests: u64,
    pub server_error_requests: u64,
    pub model_count: u64,
    pub success_model_count: u64,
    pub proxy_backed_requests: u64,
    pub local_only_requests: u64,
    pub merged_overlap_requests: u64,
    pub has_partial_status_coverage: bool,
    pub has_partial_performance_coverage: bool,
    pub materialized_at: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UnifiedDailyModelSummaryRow {
    pub local_date: String,
    pub model_name: String,
    pub request_count: u64,
    pub visible_request_count: u64,
    pub total_tokens: u64,
    pub visible_total_tokens: u64,
    pub input_tokens: u64,
    pub visible_input_tokens: u64,
    pub output_tokens: u64,
    pub visible_output_tokens: u64,
    pub cache_create_tokens: u64,
    pub visible_cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub visible_cache_read_tokens: u64,
    pub total_cost: f64,
    pub visible_cost: f64,
    pub success_request_count: u64,
    pub success_total_tokens: u64,
    pub success_input_tokens: u64,
    pub success_output_tokens: u64,
    pub success_cache_create_tokens: u64,
    pub success_cache_read_tokens: u64,
    pub success_cost: f64,
    pub client_error_requests: u64,
    pub server_error_requests: u64,
    pub local_only_requests: u64,
    pub rate_sum: f64,
    pub rate_count: u64,
    pub ttft_sum: f64,
    pub ttft_count: u64,
    pub status_code_counts: HashMap<u16, u64>,
    pub materialized_at: i64,
}
