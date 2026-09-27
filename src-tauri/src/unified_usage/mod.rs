mod aggregation_support;
mod attribution;
mod cold_facts_support;
mod derived_support;
mod inflight_support;
mod match_support;
mod materialization_support;
mod merge_engine;
mod query_support;
mod service;
mod types;

#[cfg(all(test, feature = "performance-tests"))]
mod performance_tests;

pub(crate) use merge_engine::build_coverage;
pub(crate) use service::{
    clear_runtime_caches, combined_data_time_bounds, count_stale_materialization_days,
    ensure_materialized_history_no_sync, get_manual_attribution_request_keys_for_session,
    get_manual_attribution_request_keys_for_time_range, get_merged_project_stats_no_sync,
    get_merged_request_facts_no_sync, get_merged_session_detail, get_merged_sessions_no_sync,
};
#[cfg(test)]
pub(crate) use service::{
    load_cold_facts_via_shards, runtime_merge_cache_len_for_test,
    seed_runtime_merge_cache_for_test, ColdFactsShardCache,
};
pub(crate) use types::{
    canonical_request_key_for_local, has_partial_coverage, matches_source_filter,
    normalize_model_bucket, AttributionMethod, CoverageOrigin, MergedRequestFact,
};
