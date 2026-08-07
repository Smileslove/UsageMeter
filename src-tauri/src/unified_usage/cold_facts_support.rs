use super::types::MergedRequestFact;
use std::sync::Arc;

pub(super) const COLD_FACTS_SHARD_CAPACITY: usize = 400;
pub(super) const COLD_CONCAT_MEMO_CAPACITY: usize = 3;

#[derive(Debug, Clone)]
pub(super) struct ColdDayCacheEntry {
    pub(super) materialized_at: i64,
    pub(super) facts: Arc<Vec<MergedRequestFact>>,
}

#[derive(Debug, Clone)]
pub(super) struct ColdConcatMemo {
    pub(super) fingerprint: u64,
    pub(super) facts: Arc<Vec<MergedRequestFact>>,
}

#[derive(Debug)]
pub(crate) struct ColdFactsShardCache {
    pub(super) day_boundary_mode: String,
    pub(super) shards: std::collections::HashMap<String, ColdDayCacheEntry>,
    pub(super) concat_memos: Vec<ColdConcatMemo>,
}

impl ColdFactsShardCache {
    pub(crate) fn new() -> Self {
        Self {
            day_boundary_mode: String::new(),
            shards: std::collections::HashMap::new(),
            concat_memos: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn shard_facts_for_test(
        &self,
        local_date: &str,
    ) -> Option<Arc<Vec<MergedRequestFact>>> {
        self.shards.get(local_date).map(|entry| entry.facts.clone())
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct ColdFactsLoad {
    pub(crate) facts: Arc<Vec<MergedRequestFact>>,
    pub(crate) days_cached: usize,
    pub(crate) days_fetched: usize,
    pub(crate) memo_hit: bool,
    pub(crate) fallback_uncached: bool,
    pub(crate) stale_days_skipped: usize,
}
