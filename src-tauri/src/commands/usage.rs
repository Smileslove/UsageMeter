//! 用量相关 Tauri 命令

use crate::models::{
    AppSettings, ModelRateStats, ModelTtftStats, OverallRateStats, StatusCodeCount, TtftStats,
    UsageSnapshot, WindowRateSummary, WindowUsage,
};
use crate::proxy::{compute_source_id, ProxyServer, SessionStats};
use crate::unified_usage::{CoverageOrigin, MergedCoverage, MergedRequestFact};
use chrono::{Local, NaiveDate, TimeZone};
use std::collections::{HashMap, HashSet};
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

const MERGED_SOURCE: &str = "proxy-merged";
const USAGE_WINDOWS: &[&str] = &["5h", "24h", "today", "7d", "30d", "current_month"];
const MODEL_TREND_LIMIT: usize = 6;

#[derive(Debug, Clone)]
struct WindowPreparedFacts {
    window: String,
    start_index: usize,
}

#[derive(Debug, Clone)]
struct PreparedUsageRefreshData {
    generated_at_epoch: u64,
    facts: Vec<MergedRequestFact>,
    windows: Vec<WindowPreparedFacts>,
}

#[tauri::command]
pub async fn refresh_usage_bundle(
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<UsageRefreshBundle, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let prepared = prepare_usage_refresh_data(&settings, now).await?;
    Ok(build_usage_refresh_bundle_from_prepared(
        &settings, &prepared,
    ))
}

fn usage_window_cutoff_epoch(window: &str) -> i64 {
    crate::proxy::UsageCollector::calculate_window_cutoff_public(window) / 1000
}

fn epoch_u64_to_i64_saturating(epoch: u64) -> i64 {
    i64::try_from(epoch).unwrap_or(i64::MAX)
}

fn first_fact_index_in_range(facts: &[MergedRequestFact], cutoff_epoch: i64) -> usize {
    facts.partition_point(|fact| fact.timestamp_sec < cutoff_epoch)
}

fn facts_slice_for_window<'a>(
    prepared: &'a PreparedUsageRefreshData,
    window: &str,
) -> &'a [MergedRequestFact] {
    prepared
        .windows
        .iter()
        .find(|entry| entry.window == window)
        .map(|entry| &prepared.facts[entry.start_index..])
        .unwrap_or(&prepared.facts[..0])
}

async fn prepare_usage_refresh_data(
    settings: &AppSettings,
    generated_at_epoch: u64,
) -> Result<PreparedUsageRefreshData, String> {
    let include_errors = settings.proxy.include_error_requests;
    let mut window_cutoffs: Vec<(String, i64)> = USAGE_WINDOWS
        .iter()
        .map(|window| ((*window).to_string(), usage_window_cutoff_epoch(window)))
        .collect();
    let summary_window = settings.summary_window.clone();
    if !window_cutoffs
        .iter()
        .any(|(window, _)| *window == summary_window)
    {
        window_cutoffs.push((
            summary_window.clone(),
            usage_window_cutoff_epoch(&summary_window),
        ));
    }

    let earliest_cutoff = window_cutoffs
        .iter()
        .map(|(_, cutoff)| *cutoff)
        .min()
        .unwrap_or(0);
    let end_epoch = generated_at_epoch.saturating_add(1);
    let (facts, _coverage) = crate::unified_usage::get_merged_request_facts(
        settings,
        Some(earliest_cutoff),
        Some(epoch_u64_to_i64_saturating(end_epoch)),
        include_errors,
    )
    .await?;

    let windows = window_cutoffs
        .into_iter()
        .map(|(window, cutoff_epoch)| WindowPreparedFacts {
            start_index: first_fact_index_in_range(&facts, cutoff_epoch),
            window,
        })
        .collect();

    Ok(PreparedUsageRefreshData {
        generated_at_epoch,
        facts,
        windows,
    })
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

    // 按来源分类计数
    if matches!(fact.coverage_origin, CoverageOrigin::LocalOnly) {
        acc.local_request_count += 1;
    } else {
        acc.proxy_request_count += 1;
    }

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
                trend: Vec::new(),
            }
        })
        .collect();
    models.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens));

    let top_model_names: HashSet<String> = models
        .iter()
        .take(MODEL_TREND_LIMIT)
        .map(|model| model.model_name.clone())
        .collect();
    let mut model_trend_map: HashMap<String, HashMap<i64, StatAccumulator>> = HashMap::new();

    for fact in &facts {
        let model_name = if fact.model.is_empty() {
            "unknown"
        } else {
            fact.model.as_str()
        };
        if !top_model_names.contains(model_name) {
            continue;
        }
        let bucket = bucket_start(fact.timestamp_sec, &query.bucket);
        add_fact_to_stat_acc(
            model_trend_map
                .entry(model_name.to_string())
                .or_default()
                .entry(bucket)
                .or_default(),
            fact,
        );
    }

    for model in models.iter_mut().take(MODEL_TREND_LIMIT) {
        model.trend = model_trend_map
            .get(&model.model_name)
            .map(|trend_map| trend_from_map(trend_map, start_epoch, end_epoch, &query.bucket))
            .unwrap_or_else(|| make_empty_trend(start_epoch, end_epoch, &query.bucket));
    }

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

    let totals = totals_from_acc(
        &total,
        models.len() as u64,
        // 只要有任何请求采集到了状态码就如实返回
        total.success_requests + total.client_error_requests + total.server_error_requests > 0,
    );
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

