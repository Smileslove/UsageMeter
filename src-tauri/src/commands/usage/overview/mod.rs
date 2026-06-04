use super::helpers::perf_log;
use super::types::{OverviewBreakdown, ProxyState, UsageRefreshBundle};
use crate::models::{AppSettings, WindowRateSummary};
use std::time::{SystemTime, UNIX_EPOCH};

mod breakdown;
mod rate;
mod refresh;

#[tauri::command]
pub async fn refresh_usage_bundle(
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<UsageRefreshBundle, String> {
    let started_at = std::time::Instant::now();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let prepared = refresh::prepare_usage_refresh_data(&settings, now).await?;
    let build_started_at = std::time::Instant::now();
    let bundle = refresh::build_usage_refresh_bundle_from_prepared(&settings, &prepared);
    perf_log(
        "refresh_usage_bundle",
        format!(
            "facts={} build_ms={} total_ms={}",
            prepared.facts.len(),
            build_started_at.elapsed().as_millis(),
            started_at.elapsed().as_millis(),
        ),
    );
    Ok(bundle)
}

#[tauri::command]
pub async fn get_overview_breakdown(
    window: String,
    settings: AppSettings,
) -> Result<OverviewBreakdown, String> {
    let now = chrono::Utc::now().timestamp();
    let include_errors = settings.proxy.include_error_requests;
    let cutoff_ms = crate::proxy::UsageCollector::calculate_window_cutoff_public(&window);
    let (facts, _) = crate::unified_usage::get_merged_request_facts(
        &settings,
        Some(cutoff_ms / 1000),
        Some(now + 1),
        include_errors,
    )
    .await?;
    Ok(breakdown::build_overview_breakdown_from_facts(
        &settings, window, now, &facts,
    ))
}

#[tauri::command]
pub async fn get_window_rate_summary(
    window: String,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<WindowRateSummary, String> {
    let settings = crate::commands::load_settings()?;
    let cutoff_ms = crate::proxy::UsageCollector::calculate_window_cutoff_public(&window);
    let include_errors = settings.proxy.include_error_requests;
    let (facts, _) = crate::unified_usage::get_merged_request_facts(
        &settings,
        Some(cutoff_ms / 1000),
        None,
        include_errors,
    )
    .await?;

    Ok(rate::build_window_rate_summary_from_facts(window, facts))
}

#[cfg(test)]
mod tests {
    use super::refresh::{
        build_model_distribution_from_window_stats, build_model_token_totals_from_facts,
        build_usage_summary_from_usage, build_window_usage_from_facts, ModelTokenTotals,
    };
    use crate::models::WindowUsage;
    use crate::unified_usage::{CoverageOrigin, MergedRequestFact};
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
        status_code: Option<u16>,
    ) -> MergedRequestFact {
        MergedRequestFact {
            canonical_request_key: format!("claude_code:{}:{timestamp_sec}:{model}", session_id),
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
            status_code,
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
                None,
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
                Some(200),
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
                None,
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
                Some(200),
            ),
        ];
        let (summary_usage, _) = build_window_usage_from_facts("custom", &facts);

        let summary = build_usage_summary_from_usage(&summary_usage, 1, 0, 0);

        assert_eq!(summary.total_tokens, 200);
        assert_eq!(summary.total_requests, 2);
        assert_eq!(summary.total_input_tokens, 120);
        assert_eq!(summary.total_output_tokens, 70);
        assert_eq!(summary.total_cache_create_tokens, 10);
        assert_eq!(summary.total_cache_read_tokens, 0);
        assert_eq!(summary.total_success_requests, 1);
    }

    #[test]
    fn build_window_usage_local_synthetic_200_counts_as_success() {
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
                Some(200),
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
                Some(200),
            ),
        ];
        let (summary_usage, _) = build_window_usage_from_facts("custom", &facts);

        assert_eq!(summary_usage.request_used, 2);
        assert_eq!(summary_usage.success_requests, 2);
        assert_eq!(summary_usage.client_error_requests, 0);
        assert_eq!(summary_usage.server_error_requests, 0);
    }
}
