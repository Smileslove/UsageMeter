//! 用量相关 Tauri 命令

use crate::models::{
    compute_percent, risk_level, AppSettings, ModelRateStats, ModelTtftStats, OverallRateStats,
    SourceFilter, StatusCodeCount, ToolFilter, TtftStats, UsageQueryFilter, UsageSnapshot,
    WindowRateSummary, WindowUsage,
};
use crate::proxy::{ProxyServer, SessionStats, UsageRecord};
use chrono::{Datelike, Local, NaiveDate, TimeZone};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// 全局代理服务器状态
pub struct ProxyState {
    pub server: Arc<tokio::sync::RwLock<Option<ProxyServer>>>,
}

impl Default for ProxyState {
    fn default() -> Self {
        Self {
            server: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }
}

fn build_usage_query_filter(settings: &AppSettings) -> UsageQueryFilter {
    UsageQueryFilter {
        source: settings.source_aware.build_filter(),
        tool: settings.client_tools.build_filter(),
    }
}

fn is_usage_filter_all(filter: &UsageQueryFilter) -> bool {
    matches!(filter.source, SourceFilter::All) && matches!(filter.tool, ToolFilter::All)
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

/// 从数据源获取用量快照
#[tauri::command]
pub async fn get_usage_snapshot(
    settings: AppSettings,
    proxy_state: tauri::State<'_, ProxyState>,
) -> Result<UsageSnapshot, String> {
    // 检查是否使用代理模式
    if settings.data_source == "proxy" {
        return get_proxy_usage_snapshot(&settings, &proxy_state).await;
    }

    // 默认：使用 ccusage
    tauri::async_runtime::spawn_blocking(move || match snapshot_from_ccusage(&settings) {
        Ok(snapshot) => Ok(snapshot),
        Err(ccusage_err) => match snapshot_from_local_jsonl(&settings) {
            Ok(mut snapshot) => {
                snapshot.note = Some(format!("NOTE_LOCAL_JSONL_FALLBACK: {ccusage_err}"));
                Ok(snapshot)
            }
            Err(local_err) => Ok(empty_usage_snapshot(
                &settings,
                "no-data",
                format!("NOTE_NO_REAL_DATA: ccusage={ccusage_err}; local={local_err}"),
            )),
        },
    })
    .await
    .map_err(|e| format!("ERR_SNAPSHOT_TASK_FAILED: {e}"))?
}

/// 从代理收集器获取用量快照
async fn get_proxy_usage_snapshot(
    settings: &AppSettings,
    proxy_state: &ProxyState,
) -> Result<UsageSnapshot, String> {
    let server_guard = proxy_state.server.read().await;

    if let Some(server) = server_guard.as_ref() {
        // 从代理服务器获取用量收集器
        let collector = server.get_collector();
        // 读取设置：是否包含错误请求
        let include_errors = settings.proxy.include_error_requests;
        // 构建来源过滤器
        let usage_filter = build_usage_query_filter(settings);
        let window_stats = collector
            .get_all_window_stats_with_source(include_errors, &usage_filter)
            .await;
        let pricings = effective_model_pricings(settings);
        let match_mode = &settings.model_pricing.match_mode;
        let mut window_costs = HashMap::new();
        for quota in settings.quotas.iter().filter(|quota| quota.enabled) {
            let model_distribution = collector
                .get_model_distribution_with_source(&quota.window, include_errors, &usage_filter)
                .await;
            let cost =
                estimate_cost_from_model_distribution(&model_distribution, &pricings, match_mode);
            window_costs.insert(quota.window.clone(), cost);
        }
        drop(server_guard); // 提前释放锁

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let windows: Vec<WindowUsage> = settings
            .quotas
            .iter()
            .filter(|quota| quota.enabled)
            .map(|quota| {
                let stats = window_stats.get(&quota.window);
                let token_used = stats.map(|s| s.token_used).unwrap_or(0);
                let input_tokens = stats.map(|s| s.input_tokens).unwrap_or(0);
                let output_tokens = stats.map(|s| s.output_tokens).unwrap_or(0);
                let cache_create_tokens = stats.map(|s| s.cache_create_tokens).unwrap_or(0);
                let cache_read_tokens = stats.map(|s| s.cache_read_tokens).unwrap_or(0);
                let request_used = stats.map(|s| s.request_used).unwrap_or(0);
                let success_requests = stats.map(|s| s.success_requests).unwrap_or(0);
                let client_error_requests = stats.map(|s| s.client_error_requests).unwrap_or(0);
                let server_error_requests = stats.map(|s| s.server_error_requests).unwrap_or(0);

                let token_percent = compute_percent(token_used, quota.token_limit);
                let request_percent = compute_percent(request_used, quota.request_limit);

                WindowUsage {
                    window: quota.window.clone(),
                    token_used,
                    input_tokens,
                    output_tokens,
                    cache_create_tokens,
                    cache_read_tokens,
                    request_used,
                    token_limit: quota.token_limit,
                    request_limit: quota.request_limit,
                    token_percent,
                    request_percent,
                    risk_level: risk_level(
                        token_percent,
                        request_percent,
                        settings.warning_threshold,
                        settings.critical_threshold,
                    ),
                    cost: window_costs.get(&quota.window).copied().unwrap_or(0.0),
                    success_requests,
                    client_error_requests,
                    server_error_requests,
                }
            })
            .collect();

        // 计算总体风险等级
        let overall_risk_level = windows
            .iter()
            .map(|w| &w.risk_level)
            .max_by_key(|level| match level.as_str() {
                "critical" => 2,
                "warning" => 1,
                _ => 0,
            })
            .unwrap_or(&"safe".to_string())
            .clone();

        // 计算汇总（含状态码统计）
        let total_success_requests: u64 = windows.iter().map(|w| w.success_requests).sum();
        let total_client_error_requests: u64 =
            windows.iter().map(|w| w.client_error_requests).sum();
        let total_server_error_requests: u64 =
            windows.iter().map(|w| w.server_error_requests).sum();

        // 从收集器获取模型分布
        let model_distribution_raw = collector
            .get_model_distribution_with_source(
                &settings.summary_window,
                include_errors,
                &usage_filter,
            )
            .await;

        // 计算总 token 用于百分比
        let total_model_tokens: i64 = model_distribution_raw.iter().map(|m| m.total_tokens).sum();

        // 转换为前端 ModelUsage 格式，同时计算总费用
        let mut total_cost = 0.0;
        let model_distribution: Vec<crate::models::ModelUsage> = model_distribution_raw
            .into_iter()
            .map(|m| {
                let percent = if total_model_tokens > 0 {
                    (m.total_tokens as f64 / total_model_tokens as f64) * 100.0
                } else {
                    0.0
                };
                // 解析状态码 JSON
                let status_codes: Vec<crate::models::StatusCodeCount> =
                    serde_json::from_str(&m.status_codes_json).unwrap_or_default();

                // 计算该模型的费用
                let model_cost = crate::models::estimate_session_cost(
                    m.input_tokens as u64,
                    m.output_tokens as u64,
                    m.cache_create_tokens as u64,
                    m.cache_read_tokens as u64,
                    &m.model,
                    &pricings,
                    match_mode,
                );
                total_cost += model_cost;

                crate::models::ModelUsage {
                    model_name: m.model,
                    token_used: m.total_tokens as u64,
                    input_tokens: m.input_tokens as u64,
                    output_tokens: m.output_tokens as u64,
                    cache_create_tokens: m.cache_create_tokens as u64,
                    cache_read_tokens: m.cache_read_tokens as u64,
                    request_count: m.request_count as u64,
                    percent,
                    status_codes,
                }
            })
            .collect();

        // 更新 summary 中的总费用
        let summary = crate::models::UsageSummary {
            total_tokens: windows.iter().map(|w| w.token_used).sum(),
            total_requests: windows.iter().map(|w| w.request_used).sum(),
            total_input_tokens: windows.iter().map(|w| w.input_tokens).sum(),
            total_output_tokens: windows.iter().map(|w| w.output_tokens).sum(),
            total_cache_create_tokens: windows.iter().map(|w| w.cache_create_tokens).sum(),
            total_cache_read_tokens: windows.iter().map(|w| w.cache_read_tokens).sum(),
            total_cost,
            overall_risk_level,
            total_success_requests,
            total_client_error_requests,
            total_server_error_requests,
        };

        Ok(UsageSnapshot {
            generated_at_epoch: now,
            windows,
            source: "proxy".to_string(),
            note: None,
            summary,
            model_distribution,
        })
    } else {
        // 代理未运行，返回空数据并附带警告
        Ok(empty_usage_snapshot(
            settings,
            "proxy",
            "代理未运行 - 请先启动代理服务器".to_string(),
        ))
    }
}

fn empty_usage_snapshot(settings: &AppSettings, source: &str, note: String) -> UsageSnapshot {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let windows: Vec<WindowUsage> = settings
        .quotas
        .iter()
        .filter(|quota| quota.enabled)
        .map(|quota| WindowUsage {
            window: quota.window.clone(),
            token_used: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_create_tokens: 0,
            cache_read_tokens: 0,
            request_used: 0,
            token_limit: quota.token_limit,
            request_limit: quota.request_limit,
            token_percent: compute_percent(0, quota.token_limit),
            request_percent: compute_percent(0, quota.request_limit),
            risk_level: "safe".to_string(),
            cost: 0.0,
            success_requests: 0,
            client_error_requests: 0,
            server_error_requests: 0,
        })
        .collect();

    let summary = crate::models::UsageSummary {
        total_tokens: 0,
        total_requests: 0,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_cache_create_tokens: 0,
        total_cache_read_tokens: 0,
        total_cost: 0.0,
        overall_risk_level: "safe".to_string(),
        total_success_requests: 0,
        total_client_error_requests: 0,
        total_server_error_requests: 0,
    };

    UsageSnapshot {
        generated_at_epoch: now,
        windows,
        source: source.to_string(),
        note: Some(note),
        summary,
        model_distribution: Vec::new(),
    }
}

fn snapshot_from_ccusage(settings: &AppSettings) -> Result<UsageSnapshot, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let value = run_ccusage_json()?;
    let windows_value = value
        .get("windows")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "ERR_CCUSAGE_MISSING_WINDOWS".to_string())?;

    let source = value
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("ccusage-api")
        .to_string();
    let pricings = effective_model_pricings(settings);
    let match_mode = &settings.model_pricing.match_mode;