fn build_window_usage_from_facts(
    window: &str,
    facts: &[MergedRequestFact],
    coverage: &MergedCoverage,
) -> (WindowUsage, HashMap<String, ModelTokenTotals>) {
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

    for fact in facts {
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

    let cost: f64 = facts.iter().map(|fact| fact.estimated_cost).sum();
    (
        WindowUsage {
            window: window.to_string(),
            token_used,
            input_tokens,
            output_tokens,
            cache_create_tokens,
            cache_read_tokens,
            request_used,
            cost,
            success_requests: if coverage.has_partial_status_coverage {
                0
            } else {
                success_requests
            },
            client_error_requests: if coverage.has_partial_status_coverage {
                0
            } else {
                client_error_requests
            },
            server_error_requests: if coverage.has_partial_status_coverage {
                0
            } else {
                server_error_requests
            },
        },
        model_stats,
    )
}

fn build_overview_breakdown_from_facts(
    settings: &AppSettings,
    window: String,
    generated_at_epoch: i64,
    facts: &[MergedRequestFact],
    coverage: &MergedCoverage,
) -> OverviewBreakdown {
    let mut source_map: HashMap<String, (BreakdownMeta, BreakdownAccumulator)> = HashMap::new();
    let mut tool_map: HashMap<String, (BreakdownMeta, BreakdownAccumulator)> = HashMap::new();
    let mut model_map: HashMap<String, (BreakdownMeta, BreakdownAccumulator)> = HashMap::new();

    for fact in facts {
        add_breakdown_fact(&mut source_map, source_meta_for_fact(settings, fact), fact);
        add_breakdown_fact(&mut tool_map, tool_meta_for_fact(settings, fact), fact);
        add_breakdown_fact(&mut model_map, model_meta_for_fact(fact), fact);
    }

    let capability = OverviewBreakdownCapability {
        has_source: !source_map.is_empty(),
        has_tool: !tool_map.is_empty(),
        has_cost: facts.iter().any(|fact| fact.estimated_cost > 0.0),
        has_status: !coverage.has_partial_status_coverage
            && facts.iter().any(|fact| fact.status_code.is_some()),
        has_performance: !coverage.has_partial_performance_coverage
            && facts
                .iter()
                .any(|fact| fact.output_tokens_per_second.is_some() || fact.ttft_ms.is_some()),
    };

    OverviewBreakdown {
        window,
        generated_at_epoch,
        source_ranking: overview_items_from_map(source_map),
        tool_ranking: overview_items_from_map(tool_map),
        model_ranking: overview_items_from_map(model_map),
        capability,
    }
}

fn build_usage_refresh_bundle_from_prepared(
    settings: &AppSettings,
    prepared: &PreparedUsageRefreshData,
) -> UsageRefreshBundle {
    let mut windows = Vec::new();
    let mut has_partial_snapshot_coverage = false;
    let mut summary_model_stats: Option<HashMap<String, ModelTokenTotals>> = None;

    for window_name in USAGE_WINDOWS {
        let facts = facts_slice_for_window(prepared, window_name);
        let coverage = crate::unified_usage::build_coverage(facts);
        has_partial_snapshot_coverage |=
            coverage.has_partial_status_coverage || coverage.has_partial_performance_coverage;
        let (window_usage, model_stats) =
            build_window_usage_from_facts(window_name, facts, &coverage);
        if *window_name == settings.summary_window {
            summary_model_stats = Some(model_stats.clone());
        }
        windows.push(window_usage);
    }

    let summary_facts = facts_slice_for_window(prepared, &settings.summary_window);
    let summary_coverage = crate::unified_usage::build_coverage(summary_facts);
    let (summary_window_usage, summary_window_model_stats) =
        build_window_usage_from_facts(&settings.summary_window, summary_facts, &summary_coverage);
    if summary_model_stats.is_none() {
        summary_model_stats = Some(summary_window_model_stats);
    }
    let derived_summary_model_stats = summary_model_stats
        .is_none()
        .then(|| build_model_token_totals_from_facts(summary_facts));
    let summary_model_distribution = build_model_distribution_from_window_stats(
        summary_model_stats
            .as_ref()
            .or(derived_summary_model_stats.as_ref()),
    );
    let mut summary_success_requests = 0_u64;
    let mut summary_client_error_requests = 0_u64;
    let mut summary_server_error_requests = 0_u64;

    for fact in summary_facts {
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

    let summary = build_usage_summary_from_usage(
        &summary_window_usage,
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
    let snapshot = UsageSnapshot {
        generated_at_epoch: prepared.generated_at_epoch,
        windows,
        source: MERGED_SOURCE.to_string(),
        note: (has_partial_snapshot_coverage
            || summary_coverage.has_partial_status_coverage
            || summary_coverage.has_partial_performance_coverage)
            .then_some("NOTE_PARTIAL_PROXY_COVERAGE".to_string()),
        summary,
        model_distribution: summary_model_distribution,
    };

    let rate_summary = build_window_rate_summary_from_facts(
        settings.summary_window.clone(),
        summary_facts.to_vec(),
        &summary_coverage,
    );
    let overview_breakdown = build_overview_breakdown_from_facts(
        settings,
        settings.summary_window.clone(),
        epoch_u64_to_i64_saturating(prepared.generated_at_epoch),
        summary_facts,
        &summary_coverage,
    );

    UsageRefreshBundle {
        generated_at_epoch: prepared.generated_at_epoch,
        snapshot,
        rate_summary,
        overview_breakdown,
    }
}

#[derive(Default)]
struct BreakdownAccumulator {
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
    has_status: bool,
    rate_sum: f64,
    rate_count: u64,
    ttft_sum: f64,
    ttft_count: u64,
    last_seen_ms: i64,
}

impl BreakdownAccumulator {
    fn add(&mut self, fact: &MergedRequestFact) {
        self.request_count += 1;
        self.total_tokens += fact.total_tokens;
        self.input_tokens += fact.input_tokens;
        self.output_tokens += fact.output_tokens;
        self.cache_create_tokens += fact.cache_create_tokens;
        self.cache_read_tokens += fact.cache_read_tokens;
        self.cost += fact.estimated_cost;
        self.last_seen_ms = self.last_seen_ms.max(fact.timestamp_ms);

        if let Some(status_code) = fact.status_code {
            self.has_status = true;
            if (200..300).contains(&status_code) {
                self.success_requests += 1;
            } else if (400..500).contains(&status_code) {
                self.client_error_requests += 1;
            } else if status_code >= 500 {
                self.server_error_requests += 1;
            }
        }

        if let Some(rate) = fact.output_tokens_per_second {
            if rate > 0.0 {
                self.rate_sum += rate;
                self.rate_count += 1;
            }
        }

        if let Some(ttft_ms) = fact.ttft_ms {
            if ttft_ms > 0 {
                self.ttft_sum += ttft_ms as f64;
                self.ttft_count += 1;
            }
        }
    }
}

struct BreakdownMeta {
    id: String,
    label: String,
    kind: String,
    color: Option<String>,
    icon: Option<String>,
}

#[derive(Clone, Copy)]
enum BreakdownPercentMetric {
    Cost,
    Tokens,
    Requests,
}

fn metric_for_percent(
    items: &HashMap<String, (BreakdownMeta, BreakdownAccumulator)>,
) -> (BreakdownPercentMetric, f64) {
    let cost_total: f64 = items.values().map(|(_, acc)| acc.cost).sum();
    if cost_total > 0.0 {
        return (BreakdownPercentMetric::Cost, cost_total);
    }

    let token_total: u64 = items.values().map(|(_, acc)| acc.total_tokens).sum();
    if token_total > 0 {
        return (BreakdownPercentMetric::Tokens, token_total as f64);
    }

    (
        BreakdownPercentMetric::Requests,
        items
            .values()
            .map(|(_, acc)| acc.request_count)
            .sum::<u64>() as f64,
    )
}

fn metric_value_for_percent(acc: &BreakdownAccumulator, metric: BreakdownPercentMetric) -> f64 {
    match metric {
        BreakdownPercentMetric::Cost => acc.cost,
        BreakdownPercentMetric::Tokens => acc.total_tokens as f64,
        BreakdownPercentMetric::Requests => acc.request_count as f64,
    }
}

fn overview_items_from_map(
    map: HashMap<String, (BreakdownMeta, BreakdownAccumulator)>,
) -> Vec<OverviewBreakdownItem> {
    let (percent_metric, denominator) = metric_for_percent(&map);
    let mut items: Vec<OverviewBreakdownItem> = map
        .into_values()
        .map(|(meta, acc)| {
            let error_requests = acc.client_error_requests + acc.server_error_requests;
            OverviewBreakdownItem {
                id: meta.id,
                label: meta.label,
                kind: meta.kind,
                color: meta.color,
                icon: meta.icon,
                request_count: acc.request_count,
                total_tokens: acc.total_tokens,
                input_tokens: acc.input_tokens,
                output_tokens: acc.output_tokens,
                cache_create_tokens: acc.cache_create_tokens,
                cache_read_tokens: acc.cache_read_tokens,
                cost: acc.cost,
                percent: if denominator > 0.0 {
                    (metric_value_for_percent(&acc, percent_metric) / denominator) * 100.0
                } else {
                    0.0
                },
                success_requests: acc.has_status.then_some(acc.success_requests),
                error_requests: acc.has_status.then_some(error_requests),
                avg_tokens_per_second: (acc.rate_count > 0)
                    .then_some(acc.rate_sum / acc.rate_count as f64),
                avg_ttft_ms: (acc.ttft_count > 0).then_some(acc.ttft_sum / acc.ttft_count as f64),
                last_seen_ms: (acc.last_seen_ms > 0).then_some(acc.last_seen_ms),
            }
        })
        .collect();

    items.sort_by(|a, b| {
        b.cost
            .partial_cmp(&a.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.total_tokens.cmp(&a.total_tokens))
            .then_with(|| b.request_count.cmp(&a.request_count))
    });
    items
}

fn source_label_from_url(base_url: Option<&str>) -> String {
    match base_url {
        Some(url) if !url.trim().is_empty() => url
            .split("://")
            .nth(1)
            .unwrap_or(url)
            .split('/')
            .next()
            .unwrap_or(url)
            .to_string(),
        _ => "__official_api__".to_string(),
    }
}

fn source_meta_for_fact(settings: &AppSettings, fact: &MergedRequestFact) -> BreakdownMeta {
    let matched = settings.source_aware.sources.iter().find(|source| {
        let base_url_matches = source.base_url == fact.request_base_url;
        let key_matches = fact
            .api_key_prefix
            .as_ref()
            .map(|prefix| source.api_key_prefixes.contains(prefix))
            .unwrap_or(false);
        base_url_matches && key_matches
    });

    if let Some(source) = matched {
        return BreakdownMeta {
            id: source.id.clone(),
            label: source
                .display_name
                .clone()
                .unwrap_or_else(|| source_label_from_url(source.base_url.as_deref())),
            kind: "source".to_string(),
            color: Some(source.color.clone()),
            icon: source.icon.clone(),
        };
    }

    if let Some(prefix) = fact
        .api_key_prefix
        .as_ref()
        .filter(|prefix| !prefix.trim().is_empty())
    {
        let source_id = compute_source_id(prefix, fact.request_base_url.as_deref());
        return BreakdownMeta {
            id: source_id,
            label: source_label_from_url(fact.request_base_url.as_deref()),
            kind: "source".to_string(),
            color: Some("#9CA3AF".to_string()),
            icon: None,
        };
    }

    BreakdownMeta {
        id: "__unknown__".to_string(),
        label: "__unknown__".to_string(),
        kind: "source".to_string(),
        color: Some("#9CA3AF".to_string()),
        icon: None,
    }
}

fn tool_meta_for_fact(settings: &AppSettings, fact: &MergedRequestFact) -> BreakdownMeta {
    let profile = settings
        .client_tools
        .profiles
        .iter()
        .find(|profile| profile.tool == fact.tool);
    BreakdownMeta {
        id: fact.tool.clone(),
        label: profile
            .and_then(|profile| profile.display_name.clone())
            .unwrap_or_else(|| {
                if fact.tool.trim().is_empty() {
                    "__unknown__".to_string()
                } else {
                    fact.tool.clone()
                }
            }),
        kind: "tool".to_string(),
        color: None,
        icon: profile.and_then(|profile| profile.icon.clone()),
    }
}

fn model_meta_for_fact(fact: &MergedRequestFact) -> BreakdownMeta {
    let label = if fact.model.trim().is_empty() {
        "__unknown__".to_string()
    } else {
        fact.model.clone()
    };
    BreakdownMeta {
        id: label.clone(),
        label,
        kind: "model".to_string(),
        color: None,
        icon: None,
    }
}

fn add_breakdown_fact(
    map: &mut HashMap<String, (BreakdownMeta, BreakdownAccumulator)>,
    meta: BreakdownMeta,
    fact: &MergedRequestFact,
) {
    let entry = map
        .entry(meta.id.clone())
        .or_insert_with(|| (meta, BreakdownAccumulator::default()));
    entry.1.add(fact);
}

#[tauri::command]
pub async fn get_overview_breakdown(
    window: String,
    settings: AppSettings,
) -> Result<OverviewBreakdown, String> {
    let now = chrono::Utc::now().timestamp();
    let include_errors = settings.proxy.include_error_requests;
    let cutoff_ms = crate::proxy::UsageCollector::calculate_window_cutoff_public(&window);
    let (facts, coverage) = crate::unified_usage::get_merged_request_facts(
        &settings,
        Some(cutoff_ms / 1000),
        Some(now + 1),
        include_errors,
    )
    .await?;
    Ok(build_overview_breakdown_from_facts(
        &settings, window, now, &facts, &coverage,
    ))
}

#[derive(Default, Clone)]
struct ModelTokenTotals {
    input_tokens: u64,
    output_tokens: u64,
    cache_create_tokens: u64,
    cache_read_tokens: u64,
    request_count: u64,
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

fn build_model_token_totals_from_facts(
    facts: &[MergedRequestFact],
) -> HashMap<String, ModelTokenTotals> {
    let mut model_stats: HashMap<String, ModelTokenTotals> = HashMap::new();

    for fact in facts {
        if fact.model.is_empty() {
            continue;
        }

        let entry = model_stats.entry(fact.model.clone()).or_default();
        entry.input_tokens += fact.input_tokens;
        entry.output_tokens += fact.output_tokens;
        entry.cache_create_tokens += fact.cache_create_tokens;
        entry.cache_read_tokens += fact.cache_read_tokens;
        entry.request_count += 1;
    }

    model_stats
}

fn build_usage_summary_from_usage(
    usage: &WindowUsage,
    total_success_requests: u64,
    total_client_error_requests: u64,
    total_server_error_requests: u64,
) -> crate::models::UsageSummary {
    crate::models::UsageSummary {
        total_tokens: usage.token_used,
        total_requests: usage.request_used,
        total_input_tokens: usage.input_tokens,
        total_output_tokens: usage.output_tokens,
        total_cache_create_tokens: usage.cache_create_tokens,
        total_cache_read_tokens: usage.cache_read_tokens,
        total_cost: usage.cost,
        total_success_requests,
        total_client_error_requests,
        total_server_error_requests,
    }
}

#[derive(Default, Clone)]
struct StatAccumulator {
    request_count: u64,
    local_request_count: u64,
    proxy_request_count: u64,
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
        local_request_count: acc.local_request_count,
        proxy_request_count: acc.proxy_request_count,
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

#[tauri::command]
pub async fn get_statistics_summary(
    query: StatisticsQuery,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<StatisticsSummary, String> {
    let (start_epoch, end_epoch) = normalize_range(&query);
    let include_errors = settings.proxy.include_error_requests;
    let (facts, coverage) = crate::unified_usage::get_merged_request_facts(
        &settings,
        Some(start_epoch),
        Some(end_epoch),
        include_errors,
    )
    .await?;
    Ok(build_merged_statistics(facts, &coverage, &query))
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

    let include_errors = settings.proxy.include_error_requests;
    let (facts, _coverage) = crate::unified_usage::get_merged_request_facts(
        &settings,
        Some(month_start),
        Some(month_end),
        include_errors,
    )
    .await?;
    collect_day_activity_from_facts(facts, &mut day_map, &mut partial_status_days);

    let mut days = Vec::new();
    for day in 1..=day_count {
        let Some(date) = NaiveDate::from_ymd_opt(year, month as u32, day) else {
            continue;
        };
        let key = date.format("%Y-%m-%d").to_string();
        let (acc, models) = day_map.remove(&key).unwrap_or_default();
        let error_requests = acc.client_error_requests + acc.server_error_requests;
        let status_available = !partial_status_days.contains(&key);
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

    let include_errors = settings.proxy.include_error_requests;
    let (facts, _coverage) = crate::unified_usage::get_merged_request_facts(
        &settings,
        Some(year_start),
        Some(year_end),
        include_errors,
    )
    .await?;
    collect_day_activity_from_facts(facts, &mut day_map, &mut partial_status_days);

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
        let status_available = !partial_status_days.contains(&key);
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
    crate::unified_usage::get_merged_sessions(&settings, limit, offset).await
}

/// 获取单个会话详情
#[tauri::command]
pub async fn get_session_detail(
    session_id: String,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<Option<SessionStats>, String> {
    crate::unified_usage::get_merged_session_detail(&settings, &session_id).await
}

/// 获取项目统计（基于所有会话数据聚合）
#[tauri::command]
pub async fn get_project_stats(
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<Vec<crate::proxy::ProjectStats>, String> {
    crate::unified_usage::get_merged_project_stats(&settings).await
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalUsageMaintenanceStats {
    pub total_local_facts: u64,
    pub orphan_local_facts: u64,
}

/// 获取本地缓存维护状态（用于设置页展示）。
#[tauri::command]
pub async fn get_local_usage_maintenance_stats() -> Result<LocalUsageMaintenanceStats, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let db = crate::local_usage::ensure_local_usage_synced()?;
        let total = db.count_local_request_facts()?;
        let orphan = db.count_orphan_local_facts()?;
        Ok::<_, String>(LocalUsageMaintenanceStats {
            total_local_facts: total,
            orphan_local_facts: orphan,
        })
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

/// 清理孤立的本地事实（来源文件已消失的请求记录）。
///
/// `older_than_days`：仅清理 `created_at` 早于该天数的孤立行；传 0 表示全部清理。
#[tauri::command]
pub async fn purge_orphan_local_facts(older_than_days: u32) -> Result<u64, String> {
    let seconds = (older_than_days as i64).saturating_mul(86400);
    tauri::async_runtime::spawn_blocking(move || {
        let db = crate::local_usage::ensure_local_usage_synced()?;
        db.purge_orphan_facts(seconds)
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

/// 重建本地缓存：清空所有 local_* 表，然后强制从 JSONL 全量重新解析。
#[tauri::command]
pub async fn rebuild_local_usage_cache() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        let db = crate::local_usage::LocalUsageDatabase::get_global()?;
        db.truncate_all_local_facts()?;
        db.sync_from_scanner()?;
        Ok::<_, String>(())
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn local_record_age_seconds(created_at: i64, now_ts: i64) -> Option<i64> {
        if created_at > now_ts {
            None
        } else {
            Some(now_ts - created_at)
        }
    }

    #[test]
    fn local_record_age_seconds_rejects_future_timestamps() {
        assert_eq!(local_record_age_seconds(100, 100), Some(0));
        assert_eq!(local_record_age_seconds(80, 100), Some(20));
        assert_eq!(local_record_age_seconds(101, 100), None);
    }

    #[test]
    fn build_usage_summary_from_usage_keeps_cost_in_same_window() {
        let usage = WindowUsage {
            window: "5h".to_string(),
            token_used: 120,
            input_tokens: 70,
            output_tokens: 40,
            cache_create_tokens: 5,
            cache_read_tokens: 5,
            request_used: 3,
            cost: 1.25,
            success_requests: 0,
            client_error_requests: 0,
            server_error_requests: 0,
        };

        let summary = build_usage_summary_from_usage(&usage, 0, 0, 0);

        assert_eq!(summary.total_tokens, 120);
        assert_eq!(summary.total_requests, 3);
        assert_eq!(summary.total_input_tokens, 70);
        assert_eq!(summary.total_output_tokens, 40);
        assert_eq!(summary.total_cache_create_tokens, 5);
        assert_eq!(summary.total_cache_read_tokens, 5);
        assert_eq!(summary.total_cost, 1.25);
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

    fn test_fact(
        session_id: &str,
        timestamp_sec: i64,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cache_create_tokens: u64,
        cache_read_tokens: u64,
        coverage_origin: CoverageOrigin,
    ) -> MergedRequestFact {
        MergedRequestFact {
            session_id: session_id.to_string(),
            project_name: None,
            project_path: None,
            api_key_prefix: None,
            request_base_url: None,
            tool: "claude_code".to_string(),
            timestamp_sec,
            timestamp_ms: timestamp_sec.saturating_mul(1000),
            model: model.to_string(),
            input_tokens,
            output_tokens,
            cache_create_tokens,
            cache_read_tokens,
            total_tokens: input_tokens + output_tokens + cache_create_tokens + cache_read_tokens,
            estimated_cost: 0.0,
            coverage_origin,
            status_code: Some(200),
            duration_ms: None,
            output_tokens_per_second: None,
            ttft_ms: None,
            source_label: None,
        }
    }

    #[test]
    fn build_model_distribution_from_facts_supports_custom_summary_window() {
        let facts = vec![
            test_fact(
                "session-1",
                1,
                "model-a",
                100,
                50,
                10,
                0,
                CoverageOrigin::LocalOnly,
            ),
            test_fact(
                "session-2",
                2,
                "model-b",
                20,
                20,
                0,
                0,
                CoverageOrigin::ProxyOnly,
            ),
        ];

        let distribution = build_model_distribution_from_window_stats(Some(
            &build_model_token_totals_from_facts(&facts),
        ));

        assert_eq!(distribution.len(), 2);
        assert_eq!(distribution[0].model_name, "model-a");
        assert_eq!(distribution[0].token_used, 160);
        assert_eq!(distribution[0].request_count, 1);
        assert!((distribution[0].percent - 80.0).abs() < f64::EPSILON);
        assert_eq!(distribution[1].model_name, "model-b");
        assert_eq!(distribution[1].token_used, 40);
        assert_eq!(distribution[1].request_count, 1);
        assert!((distribution[1].percent - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn build_usage_summary_from_usage_supports_custom_summary_window() {
        let facts = vec![
            test_fact(
                "session-1",
                1,
                "model-a",
                100,
                50,
                10,
                0,
                CoverageOrigin::LocalOnly,
            ),
            test_fact(
                "session-2",
                2,
                "model-b",
                20,
                20,
                0,
                0,
                CoverageOrigin::ProxyOnly,
            ),
        ];
        let coverage = crate::unified_usage::build_coverage(&facts);
        let (summary_usage, _) = build_window_usage_from_facts("custom", &facts, &coverage);

        let summary = build_usage_summary_from_usage(&summary_usage, 1, 0, 0);

        assert_eq!(summary.total_tokens, 200);
        assert_eq!(summary.total_requests, 2);
        assert_eq!(summary.total_input_tokens, 120);
        assert_eq!(summary.total_output_tokens, 70);
        assert_eq!(summary.total_cache_create_tokens, 10);
        assert_eq!(summary.total_cache_read_tokens, 0);
        assert_eq!(summary.total_success_requests, 1);
    }
}
