//! 复杂用量查询响应时间测试。
//!
//! 运行：
//! `cargo test --package usagemeter --lib --features performance-tests query_response_time -- --nocapture --test-threads=1`
//!
//! 测试读取当前 UsageMeter 数据库，并可能执行 schema migration 或写入派生物化数据；
//! 因此默认测试套件不编译本模块，必须显式启用 `performance-tests` feature。
//! 测试以结构化 `QUERY_TIME` 行输出冷、热查询耗时。
//! 阈值仅用于观测告警，不作为跨机器、跨数据规模的硬性断言。

use super::overview::{
    get_overview_breakdown_no_sync, get_overview_deferred_bundle_no_sync,
    refresh_usage_bundle_no_sync, report_overview_aggregation_stages_for_test,
};
use super::statistics::activity::{
    clear_activity_cache, get_month_activity_impl, get_year_activity_impl,
};
use super::statistics::get_statistics_summary_no_sync;
use super::types::{StatisticsBucket, StatisticsMetric, StatisticsQuery};
use crate::models::AppSettings;
use chrono::{Datelike, Utc};
use std::future::Future;
use std::time::{Duration, Instant};

const COLD_TARGET: Duration = Duration::from_secs(2);
const WARM_TARGET: Duration = Duration::from_millis(200);

fn load_settings() -> AppSettings {
    crate::commands::load_settings_blocking().unwrap_or_default()
}

fn report(label: &str, phase: &str, elapsed: Duration, result_size: usize) {
    println!(
        "QUERY_TIME name={label} phase={phase} elapsed_ms={:.3} result_size={result_size}",
        elapsed.as_secs_f64() * 1000.0
    );
}

fn warn_if_slow(label: &str, elapsed: Duration, target: Duration, phase: &str) {
    if elapsed > target {
        eprintln!(
            "WARNING {label} {phase}: {:.3} ms exceeds observational target {:.3} ms",
            elapsed.as_secs_f64() * 1000.0,
            target.as_secs_f64() * 1000.0
        );
    }
}

async fn measure<T, F>(
    label: &str,
    phase: &str,
    target: Duration,
    future: F,
    result_size: impl FnOnce(&T) -> usize,
) -> T
where
    F: Future<Output = Result<T, String>>,
{
    let started = Instant::now();
    let value = future
        .await
        .unwrap_or_else(|error| panic!("{label} {phase} failed: {error}"));
    let elapsed = started.elapsed();
    report(label, phase, elapsed, result_size(&value));
    warn_if_slow(label, elapsed, target, phase);
    value
}

#[tokio::test]
async fn session_detail_matches_full_session_derivation() {
    let settings = load_settings();
    crate::unified_usage::clear_runtime_caches();
    let sessions = crate::unified_usage::get_merged_sessions_no_sync(&settings, i64::MAX / 4, 0)
        .await
        .expect("derive full session list for detail equivalence");

    for expected in sessions.iter().take(8) {
        // 强制清空完整会话列表与详情缓存，确保断言覆盖定向历史 SQL + 今日热段
        // 实时合并路径，而不是从刚构建的 SessionDerivedCache 直接返回。
        crate::unified_usage::clear_runtime_caches();
        let actual =
            crate::unified_usage::get_merged_session_detail(&settings, &expected.session_id)
                .await
                .expect("derive targeted session detail")
                .expect("targeted session detail exists");
        assert_eq!(
            serde_json::to_value(&actual).expect("serialize targeted session detail"),
            serde_json::to_value(expected).expect("serialize full-list session detail"),
            "targeted detail diverged for session {}",
            expected.session_id
        );
    }

    assert!(
        crate::unified_usage::get_merged_session_detail(&settings, "")
            .await
            .expect("empty session id is handled")
            .is_none()
    );
    assert!(crate::unified_usage::get_merged_session_detail(
        &settings,
        "__usagemeter_missing_session__",
    )
    .await
    .expect("missing session id is handled")
    .is_none());
}