    let mut windows = Vec::new();
    for quota in &settings.quotas {
        if !quota.enabled {
            continue;
        }

        let metric = windows_value
            .iter()
            .find(|item| {
                item.get("window")
                    .and_then(|v| v.as_str())
                    .map(|w| w == quota.window)
                    .unwrap_or(false)
            })
            .ok_or_else(|| format!("ERR_CCUSAGE_MISSING_WINDOW_METRIC: {}", quota.window))?;

        let token_used = metric
            .get("tokenUsed")
            .or_else(|| metric.get("token_used"))
            .and_then(parse_u64_from_value)
            .unwrap_or(0);

        let input_tokens = metric
            .get("inputTokens")
            .or_else(|| metric.get("input_tokens"))
            .and_then(parse_u64_from_value)
            .unwrap_or(0);

        let output_tokens = metric
            .get("outputTokens")
            .or_else(|| metric.get("output_tokens"))
            .and_then(parse_u64_from_value)
            .unwrap_or(0);

        let cache_create_tokens = metric
            .get("cacheCreateTokens")
            .or_else(|| metric.get("cache_create_tokens"))
            .and_then(parse_u64_from_value)
            .unwrap_or(0);

        let cache_read_tokens = metric
            .get("cacheReadTokens")
            .or_else(|| metric.get("cache_read_tokens"))
            .and_then(parse_u64_from_value)
            .unwrap_or(0);

        let request_used = metric
            .get("requestUsed")
            .or_else(|| metric.get("request_used"))
            .and_then(parse_u64_from_value)
            .unwrap_or(0);

        // ccusage 只负责提供窗口内按模型拆分的 token 用量；
        // 费用统一使用 UsageMeter 本地价格表逐模型计算，避免和自定义价格口径不一致。
        let window_cost =
            estimate_ccusage_window_cost(&value, &quota.window, &pricings, match_mode);

        let token_percent = compute_percent(token_used, quota.token_limit);
        let request_percent = compute_percent(request_used, quota.request_limit);

        windows.push(WindowUsage {
            window: quota.window.clone(),
            token_used,
            input_tokens,
            output_tokens,
            cache_create_tokens,
            cache_read_tokens,
            request_used,
            token_limit: quota.token_limit,
            request_limit: quota.request_limit,
            token_percent,
            request_percent,
            risk_level: risk_level(
                token_percent,
                request_percent,
                settings.warning_threshold,
                settings.critical_threshold,
            ),
            cost: window_cost,
            success_requests: 0, // ccusage 模式不包含状态码信息
            client_error_requests: 0,
            server_error_requests: 0,
        });
    }

    // 解析模型分布
    let model_distribution: Vec<crate::models::ModelUsage> = value
        .get("modelDistribution")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    Some(crate::models::ModelUsage {
                        model_name: item.get("modelName")?.as_str()?.to_string(),
                        token_used: parse_u64_from_value(item.get("tokenUsed")?)?,
                        input_tokens: parse_u64_from_value(item.get("inputTokens")?)?,
                        output_tokens: parse_u64_from_value(item.get("outputTokens")?)?,
                        cache_create_tokens: item
                            .get("cacheCreateTokens")
                            .and_then(parse_u64_from_value)
                            .unwrap_or(0),
                        cache_read_tokens: item
                            .get("cacheReadTokens")
                            .and_then(parse_u64_from_value)
                            .unwrap_or(0),
                        request_count: parse_u64_from_value(item.get("requestCount")?)?,
                        percent: item.get("percent").and_then(|v| v.as_f64()).unwrap_or(0.0),
                        // ccusage 模式不包含状态码信息
                        status_codes: Vec::new(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // 计算总体风险等级
    let overall_risk_level = windows
        .iter()
        .map(|w| &w.risk_level)
        .max_by_key(|level| match level.as_str() {
            "critical" => 2,
            "warning" => 1,
            _ => 0,
        })
        .unwrap_or(&"safe".to_string())
        .clone();

    // 总费用使用 summaryWindow 的费用，和概览面板展示窗口保持一致。
    let summary_window = &settings.summary_window;
    let total_cost = windows
        .iter()
        .find(|w| &w.window == summary_window)
        .map(|w| w.cost)
        .unwrap_or(0.0);

    let summary = crate::models::UsageSummary {
        total_tokens: windows.iter().map(|w| w.token_used).sum(),
        total_requests: windows.iter().map(|w| w.request_used).sum(),
        total_input_tokens: windows.iter().map(|w| w.input_tokens).sum(),
        total_output_tokens: windows.iter().map(|w| w.output_tokens).sum(),
        total_cache_create_tokens: windows.iter().map(|w| w.cache_create_tokens).sum(),
        total_cache_read_tokens: windows.iter().map(|w| w.cache_read_tokens).sum(),
        total_cost,
        overall_risk_level,
        total_success_requests: 0, // ccusage 模式不包含状态码信息
        total_client_error_requests: 0,
        total_server_error_requests: 0,
    };

    Ok(UsageSnapshot {
        generated_at_epoch: now,
        windows,
        source,
        note: None,
        summary,
        model_distribution,
    })
}

fn run_ccusage_json() -> Result<serde_json::Value, String> {
    let app_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| "ERR_PROJECT_ROOT_NOT_FOUND".to_string())?
        .to_path_buf();

    let script_path = app_root.join("scripts").join("ccusage-snapshot.mjs");

    // Windows 兼容：先尝试直接调用 node，失败时使用 cmd /c node
    let output = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/c", "node"])
            .current_dir(&app_root)
            .arg(&script_path)
            .output()
            .or_else(|_| {
                Command::new("node")
                    .current_dir(&app_root)
                    .arg(&script_path)
                    .output()
            })
            .map_err(|e| format!("ERR_CCUSAGE_NODE_NOT_FOUND: Node.js is required to read ccusage data. Please install Node.js and ensure it is in your PATH. Error: {e}"))?
    } else {
        Command::new("node")
            .current_dir(&app_root)
            .arg(&script_path)
            .output()
            .map_err(|e| format!("ERR_CCUSAGE_NODE_NOT_FOUND: Node.js is required to read ccusage data. Please install Node.js and ensure it is in your PATH. Error: {e}"))?
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ERR_CCUSAGE_SCRIPT_FAILED: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Err("ERR_CCUSAGE_OUTPUT_EMPTY".to_string());
    }

    serde_json::from_str(&stdout).map_err(|e| format!("ERR_CCUSAGE_PARSE_JSON_FAILED: {e}"))
}

fn estimate_cost_from_model_distribution(
    models: &[crate::proxy::ModelDistribution],
    pricings: &[crate::models::ModelPricingConfig],
    match_mode: &str,
) -> f64 {
    models
        .iter()
        .map(|model| {
            crate::models::estimate_session_cost(
                model.input_tokens.max(0) as u64,
                model.output_tokens.max(0) as u64,
                model.cache_create_tokens.max(0) as u64,
                model.cache_read_tokens.max(0) as u64,
                &model.model,
                pricings,
                match_mode,
            )
        })
        .sum()
}

fn estimate_ccusage_window_cost(
    value: &serde_json::Value,
    window: &str,
    pricings: &[crate::models::ModelPricingConfig],
    match_mode: &str,
) -> f64 {
    let Some(models) = value
        .get("windowModelDistribution")
        .and_then(|v| v.get(window))
        .and_then(|v| v.as_array())
    else {
        return 0.0;
    };

    models
        .iter()
        .filter_map(|model| {
            let model_name = model.get("modelName")?.as_str()?;
            Some(crate::models::estimate_session_cost(
                model
                    .get("inputTokens")
                    .and_then(parse_u64_from_value)
                    .unwrap_or(0),
                model
                    .get("outputTokens")
                    .and_then(parse_u64_from_value)
                    .unwrap_or(0),
                model
                    .get("cacheCreateTokens")
                    .and_then(parse_u64_from_value)
                    .unwrap_or(0),
                model
                    .get("cacheReadTokens")
                    .and_then(parse_u64_from_value)
                    .unwrap_or(0),
                model_name,
                pricings,
                match_mode,
            ))
        })
        .sum()
}

fn effective_model_pricings(settings: &AppSettings) -> Vec<crate::models::ModelPricingConfig> {
    let mut pricings = settings.model_pricing.pricings.clone();

    match crate::proxy::ProxyDatabase::new().and_then(|db| db.get_all_model_pricings()) {
        Ok(db_pricings) => pricings.extend(db_pricings),
        Err(e) => eprintln!("[usage] Failed to load model pricing database: {e}"),
    }

    pricings
}

fn snapshot_from_local_jsonl(settings: &AppSettings) -> Result<UsageSnapshot, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let files = collect_claude_jsonl_files();
    if files.is_empty() {
        return Err("ERR_LOCAL_JSONL_NOT_FOUND".to_string());
    }

    // 使用 HashMap 按 message.id 去重，保留最新（token 数最多）的记录
    let mut request_map: std::collections::HashMap<String, RequestRecord> =
        std::collections::HashMap::new();

    for file in files {
        let file_handle = match fs::File::open(&file) {
            Ok(h) => h,
            Err(_) => continue,
        };
        let reader = BufReader::new(file_handle);

        for line in reader.lines().map_while(Result::ok) {
            let parsed: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // 仅处理助手类型的消息
            if parsed.get("type").and_then(|t| t.as_str()) != Some("assistant") {
                continue;
            }

            let message_id = match extract_message_id(&parsed) {
                Some(id) => id,
                None => continue,
            };

            let event_time = match extract_event_epoch(&parsed) {
                Some(t) if now >= t => t,
                _ => continue,
            };

            let tokens = extract_total_tokens(&parsed).unwrap_or(0);
            if tokens == 0 {
                continue;
            }

            let breakdown = extract_token_breakdown(&parsed);

            // 提取模型名称
            let model = extract_model_name(&parsed);

            // 对于相同的 message.id，保留 token 数最多的记录（最终统计）
            request_map
                .entry(message_id)
                .and_modify(|existing| {
                    if tokens > existing.tokens {
                        existing.tokens = tokens;
                        existing.input_tokens = breakdown.input;
                        existing.output_tokens = breakdown.output;
                        existing.cache_create_tokens = breakdown.cache_create;
                        existing.cache_read_tokens = breakdown.cache_read;
                        existing.timestamp = event_time;
                        existing.model = model.clone();
                    }
                })
                .or_insert(RequestRecord {
                    timestamp: event_time,
                    tokens,
                    input_tokens: breakdown.input,
                    output_tokens: breakdown.output,
                    cache_create_tokens: breakdown.cache_create,
                    cache_read_tokens: breakdown.cache_read,
                    model,
                });
        }
    }

