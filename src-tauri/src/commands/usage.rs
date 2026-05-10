//! 用量相关 Tauri 命令

use crate::models::{
    compute_percent, risk_level, AppSettings, ModelRateStats, ModelTtftStats, OverallRateStats,
    StatusCodeCount, ToolFilter, TtftStats, UsageSnapshot, WindowRateSummary, WindowUsage,
};
use crate::proxy::{ProjectToolStats, ProxyServer, SessionStats};
use crate::unified_usage::{CoverageOrigin, MergedCoverage, MergedRequestFact};
use chrono::{Datelike, Local, NaiveDate, TimeZone};
use std::collections::HashMap;
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

const MERGED_SOURCE: &str = "proxy-merged";

/// 从数据源获取用量快照
#[tauri::command]
pub async fn get_usage_snapshot(
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<UsageSnapshot, String> {
    // 检查是否使用代理模式
    if settings.data_source == "proxy" {
        return get_proxy_usage_snapshot(&settings).await;
    }

    tauri::async_runtime::spawn_blocking(move || match snapshot_from_local_jsonl(&settings) {
        Ok(snapshot) => Ok(snapshot),
        Err(local_err) => Ok(empty_usage_snapshot(
            &settings,
            "no-data",
            format!("NOTE_NO_REAL_DATA: local={local_err}"),
        )),
    })
    .await
    .map_err(|e| format!("ERR_SNAPSHOT_TASK_FAILED: {e}"))?
}

/// 从代理收集器获取用量快照
async fn get_proxy_usage_snapshot(settings: &AppSettings) -> Result<UsageSnapshot, String> {
    build_merged_usage_snapshot(settings).await
}

fn merged_stat_capability_from_facts(
    facts: &[MergedRequestFact],
    coverage: &MergedCoverage,
) -> StatisticsCapability {
    let has_status_codes = !coverage.has_partial_status_coverage
        && facts.iter().any(|fact| fact.status_code.is_some());
    let has_performance = facts
        .iter()
        .any(|fact| fact.output_tokens_per_second.is_some() || fact.ttft_ms.is_some());

    StatisticsCapability {
        has_basic_usage: true,
        has_performance,
        has_status_codes,
    }
}

fn add_fact_to_stat_acc(acc: &mut StatAccumulator, fact: &MergedRequestFact) {
    acc.add_tokens(
        fact.input_tokens,
        fact.output_tokens,
        fact.cache_create_tokens,
        fact.cache_read_tokens,
        1,
        fact.estimated_cost,
    );

    if let Some(status_code) = fact.status_code {
        if (200..300).contains(&status_code) {
            acc.success_requests += 1;
        } else if (400..500).contains(&status_code) {
            acc.client_error_requests += 1;
        } else if status_code >= 500 {
            acc.server_error_requests += 1;
        }
        *acc.status_code_counts.entry(status_code).or_insert(0) += 1;
    }

    if let Some(rate) = fact.output_tokens_per_second {
        if rate > 0.0 {
            acc.rate_sum += rate;
            acc.rate_count += 1;
        }
    }
    if let Some(ttft) = fact.ttft_ms {
        if ttft > 0 {
            acc.ttft_sum += ttft as f64;
            acc.ttft_count += 1;
        }
    }
}

fn build_merged_statistics(
    facts: Vec<MergedRequestFact>,
    coverage: &MergedCoverage,
    query: &StatisticsQuery,
) -> StatisticsSummary {
    let (start_epoch, end_epoch) = normalize_range(query);
    let mut total = StatAccumulator::default();
    let mut trend_map: HashMap<i64, StatAccumulator> = HashMap::new();
    let mut model_map: HashMap<String, StatAccumulator> = HashMap::new();
    let mut model_trend_map: HashMap<String, HashMap<i64, StatAccumulator>> = HashMap::new();

    for fact in &facts {
        let model_name = if fact.model.is_empty() {
            "unknown".to_string()
        } else {
            fact.model.clone()
        };
        let bucket = bucket_start(fact.timestamp_sec, &query.bucket);
        add_fact_to_stat_acc(&mut total, fact);
        add_fact_to_stat_acc(trend_map.entry(bucket).or_default(), fact);
        add_fact_to_stat_acc(model_map.entry(model_name.clone()).or_default(), fact);
        add_fact_to_stat_acc(
            model_trend_map
                .entry(model_name)
                .or_default()
                .entry(bucket)
                .or_default(),
            fact,
        );
    }

    let mut trend = trend_from_map(&trend_map, start_epoch, end_epoch, &query.bucket);

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

            let has_status = !status_codes.is_empty();
            let has_perf = acc.rate_count > 0 || acc.ttft_count > 0;

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
                avg_tokens_per_second: has_perf
                    .then_some(acc.rate_sum / acc.rate_count.max(1) as f64),
                avg_ttft_ms: (acc.ttft_count > 0).then_some(acc.ttft_sum / acc.ttft_count as f64),
                error_requests: has_status
                    .then_some(acc.client_error_requests + acc.server_error_requests),
                success_requests: has_status.then_some(acc.success_requests),
                client_error_requests: has_status.then_some(acc.client_error_requests),
                server_error_requests: has_status.then_some(acc.server_error_requests),
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

    let capability = merged_stat_capability_from_facts(&facts, coverage);
    if !capability.has_performance {
        for point in &mut trend {
            point.avg_tokens_per_second = None;
        }
        for model in &mut models {
            model.avg_tokens_per_second = None;
            model.avg_ttft_ms = None;
            for point in &mut model.trend {
                point.avg_tokens_per_second = None;
            }
        }
    }
    if !capability.has_status_codes {
        for model in &mut models {
            model.error_requests = None;
            model.success_requests = None;
            model.client_error_requests = None;
            model.server_error_requests = None;
            model.status_codes.clear();
        }
    }
    let performance = if capability.has_performance {
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

    let status = if capability.has_status_codes {
        let status_total =
            total.success_requests + total.client_error_requests + total.server_error_requests;
        Some(StatisticsStatusBreakdown {
            success_requests: total.success_requests,
            client_error_requests: total.client_error_requests,
            server_error_requests: total.server_error_requests,
            success_rate: if status_total > 0 {
                (total.success_requests as f64 / status_total as f64) * 100.0
            } else {
                0.0
            },
        })
    } else {
        None
    };

    let totals = totals_from_acc(&total, models.len() as u64, capability.has_status_codes);
    let insights = build_insights(
        &totals,
        &trend,
        &models,
        &query.metric,
        performance.as_ref(),
    );

    StatisticsSummary {
        generated_at_epoch: chrono::Utc::now().timestamp(),
        source: MERGED_SOURCE.to_string(),
        capability,
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

fn collect_day_activity_from_facts(
    facts: Vec<MergedRequestFact>,
    day_map: &mut HashMap<String, (StatAccumulator, std::collections::HashSet<String>)>,
    partial_status_days: &mut std::collections::HashSet<String>,
) {
    for fact in facts {
        let date = Local
            .timestamp_opt(fact.timestamp_sec, 0)
            .single()
            .unwrap_or_else(Local::now)
            .format("%Y-%m-%d")
            .to_string();
        if matches!(fact.coverage_origin, CoverageOrigin::LocalOnly) {
            partial_status_days.insert(date.clone());
        }
        let entry = day_map.entry(date).or_default();
        add_fact_to_stat_acc(&mut entry.0, &fact);
        if !fact.model.is_empty() {
            entry.1.insert(fact.model);
        }
    }
}

fn empty_window_rate_summary(window: String) -> WindowRateSummary {
    WindowRateSummary {
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
    }
}

fn build_window_rate_summary_from_facts(
    window: String,
    facts: Vec<MergedRequestFact>,
    _coverage: &MergedCoverage,
) -> WindowRateSummary {
    #[derive(Default)]
    struct PerfAccumulator {
        request_count: u64,
        total_output_tokens: u64,
        total_duration_ms: u64,
        rate_sum: f64,
        rate_count: u64,
        min_rate: Option<f64>,
        max_rate: Option<f64>,
        ttft_sum: f64,
        ttft_count: u64,
        min_ttft_ms: Option<u64>,
        max_ttft_ms: Option<u64>,
    }

    impl PerfAccumulator {
        fn add(&mut self, fact: &MergedRequestFact) {
            if let (Some(duration_ms), Some(rate)) =
                (fact.duration_ms, fact.output_tokens_per_second)
            {
                if duration_ms > 0 && rate > 0.0 {
                    self.request_count += 1;
                    self.total_output_tokens += fact.output_tokens;
                    self.total_duration_ms += duration_ms;
                    self.rate_sum += rate;
                    self.rate_count += 1;
                    self.min_rate = Some(self.min_rate.map_or(rate, |current| current.min(rate)));
                    self.max_rate = Some(self.max_rate.map_or(rate, |current| current.max(rate)));
                }
            }

            if let Some(ttft_ms) = fact.ttft_ms {
                if ttft_ms > 0 {
                    self.ttft_sum += ttft_ms as f64;
                    self.ttft_count += 1;
                    self.min_ttft_ms = Some(
                        self.min_ttft_ms
                            .map_or(ttft_ms, |current| current.min(ttft_ms)),
                    );
                    self.max_ttft_ms = Some(
                        self.max_ttft_ms
                            .map_or(ttft_ms, |current| current.max(ttft_ms)),
                    );
                }
            }
        }
    }

    let mut overall = PerfAccumulator::default();
    let mut by_model: HashMap<String, PerfAccumulator> = HashMap::new();

    for fact in &facts {
        overall.add(fact);
        if !fact.model.trim().is_empty() {
            by_model.entry(fact.model.clone()).or_default().add(fact);
        }
    }

    if overall.request_count == 0 {
        return empty_window_rate_summary(window);
    }

    let mut by_model_stats: Vec<ModelRateStats> = by_model
        .into_iter()
        .filter_map(|(model_name, acc)| {
            (acc.request_count > 0).then_some(ModelRateStats {
                model_name,
                request_count: acc.request_count,
                total_output_tokens: acc.total_output_tokens,
                total_duration_ms: acc.total_duration_ms,
                avg_tokens_per_second: if acc.rate_count > 0 {
                    acc.rate_sum / acc.rate_count as f64
                } else {
                    0.0
                },
                min_tokens_per_second: acc.min_rate.unwrap_or(0.0),
                max_tokens_per_second: acc.max_rate.unwrap_or(0.0),
            })
        })
        .collect();
    by_model_stats.sort_by(|a, b| {
        b.avg_tokens_per_second
            .partial_cmp(&a.avg_tokens_per_second)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut ttft_by_model: Vec<ModelTtftStats> = facts
        .iter()
        .fold(
            HashMap::<String, PerfAccumulator>::new(),
            |mut acc, fact| {
                if !fact.model.trim().is_empty() {
                    acc.entry(fact.model.clone()).or_default().add(fact);
                }
                acc
            },
        )
        .into_iter()
        .filter_map(|(model_name, acc)| {
            (acc.ttft_count > 0).then_some(ModelTtftStats {
                model_name,
                request_count: acc.ttft_count,
                avg_ttft_ms: acc.ttft_sum / acc.ttft_count as f64,
                min_ttft_ms: acc.min_ttft_ms.unwrap_or(0),
                max_ttft_ms: acc.max_ttft_ms.unwrap_or(0),
            })
        })
        .collect();
    ttft_by_model.sort_by(|a, b| {
        a.avg_ttft_ms
            .partial_cmp(&b.avg_ttft_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    WindowRateSummary {
        window,
        overall: OverallRateStats {
            request_count: overall.request_count,
            total_output_tokens: overall.total_output_tokens,
            total_duration_ms: overall.total_duration_ms,
            avg_tokens_per_second: if overall.rate_count > 0 {
                overall.rate_sum / overall.rate_count as f64
            } else {
                0.0
            },
        },
        by_model: by_model_stats,
        ttft: TtftStats {
            request_count: overall.ttft_count,
            avg_ttft_ms: if overall.ttft_count > 0 {
                overall.ttft_sum / overall.ttft_count as f64
            } else {
                0.0
            },
            min_ttft_ms: overall.min_ttft_ms.unwrap_or(0),
            max_ttft_ms: overall.max_ttft_ms.unwrap_or(0),
        },
        ttft_by_model,
    }
}

async fn build_merged_usage_snapshot(settings: &AppSettings) -> Result<UsageSnapshot, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let include_errors = settings.proxy.include_error_requests;

    let mut windows = Vec::new();
    let mut has_partial_snapshot_coverage = false;
    for quota in settings.quotas.iter().filter(|quota| quota.enabled) {
        let cutoff_ms = crate::proxy::UsageCollector::calculate_window_cutoff_public(&quota.window);
        let (facts, capability) = crate::unified_usage::get_merged_request_facts(
            settings,
            Some(cutoff_ms / 1000),
            Some(now as i64 + 1),
            include_errors,
        )
        .await?;
        has_partial_snapshot_coverage |=
            capability.has_partial_status_coverage || capability.has_partial_performance_coverage;

        let mut model_stats: HashMap<String, ModelTokenTotals> = HashMap::new();
        let mut token_used = 0_u64;
        let mut input_tokens = 0_u64;
        let mut output_tokens = 0_u64;
        let mut cache_create_tokens = 0_u64;
        let mut cache_read_tokens = 0_u64;
        let request_used = facts.len() as u64;
        let mut success_requests = 0_u64;
        let mut client_error_requests = 0_u64;
        let mut server_error_requests = 0_u64;

        for fact in &facts {
            token_used += fact.total_tokens;
            input_tokens += fact.input_tokens;
            output_tokens += fact.output_tokens;
            cache_create_tokens += fact.cache_create_tokens;
            cache_read_tokens += fact.cache_read_tokens;

            if let Some(status_code) = fact.status_code {
                if (200..300).contains(&status_code) {
                    success_requests += 1;
                } else if (400..500).contains(&status_code) {
                    client_error_requests += 1;
                } else if status_code >= 500 {
                    server_error_requests += 1;
                }
            }

            if !fact.model.is_empty() {
                let entry = model_stats.entry(fact.model.clone()).or_default();
                entry.input_tokens += fact.input_tokens;
                entry.output_tokens += fact.output_tokens;
                entry.cache_create_tokens += fact.cache_create_tokens;
                entry.cache_read_tokens += fact.cache_read_tokens;
                entry.request_count += 1;
            }
        }

        let token_percent = compute_percent(token_used, quota.token_limit);
        let request_percent = compute_percent(request_used, quota.request_limit);
        let cost: f64 = facts.iter().map(|fact| fact.estimated_cost).sum();

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
            cost,
            success_requests: if capability.has_partial_status_coverage {
                0
            } else {
                success_requests
            },
            client_error_requests: if capability.has_partial_status_coverage {
                0
            } else {
                client_error_requests
            },
            server_error_requests: if capability.has_partial_status_coverage {
                0
            } else {
                server_error_requests
            },
        });
    }

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

    let summary_cutoff_ms =
        crate::proxy::UsageCollector::calculate_window_cutoff_public(&settings.summary_window);
    let (summary_facts, summary_coverage) = crate::unified_usage::get_merged_request_facts(
        settings,
        Some(summary_cutoff_ms / 1000),
        Some(now as i64 + 1),
        include_errors,
    )
    .await?;

    let mut summary_model_stats: HashMap<String, ModelTokenTotals> = HashMap::new();
    let mut summary_success_requests = 0_u64;
    let mut summary_client_error_requests = 0_u64;
    let mut summary_server_error_requests = 0_u64;
    for fact in &summary_facts {
        if !fact.model.is_empty() {
            let entry = summary_model_stats.entry(fact.model.clone()).or_default();
            entry.input_tokens += fact.input_tokens;
            entry.output_tokens += fact.output_tokens;
            entry.cache_create_tokens += fact.cache_create_tokens;
            entry.cache_read_tokens += fact.cache_read_tokens;
            entry.request_count += 1;
        }
        if let Some(status_code) = fact.status_code {
            if (200..300).contains(&status_code) {
                summary_success_requests += 1;
            } else if (400..500).contains(&status_code) {
                summary_client_error_requests += 1;
            } else if status_code >= 500 {
                summary_server_error_requests += 1;
            }
        }
    }

    let summary = build_usage_summary_from_window(
        &windows,
        &settings.summary_window,
        overall_risk_level,
        if summary_coverage.has_partial_status_coverage {
            0
        } else {
            summary_success_requests
        },
        if summary_coverage.has_partial_status_coverage {
            0
        } else {
            summary_client_error_requests
        },
        if summary_coverage.has_partial_status_coverage {
            0
        } else {
            summary_server_error_requests
        },
    );

    Ok(UsageSnapshot {
        generated_at_epoch: now,
        windows,
        source: MERGED_SOURCE.to_string(),
        note: (has_partial_snapshot_coverage
            || summary_coverage.has_partial_status_coverage
            || summary_coverage.has_partial_performance_coverage)
            .then_some("NOTE_PARTIAL_PROXY_COVERAGE".to_string()),
        summary,
        model_distribution: build_model_distribution_from_window_stats(Some(&summary_model_stats)),
    })
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
        note: (!note.is_empty()).then_some(note),
        summary,
        model_distribution: Vec::new(),
    }
}

fn estimate_cost_for_local_request(
    record: &crate::session::LocalRequestRecord,
    pricings: &[crate::models::ModelPricingConfig],
    match_mode: &str,
) -> f64 {
    crate::models::estimate_session_cost(
        record.input_tokens,
        record.output_tokens,
        record.cache_create_tokens,
        record.cache_read_tokens,
        &record.model,
        pricings,
        match_mode,
    )
}

fn local_request_records(
    settings: &AppSettings,
) -> Result<Vec<crate::session::LocalRequestRecord>, String> {
    let tool_filter = settings.client_tools.build_filter();
    let db = crate::local_usage::ensure_local_usage_synced()?;
    db.get_all_request_records(&tool_filter)
}

fn local_request_records_by_session(
    db: &crate::local_usage::LocalUsageDatabase,
    session_id: &str,
) -> Result<Vec<crate::session::LocalRequestRecord>, String> {
    db.get_request_records_by_session(session_id)
}

fn local_session_meta_by_id(
    db: &crate::local_usage::LocalUsageDatabase,
    session_id: &str,
) -> Result<Option<crate::session::SessionMeta>, String> {
    db.get_session_by_id(session_id)
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
    if !local_tool_filter_matches_local_jsonl(settings) {
        return Ok(empty_usage_snapshot(settings, "local-files", String::new()));
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let requests = local_request_records(settings)?;
    if requests.is_empty() {
        return Err("ERR_LOCAL_JSONL_NOT_FOUND".to_string());
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

    // 各窗口模型统计
    let mut window_model_stats: HashMap<String, HashMap<String, ModelTokenTotals>> = HashMap::new();
    let pricings = effective_model_pricings(settings);
    let match_mode = &settings.model_pricing.match_mode;

    for record in &requests {
        let record_timestamp = record.timestamp.max(0) as u64;
        let Some(age) = local_record_age_seconds(record_timestamp, now) else {
            continue;
        };
        if age <= 5 * 60 * 60 {
            total_5h_tokens += record.total_tokens;
            total_5h_input_tokens += record.input_tokens;
            total_5h_output_tokens += record.output_tokens;
            total_5h_cache_create_tokens += record.cache_create_tokens;
            total_5h_cache_read_tokens += record.cache_read_tokens;
            total_5h_requests += 1;
            add_window_model_stats(&mut window_model_stats, "5h", record);
        }
        if age <= 24 * 60 * 60 {
            total_24h_tokens += record.total_tokens;
            total_24h_input_tokens += record.input_tokens;
            total_24h_output_tokens += record.output_tokens;
            total_24h_cache_create_tokens += record.cache_create_tokens;
            total_24h_cache_read_tokens += record.cache_read_tokens;
            total_24h_requests += 1;
            add_window_model_stats(&mut window_model_stats, "24h", record);
        }
        // 今天：记录时间戳在今天内
        if record_timestamp >= today_start {
            total_today_tokens += record.total_tokens;
            total_today_input_tokens += record.input_tokens;
            total_today_output_tokens += record.output_tokens;
            total_today_cache_create_tokens += record.cache_create_tokens;
            total_today_cache_read_tokens += record.cache_read_tokens;
            total_today_requests += 1;
            add_window_model_stats(&mut window_model_stats, "today", record);
        }
        if age <= 7 * 24 * 60 * 60 {
            total_7d_tokens += record.total_tokens;
            total_7d_input_tokens += record.input_tokens;
            total_7d_output_tokens += record.output_tokens;
            total_7d_cache_create_tokens += record.cache_create_tokens;
            total_7d_cache_read_tokens += record.cache_read_tokens;
            total_7d_requests += 1;
            add_window_model_stats(&mut window_model_stats, "7d", record);
        }
        if age <= 30 * 24 * 60 * 60 {
            total_30d_tokens += record.total_tokens;
            total_30d_input_tokens += record.input_tokens;
            total_30d_output_tokens += record.output_tokens;
            total_30d_cache_create_tokens += record.cache_create_tokens;
            total_30d_cache_read_tokens += record.cache_read_tokens;
            total_30d_requests += 1;
            add_window_model_stats(&mut window_model_stats, "30d", record);
        }
        // 当前月份：记录时间戳在本月内
        if record_timestamp >= current_month_start {
            total_current_month_tokens += record.total_tokens;
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
        let request_limit = quota.request_limit;
        let request_percent = compute_percent(request_used, request_limit);

        windows.push(WindowUsage {
            window: quota.window.clone(),
            token_used,
            input_tokens,
            output_tokens,
            cache_create_tokens,
            cache_read_tokens,
            request_used,
            token_limit: quota.token_limit,
            request_limit,
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

    let model_distribution = build_model_distribution_from_window_stats(
        window_model_stats.get(&settings.summary_window),
    );

    let summary = build_usage_summary_from_window(
        &windows,
        &settings.summary_window,
        overall_risk_level,
        0,
        0,
        0,
    );

    Ok(UsageSnapshot {
        generated_at_epoch: now,
        windows,
        source: "local-files".to_string(),
        note: None,
        summary,
        model_distribution,
    })
}

// 辅助类型和函数
#[derive(Default)]
struct ModelTokenTotals {
    input_tokens: u64,
    output_tokens: u64,
    cache_create_tokens: u64,
    cache_read_tokens: u64,
    request_count: u64,
}

fn add_window_model_stats(
    window_model_stats: &mut HashMap<String, HashMap<String, ModelTokenTotals>>,
    window: &str,
    record: &crate::session::LocalRequestRecord,
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
    entry.request_count += 1;
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

fn build_model_distribution_from_window_stats(
    window_stats: Option<&HashMap<String, ModelTokenTotals>>,
) -> Vec<crate::models::ModelUsage> {
    let total_model_tokens: u64 = window_stats
        .into_iter()
        .flat_map(|stats| stats.values())
        .map(|totals| {
            totals.input_tokens
                + totals.output_tokens
                + totals.cache_create_tokens
                + totals.cache_read_tokens
        })
        .sum();

    let mut model_distribution: Vec<crate::models::ModelUsage> = window_stats
        .into_iter()
        .flat_map(|stats| stats.iter())
        .map(|(model_name, totals)| {
            let tokens = totals.input_tokens
                + totals.output_tokens
                + totals.cache_create_tokens
                + totals.cache_read_tokens;
            let percent = if total_model_tokens > 0 {
                (tokens as f64 / total_model_tokens as f64) * 100.0
            } else {
                0.0
            };

            crate::models::ModelUsage {
                model_name: model_name.clone(),
                token_used: tokens,
                input_tokens: totals.input_tokens,
                output_tokens: totals.output_tokens,
                cache_create_tokens: totals.cache_create_tokens,
                cache_read_tokens: totals.cache_read_tokens,
                request_count: totals.request_count,
                percent,
                status_codes: Vec::new(),
            }
        })
        .collect();

    model_distribution.sort_by_key(|entry| std::cmp::Reverse(entry.token_used));
    model_distribution.truncate(5);
    model_distribution
}

fn build_usage_summary_from_window(
    windows: &[WindowUsage],
    summary_window: &str,
    overall_risk_level: String,
    total_success_requests: u64,
    total_client_error_requests: u64,
    total_server_error_requests: u64,
) -> crate::models::UsageSummary {
    let selected_window = windows.iter().find(|w| w.window == summary_window);

    crate::models::UsageSummary {
        total_tokens: selected_window.map(|w| w.token_used).unwrap_or(0),
        total_requests: selected_window.map(|w| w.request_used).unwrap_or(0),
        total_input_tokens: selected_window.map(|w| w.input_tokens).unwrap_or(0),
        total_output_tokens: selected_window.map(|w| w.output_tokens).unwrap_or(0),
        total_cache_create_tokens: selected_window.map(|w| w.cache_create_tokens).unwrap_or(0),
        total_cache_read_tokens: selected_window.map(|w| w.cache_read_tokens).unwrap_or(0),
        total_cost: selected_window.map(|w| w.cost).unwrap_or(0.0),
        overall_risk_level,
        total_success_requests,
        total_client_error_requests,
        total_server_error_requests,
    }
}

fn local_record_age_seconds(record_timestamp: u64, now: u64) -> Option<u64> {
    if record_timestamp > now {
        None
    } else {
        Some(now - record_timestamp)
    }
}

fn local_tool_filter_matches_local_jsonl(settings: &AppSettings) -> bool {
    match settings.client_tools.build_filter() {
        ToolFilter::All => true,
        ToolFilter::Tool(tool) => {
            let tool = tool.trim();
            tool.is_empty() || tool == "claude_code" || tool == "codex"
        }
    }
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
        // 总 Token = 输入 + 缓存创建 + 缓存读取 + 输出
        self.total_tokens += input + output + cache_create + cache_read;
        self.cost += cost;
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

fn build_jsonl_statistics(
    query: &StatisticsQuery,
    settings: &AppSettings,
) -> Result<StatisticsSummary, String> {
    let (start_epoch, end_epoch) = normalize_range(query);
    let pricings = effective_model_pricings(settings);
    let match_mode = settings.model_pricing.match_mode.clone();
    let mut total = StatAccumulator::default();
    let mut trend_map: HashMap<i64, StatAccumulator> = HashMap::new();
    let mut model_map: HashMap<String, StatAccumulator> = HashMap::new();
    let mut model_trend_map: HashMap<String, HashMap<i64, StatAccumulator>> = HashMap::new();

    for record in local_request_records(settings)? {
        let event_epoch = record.timestamp;
        if event_epoch < start_epoch || event_epoch >= end_epoch {
            continue;
        }
        let model = if record.model.is_empty() {
            "unknown".to_string()
        } else {
            record.model.clone()
        };
        let cost = estimate_cost_for_local_request(&record, &pricings, &match_mode);
        let bucket = bucket_start(event_epoch, &query.bucket);
        total.add_tokens(
            record.input_tokens,
            record.output_tokens,
            record.cache_create_tokens,
            record.cache_read_tokens,
            1,
            cost,
        );
        trend_map.entry(bucket).or_default().add_tokens(
            record.input_tokens,
            record.output_tokens,
            record.cache_create_tokens,
            record.cache_read_tokens,
            1,
            cost,
        );
        model_map.entry(model.clone()).or_default().add_tokens(
            record.input_tokens,
            record.output_tokens,
            record.cache_create_tokens,
            record.cache_read_tokens,
            1,
            cost,
        );
        model_trend_map
            .entry(model.clone())
            .or_default()
            .entry(bucket)
            .or_default()
            .add_tokens(
                record.input_tokens,
                record.output_tokens,
                record.cache_create_tokens,
                record.cache_read_tokens,
                1,
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

    Ok(StatisticsSummary {
        generated_at_epoch: chrono::Utc::now().timestamp(),
        source: "local-files".to_string(),
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
    })
}

#[tauri::command]
pub async fn get_statistics_summary(
    query: StatisticsQuery,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<StatisticsSummary, String> {
    let (start_epoch, end_epoch) = normalize_range(&query);
    if settings.data_source == "proxy" {
        let include_errors = settings.proxy.include_error_requests;
        let (facts, coverage) = crate::unified_usage::get_merged_request_facts(
            &settings,
            Some(start_epoch),
            Some(end_epoch),
            include_errors,
        )
        .await?;
        return Ok(build_merged_statistics(facts, &coverage, &query));
    }

    build_jsonl_statistics(&query, &settings)
}

fn month_day_count(year: i32, month: u8) -> u32 {
    for day in (28..=31).rev() {
        if NaiveDate::from_ymd_opt(year, month as u32, day).is_some() {
            return day;
        }
    }
    30
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
    let mut partial_status_days = std::collections::HashSet::new();

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
        let include_errors = settings.proxy.include_error_requests;
        let (facts, _coverage) = crate::unified_usage::get_merged_request_facts(
            &settings,
            Some(month_start),
            Some(month_end),
            include_errors,
        )
        .await?;
        collect_day_activity_from_facts(facts, &mut day_map, &mut partial_status_days);
    } else {
        for record in local_request_records(&settings)? {
            let event_epoch = record.timestamp;
            if event_epoch < month_start || event_epoch >= month_end {
                continue;
            }
            let date = Local
                .timestamp_opt(event_epoch, 0)
                .single()
                .unwrap_or_else(Local::now)
                .format("%Y-%m-%d")
                .to_string();
            let cost = estimate_cost_for_local_request(&record, &pricings, &match_mode);
            let entry = day_map.entry(date).or_default();
            entry.0.add_tokens(
                record.input_tokens,
                record.output_tokens,
                record.cache_create_tokens,
                record.cache_read_tokens,
                1,
                cost,
            );
            if !record.model.is_empty() {
                entry.1.insert(record.model);
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
        let status_available =
            settings.data_source == "proxy" && !partial_status_days.contains(&key);
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
            success_requests: status_available.then_some(acc.success_requests),
            error_requests: status_available.then_some(error_requests),
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
    let mut partial_status_days = std::collections::HashSet::new();

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
        let include_errors = settings.proxy.include_error_requests;
        let (facts, _coverage) = crate::unified_usage::get_merged_request_facts(
            &settings,
            Some(year_start),
            Some(year_end),
            include_errors,
        )
        .await?;
        collect_day_activity_from_facts(facts, &mut day_map, &mut partial_status_days);
    } else {
        for record in local_request_records(&settings)? {
            let event_epoch = record.timestamp;
            if event_epoch < year_start || event_epoch >= year_end {
                continue;
            }
            let date = Local
                .timestamp_opt(event_epoch, 0)
                .single()
                .unwrap_or_else(Local::now)
                .format("%Y-%m-%d")
                .to_string();
            let cost = estimate_cost_for_local_request(&record, &pricings, &match_mode);
            let entry = day_map.entry(date).or_default();
            entry.0.add_tokens(
                record.input_tokens,
                record.output_tokens,
                record.cache_create_tokens,
                record.cache_read_tokens,
                1,
                cost,
            );
            if !record.model.is_empty() {
                entry.1.insert(record.model);
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
        let status_available =
            settings.data_source == "proxy" && !partial_status_days.contains(&key);
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
            success_requests: status_available.then_some(acc.success_requests),
            error_requests: status_available.then_some(error_requests),
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
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<WindowRateSummary, String> {
    let settings = crate::commands::load_settings()?;
    if settings.data_source != "proxy" {
        return Ok(empty_window_rate_summary(window));
    }

    let cutoff_ms = crate::proxy::UsageCollector::calculate_window_cutoff_public(&window);
    let include_errors = settings.proxy.include_error_requests;
    let (facts, coverage) = crate::unified_usage::get_merged_request_facts(
        &settings,
        Some(cutoff_ms / 1000),
        None,
        include_errors,
    )
    .await?;

    Ok(build_window_rate_summary_from_facts(
        window, facts, &coverage,
    ))
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
    if settings.data_source == "proxy" {
        return crate::unified_usage::get_merged_sessions(&settings, limit, offset).await;
    }

    // 获取价格配置
    let pricings = effective_model_pricings(&settings);
    let match_mode = settings.model_pricing.match_mode.clone();

    // 1. 从 JSONL 文件获取会话列表（主数据源）
    // 使用缓存版本避免频繁扫描文件系统
    let tool_filter = settings.client_tools.build_filter();
    let db = crate::local_usage::ensure_local_usage_synced()?;
    let all_meta = db.get_all_sessions(&tool_filter)?;

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
            // 本地文件模式下不查询代理性能数据
            std::collections::HashMap::new()
        };

    // 4. 构建 SessionStats，合并 JSONL 数据和 session_stats 数据
    let sessions: Vec<SessionStats> = meta_list
        .into_iter()
        .map(|meta| {
            let session_requests =
                local_request_records_by_session(db.as_ref(), &meta.session_id).unwrap_or_default();
            let jsonl_cost: f64 = session_requests
                .iter()
                .map(|record| estimate_cost_for_local_request(record, &pricings, &match_mode))
                .sum();

            // 尝试从 session_stats 获取性能指标
            if let Some(proxy) = proxy_stats_map.get(&meta.session_id) {
                // 合并数据：JSONL 的 token 统计 + session_stats 的性能指标
                SessionStats {
                    session_id: meta.session_id,
                    tool: meta.tool,
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
                    tool: meta.tool,
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
    if settings.data_source == "proxy" {
        return crate::unified_usage::get_merged_session_detail(&settings, &session_id).await;
    }

    // 获取价格配置
    let pricings = effective_model_pricings(&settings);
    let match_mode = settings.model_pricing.match_mode.clone();

    // 1. 从 JSONL 获取会话元信息
    let db = crate::local_usage::ensure_local_usage_synced()?;
    let meta = match local_session_meta_by_id(db.as_ref(), &session_id)? {
        Some(m) => m,
        None => return Ok(None),
    };

    // 2. 计算基于 JSONL 的费用
    let session_requests = local_request_records_by_session(db.as_ref(), &meta.session_id)?;
    let jsonl_cost: f64 = session_requests
        .iter()
        .map(|record| estimate_cost_for_local_request(record, &pricings, &match_mode))
        .sum();

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
        // 本地文件模式下不查询代理性能数据
        None
    };

    // 4. 合并数据：JSONL 的 token 统计 + session_stats 的性能指标
    let stats = if let Some(proxy) = proxy_stats {
        SessionStats {
            session_id: meta.session_id,
            tool: meta.tool,
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
            tool: meta.tool,
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
    if settings.data_source == "proxy" {
        return crate::unified_usage::get_merged_project_stats(&settings).await;
    }

    // 获取价格配置
    let pricings = effective_model_pricings(&settings);
    let match_mode = settings.model_pricing.match_mode.clone();
    let db = crate::local_usage::ensure_local_usage_synced()?;

    // 1. 从 JSONL 文件获取所有会话元信息
    // 使用缓存版本避免频繁扫描文件系统
    let tool_filter = ToolFilter::All;
    let all_meta = db.get_all_sessions(&tool_filter)?;
    let all_requests = db.get_all_request_records(&tool_filter)?;
    let mut cost_by_session: HashMap<String, f64> = HashMap::new();
    for record in &all_requests {
        let cost = estimate_cost_for_local_request(record, &pricings, &match_mode);
        *cost_by_session
            .entry(record.session_id.clone())
            .or_insert(0.0) += cost;
    }

    // 2. 按项目名称聚合
    let mut project_map: std::collections::HashMap<String, crate::proxy::ProjectStats> =
        std::collections::HashMap::new();
    let mut tool_sessions_by_project: HashMap<
        String,
        HashMap<String, std::collections::HashSet<String>>,
    > = HashMap::new();

    for meta in all_meta {
        let project_name = meta
            .project_name
            .clone()
            .unwrap_or_else(|| "未命名项目".to_string());
        let project_path = meta.cwd.clone();
        let aggregate_key = project_path
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| project_name.clone());

        let cost = cost_by_session
            .get(&meta.session_id)
            .copied()
            .unwrap_or(0.0);

        let entry =
            project_map
                .entry(aggregate_key.clone())
                .or_insert(crate::proxy::ProjectStats {
                    name: project_name.clone(),
                    project_path: project_path.clone(),
                    ..Default::default()
                });

        if entry.project_path.is_none() {
            entry.project_path = project_path.clone();
        }

        entry.session_count += 1;
        entry.total_input_tokens += meta.total_input_tokens;
        entry.total_output_tokens += meta.total_output_tokens;
        entry.total_cache_create_tokens += meta.total_cache_create_tokens;
        entry.total_cache_read_tokens += meta.total_cache_read_tokens;
        entry.total_cost += cost;
        entry.request_count += meta.message_count;
        if meta.end_time > entry.last_active {
            entry.last_active = meta.end_time;
        }

        let tool_stats = entry
            .tool_breakdown
            .iter_mut()
            .find(|stats| stats.tool == meta.tool);
        let tool_stats = match tool_stats {
            Some(stats) => stats,
            None => {
                entry.tool_breakdown.push(ProjectToolStats {
                    tool: meta.tool.clone(),
                    ..Default::default()
                });
                entry.tool_breakdown.last_mut().unwrap()
            }
        };
        tool_stats.total_input_tokens += meta.total_input_tokens;
        tool_stats.total_output_tokens += meta.total_output_tokens;
        tool_stats.total_cache_create_tokens += meta.total_cache_create_tokens;
        tool_stats.total_cache_read_tokens += meta.total_cache_read_tokens;
        tool_stats.total_cost += cost;
        tool_stats.request_count += meta.message_count;
        if meta.end_time > tool_stats.last_active {
            tool_stats.last_active = meta.end_time;
        }

        tool_sessions_by_project
            .entry(aggregate_key)
            .or_default()
            .entry(meta.tool)
            .or_default()
            .insert(meta.session_id);
    }

    // 4. 按最后活跃时间倒序排序
    let mut projects: Vec<_> = project_map.into_iter().collect();
    for (aggregate_key, project) in &mut projects {
        if let Some(tool_sessions) = tool_sessions_by_project.get(aggregate_key) {
            for tool_stats in &mut project.tool_breakdown {
                tool_stats.session_count = tool_sessions
                    .get(&tool_stats.tool)
                    .map(|sessions| sessions.len() as u64)
                    .unwrap_or(0);
            }
        }
        project
            .tool_breakdown
            .sort_by_key(|tool| std::cmp::Reverse(tool.last_active));
    }
    let mut projects: Vec<_> = projects.into_iter().map(|(_, project)| project).collect();
    projects.sort_by_key(|b| std::cmp::Reverse(b.last_active));

    Ok(projects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn local_record_age_seconds_rejects_future_timestamps() {
        assert_eq!(local_record_age_seconds(100, 100), Some(0));
        assert_eq!(local_record_age_seconds(80, 100), Some(20));
        assert_eq!(local_record_age_seconds(101, 100), None);
    }

    #[test]
    fn build_usage_summary_from_window_keeps_cost_in_same_window() {
        let windows = vec![
            WindowUsage {
                window: "5h".to_string(),
                token_used: 120,
                input_tokens: 70,
                output_tokens: 40,
                cache_create_tokens: 5,
                cache_read_tokens: 5,
                request_used: 3,
                token_limit: None,
                request_limit: None,
                token_percent: None,
                request_percent: None,
                risk_level: "safe".to_string(),
                cost: 1.25,
                success_requests: 0,
                client_error_requests: 0,
                server_error_requests: 0,
            },
            WindowUsage {
                window: "30d".to_string(),
                token_used: 2400,
                input_tokens: 1400,
                output_tokens: 800,
                cache_create_tokens: 100,
                cache_read_tokens: 100,
                request_used: 60,
                token_limit: None,
                request_limit: None,
                token_percent: None,
                request_percent: None,
                risk_level: "warning".to_string(),
                cost: 9.75,
                success_requests: 0,
                client_error_requests: 0,
                server_error_requests: 0,
            },
        ];

        let summary =
            build_usage_summary_from_window(&windows, "5h", "warning".to_string(), 0, 0, 0);

        assert_eq!(summary.total_tokens, 120);
        assert_eq!(summary.total_requests, 3);
        assert_eq!(summary.total_input_tokens, 70);
        assert_eq!(summary.total_output_tokens, 40);
        assert_eq!(summary.total_cache_create_tokens, 5);
        assert_eq!(summary.total_cache_read_tokens, 5);
        assert_eq!(summary.total_cost, 1.25);
        assert_eq!(summary.overall_risk_level, "warning");
    }

    #[test]
    fn build_model_distribution_from_window_stats_uses_selected_window_only() {
        let mut five_hour = HashMap::new();
        five_hour.insert(
            "model-a".to_string(),
            ModelTokenTotals {
                input_tokens: 70,
                output_tokens: 30,
                cache_create_tokens: 0,
                cache_read_tokens: 0,
                request_count: 2,
            },
        );

        let mut thirty_day = HashMap::new();
        thirty_day.insert(
            "model-b".to_string(),
            ModelTokenTotals {
                input_tokens: 700,
                output_tokens: 300,
                cache_create_tokens: 0,
                cache_read_tokens: 0,
                request_count: 20,
            },
        );

        let five_hour_distribution = build_model_distribution_from_window_stats(Some(&five_hour));
        let thirty_day_distribution = build_model_distribution_from_window_stats(Some(&thirty_day));

        assert_eq!(five_hour_distribution.len(), 1);
        assert_eq!(five_hour_distribution[0].model_name, "model-a");
        assert_eq!(five_hour_distribution[0].token_used, 100);
        assert_eq!(five_hour_distribution[0].request_count, 2);
        assert_eq!(five_hour_distribution[0].percent, 100.0);

        assert_eq!(thirty_day_distribution.len(), 1);
        assert_eq!(thirty_day_distribution[0].model_name, "model-b");
        assert_eq!(thirty_day_distribution[0].token_used, 1000);
        assert_eq!(thirty_day_distribution[0].request_count, 20);
    }
}
