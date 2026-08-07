mod aggregation_support;
mod cold_facts_support;
mod derived_support;
mod inflight_support;
mod match_support;
mod materialization_support;
mod query_support;
mod reasonix_support;
mod service;
mod types;

#[cfg(all(test, feature = "performance-tests"))]
mod performance_tests;

pub(crate) use service::{
    build_coverage, clear_runtime_caches, ensure_materialized_history_no_sync,
    get_merged_project_stats_no_sync, get_merged_request_facts_no_sync, get_merged_session_detail,
    get_merged_sessions_no_sync,
};
#[cfg(test)]
pub(crate) use service::{
    load_cold_facts_via_shards, runtime_merge_cache_len_for_test,
    seed_runtime_merge_cache_for_test, ColdFactsShardCache,
};
pub(crate) use types::{
    canonical_request_key_for_local, has_partial_coverage, matches_source_filter,
    normalize_model_bucket, CoverageOrigin, MergedRequestFact,
};
