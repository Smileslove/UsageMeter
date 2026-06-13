mod service;
mod types;

pub(crate) use service::{
    build_coverage, clear_runtime_caches, ensure_materialized_history, get_merged_project_stats,
    get_merged_request_facts, get_merged_session_detail, get_merged_sessions,
};
#[cfg(test)]
pub(crate) use service::{runtime_merge_cache_len_for_test, seed_runtime_merge_cache_for_test};
pub(crate) use types::{
    canonical_request_key_for_local, has_partial_coverage, matches_source_filter,
    normalize_model_bucket, CoverageOrigin, MergedRequestFact,
};
