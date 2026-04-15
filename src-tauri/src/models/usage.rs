//! Usage data models

use serde::{Deserialize, Serialize};

/// 状态码计数
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusCodeCount {
    pub status_code: u16,
    pub count: u64,
}

/// 单模型速率统计（用于前端展示）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRateStats {
    /// 模型名称
    pub model_name: String,
    /// 请求数量
    pub request_count: u64,
    /// 总输出 Token 数
    pub total_output_tokens: u64,
    /// 总耗时（毫秒）
    pub total_duration_ms: u64,
    /// 平均生成速率（tokens/s）
    pub avg_tokens_per_second: f64,
    /// 最小生成速率（tokens/s）
    pub min_tokens_per_second: f64,
    /// 最大生成速率（tokens/s）
    pub max_tokens_per_second: f64,
}

/// 整体速率统计
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverallRateStats {
    /// 请求数量
    pub request_count: u64,
    /// 总输出 Token 数
    pub total_output_tokens: u64,
    /// 总耗时（毫秒）
    pub total_duration_ms: u64,
    /// 平均生成速率（tokens/s）
    pub avg_tokens_per_second: f64,
}

/// 窗口速率汇总（整体 + 按模型分组）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowRateSummary {
    /// 窗口名称
    pub window: String,
    /// 整体速率统计
    pub overall: OverallRateStats,
    /// 按模型分组的速率统计
    pub by_model: Vec<ModelRateStats>,
    /// TTFT 统计（首 Token 生成时间）
    pub ttft: TtftStats,
    /// 按模型分组的 TTFT 统计
    pub ttft_by_model: Vec<ModelTtftStats>,
}

/// TTFT 统计（首 Token 生成时间）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TtftStats {
    /// 请求数量
    pub request_count: u64,
    /// 平均 TTFT（毫秒）
    pub avg_ttft_ms: f64,
    /// 最小 TTFT（毫秒）
    pub min_ttft_ms: u64,
    /// 最大 TTFT（毫秒）
    pub max_ttft_ms: u64,
}

/// 单模型 TTFT 统计
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTtftStats {
    /// 模型名称
    pub model_name: String,
    /// 请求数量
    pub request_count: u64,
    /// 平均 TTFT（毫秒）
    pub avg_ttft_ms: f64,
    /// 最小 TTFT（毫秒）
    pub min_ttft_ms: u64,
    /// 最大 TTFT（毫秒）
    pub max_ttft_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowUsage {
    pub window: String,
    pub token_used: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub request_used: u64,
    pub token_limit: Option<u64>,
    pub request_limit: Option<u64>,
    pub token_percent: Option<f64>,
    pub request_percent: Option<f64>,
    pub risk_level: String,
    #[serde(default)]
    pub success_requests: u64,
    #[serde(default)]
    pub client_error_requests: u64,
    #[serde(default)]
    pub server_error_requests: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    pub model_name: String,
    pub token_used: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub request_count: u64,
    pub percent: f64,
    #[serde(default)]
    pub status_codes: Vec<StatusCodeCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub total_tokens: u64,
    pub total_requests: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_create_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_cost: f64,
    pub overall_risk_level: String,
    #[serde(default)]
    pub total_success_requests: u64,
    #[serde(default)]
    pub total_client_error_requests: u64,
    #[serde(default)]
    pub total_server_error_requests: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub generated_at_epoch: u64,
    pub windows: Vec<WindowUsage>,
    pub source: String,
    pub note: Option<String>,
    pub summary: UsageSummary,
    pub model_distribution: Vec<ModelUsage>,
}

/// 安全计算百分比
pub fn compute_percent(used: u64, limit: Option<u64>) -> Option<f64> {
    match limit {
        Some(0) => None,
        Some(value) => Some((used as f64 / value as f64) * 100.0),
        None => None,
    }
}

/// 根据百分比确定风险等级
pub fn risk_level(
    token_percent: Option<f64>,
    request_percent: Option<f64>,
    warning: u8,
    critical: u8,
) -> String {
    let max_percent = token_percent.unwrap_or(0.0).max(request_percent.unwrap_or(0.0));
    if max_percent >= critical as f64 {
        "critical".to_string()
    } else if max_percent >= warning as f64 {
        "warning".to_string()
    } else {
        "safe".to_string()
    }
}