#[tokio::test]
async fn query_response_time_session_detail() {
    let settings = load_settings();
    let sessions = crate::unified_usage::get_merged_sessions_no_sync(&settings, 1, 0)
        .await
        .expect("load one session for detail performance test");
    let Some(session_id) = sessions.first().map(|session| session.session_id.clone()) else {
        println!("QUERY_TIME name=get_session_detail phase=skipped elapsed_ms=0.000 result_size=0");
        return;
    };

    crate::unified_usage::clear_runtime_caches();
    let cold = measure(
        "get_session_detail",
        "cold",
        COLD_TARGET,
        crate::unified_usage::get_merged_session_detail(&settings, &session_id),
        |detail| usize::from(detail.is_some()),
    )
    .await;

    let warm = measure(
        "get_session_detail",
        "warm",
        WARM_TARGET,
        crate::unified_usage::get_merged_session_detail(&settings, &session_id),
        |detail| usize::from(detail.is_some()),
    )
    .await;

    assert_eq!(cold.is_some(), warm.is_some());
}

#[tokio::test]
async fn query_response_time_refresh_usage_bundle_30d() {
    let settings = load_settings();
    let now = Utc::now().timestamp().max(0) as u64;

    crate::unified_usage::clear_runtime_caches();
    let cold = measure(
        "refresh_usage_bundle_30d",
        "cold",
        COLD_TARGET,
        refresh_usage_bundle_no_sync(&settings, now),
        |bundle| bundle.snapshot.windows.len(),
    )
    .await;

    let warm = measure(
        "refresh_usage_bundle_30d",
        "warm",
        WARM_TARGET,
        refresh_usage_bundle_no_sync(&settings, now),
        |bundle| bundle.snapshot.windows.len(),
    )
    .await;

    assert_eq!(cold.snapshot.windows.len(), warm.snapshot.windows.len());
}

#[tokio::test]
async fn query_response_time_overview_aggregates_30d() {
    let settings = load_settings();
    let now = Utc::now().timestamp();

    crate::unified_usage::clear_runtime_caches();
    let cold = measure(
        "get_overview_deferred_bundle_30d",
        "cold",
        COLD_TARGET,
        get_overview_deferred_bundle_no_sync("30d".to_string(), &settings, now),
        |bundle| bundle.overview_breakdown.model_ranking.len(),
    )
    .await;

    let warm = measure(
        "get_overview_deferred_bundle_30d",
        "warm",
        WARM_TARGET,
        get_overview_deferred_bundle_no_sync("30d".to_string(), &settings, now),
        |bundle| bundle.overview_breakdown.model_ranking.len(),
    )
    .await;

    let breakdown = measure(
        "get_overview_breakdown_30d",
        "warm",
        WARM_TARGET,
        get_overview_breakdown_no_sync("30d".to_string(), &settings, now),
        |value| value.source_ranking.len() + value.tool_ranking.len() + value.model_ranking.len(),
    )
    .await;

    assert_eq!(
        cold.overview_breakdown.model_ranking.len(),
        warm.overview_breakdown.model_ranking.len()
    );
    assert_eq!(
        warm.overview_breakdown.model_ranking.len(),
        breakdown.model_ranking.len()
    );
}