    // 计算各时间窗口的统计数据
    let mut total_5h_tokens = 0_u64;
    let mut total_5h_input_tokens = 0_u64;
    let mut total_5h_output_tokens = 0_u64;
    let mut total_5h_cache_create_tokens = 0_u64;
    let mut total_5h_cache_read_tokens = 0_u64;
    let mut total_5h_requests = 0_u64;
    let mut total_24h_tokens = 0_u64;
    let mut total_24h_input_tokens = 0_u64;
    let mut total_24h_output_tokens = 0_u64;
    let mut total_24h_cache_create_tokens = 0_u64;
    let mut total_24h_cache_read_tokens = 0_u64;
    let mut total_24h_requests = 0_u64;
    let mut total_today_tokens = 0_u64;
    let mut total_today_input_tokens = 0_u64;
    let mut total_today_output_tokens = 0_u64;
    let mut total_today_cache_create_tokens = 0_u64;
    let mut total_today_cache_read_tokens = 0_u64;
    let mut total_today_requests = 0_u64;
    let mut total_7d_tokens = 0_u64;
    let mut total_7d_input_tokens = 0_u64;
    let mut total_7d_output_tokens = 0_u64;
    let mut total_7d_cache_create_tokens = 0_u64;
    let mut total_7d_cache_read_tokens = 0_u64;
    let mut total_7d_requests = 0_u64;
    let mut total_30d_tokens = 0_u64;
    let mut total_30d_input_tokens = 0_u64;
    let mut total_30d_output_tokens = 0_u64;
    let mut total_30d_cache_create_tokens = 0_u64;
    let mut total_30d_cache_read_tokens = 0_u64;
    let mut total_30d_requests = 0_u64;
    let mut total_current_month_tokens = 0_u64;
    let mut total_current_month_input_tokens = 0_u64;
    let mut total_current_month_output_tokens = 0_u64;
    let mut total_current_month_cache_create_tokens = 0_u64;
    let mut total_current_month_cache_read_tokens = 0_u64;
    let mut total_current_month_requests = 0_u64;

    // 计算当前月份起始时间戳（本月第1天，00:00:00 本地时间）
    let current_month_start = {
        let now_dt = Local
            .timestamp_opt(now as i64, 0)
            .single()
            .unwrap_or_else(Local::now);
        Local
            .with_ymd_and_hms(now_dt.year(), now_dt.month(), 1, 0, 0, 0)
            .single()
            .map(|dt| dt.timestamp() as u64)
            .unwrap_or(0)
    };

    // 计算今天起始时间戳（今天 00:00:00 本地时间）
    let today_start = {
        let now_dt = Local
            .timestamp_opt(now as i64, 0)
            .single()
            .unwrap_or_else(Local::now);
        now_dt
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .map(|dt| Local.from_local_datetime(&dt).unwrap().timestamp() as u64)
            .unwrap_or(0)
    };

    // 模型分布统计（仅统计30天内的数据）
    // (tokens, input, output, cache_create, cache_read, requests) 总Token, 输入, 输出, 缓存创建, 缓存读取, 请求数
    let mut model_stats: HashMap<String, (u64, u64, u64, u64, u64, u64)> = HashMap::new();
    let mut window_model_stats: HashMap<String, HashMap<String, ModelTokenTotals>> = HashMap::new();
    let pricings = effective_model_pricings(settings);
    let match_mode = &settings.model_pricing.match_mode;

    for record in request_map.values() {
        let age = now - record.timestamp;
        if age <= 5 * 60 * 60 {
            total_5h_tokens += record.tokens;
            total_5h_input_tokens += record.input_tokens;
            total_5h_output_tokens += record.output_tokens;
            total_5h_cache_create_tokens += record.cache_create_tokens;
            total_5h_cache_read_tokens += record.cache_read_tokens;
            total_5h_requests += 1;
            add_window_model_stats(&mut window_model_stats, "5h", record);
        }
        if age <= 24 * 60 * 60 {
            total_24h_tokens += record.tokens;
            total_24h_input_tokens += record.input_tokens;
            total_24h_output_tokens += record.output_tokens;
            total_24h_cache_create_tokens += record.cache_create_tokens;
            total_24h_cache_read_tokens += record.cache_read_tokens;
            total_24h_requests += 1;
            add_window_model_stats(&mut window_model_stats, "24h", record);
        }
        // 今天：记录时间戳在今天内
        if record.timestamp >= today_start {
            total_today_tokens += record.tokens;
            total_today_input_tokens += record.input_tokens;
            total_today_output_tokens += record.output_tokens;
            total_today_cache_create_tokens += record.cache_create_tokens;
            total_today_cache_read_tokens += record.cache_read_tokens;
            total_today_requests += 1;
            add_window_model_stats(&mut window_model_stats, "today", record);
        }
        if age <= 7 * 24 * 60 * 60 {
            total_7d_tokens += record.tokens;
            total_7d_input_tokens += record.input_tokens;
            total_7d_output_tokens += record.output_tokens;
            total_7d_cache_create_tokens += record.cache_create_tokens;
            total_7d_cache_read_tokens += record.cache_read_tokens;
            total_7d_requests += 1;
            add_window_model_stats(&mut window_model_stats, "7d", record);
        }
        if age <= 30 * 24 * 60 * 60 {
            total_30d_tokens += record.tokens;
            total_30d_input_tokens += record.input_tokens;
            total_30d_output_tokens += record.output_tokens;
            total_30d_cache_create_tokens += record.cache_create_tokens;
            total_30d_cache_read_tokens += record.cache_read_tokens;
            total_30d_requests += 1;
            add_window_model_stats(&mut window_model_stats, "30d", record);

            // 累计模型统计
            if !record.model.is_empty() {
                let entry = model_stats
                    .entry(record.model.clone())
                    .or_insert((0, 0, 0, 0, 0, 0));
                entry.0 += record.tokens;
                entry.1 += record.input_tokens;
                entry.2 += record.output_tokens;
                entry.3 += record.cache_create_tokens;
                entry.4 += record.cache_read_tokens;
                entry.5 += 1;
            }
        }
        // 当前月份：记录时间戳在本月内
        if record.timestamp >= current_month_start {
            total_current_month_tokens += record.tokens;
            total_current_month_input_tokens += record.input_tokens;
            total_current_month_output_tokens += record.output_tokens;
            total_current_month_cache_create_tokens += record.cache_create_tokens;
            total_current_month_cache_read_tokens += record.cache_read_tokens;
            total_current_month_requests += 1;
            add_window_model_stats(&mut window_model_stats, "current_month", record);
        }
    }

    let mut windows = Vec::new();
    for quota in &settings.quotas {
        if !quota.enabled {
            continue;
        }

        let (
            token_used,
            input_tokens,
            output_tokens,
            cache_create_tokens,
            cache_read_tokens,
            request_used,
        ) = match quota.window.as_str() {
            "5h" => (
                total_5h_tokens,
                total_5h_input_tokens,
                total_5h_output_tokens,
                total_5h_cache_create_tokens,
                total_5h_cache_read_tokens,
                total_5h_requests,
            ),
            "24h" => (
                total_24h_tokens,
                total_24h_input_tokens,
                total_24h_output_tokens,
                total_24h_cache_create_tokens,
                total_24h_cache_read_tokens,
                total_24h_requests,
            ),
            "today" => (
                total_today_tokens,
                total_today_input_tokens,
                total_today_output_tokens,
                total_today_cache_create_tokens,
                total_today_cache_read_tokens,
                total_today_requests,
            ),
            "7d" => (
                total_7d_tokens,
                total_7d_input_tokens,
                total_7d_output_tokens,
                total_7d_cache_create_tokens,
                total_7d_cache_read_tokens,
                total_7d_requests,
            ),
            "30d" => (
                total_30d_tokens,
                total_30d_input_tokens,
                total_30d_output_tokens,
                total_30d_cache_create_tokens,
                total_30d_cache_read_tokens,
                total_30d_requests,
            ),
            "current_month" => (
                total_current_month_tokens,
                total_current_month_input_tokens,
                total_current_month_output_tokens,
                total_current_month_cache_create_tokens,
                total_current_month_cache_read_tokens,
                total_current_month_requests,
            ),
            _ => (0, 0, 0, 0, 0, 0),
        };

        let token_percent = compute_percent(token_used, quota.token_limit);
        let request_percent = compute_percent(request_used, quota.request_limit);

        windows.push(WindowUsage {
            window: quota.window.clone(),
            token_used,
            input_tokens,
            output_tokens,
            cache_create_tokens,
            cache_read_tokens,
            request_used,
            token_limit: quota.token_limit,
            request_limit: quota.request_limit,
            token_percent,
            request_percent,
            risk_level: risk_level(
                token_percent,
                request_percent,
                settings.warning_threshold,
                settings.critical_threshold,
            ),
            cost: estimate_cost_from_window_model_stats(
                window_model_stats.get(&quota.window),
                &pricings,
                match_mode,
            ),
            success_requests: 0, // 本地 JSONL 模式不包含状态码信息
            client_error_requests: 0,
            server_error_requests: 0,
        });
    }

    // 计算总体风险等级
    let overall_risk_level = windows
        .iter()
        .map(|w| &w.risk_level)
        .max_by_key(|level| match level.as_str() {
            "critical" => 2,
            "warning" => 1,
            _ => 0,
        })
        .unwrap_or(&"safe".to_string())
        .clone();

    // 计算模型分布
    let total_model_tokens: u64 = model_stats.values().map(|(t, _, _, _, _, _)| t).sum();

    // 计算总费用（在截断之前，基于所有模型计算）
    let total_cost: f64 = model_stats
        .iter()
        .map(
            |(model_name, (_tokens, input, output, cache_create, cache_read, _requests))| {
                crate::models::estimate_session_cost(
                    *input,
                    *output,
                    *cache_create,
                    *cache_read,
                    model_name,
                    &pricings,
                    match_mode,
                )
            },
        )
        .sum();

    let mut model_distribution: Vec<crate::models::ModelUsage> = model_stats
        .into_iter()
        .map(
            |(model_name, (tokens, input, output, cache_create, cache_read, requests))| {
                let percent = if total_model_tokens > 0 {
                    (tokens as f64 / total_model_tokens as f64) * 100.0
                } else {
                    0.0
                };
                crate::models::ModelUsage {
                    model_name,
                    token_used: tokens,
                    input_tokens: input,
                    output_tokens: output,
                    cache_create_tokens: cache_create,
                    cache_read_tokens: cache_read,
                    request_count: requests,
                    percent,
                    status_codes: Vec::new(), // 本地 JSONL 模式不包含状态码信息
                }
            },
        )
        .collect();
    // 按 token 使用量降序排序，取 Top 5（仅用于显示，不影响费用计算）
    model_distribution.sort_by_key(|b| std::cmp::Reverse(b.token_used));
    model_distribution.truncate(5);

    let summary = crate::models::UsageSummary {
        total_tokens: windows.iter().map(|w| w.token_used).sum(),
        total_requests: windows.iter().map(|w| w.request_used).sum(),
        total_input_tokens: windows.iter().map(|w| w.input_tokens).sum(),
        total_output_tokens: windows.iter().map(|w| w.output_tokens).sum(),
        total_cache_create_tokens: windows.iter().map(|w| w.cache_create_tokens).sum(),
        total_cache_read_tokens: windows.iter().map(|w| w.cache_read_tokens).sum(),
        total_cost,
        overall_risk_level,
        total_success_requests: 0,
        total_client_error_requests: 0,
        total_server_error_requests: 0,
    };

