use crate::unified_usage::MergedRequestFact;

pub(crate) fn usage_window_cutoff_epoch(window: &str) -> i64 {
    crate::proxy::UsageCollector::calculate_window_cutoff_public(window) / 1000
}

pub(crate) fn perf_logging_enabled() -> bool {
    cfg!(debug_assertions) || matches!(std::env::var("USAGEMETER_DEBUG_PERF"), Ok(v) if v == "1")
}

pub(crate) fn perf_log(event: &str, message: impl AsRef<str>) {
    if perf_logging_enabled() {
        eprintln!("[UsageMeter][perf][{event}] {}", message.as_ref());
    }
}

pub(crate) fn epoch_u64_to_i64_saturating(epoch: u64) -> i64 {
    i64::try_from(epoch).unwrap_or(i64::MAX)
}

pub(crate) fn first_fact_index_in_range(facts: &[MergedRequestFact], cutoff_epoch: i64) -> usize {
    facts.partition_point(|fact| fact.timestamp_sec < cutoff_epoch)
}