#[tokio::test]
async fn query_stage_diagnostics_overview_30d() {
    let settings = load_settings();
    let now = Utc::now().timestamp();
    let cutoff_epoch =
        crate::utils::business_time::business_window_cutoff_epoch_at("30d", &settings, now);
    let include_errors = settings.proxy.include_error_requests;

    crate::unified_usage::clear_runtime_caches();
    let local_db = crate::local_usage::get_local_usage_db()
        .expect("open local usage database for signature diagnostics");
    let signature_started = Instant::now();
    let _local_signature = local_db
        .get_merge_cache_signature()
        .expect("load local merge signature");
    println!(
        "QUERY_STAGE name=overview_30d stage=local_cache_signature elapsed_ms={:.3} input_size=1",
        signature_started.elapsed().as_secs_f64() * 1000.0
    );

    // 首次 get_global 可能包含打开连接、建表检查与 schema migration；单独计时，
    // 避免把一次性初始化成本误判为 generation 主键点查成本。
    let proxy_acquire_started = Instant::now();
    let proxy_db = crate::proxy::ProxyDatabase::get_global();
    println!(
        "QUERY_STAGE name=overview_30d stage=proxy_db_acquire elapsed_ms={:.3} input_size={}",
        proxy_acquire_started.elapsed().as_secs_f64() * 1000.0,
        usize::from(proxy_db.is_some())
    );

    let signature_started = Instant::now();
    let proxy_signature = proxy_db
        .as_ref()
        .map(|db| db.get_merge_cache_signature())
        .transpose()
        .expect("load proxy merge signature");
    println!(
        "QUERY_STAGE name=overview_30d stage=proxy_cache_signature elapsed_ms={:.3} input_size={}",
        signature_started.elapsed().as_secs_f64() * 1000.0,
        usize::from(proxy_signature.is_some())
    );

    let proxy_reacquire_started = Instant::now();
    let reused_proxy_db = crate::proxy::ProxyDatabase::get_global();
    println!(
        "QUERY_STAGE name=overview_30d stage=proxy_db_reacquire elapsed_ms={:.3} input_size={}",
        proxy_reacquire_started.elapsed().as_secs_f64() * 1000.0,
        usize::from(reused_proxy_db.is_some())
    );

    let proxy_fresh_started = Instant::now();
    let fresh_proxy_db = crate::proxy::ProxyDatabase::new()
        .expect("open fresh proxy database connection for control timing");
    println!(
        "QUERY_STAGE name=overview_30d stage=proxy_db_fresh_connection_control elapsed_ms={:.3} input_size=1",
        proxy_fresh_started.elapsed().as_secs_f64() * 1000.0
    );
    drop(fresh_proxy_db);

    let history_started = Instant::now();
    crate::unified_usage::ensure_materialized_history_no_sync(&settings, cutoff_epoch, now + 1)
        .await
        .expect("ensure 30d history materialization");
    println!(
        "QUERY_STAGE name=overview_30d stage=history_validation elapsed_ms={:.3} input_size=30",
        history_started.elapsed().as_secs_f64() * 1000.0
    );

    let facts_started = Instant::now();
    let (facts, _) = crate::unified_usage::get_merged_request_facts_no_sync(
        &settings,
        Some(cutoff_epoch),
        Some(now + 1),
        include_errors,
    )
    .await
    .expect("load 30d facts for overview stage diagnostics");
    println!(
        "QUERY_STAGE name=overview_30d stage=facts_after_history_ready elapsed_ms={:.3} input_size={}",
        facts_started.elapsed().as_secs_f64() * 1000.0,
        facts.len()
    );

    report_overview_aggregation_stages_for_test("30d", &settings, now, &facts);

    let cached_facts_started = Instant::now();
    let (cached_facts, _) = crate::unified_usage::get_merged_request_facts_no_sync(
        &settings,
        Some(cutoff_epoch),
        Some(now + 1),
        include_errors,
    )
    .await
    .expect("reload cached 30d facts for overview stage diagnostics");
    println!(
        "QUERY_STAGE name=overview_30d stage=facts_cache_hit elapsed_ms={:.3} input_size={}",
        cached_facts_started.elapsed().as_secs_f64() * 1000.0,
        cached_facts.len()
    );
}

#[tokio::test]
async fn query_stage_diagnostics_history_validation_year() {
    let settings = load_settings();
    let now = Utc::now();
    let year_start = crate::local_usage::LocalUsageDatabase::local_date_epoch_bounds_with_settings(
        &format!("{}-01-01", now.year()),
        &settings,
    )
    .expect("resolve year start for history diagnostics")
    .0;
    let year_end = crate::local_usage::LocalUsageDatabase::local_date_epoch_bounds_with_settings(
        &format!("{}-01-01", now.year() + 1),
        &settings,
    )
    .expect("resolve year end for history diagnostics")
    .0;

    crate::unified_usage::clear_runtime_caches();
    let started = Instant::now();
    crate::unified_usage::ensure_materialized_history_no_sync(&settings, year_start, year_end)
        .await
        .expect("ensure yearly history materialization");
    println!(
        "QUERY_STAGE name=year_activity stage=history_validation elapsed_ms={:.3} input_size=365",
        started.elapsed().as_secs_f64() * 1000.0
    );

    let started = Instant::now();
    crate::unified_usage::ensure_materialized_history_no_sync(&settings, year_start, year_end)
        .await
        .expect("recheck yearly history materialization cache");
    println!(
        "QUERY_STAGE name=year_activity stage=history_validation_second elapsed_ms={:.3} input_size=365",
        started.elapsed().as_secs_f64() * 1000.0
    );

    let started = Instant::now();
    crate::unified_usage::ensure_materialized_history_no_sync(&settings, year_start, year_end)
        .await
        .expect("recheck stable yearly history materialization cache");
    println!(
        "QUERY_STAGE name=year_activity stage=history_validation_third elapsed_ms={:.3} input_size=365",
        started.elapsed().as_secs_f64() * 1000.0
    );
}