    Ok(UsageSnapshot {
        generated_at_epoch: now,
        windows,
        source: "local-jsonl".to_string(),
        note: Some("NOTE_LOCAL_JSONL_FALLBACK".to_string()),
        summary,
        model_distribution,
    })
}

// 辅助类型和函数
struct RequestRecord {
    timestamp: u64,
    tokens: u64, // total_tokens: 总 Token = input + cache_create + cache_read + output
    input_tokens: u64, // input_tokens: 实际输入（不含缓存）
    output_tokens: u64,
    cache_create_tokens: u64,
    cache_read_tokens: u64,
    model: String, // model: 模型名称
}

#[derive(Default)]
struct ModelTokenTotals {
    input_tokens: u64,
    output_tokens: u64,
    cache_create_tokens: u64,
    cache_read_tokens: u64,
}

fn add_window_model_stats(
    window_model_stats: &mut HashMap<String, HashMap<String, ModelTokenTotals>>,
    window: &str,
    record: &RequestRecord,
) {
    if record.model.is_empty() {
        return;
    }

    let entry = window_model_stats
        .entry(window.to_string())
        .or_default()
        .entry(record.model.clone())
        .or_default();
    entry.input_tokens += record.input_tokens;
    entry.output_tokens += record.output_tokens;
    entry.cache_create_tokens += record.cache_create_tokens;
    entry.cache_read_tokens += record.cache_read_tokens;
}

fn estimate_cost_from_window_model_stats(
    window_stats: Option<&HashMap<String, ModelTokenTotals>>,
    pricings: &[crate::models::ModelPricingConfig],
    match_mode: &str,
) -> f64 {
    window_stats
        .into_iter()
        .flat_map(|stats| stats.iter())
        .map(|(model_name, totals)| {
            crate::models::estimate_session_cost(
                totals.input_tokens,
                totals.output_tokens,
                totals.cache_create_tokens,
                totals.cache_read_tokens,
                model_name,
                pricings,
                match_mode,
            )
        })
        .sum()
}

fn collect_claude_jsonl_files() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".config").join("claude").join("projects"));
        roots.push(home.join(".claude").join("projects"));
    }

    let mut queue: VecDeque<PathBuf> = roots.into_iter().filter(|p| p.exists()).collect();
    let mut files = Vec::new();

    while let Some(path) = queue.pop_front() {
        if let Ok(read_dir) = fs::read_dir(path) {
            for entry in read_dir.flatten() {
                let entry_path = entry.path();
                if entry_path.is_dir() {
                    queue.push_back(entry_path);
                } else if entry_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("jsonl"))
                    .unwrap_or(false)
                {
                    files.push(entry_path);
                }
            }
        }
    }

    files
}

fn extract_message_id(value: &serde_json::Value) -> Option<String> {
    value
        .get("message")
        .and_then(|m| m.get("id"))
        .and_then(|id| id.as_str())
        .map(|s| s.to_string())
}

fn extract_event_epoch(value: &serde_json::Value) -> Option<u64> {
    let ts_keys = ["timestamp", "created_at", "createdAt", "time", "date"];

    let raw_ts = ts_keys
        .iter()
        .find_map(|k| find_number_by_keys(value, &[*k]))
        .or_else(|| match value {
            serde_json::Value::Object(map) => ts_keys.iter().find_map(|k| {
                map.get(*k).and_then(|v| v.as_str()).and_then(|text| {
                    chrono::DateTime::parse_from_rfc3339(text)
                        .ok()
                        .map(|dt| dt.timestamp() as u64)
                })
            }),
            _ => None,
        });

    raw_ts.map(|num| {
        if num > 10_000_000_000 {
            num / 1000
        } else {
            num
        }
    })
}

fn extract_total_tokens(value: &serde_json::Value) -> Option<u64> {
    // 计算总 Token：input + cache_create + cache_read + output（含缓存）
    let in_keys = ["input_tokens", "inputTokens", "input"];
    let out_keys = ["output_tokens", "outputTokens", "output"];
    let cache_create_keys = [
        "cache_creation_input_tokens",
        "cacheCreationInputTokens",
        "cache_create_tokens",
    ];
    let cache_read_keys = [
        "cache_read_input_tokens",
        "cacheReadInputTokens",
        "cache_read_tokens",
    ];

    let input = find_number_by_keys(value, &in_keys).unwrap_or(0);
    let output = find_number_by_keys(value, &out_keys).unwrap_or(0);
    let cache_create = find_number_by_keys(value, &cache_create_keys).unwrap_or(0);
    let cache_read = find_number_by_keys(value, &cache_read_keys).unwrap_or(0);

    let sum = input + cache_create + cache_read + output;
    if sum > 0 {
        Some(sum)
    } else {
        None
    }
}

fn extract_model_name(value: &serde_json::Value) -> String {
    // 模型名称可能在 message.model 或直接在顶层 model 字段
    value
        .get("message")
        .and_then(|m| m.get("model"))
        .and_then(|m| m.as_str())
        .or_else(|| value.get("model").and_then(|m| m.as_str()))
        .unwrap_or("unknown")
        .to_string()
}

struct TokenBreakdown {
    input: u64,
    output: u64,
    cache_create: u64,
    cache_read: u64,
}

fn extract_token_breakdown(value: &serde_json::Value) -> TokenBreakdown {
    let in_keys = ["input_tokens", "inputTokens", "input"];
    let out_keys = ["output_tokens", "outputTokens", "output"];
    let cache_create_keys = ["cache_creation_input_tokens", "cacheCreateTokens"];
    let cache_read_keys = ["cache_read_input_tokens", "cacheReadTokens"];

    TokenBreakdown {
        input: find_number_by_keys(value, &in_keys).unwrap_or(0),
        output: find_number_by_keys(value, &out_keys).unwrap_or(0),
        cache_create: find_number_by_keys(value, &cache_create_keys).unwrap_or(0),
        cache_read: find_number_by_keys(value, &cache_read_keys).unwrap_or(0),
    }
}

