use crate::models::{StatusCodeCount, UsageSnapshot, WindowRateSummary};
use crate::proxy::ProxyServer;
use crate::unified_usage::MergedRequestFact;
use std::sync::Arc;
use tokio::sync::{oneshot, RwLock};

/// 全局代理服务器状态
pub struct ProxyState {
    pub server: Arc<RwLock<Option<ProxyServer>>>,
    pub passive_monitor_handle: Arc<RwLock<Option<tauri::async_runtime::JoinHandle<()>>>>,
    pub passive_monitor_shutdown: Arc<RwLock<Option<oneshot::Sender<()>>>>,
}

impl Default for ProxyState {
    fn default() -> Self {
        Self {
            server: Arc::new(RwLock::new(None)),
            passive_monitor_handle: Arc::new(RwLock::new(None)),
            passive_monitor_shutdown: Arc::new(RwLock::new(None)),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatisticsQuery {
    pub start_epoch: i64,
    pub end_epoch: i64,
    pub timezone: String,
    pub bucket: StatisticsBucket,
    pub metric: StatisticsMetric,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StatisticsBucket {
    Hour,
    Day,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StatisticsMetric {
    Cost,
    Requests,
    Tokens,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatisticsRange {
    pub start_epoch: i64,
    pub end_epoch: i64,
    pub timezone: String,
    pub bucket: String,
}

#[derive(Debug, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StatisticsCapability {
    pub has_basic_usage: bool,
    pub has_performance: bool,
    pub has_status_codes: bool,
}

#[derive(Debug, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StatisticsTotals {
    pub request_count: u64,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost: f64,
    pub model_count: u64,
    pub local_request_count: u64,
    pub proxy_request_count: u64,
    pub success_requests: Option<u64>,
    pub error_requests: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StatisticsTrendPoint {
    pub start_epoch: i64,
    pub label: String,
    pub request_count: u64,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost: f64,
    pub avg_tokens_per_second: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StatisticsModelBreakdown {
    pub model_name: String,
    pub request_count: u64,
    pub local_request_count: u64,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost: f64,
    pub percent: f64,
    pub avg_tokens_per_second: Option<f64>,
    pub avg_ttft_ms: Option<f64>,
    pub error_requests: Option<u64>,
    pub success_requests: Option<u64>,
    pub client_error_requests: Option<u64>,
    pub server_error_requests: Option<u64>,
    pub status_codes: Vec<StatusCodeCount>,
    pub trend: Vec<StatisticsTrendPoint>,
}

#[derive(Debug, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StatisticsPerformance {
    pub request_count: u64,
    pub avg_tokens_per_second: f64,
    pub avg_ttft_ms: f64,
    pub slowest_model: Option<String>,
    pub fastest_model: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StatisticsStatusBreakdown {
    pub success_requests: u64,
    pub client_error_requests: u64,
    pub server_error_requests: u64,
    pub success_rate: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatisticsInsight {
    pub kind: String,
    pub level: String,
    pub value: String,
    pub model_name: Option<String>,
    pub date: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatisticsSummary {
    pub generated_at_epoch: i64,
    pub source: String,
    pub capability: StatisticsCapability,
    pub range: StatisticsRange,
    pub totals: StatisticsTotals,
    pub trend: Vec<StatisticsTrendPoint>,
    pub models: Vec<StatisticsModelBreakdown>,
    pub performance: Option<StatisticsPerformance>,
    pub status: Option<StatisticsStatusBreakdown>,
    pub insights: Vec<StatisticsInsight>,
}

#[derive(Debug, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OverviewBreakdownCapability {
    pub has_source: bool,
    pub has_tool: bool,
    pub has_cost: bool,
    pub has_status: bool,
    pub has_performance: bool,
}

#[derive(Debug, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OverviewBreakdownItem {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub color: Option<String>,
    pub icon: Option<String>,
    pub request_count: u64,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost: f64,
    pub percent: f64,
    pub success_requests: Option<u64>,
    pub error_requests: Option<u64>,
    pub avg_tokens_per_second: Option<f64>,
    pub avg_ttft_ms: Option<f64>,
    pub last_seen_ms: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverviewBreakdown {
    pub window: String,
    pub generated_at_epoch: i64,
    pub source_ranking: Vec<OverviewBreakdownItem>,
    pub tool_ranking: Vec<OverviewBreakdownItem>,
    pub model_ranking: Vec<OverviewBreakdownItem>,
    pub capability: OverviewBreakdownCapability,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRefreshBundle {
    pub generated_at_epoch: u64,
    pub snapshot: UsageSnapshot,
    pub rate_summary: WindowRateSummary,
    pub overview_breakdown: OverviewBreakdown,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthActivity {
    pub year: i32,
    pub month: u8,
    pub timezone: String,
    pub metric: StatisticsMetric,
    pub days: Vec<DayActivity>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YearActivity {
    pub year: i32,
    pub timezone: String,
    pub metric: StatisticsMetric,
    pub days: Vec<DayActivity>,
}

#[derive(Debug, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DayActivity {
    pub date: String,
    pub request_count: u64,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost: f64,
    pub model_count: u64,
    pub success_requests: Option<u64>,
    pub error_requests: Option<u64>,
}

pub(super) const MERGED_SOURCE: &str = "proxy-merged";
pub(super) const USAGE_WINDOWS: &[&str] = &["5h", "24h", "today", "7d", "30d", "current_month"];
pub(super) const MODEL_TREND_LIMIT: usize = 6;

pub(super) struct WindowPreparedFacts {
    pub(super) window: String,
    pub(super) start_index: usize,
}

pub(super) struct PreparedUsageRefreshData {
    pub(super) generated_at_epoch: u64,
    pub(super) facts: Vec<MergedRequestFact>,
    pub(super) windows: Vec<WindowPreparedFacts>,
}
