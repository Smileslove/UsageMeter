//! 用量相关 Tauri 命令

use crate::models::{
    compute_percent, risk_level, AppSettings, ModelRateStats, ModelTtftStats, OverallRateStats,
    TtftStats, UsageSnapshot, WindowRateSummary, WindowUsage,
};
use crate::proxy::{ProxyServer, SessionStats};
use chrono::{Datelike, Local, TimeZone};
use std::collections::VecDeque;
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
        let window_stats = collector.get_all_window_stats(include_errors).await;
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
            .get_model_distribution(&settings.summary_window)
            .await;

        // 计算总 token 用于百分比
        let total_model_tokens: i64 = model_distribution_raw.iter().map(|m| m.total_tokens).sum();

        // 获取价格配置
        let pricings = &settings.model_pricing.pricings;
        let match_mode = &settings.model_pricing.match_mode;

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
                    pricings,
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

    // 计算总费用：优先使用数据库价格配置，其次使用 ccusage 提供的费用
    let pricings = &settings.model_pricing.pricings;
    let match_mode = &settings.model_pricing.match_mode;

    // 计算基于模型分布的费用
    let total_cost: f64 = model_distribution
        .iter()
        .map(|m| {
            crate::models::estimate_session_cost(
                m.input_tokens,
                m.output_tokens,
                m.cache_create_tokens,
                m.cache_read_tokens,
                &m.model_name,
                pricings,
                match_mode,
            )
        })
        .sum();

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

    let output = Command::new("node")
        .current_dir(&app_root)
        .arg(script_path)
        .output()
        .map_err(|e| format!("ERR_CCUSAGE_SCRIPT_FAILED: {e}"))?;

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
    let mut total_1d_tokens = 0_u64;
    let mut total_1d_input_tokens = 0_u64;
    let mut total_1d_output_tokens = 0_u64;
    let mut total_1d_cache_create_tokens = 0_u64;
    let mut total_1d_cache_read_tokens = 0_u64;
    let mut total_1d_requests = 0_u64;
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

    // 模型分布统计（仅统计30天内的数据）
    let mut model_stats: std::collections::HashMap<String, (u64, u64, u64, u64, u64, u64)> =
        std::collections::HashMap::new(); // (tokens, input, output, cache_create, cache_read, requests)

    for record in request_map.values() {
        let age = now - record.timestamp;
        if age <= 5 * 60 * 60 {
            total_5h_tokens += record.tokens;
            total_5h_input_tokens += record.input_tokens;
            total_5h_output_tokens += record.output_tokens;
            total_5h_cache_create_tokens += record.cache_create_tokens;
            total_5h_cache_read_tokens += record.cache_read_tokens;
            total_5h_requests += 1;
        }
        if age <= 24 * 60 * 60 {
            total_1d_tokens += record.tokens;
            total_1d_input_tokens += record.input_tokens;
            total_1d_output_tokens += record.output_tokens;
            total_1d_cache_create_tokens += record.cache_create_tokens;
            total_1d_cache_read_tokens += record.cache_read_tokens;
            total_1d_requests += 1;
        }
        if age <= 7 * 24 * 60 * 60 {
            total_7d_tokens += record.tokens;
            total_7d_input_tokens += record.input_tokens;
            total_7d_output_tokens += record.output_tokens;
            total_7d_cache_create_tokens += record.cache_create_tokens;
            total_7d_cache_read_tokens += record.cache_read_tokens;
            total_7d_requests += 1;
        }
        if age <= 30 * 24 * 60 * 60 {
            total_30d_tokens += record.tokens;
            total_30d_input_tokens += record.input_tokens;
            total_30d_output_tokens += record.output_tokens;
            total_30d_cache_create_tokens += record.cache_create_tokens;
            total_30d_cache_read_tokens += record.cache_read_tokens;
            total_30d_requests += 1;

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
            "1d" => (
                total_1d_tokens,
                total_1d_input_tokens,
                total_1d_output_tokens,
                total_1d_cache_create_tokens,
                total_1d_cache_read_tokens,
                total_1d_requests,
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

    // 获取价格配置
    let pricings = &settings.model_pricing.pricings;
    let match_mode = &settings.model_pricing.match_mode;

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
                    pricings,
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
    tokens: u64,       // 总 Token = input + cache_create + cache_read + output
    input_tokens: u64, // 实际输入（不含缓存）
    output_tokens: u64,
    cache_create_tokens: u64,
    cache_read_tokens: u64,
    model: String, // 模型名称
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
    let pricings = settings.model_pricing.pricings.clone();
    let match_mode = settings.model_pricing.match_mode.clone();

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
    let pricings = settings.model_pricing.pricings.clone();
    let match_mode = settings.model_pricing.match_mode.clone();

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
    let pricings = settings.model_pricing.pricings.clone();
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