fn find_number_by_keys(value: &serde_json::Value, keys: &[&str]) -> Option<u64> {
    match value {
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(found) = map.get(*key).and_then(parse_u64_from_value) {
                    return Some(found);
                }
            }

            for child in map.values() {
                if let Some(found) = find_number_by_keys(child, keys) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(items) => {
            for item in items {
                if let Some(found) = find_number_by_keys(item, keys) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn parse_u64_from_value(value: &serde_json::Value) -> Option<u64> {
    if let Some(v) = value.as_u64() {
        return Some(v);
    }
    if let Some(v) = value.as_f64() {
        return Some(v.max(0.0) as u64);
    }
    if let Some(v) = value.as_i64() {
        return Some(v.max(0) as u64);
    }
    None
}

#[derive(Default, Clone)]
struct StatAccumulator {
    request_count: u64,
    total_tokens: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_create_tokens: u64,
    cache_read_tokens: u64,
    cost: f64,
    success_requests: u64,
    client_error_requests: u64,
    server_error_requests: u64,
    rate_sum: f64,
    rate_count: u64,
    ttft_sum: f64,
    ttft_count: u64,
    status_code_counts: HashMap<u16, u64>,
}

impl StatAccumulator {
    fn add_tokens(
        &mut self,
        input: u64,
        output: u64,
        cache_create: u64,
        cache_read: u64,
        requests: u64,
        cost: f64,
    ) {
        self.request_count += requests;
        self.input_tokens += input;
        self.output_tokens += output;
        self.cache_create_tokens += cache_create;
        self.cache_read_tokens += cache_read;
        // 总 Token = 输入 + 缓存读取 + 输出（不包含缓存创建）
        self.total_tokens += input + output + cache_read;
        self.cost += cost;
    }

    fn add_record(&mut self, record: &UsageRecord, cost: f64) {
        self.add_tokens(
            record.input_tokens,
            record.output_tokens,
            record.cache_create_tokens,
            record.cache_read_tokens,
            1,
            cost,
        );

        if (200..300).contains(&record.status_code) {
            self.success_requests += 1;
        } else if (400..500).contains(&record.status_code) {
            self.client_error_requests += 1;
        } else if record.status_code >= 500 {
            self.server_error_requests += 1;
        }
        *self
            .status_code_counts
            .entry(record.status_code)
            .or_insert(0) += 1;

        if let Some(rate) = record.output_tokens_per_second {
            if rate > 0.0 {
                self.rate_sum += rate;
                self.rate_count += 1;
            }
        }
        if let Some(ttft) = record.ttft_ms {
            if ttft > 0 {
                self.ttft_sum += ttft as f64;
                self.ttft_count += 1;
            }
        }
    }
}

fn normalize_range(query: &StatisticsQuery) -> (i64, i64) {
    let start = query.start_epoch.max(0);
    let end = query.end_epoch.max(start + 1);
    (start, end)
}

fn bucket_step_seconds(bucket: &StatisticsBucket) -> i64 {
    match bucket {
        StatisticsBucket::Hour => 60 * 60,
        StatisticsBucket::Day => 24 * 60 * 60,
    }
}

fn bucket_name(bucket: &StatisticsBucket) -> String {
    match bucket {
        StatisticsBucket::Hour => "hour".to_string(),
        StatisticsBucket::Day => "day".to_string(),
    }
}

fn bucket_start(epoch: i64, bucket: &StatisticsBucket) -> i64 {
    let step = bucket_step_seconds(bucket);
    (epoch / step) * step
}

fn bucket_label(epoch: i64, bucket: &StatisticsBucket) -> String {
    let dt = Local
        .timestamp_opt(epoch, 0)
        .single()
        .unwrap_or_else(Local::now);
    match bucket {
        StatisticsBucket::Hour => dt.format("%m-%d %H:00").to_string(),
        StatisticsBucket::Day => dt.format("%m-%d").to_string(),
    }
}

fn make_empty_trend(
    start_epoch: i64,
    end_epoch: i64,
    bucket: &StatisticsBucket,
) -> Vec<StatisticsTrendPoint> {
    let step = bucket_step_seconds(bucket);
    let mut points = Vec::new();
    let mut cursor = bucket_start(start_epoch, bucket);
    while cursor < end_epoch {
        points.push(StatisticsTrendPoint {
            start_epoch: cursor,
            label: bucket_label(cursor, bucket),
            ..Default::default()
        });
        cursor += step;
    }
    points
}

fn apply_acc_to_trend_point(point: &mut StatisticsTrendPoint, acc: &StatAccumulator) {
    point.request_count = acc.request_count;
    point.total_tokens = acc.total_tokens;
    point.input_tokens = acc.input_tokens;
    point.output_tokens = acc.output_tokens;
    point.cache_create_tokens = acc.cache_create_tokens;
    point.cache_read_tokens = acc.cache_read_tokens;
    point.cost = acc.cost;
    point.avg_tokens_per_second =
        (acc.rate_count > 0).then_some(acc.rate_sum / acc.rate_count as f64);
}

fn trend_from_map(
    trend_map: &HashMap<i64, StatAccumulator>,
    start_epoch: i64,
    end_epoch: i64,
    bucket: &StatisticsBucket,
) -> Vec<StatisticsTrendPoint> {
    let mut trend = make_empty_trend(start_epoch, end_epoch, bucket);
    for point in &mut trend {
        if let Some(acc) = trend_map.get(&point.start_epoch) {
            apply_acc_to_trend_point(point, acc);
        }
    }
    trend
}

fn value_for_metric(point: &StatisticsTrendPoint, metric: &StatisticsMetric) -> f64 {
    match metric {
        StatisticsMetric::Cost => point.cost,
        StatisticsMetric::Requests => point.request_count as f64,
        StatisticsMetric::Tokens => point.total_tokens as f64,
    }
}

fn totals_from_acc(acc: &StatAccumulator, model_count: u64, with_status: bool) -> StatisticsTotals {
    let error_requests = acc.client_error_requests + acc.server_error_requests;
    StatisticsTotals {
        request_count: acc.request_count,
        total_tokens: acc.total_tokens,
        input_tokens: acc.input_tokens,
        output_tokens: acc.output_tokens,
        cache_create_tokens: acc.cache_create_tokens,
        cache_read_tokens: acc.cache_read_tokens,
        cost: acc.cost,
        model_count,
        success_requests: with_status.then_some(acc.success_requests),
        error_requests: with_status.then_some(error_requests),
    }
}

fn build_insights(
    totals: &StatisticsTotals,
    trend: &[StatisticsTrendPoint],
    models: &[StatisticsModelBreakdown],
    metric: &StatisticsMetric,
    performance: Option<&StatisticsPerformance>,
) -> Vec<StatisticsInsight> {
    let mut insights = Vec::new();

    if let Some(peak) = trend.iter().max_by(|a, b| {
        value_for_metric(a, metric)
            .partial_cmp(&value_for_metric(b, metric))
            .unwrap_or(std::cmp::Ordering::Equal)
    }) {
        if value_for_metric(peak, metric) > 0.0 {
            insights.push(StatisticsInsight {
                kind: "peak".to_string(),
                level: "info".to_string(),
                value: match metric {
                    StatisticsMetric::Cost => format!("{:.4}", peak.cost),
                    StatisticsMetric::Requests => peak.request_count.to_string(),
                    StatisticsMetric::Tokens => peak.total_tokens.to_string(),
                },
                model_name: None,
                date: Some(peak.label.clone()),
            });
        }
    }

    if let Some(model) = models.first() {
        insights.push(StatisticsInsight {
            kind: "topModel".to_string(),
            level: "info".to_string(),
            value: format!("{:.1}", model.percent),
            model_name: Some(model.model_name.clone()),
            date: None,
        });
    }

    if let Some(error_requests) = totals.error_requests {
        if error_requests > 0 {
            insights.push(StatisticsInsight {
                kind: "errors".to_string(),
                level: "warning".to_string(),
                value: error_requests.to_string(),
                model_name: None,
                date: None,
            });
        }
    }

    if let Some(perf) = performance {
        if let Some(model) = &perf.slowest_model {
            insights.push(StatisticsInsight {
                kind: "slowestModel".to_string(),
                level: "info".to_string(),
                value: format!("{:.0}", perf.avg_ttft_ms),
                model_name: Some(model.clone()),
                date: None,
            });
        }
    }

    insights.truncate(4);
    insights
}

fn build_proxy_statistics(
    records: Vec<UsageRecord>,
    query: &StatisticsQuery,
    _settings: &AppSettings,
) -> StatisticsSummary {
    let (start_epoch, end_epoch) = normalize_range(query);
    let mut total = StatAccumulator::default();
    let mut trend_map: HashMap<i64, StatAccumulator> = HashMap::new();
    let mut model_map: HashMap<String, StatAccumulator> = HashMap::new();
    let mut model_trend_map: HashMap<String, HashMap<i64, StatAccumulator>> = HashMap::new();

    for record in &records {
        let cost = record.estimated_cost;
        let model_name = if record.model.is_empty() {
            "unknown".to_string()
        } else {
            record.model.clone()
        };
        let bucket = bucket_start(record.timestamp / 1000, &query.bucket);
        total.add_record(record, cost);
        trend_map
            .entry(bucket)
            .or_default()
            .add_record(record, cost);
        model_map
            .entry(model_name.clone())
            .or_default()
            .add_record(record, cost);
        model_trend_map
            .entry(model_name)
            .or_default()
            .entry(bucket)
            .or_default()
            .add_record(record, cost);
    }

    let trend = trend_from_map(&trend_map, start_epoch, end_epoch, &query.bucket);

    let mut models: Vec<StatisticsModelBreakdown> = model_map
        .into_iter()
        .map(|(model_name, acc)| {
            let mut status_codes: Vec<StatusCodeCount> = acc
                .status_code_counts
                .iter()
                .map(|(status_code, count)| StatusCodeCount {
                    status_code: *status_code,
                    count: *count,
                })
                .collect();
            status_codes.sort_by(|a, b| a.status_code.cmp(&b.status_code));

            StatisticsModelBreakdown {
                model_name: model_name.clone(),
                request_count: acc.request_count,
                total_tokens: acc.total_tokens,
                input_tokens: acc.input_tokens,
                output_tokens: acc.output_tokens,
                cache_create_tokens: acc.cache_create_tokens,
                cache_read_tokens: acc.cache_read_tokens,
                cost: acc.cost,
                percent: if total.total_tokens > 0 {
                    (acc.total_tokens as f64 / total.total_tokens as f64) * 100.0
                } else {
                    0.0
                },
                avg_tokens_per_second: (acc.rate_count > 0)
                    .then_some(acc.rate_sum / acc.rate_count as f64),
                avg_ttft_ms: (acc.ttft_count > 0).then_some(acc.ttft_sum / acc.ttft_count as f64),
                error_requests: Some(acc.client_error_requests + acc.server_error_requests),
                success_requests: Some(acc.success_requests),
                client_error_requests: Some(acc.client_error_requests),
                server_error_requests: Some(acc.server_error_requests),
                status_codes,
                trend: model_trend_map
                    .get(&model_name)
                    .map(|trend_map| {
                        trend_from_map(trend_map, start_epoch, end_epoch, &query.bucket)
                    })
                    .unwrap_or_else(|| make_empty_trend(start_epoch, end_epoch, &query.bucket)),
            }
        })
        .collect();
    models.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens));

    let performance = if total.rate_count > 0 || total.ttft_count > 0 {
        let fastest_model = models
            .iter()
            .filter_map(|m| m.avg_tokens_per_second.map(|v| (m.model_name.clone(), v)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|m| m.0);
        let slowest_model = models
            .iter()
            .filter_map(|m| m.avg_ttft_ms.map(|v| (m.model_name.clone(), v)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|m| m.0);

        Some(StatisticsPerformance {
            request_count: total.rate_count.max(total.ttft_count),
            avg_tokens_per_second: if total.rate_count > 0 {
                total.rate_sum / total.rate_count as f64
            } else {
                0.0
            },
            avg_ttft_ms: if total.ttft_count > 0 {
                total.ttft_sum / total.ttft_count as f64
            } else {
                0.0
            },
            slowest_model,
            fastest_model,
        })
    } else {
        None
    };

    let status_total =
        total.success_requests + total.client_error_requests + total.server_error_requests;
    let status = Some(StatisticsStatusBreakdown {
        success_requests: total.success_requests,
        client_error_requests: total.client_error_requests,
        server_error_requests: total.server_error_requests,
        success_rate: if status_total > 0 {
            (total.success_requests as f64 / status_total as f64) * 100.0
        } else {
            0.0
        },
    });

    let totals = totals_from_acc(&total, models.len() as u64, true);
    let insights = build_insights(
        &totals,
        &trend,
        &models,
        &query.metric,
        performance.as_ref(),
    );

    StatisticsSummary {
        generated_at_epoch: chrono::Utc::now().timestamp(),
        source: "proxy".to_string(),
        capability: StatisticsCapability {
            has_basic_usage: true,
            has_performance: performance.is_some(),
            has_status_codes: true,
        },
        range: StatisticsRange {
            start_epoch,
            end_epoch,
            timezone: query.timezone.clone(),
            bucket: bucket_name(&query.bucket),
        },
        totals,
        trend,
        models,
        performance,
        status,
        insights,
    }
}

fn build_jsonl_statistics(query: &StatisticsQuery, settings: &AppSettings) -> StatisticsSummary {
    let (start_epoch, end_epoch) = normalize_range(query);
    let pricings = effective_model_pricings(settings);
    let match_mode = settings.model_pricing.match_mode.clone();
    let mut total = StatAccumulator::default();
    let mut trend_map: HashMap<i64, StatAccumulator> = HashMap::new();
    let mut model_map: HashMap<String, StatAccumulator> = HashMap::new();
    let mut model_trend_map: HashMap<String, HashMap<i64, StatAccumulator>> = HashMap::new();

    for meta in crate::session::get_all_session_meta_cached() {
        let event_epoch = if meta.end_time > 0 {
            meta.end_time
        } else {
            meta.last_modified
        };
        if event_epoch < start_epoch || event_epoch >= end_epoch {
            continue;
        }
        let model = meta
            .models
            .first()
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        let cost = crate::models::estimate_session_cost(
            meta.total_input_tokens,
            meta.total_output_tokens,
            meta.total_cache_create_tokens,
            meta.total_cache_read_tokens,
            &model,
            &pricings,
            &match_mode,
        );
        let requests = meta.message_count.max(1);
        let bucket = bucket_start(event_epoch, &query.bucket);
        total.add_tokens(
            meta.total_input_tokens,
            meta.total_output_tokens,
            meta.total_cache_create_tokens,
            meta.total_cache_read_tokens,
            requests,
            cost,
        );
        trend_map.entry(bucket).or_default().add_tokens(
            meta.total_input_tokens,
            meta.total_output_tokens,
            meta.total_cache_create_tokens,
            meta.total_cache_read_tokens,
            requests,
            cost,
        );
        model_map.entry(model.clone()).or_default().add_tokens(
            meta.total_input_tokens,
            meta.total_output_tokens,
            meta.total_cache_create_tokens,
            meta.total_cache_read_tokens,
            requests,
            cost,
        );
        model_trend_map
            .entry(model.clone())
            .or_default()
            .entry(bucket)
            .or_default()
            .add_tokens(
                meta.total_input_tokens,
                meta.total_output_tokens,
                meta.total_cache_create_tokens,
                meta.total_cache_read_tokens,
                requests,
                cost,
            );
    }

    let trend = trend_from_map(&trend_map, start_epoch, end_epoch, &query.bucket);

    let mut models: Vec<StatisticsModelBreakdown> = model_map
        .into_iter()
        .map(|(model_name, acc)| StatisticsModelBreakdown {
            model_name: model_name.clone(),
            request_count: acc.request_count,
            total_tokens: acc.total_tokens,
            input_tokens: acc.input_tokens,
            output_tokens: acc.output_tokens,
            cache_create_tokens: acc.cache_create_tokens,
            cache_read_tokens: acc.cache_read_tokens,
            cost: acc.cost,
            percent: if total.total_tokens > 0 {
                (acc.total_tokens as f64 / total.total_tokens as f64) * 100.0
            } else {
                0.0
            },
            avg_tokens_per_second: None,
            avg_ttft_ms: None,
            error_requests: None,
            success_requests: None,
            client_error_requests: None,
            server_error_requests: None,
            status_codes: Vec::new(),
            trend: model_trend_map
                .get(&model_name)
                .map(|trend_map| trend_from_map(trend_map, start_epoch, end_epoch, &query.bucket))
                .unwrap_or_else(|| make_empty_trend(start_epoch, end_epoch, &query.bucket)),
        })
        .collect();
    models.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens));

    let totals = totals_from_acc(&total, models.len() as u64, false);
    let insights = build_insights(&totals, &trend, &models, &query.metric, None);

    StatisticsSummary {
        generated_at_epoch: chrono::Utc::now().timestamp(),
        source: "local-jsonl".to_string(),
        capability: StatisticsCapability {
            has_basic_usage: true,
            has_performance: false,
            has_status_codes: false,
        },
        range: StatisticsRange {
            start_epoch,
            end_epoch,
            timezone: query.timezone.clone(),
            bucket: bucket_name(&query.bucket),
        },
        totals,
        trend,
        models,
        performance: None,
        status: None,
        insights,
    }
}

#[tauri::command]
pub async fn get_statistics_summary(
    query: StatisticsQuery,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<StatisticsSummary, String> {
    let (start_epoch, end_epoch) = normalize_range(&query);
    if settings.data_source == "proxy" {
        if let Some(db) = crate::proxy::ProxyDatabase::get_global() {
            db.backfill_unlocked_costs().await?;
            // 构建来源过滤器
            let usage_filter = build_usage_query_filter(&settings);
            let records = db
                .get_records_between_with_source(
                    start_epoch * 1000,
                    end_epoch * 1000,
                    true,
                    &usage_filter,
                )
                .await?;
            return Ok(build_proxy_statistics(records, &query, &settings));
        }
    }

    Ok(build_jsonl_statistics(&query, &settings))
}

fn month_day_count(year: i32, month: u8) -> u32 {
    for day in (28..=31).rev() {
        if NaiveDate::from_ymd_opt(year, month as u32, day).is_some() {
            return day;
        }
    }
    30
}

async fn collect_proxy_records_by_day(
    db: &crate::proxy::ProxyDatabase,
    day_map: &mut HashMap<String, (StatAccumulator, std::collections::HashSet<String>)>,
    start_epoch: i64,
    end_epoch: i64,
    include_errors: bool,
    usage_filter: &UsageQueryFilter,
) -> Result<(), String> {
    db.backfill_unlocked_costs().await?;
    let records = db
        .get_records_between_with_source(
            start_epoch * 1000,
            end_epoch * 1000,
            include_errors,
            usage_filter,
        )
        .await?;

    for record in records {
        let date = Local
            .timestamp_opt(record.timestamp / 1000, 0)
            .single()
            .unwrap_or_else(Local::now)
            .format("%Y-%m-%d")
            .to_string();
        let cost = record.estimated_cost;
        let entry = day_map.entry(date).or_default();
        entry.0.add_record(&record, cost);
        if !record.model.is_empty() {
            entry.1.insert(record.model);
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn get_month_activity(
    year: i32,
    month: u8,
    metric: StatisticsMetric,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<MonthActivity, String> {
    let day_count = month_day_count(year, month);
    let pricings = effective_model_pricings(&settings);
    let match_mode = settings.model_pricing.match_mode.clone();
    let mut day_map: HashMap<String, (StatAccumulator, std::collections::HashSet<String>)> =
        HashMap::new();

    let month_start = Local
        .with_ymd_and_hms(year, month as u32, 1, 0, 0, 0)
        .single()
        .unwrap_or_else(Local::now)
        .timestamp();
    let next_month = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month as u32 + 1)
    };
    let month_end = Local
        .with_ymd_and_hms(next_month.0, next_month.1, 1, 0, 0, 0)
        .single()
        .unwrap_or_else(Local::now)
        .timestamp();

    if settings.data_source == "proxy" {
        if let Some(db) = crate::proxy::ProxyDatabase::get_global() {
            let usage_filter = build_usage_query_filter(&settings);
            if !is_usage_filter_all(&usage_filter) {
                collect_proxy_records_by_day(
                    &db,
                    &mut day_map,
                    month_start,
                    month_end,
                    settings.proxy.include_error_requests,
                    &usage_filter,
                )
                .await?;
            } else {
                let month_start_date = NaiveDate::from_ymd_opt(year, month as u32, 1)
                    .unwrap_or_else(|| Local::now().date_naive());
                let next_month_date = if month == 12 {
                    NaiveDate::from_ymd_opt(year + 1, 1, 1)
                } else {
                    NaiveDate::from_ymd_opt(year, month as u32 + 1, 1)
                }
                .unwrap_or_else(|| Local::now().date_naive());
                let today_date = Local::now().date_naive();
                let summary_end_date = next_month_date.min(today_date);

                if summary_end_date > month_start_date {
                    let summary_start_key = month_start_date.format("%Y-%m-%d").to_string();
                    let summary_end_key = summary_end_date.format("%Y-%m-%d").to_string();
                    db.ensure_daily_summaries(&summary_start_key, &summary_end_key)
                        .await?;
                    for summary in db
                        .get_daily_activity_summaries(&summary_start_key, &summary_end_key)
                        .await?
                    {
                        let mut acc = StatAccumulator::default();
                        if settings.proxy.include_error_requests {
                            acc.request_count = summary.request_count;
                            acc.total_tokens = summary.total_tokens;
                            acc.input_tokens = summary.input_tokens;
                            acc.output_tokens = summary.output_tokens;
                            acc.cache_create_tokens = summary.cache_create_tokens;
                            acc.cache_read_tokens = summary.cache_read_tokens;
                            acc.cost = summary.cost;
                        } else {
                            acc.request_count = summary.success_requests;
                            acc.total_tokens = summary.success_total_tokens;
                            acc.input_tokens = summary.success_input_tokens;
                            acc.output_tokens = summary.success_output_tokens;
                            acc.cache_create_tokens = summary.success_cache_create_tokens;
                            acc.cache_read_tokens = summary.success_cache_read_tokens;
                            acc.cost = summary.success_cost;
                        }
                        acc.success_requests = summary.success_requests;
                        acc.client_error_requests = summary.client_error_requests;
                        acc.server_error_requests = summary.server_error_requests;
                        let models = (0..summary.model_count)
                            .map(|idx| format!("__cached_model_{idx}"))
                            .collect();
                        day_map.insert(summary.date, (acc, models));
                    }
                } else {
                    db.backfill_unlocked_costs().await?;
                }

                let live_start = month_start_date.max(today_date);
                if live_start < next_month_date {
                    let live_start_epoch = Local
                        .with_ymd_and_hms(
                            live_start.year(),
                            live_start.month(),
                            live_start.day(),
                            0,
                            0,
                            0,
                        )
                        .single()
                        .unwrap_or_else(Local::now)
                        .timestamp();
                    let live_end_epoch = month_end;
                    let records = db
                        .get_records_between(
                            live_start_epoch * 1000,
                            live_end_epoch * 1000,
                            settings.proxy.include_error_requests,
                        )
                        .await?;
                    for record in records {
                        let date = Local
                            .timestamp_opt(record.timestamp / 1000, 0)
                            .single()
                            .unwrap_or_else(Local::now)
                            .format("%Y-%m-%d")
                            .to_string();
                        let cost = record.estimated_cost;
                        let entry = day_map.entry(date).or_default();
                        entry.0.add_record(&record, cost);
                        if !record.model.is_empty() {
                            entry.1.insert(record.model);
                        }
                    }
                }
            }
        }
    } else {
        for meta in crate::session::get_all_session_meta_cached() {
            let event_epoch = if meta.end_time > 0 {
                meta.end_time
            } else {
                meta.last_modified
            };
            if event_epoch < month_start || event_epoch >= month_end {
                continue;
            }
            let date = Local
                .timestamp_opt(event_epoch, 0)
                .single()
                .unwrap_or_else(Local::now)
                .format("%Y-%m-%d")
                .to_string();
            let model = meta
                .models
                .first()
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());
            let cost = crate::models::estimate_session_cost(
                meta.total_input_tokens,
                meta.total_output_tokens,
                meta.total_cache_create_tokens,
                meta.total_cache_read_tokens,
                &model,
                &pricings,
                &match_mode,
            );
            let entry = day_map.entry(date).or_default();
            entry.0.add_tokens(
                meta.total_input_tokens,
                meta.total_output_tokens,
                meta.total_cache_create_tokens,
                meta.total_cache_read_tokens,
                meta.message_count.max(1),
                cost,
            );
            for model in meta.models {
                entry.1.insert(model);
            }
        }
    }

    let mut days = Vec::new();
    for day in 1..=day_count {
        let Some(date) = NaiveDate::from_ymd_opt(year, month as u32, day) else {
            continue;
        };
        let key = date.format("%Y-%m-%d").to_string();
        let (acc, models) = day_map.remove(&key).unwrap_or_default();
        let error_requests = acc.client_error_requests + acc.server_error_requests;
        days.push(DayActivity {
            date: key,
            request_count: acc.request_count,
            total_tokens: acc.total_tokens,
            input_tokens: acc.input_tokens,
            output_tokens: acc.output_tokens,
            cache_create_tokens: acc.cache_create_tokens,
            cache_read_tokens: acc.cache_read_tokens,
            cost: acc.cost,
            model_count: models.len() as u64,
            success_requests: (settings.data_source == "proxy").then_some(acc.success_requests),
            error_requests: (settings.data_source == "proxy").then_some(error_requests),
        });
    }

    Ok(MonthActivity {
        year,
        month,
        timezone: settings.timezone,
        metric,
        days,
    })
}

#[tauri::command]
pub async fn get_year_activity(
    year: i32,
    metric: StatisticsMetric,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<YearActivity, String> {
    let pricings = effective_model_pricings(&settings);
    let match_mode = settings.model_pricing.match_mode.clone();
    let mut day_map: HashMap<String, (StatAccumulator, std::collections::HashSet<String>)> =
        HashMap::new();

    let year_start = Local
        .with_ymd_and_hms(year, 1, 1, 0, 0, 0)
        .single()
        .unwrap_or_else(Local::now)
        .timestamp();
    let year_end = Local
        .with_ymd_and_hms(year + 1, 1, 1, 0, 0, 0)
        .single()
        .unwrap_or_else(Local::now)
        .timestamp();

    if settings.data_source == "proxy" {
        if let Some(db) = crate::proxy::ProxyDatabase::get_global() {
            let usage_filter = build_usage_query_filter(&settings);
            if !is_usage_filter_all(&usage_filter) {
                collect_proxy_records_by_day(
                    &db,
                    &mut day_map,
                    year_start,
                    year_end,
                    settings.proxy.include_error_requests,
                    &usage_filter,
                )
                .await?;
            } else {
                let year_start_date = NaiveDate::from_ymd_opt(year, 1, 1)
                    .unwrap_or_else(|| Local::now().date_naive());
                let next_year_date = NaiveDate::from_ymd_opt(year + 1, 1, 1)
                    .unwrap_or_else(|| Local::now().date_naive());
                let today_date = Local::now().date_naive();
                let summary_end_date = next_year_date.min(today_date);

                if summary_end_date > year_start_date {
                    let summary_start_key = year_start_date.format("%Y-%m-%d").to_string();
                    let summary_end_key = summary_end_date.format("%Y-%m-%d").to_string();
                    db.ensure_daily_summaries(&summary_start_key, &summary_end_key)
                        .await?;
                    for summary in db
                        .get_daily_activity_summaries(&summary_start_key, &summary_end_key)
                        .await?
                    {
                        let mut acc = StatAccumulator::default();
                        if settings.proxy.include_error_requests {
                            acc.request_count = summary.request_count;
                            acc.total_tokens = summary.total_tokens;
                            acc.input_tokens = summary.input_tokens;
                            acc.output_tokens = summary.output_tokens;
                            acc.cache_create_tokens = summary.cache_create_tokens;
                            acc.cache_read_tokens = summary.cache_read_tokens;
                            acc.cost = summary.cost;
                        } else {
                            acc.request_count = summary.success_requests;
                            acc.total_tokens = summary.success_total_tokens;
                            acc.input_tokens = summary.success_input_tokens;
                            acc.output_tokens = summary.success_output_tokens;
                            acc.cache_create_tokens = summary.success_cache_create_tokens;
                            acc.cache_read_tokens = summary.success_cache_read_tokens;
                            acc.cost = summary.success_cost;
                        }
                        acc.success_requests = summary.success_requests;
                        acc.client_error_requests = summary.client_error_requests;
                        acc.server_error_requests = summary.server_error_requests;
                        let models = (0..summary.model_count)
                            .map(|idx| format!("__cached_model_{idx}"))
                            .collect();
                        day_map.insert(summary.date, (acc, models));
                    }
                } else {
                    db.backfill_unlocked_costs().await?;
                }

                let live_start = year_start_date.max(today_date);
                if live_start < next_year_date {
                    let live_start_epoch = Local
                        .with_ymd_and_hms(
                            live_start.year(),
                            live_start.month(),
                            live_start.day(),
                            0,
                            0,
                            0,
                        )
                        .single()
                        .unwrap_or_else(Local::now)
                        .timestamp();
                    let records = db
                        .get_records_between(
                            live_start_epoch * 1000,
                            year_end * 1000,
                            settings.proxy.include_error_requests,
                        )
                        .await?;
                    for record in records {
                        let date = Local
                            .timestamp_opt(record.timestamp / 1000, 0)
                            .single()
                            .unwrap_or_else(Local::now)
                            .format("%Y-%m-%d")
                            .to_string();
                        let cost = record.estimated_cost;
                        let entry = day_map.entry(date).or_default();
                        entry.0.add_record(&record, cost);
                        if !record.model.is_empty() {
                            entry.1.insert(record.model);
                        }
                    }
                }
            }
        }
    } else {
        for meta in crate::session::get_all_session_meta_cached() {
            let event_epoch = if meta.end_time > 0 {
                meta.end_time
            } else {
                meta.last_modified
            };
            if event_epoch < year_start || event_epoch >= year_end {
                continue;
            }
            let date = Local
                .timestamp_opt(event_epoch, 0)
                .single()
                .unwrap_or_else(Local::now)
                .format("%Y-%m-%d")
                .to_string();
            let model = meta
                .models
                .first()
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());
            let cost = crate::models::estimate_session_cost(
                meta.total_input_tokens,
                meta.total_output_tokens,
                meta.total_cache_create_tokens,
                meta.total_cache_read_tokens,
                &model,
                &pricings,
                &match_mode,
            );
            let entry = day_map.entry(date).or_default();
            entry.0.add_tokens(
                meta.total_input_tokens,
                meta.total_output_tokens,
                meta.total_cache_create_tokens,
                meta.total_cache_read_tokens,
                meta.message_count.max(1),
                cost,
            );
            for model in meta.models {
                entry.1.insert(model);
            }
        }
    }

    let Some(mut date) = NaiveDate::from_ymd_opt(year, 1, 1) else {
        return Ok(YearActivity {
            year,
            timezone: settings.timezone,
            metric,
            days: Vec::new(),
        });
    };
    let Some(end_date) = NaiveDate::from_ymd_opt(year + 1, 1, 1) else {
        return Ok(YearActivity {
            year,
            timezone: settings.timezone,
            metric,
            days: Vec::new(),
        });
    };

    let mut days = Vec::new();
    while date < end_date {
        let key = date.format("%Y-%m-%d").to_string();
        let (acc, models) = day_map.remove(&key).unwrap_or_default();
        let error_requests = acc.client_error_requests + acc.server_error_requests;
        days.push(DayActivity {
            date: key,
            request_count: acc.request_count,
            total_tokens: acc.total_tokens,
            input_tokens: acc.input_tokens,
            output_tokens: acc.output_tokens,
            cache_create_tokens: acc.cache_create_tokens,
            cache_read_tokens: acc.cache_read_tokens,
            cost: acc.cost,
            model_count: models.len() as u64,
            success_requests: (settings.data_source == "proxy").then_some(acc.success_requests),
            error_requests: (settings.data_source == "proxy").then_some(error_requests),
        });
        let Some(next_date) = date.succ_opt() else {
            break;
        };
        date = next_date;
    }

    Ok(YearActivity {
        year,
        timezone: settings.timezone,
        metric,
        days,
    })
}

/// 获取窗口速率汇总（整体 + 按模型）用于代理模式
/// 返回速率统计，包括每个模型的平均 tokens/second
#[tauri::command]
pub async fn get_window_rate_summary(
    window: String,
    proxy_state: tauri::State<'_, ProxyState>,
) -> Result<WindowRateSummary, String> {
    let server_guard = proxy_state.server.read().await;

    if let Some(server) = server_guard.as_ref() {
        let collector = server.get_collector();
        let db_summary = collector.get_window_rate_summary(&window).await;

        // 获取 TTFT 统计
        let cutoff_ms = crate::proxy::UsageCollector::calculate_window_cutoff_public(&window);
        let ttft_stats = collector.get_ttft_stats(cutoff_ms).await;
        let ttft_by_model = collector.get_model_ttft_stats(cutoff_ms).await;

        drop(server_guard); // 提前释放锁

        // 转换数据库类型为模型类型
        let overall = OverallRateStats {
            request_count: db_summary.overall.request_count as u64,
            total_output_tokens: db_summary.overall.total_output_tokens as u64,
            total_duration_ms: db_summary.overall.total_duration_ms as u64,
            avg_tokens_per_second: db_summary.overall.avg_output_tokens_per_second,
        };

        let by_model: Vec<ModelRateStats> = db_summary
            .by_model
            .into_iter()
            .map(|m| ModelRateStats {
                model_name: m.model,
                request_count: m.request_count as u64,
                total_output_tokens: m.total_output_tokens as u64,
                total_duration_ms: m.total_duration_ms as u64,
                avg_tokens_per_second: m.avg_tokens_per_second,
                min_tokens_per_second: m.min_tokens_per_second,
                max_tokens_per_second: m.max_tokens_per_second,
            })
            .collect();

        // 转换 TTFT 统计
        let ttft = TtftStats {
            request_count: ttft_stats.request_count as u64,
            avg_ttft_ms: ttft_stats.avg_ttft_ms,
            min_ttft_ms: ttft_stats.min_ttft_ms as u64,
            max_ttft_ms: ttft_stats.max_ttft_ms as u64,
        };

        let ttft_by_model: Vec<ModelTtftStats> = ttft_by_model
            .into_iter()
            .map(|m| ModelTtftStats {
                model_name: m.model,
                request_count: m.request_count as u64,
                avg_ttft_ms: m.avg_ttft_ms,
                min_ttft_ms: m.min_ttft_ms as u64,
                max_ttft_ms: m.max_ttft_ms as u64,
            })
            .collect();

        Ok(WindowRateSummary {
            window: db_summary.window,
            overall,
            by_model,
            ttft,
            ttft_by_model,
        })
    } else {
        // 代理未运行，返回空统计
        Ok(WindowRateSummary {
            window,
            overall: OverallRateStats {
                request_count: 0,
                total_output_tokens: 0,
                total_duration_ms: 0,
                avg_tokens_per_second: 0.0,
            },
            by_model: Vec::new(),
            ttft: TtftStats::default(),
            ttft_by_model: Vec::new(),
        })
    }
}

/// 获取会话列表（按最后修改时间倒序，支持分页）
/// 数据源逻辑：
/// - JSONL：会话元信息（项目名、主题、token 统计）
/// - session_stats 表：性能指标（速率、TTFT、耗时）
#[tauri::command]
pub async fn get_sessions(
    limit: i64,
    offset: i64,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<Vec<SessionStats>, String> {
    // 获取价格配置
    let pricings = effective_model_pricings(&settings);
    let match_mode = settings.model_pricing.match_mode.clone();

    if settings.data_source == "proxy" {
        let usage_filter = build_usage_query_filter(&settings);
        if !is_usage_filter_all(&usage_filter) {
            if let Some(db) = crate::proxy::ProxyDatabase::get_global() {
                let mut sessions = db
                    .get_sessions_with_source(limit, offset, &usage_filter)
                    .await?;
                for session in sessions.iter_mut() {
                    if let Some(meta) = crate::session::get_session_meta_by_id(&session.session_id)
                    {
                        session.cwd = meta.cwd;
                        session.project_name = meta.project_name;
                        session.topic = meta.topic;
                        session.last_prompt = meta.last_prompt;
                        session.session_name = meta.session_name;
                    } else {
                        // 无 JSONL 元数据的代理会话，使用首条请求时间和模型作为展示名
                        let first = if session.first_request_time > 0 {
                            chrono::DateTime::from_timestamp_millis(session.first_request_time)
                                .map(|d| d.format("%m/%d %H:%M").to_string())
                        } else {
                            None
                        };
                        session.session_name =
                            Some(first.unwrap_or_else(|| "Proxy Session".to_string()));
                    }
                }
                return Ok(sessions);
            }
            return Ok(Vec::new());
        }
    }

    // 1. 从 JSONL 文件获取会话列表（主数据源）
    // 使用缓存版本避免频繁扫描文件系统
    let all_meta = crate::session::get_all_session_meta_cached();

    // 2. 应用分页
    let meta_list: Vec<_> = all_meta
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();

    // 3. 仅在代理模式下从 session_stats 表获取性能指标
    let proxy_stats_map: std::collections::HashMap<String, SessionStats> =
        if settings.data_source == "proxy" {
            let session_ids: Vec<String> = meta_list.iter().map(|m| m.session_id.clone()).collect();

            match crate::proxy::ProxyDatabase::get_global() {
                Some(db) => db
                    .get_session_stats_batch(&session_ids)
                    .await
                    .unwrap_or_default(),
                None => std::collections::HashMap::new(),
            }
        } else {
            // ccusage 模式下不查询代理性能数据
            std::collections::HashMap::new()
        };

    // 4. 构建 SessionStats，合并 JSONL 数据和 session_stats 数据
    let sessions: Vec<SessionStats> = meta_list
        .into_iter()
        .map(|meta| {
            // 计算基于 JSONL 的费用
            let first_model = meta.models.first().map(|s| s.as_str()).unwrap_or("");
            let jsonl_cost = crate::models::estimate_session_cost(
                meta.total_input_tokens,
                meta.total_output_tokens,
                meta.total_cache_create_tokens,
                meta.total_cache_read_tokens,
                first_model,
                &pricings,
                &match_mode,
            );

            // 尝试从 session_stats 获取性能指标
            if let Some(proxy) = proxy_stats_map.get(&meta.session_id) {
                // 合并数据：JSONL 的 token 统计 + session_stats 的性能指标
                SessionStats {
                    session_id: meta.session_id,
                    // Token 统计来自 JSONL（完整数据）
                    total_input_tokens: meta.total_input_tokens,
                    total_output_tokens: meta.total_output_tokens,
                    total_cache_create_tokens: meta.total_cache_create_tokens,
                    total_cache_read_tokens: meta.total_cache_read_tokens,
                    // 性能指标来自 session_stats
                    total_duration_ms: proxy.total_duration_ms,
                    avg_output_tokens_per_second: proxy.avg_output_tokens_per_second,
                    avg_ttft_ms: proxy.avg_ttft_ms,
                    success_requests: proxy.success_requests,
                    error_requests: proxy.error_requests,
                    // 其他
                    total_requests: meta.message_count,
                    first_request_time: meta.start_time,
                    last_request_time: meta.end_time,
                    models: meta.models,
                    estimated_cost: jsonl_cost,
                    is_cost_estimated: true,
                    // JSONL 元信息
                    cwd: meta.cwd,
                    project_name: meta.project_name,
                    topic: meta.topic,
                    last_prompt: meta.last_prompt,
                    session_name: meta.session_name,
                }
            } else {
                // 没有代理数据，仅使用 JSONL
                SessionStats {
                    session_id: meta.session_id,
                    total_requests: meta.message_count,
                    total_input_tokens: meta.total_input_tokens,
                    total_output_tokens: meta.total_output_tokens,
                    total_cache_create_tokens: meta.total_cache_create_tokens,
                    total_cache_read_tokens: meta.total_cache_read_tokens,
                    total_duration_ms: 0,
                    avg_output_tokens_per_second: 0.0,
                    first_request_time: meta.start_time,
                    last_request_time: meta.end_time,
                    models: meta.models,
                    avg_ttft_ms: 0.0,
                    success_requests: 0,
                    error_requests: 0,
                    estimated_cost: jsonl_cost,
                    is_cost_estimated: true,
                    cwd: meta.cwd,
                    project_name: meta.project_name,
                    topic: meta.topic,
                    last_prompt: meta.last_prompt,
                    session_name: meta.session_name,
                }
            }
        })
        .collect();

    Ok(sessions)
}

/// 获取单个会话详情
/// 数据源逻辑：
/// - JSONL：会话元信息（项目名、主题、token 统计）
/// - session_stats 表：性能指标（速率、TTFT、耗时）
#[tauri::command]
pub async fn get_session_detail(
    session_id: String,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<Option<SessionStats>, String> {
    // 获取价格配置
    let pricings = effective_model_pricings(&settings);
    let match_mode = settings.model_pricing.match_mode.clone();

    if settings.data_source == "proxy" {
        let usage_filter = build_usage_query_filter(&settings);
        if !is_usage_filter_all(&usage_filter) {
            if let Some(db) = crate::proxy::ProxyDatabase::get_global() {
                let Some(mut stats) = db
                    .get_session_detail_with_source(&session_id, &usage_filter)
                    .await?
                else {
                    return Ok(None);
                };

                if let Some(meta) = crate::session::get_session_meta_by_id(&session_id) {
                    stats.cwd = meta.cwd;
                    stats.project_name = meta.project_name;
                    stats.topic = meta.topic;
                    stats.last_prompt = meta.last_prompt;
                    stats.session_name = meta.session_name;
                }

                return Ok(Some(stats));
            }
            return Ok(None);
        }
    }

    // 1. 从 JSONL 获取会话元信息
    let meta = match crate::session::get_session_meta_by_id(&session_id) {
        Some(m) => m,
        None => return Ok(None),
    };

    // 2. 计算基于 JSONL 的费用
    let first_model = meta.models.first().map(|s| s.as_str()).unwrap_or("");
    let jsonl_cost = crate::models::estimate_session_cost(
        meta.total_input_tokens,
        meta.total_output_tokens,
        meta.total_cache_create_tokens,
        meta.total_cache_read_tokens,
        first_model,
        &pricings,
        &match_mode,
    );

    // 3. 仅在代理模式下从 session_stats 表获取性能指标
    let proxy_stats: Option<SessionStats> = if settings.data_source == "proxy" {
        match crate::proxy::ProxyDatabase::get_global() {
            Some(db) => match db
                .get_session_stats_batch(std::slice::from_ref(&meta.session_id))
                .await
            {
                Ok(stats_map) => stats_map.get(&meta.session_id).cloned(),
                Err(_) => None,
            },
            None => None,
        }
    } else {
        // ccusage 模式下不查询代理性能数据
        None
    };

    // 4. 合并数据：JSONL 的 token 统计 + session_stats 的性能指标
    let stats = if let Some(proxy) = proxy_stats {
        SessionStats {
            session_id: meta.session_id,
            // Token 统计来自 JSONL（完整数据）
            total_input_tokens: meta.total_input_tokens,
            total_output_tokens: meta.total_output_tokens,
            total_cache_create_tokens: meta.total_cache_create_tokens,
            total_cache_read_tokens: meta.total_cache_read_tokens,
            // 性能指标来自 session_stats
            total_duration_ms: proxy.total_duration_ms,
            avg_output_tokens_per_second: proxy.avg_output_tokens_per_second,
            avg_ttft_ms: proxy.avg_ttft_ms,
            success_requests: proxy.success_requests,
            error_requests: proxy.error_requests,
            // 其他
            total_requests: meta.message_count,
            first_request_time: meta.start_time,
            last_request_time: meta.end_time,
            models: meta.models,
            estimated_cost: jsonl_cost,
            is_cost_estimated: true,
            // JSONL 元信息
            cwd: meta.cwd,
            project_name: meta.project_name,
            topic: meta.topic,
            last_prompt: meta.last_prompt,
            session_name: meta.session_name,
        }
    } else {
        SessionStats {
            session_id: meta.session_id,
            total_requests: meta.message_count,
            total_input_tokens: meta.total_input_tokens,
            total_output_tokens: meta.total_output_tokens,
            total_cache_create_tokens: meta.total_cache_create_tokens,
            total_cache_read_tokens: meta.total_cache_read_tokens,
            total_duration_ms: 0,
            avg_output_tokens_per_second: 0.0,
            first_request_time: meta.start_time,
            last_request_time: meta.end_time,
            models: meta.models,
            avg_ttft_ms: 0.0,
            success_requests: 0,
            error_requests: 0,
            estimated_cost: jsonl_cost,
            is_cost_estimated: true,
            cwd: meta.cwd,
            project_name: meta.project_name,
            topic: meta.topic,
            last_prompt: meta.last_prompt,
            session_name: meta.session_name,
        }
    };

    Ok(Some(stats))
}

/// 获取项目统计（基于所有会话数据聚合）
/// 数据源逻辑：
/// - JSONL：会话元信息（项目名、token 统计）
#[tauri::command]
pub async fn get_project_stats(
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<Vec<crate::proxy::ProjectStats>, String> {
    // 获取价格配置
    let pricings = effective_model_pricings(&settings);
    let match_mode = settings.model_pricing.match_mode.clone();

    // 1. 从 JSONL 文件获取所有会话元信息
    // 使用缓存版本避免频繁扫描文件系统
    let all_meta = crate::session::get_all_session_meta_cached();

    // 2. 按项目名称聚合
    let mut project_map: std::collections::HashMap<String, crate::proxy::ProjectStats> =
        std::collections::HashMap::new();

    for meta in all_meta {
        let project_name = meta
            .project_name
            .clone()
            .unwrap_or_else(|| "未命名项目".to_string());

        // 计算费用（JSONL token 统计 + 价格配置）
        let first_model = meta.models.first().map(|s| s.as_str()).unwrap_or("");
        let cost = crate::models::estimate_session_cost(
            meta.total_input_tokens,
            meta.total_output_tokens,
            meta.total_cache_create_tokens,
            meta.total_cache_read_tokens,
            first_model,
            &pricings,
            &match_mode,
        );

        let entry = project_map
            .entry(project_name)
            .or_insert(crate::proxy::ProjectStats {
                name: String::new(),
                session_count: 0,
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_cost: 0.0,
                last_active: 0,
            });

        entry.name = meta
            .project_name
            .clone()
            .unwrap_or_else(|| "未命名项目".to_string());
        entry.session_count += 1;
        entry.total_input_tokens += meta.total_input_tokens;
        entry.total_output_tokens += meta.total_output_tokens;
        entry.total_cost += cost;
        if meta.end_time > entry.last_active {
            entry.last_active = meta.end_time;
        }
    }

    // 4. 按最后活跃时间倒序排序
    let mut projects: Vec<_> = project_map.into_values().collect();
    projects.sort_by_key(|b| std::cmp::Reverse(b.last_active));

    Ok(projects)
}
