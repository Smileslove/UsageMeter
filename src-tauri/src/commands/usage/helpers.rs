use crate::{models::AppSettings, unified_usage::MergedRequestFact};

pub(crate) fn usage_window_cutoff_epoch(window: &str, settings: &AppSettings) -> i64 {
    crate::utils::business_time::business_window_cutoff_epoch(window, settings)
}

pub(crate) fn epoch_u64_to_i64_saturating(epoch: u64) -> i64 {
    i64::try_from(epoch).unwrap_or(i64::MAX)
}

pub(crate) fn first_fact_index_in_range(facts: &[MergedRequestFact], cutoff_epoch: i64) -> usize {
    facts.partition_point(|fact| fact.timestamp_sec < cutoff_epoch)
}