#[tokio::test]
async fn query_response_time_statistics_summary_30d() {
    let settings = load_settings();
    let end_epoch = Utc::now().timestamp() + 1;
    let query = StatisticsQuery {
        start_epoch: end_epoch - 30 * 24 * 60 * 60,
        end_epoch,
        timezone: settings.timezone.clone(),
        bucket: StatisticsBucket::Day,
    };

    crate::unified_usage::clear_runtime_caches();
    let cold = measure(
        "get_statistics_summary_30d",
        "cold",
        COLD_TARGET,
        get_statistics_summary_no_sync(&query, &settings),
        |summary| summary.trend.len() + summary.models.len(),
    )
    .await;

    let warm = measure(
        "get_statistics_summary_30d",
        "warm",
        WARM_TARGET,
        get_statistics_summary_no_sync(&query, &settings),
        |summary| summary.trend.len() + summary.models.len(),
    )
    .await;

    assert_eq!(cold.totals.request_count, warm.totals.request_count);
    assert_eq!(cold.totals.total_tokens, warm.totals.total_tokens);
}

#[tokio::test]
async fn query_response_time_month_and_year_activity() {
    let settings = load_settings();
    let now = Utc::now();
    let year = now.year();
    let month = now.month() as u8;

    clear_activity_cache();
    crate::unified_usage::clear_runtime_caches();
    let month_cold = measure(
        "get_month_activity_tokens",
        "cold",
        COLD_TARGET,
        get_month_activity_impl(year, month, StatisticsMetric::Tokens, settings.clone()),
        |activity| activity.days.len(),
    )
    .await;

    let month_warm = measure(
        "get_month_activity_tokens",
        "warm",
        WARM_TARGET,
        get_month_activity_impl(year, month, StatisticsMetric::Tokens, settings.clone()),
        |activity| activity.days.len(),
    )
    .await;

    clear_activity_cache();
    crate::unified_usage::clear_runtime_caches();
    let year_cold = measure(
        "get_year_activity_tokens",
        "cold",
        COLD_TARGET,
        get_year_activity_impl(year, StatisticsMetric::Tokens, settings.clone()),
        |activity| activity.days.len(),
    )
    .await;

    let year_warm = measure(
        "get_year_activity_tokens",
        "warm",
        WARM_TARGET,
        get_year_activity_impl(year, StatisticsMetric::Tokens, settings),
        |activity| activity.days.len(),
    )
    .await;

    assert_eq!(month_cold.days.len(), month_warm.days.len());
    assert_eq!(year_cold.days.len(), year_warm.days.len());
}

#[tokio::test]
async fn query_response_time_summary_report() {
    let settings = load_settings();
    let include_errors = settings.proxy.include_error_requests;

    crate::unified_usage::clear_runtime_caches();
    let facts = measure(
        "merged_request_facts_all",
        "cold",
        COLD_TARGET,
        crate::unified_usage::get_merged_request_facts_no_sync(
            &settings,
            None,
            None,
            include_errors,
        ),
        |(facts, _)| facts.len(),
    )
    .await;

    let sessions = measure(
        "get_sessions",
        "warm",
        WARM_TARGET,
        crate::unified_usage::get_merged_sessions_no_sync(&settings, 20, 0),
        Vec::len,
    )
    .await;

    let projects = measure(
        "get_project_stats",
        "warm",
        WARM_TARGET,
        crate::unified_usage::get_merged_project_stats_no_sync(&settings),
        Vec::len,
    )
    .await;

    assert!(facts.0.len() >= sessions.len());
    println!(
        "QUERY_DATASET facts={} sessions={} projects={}",
        facts.0.len(),
        sessions.len(),
        projects.len()
    );
}
