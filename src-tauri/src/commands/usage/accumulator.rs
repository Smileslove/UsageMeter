use crate::unified_usage::{CoverageOrigin, MergedRequestFact};
use std::collections::HashMap;

#[derive(Default, Clone)]
pub(super) struct FactAccumulator {
    pub(super) request_count: u64,
    pub(super) local_request_count: u64,
    pub(super) proxy_request_count: u64,
    pub(super) total_tokens: u64,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_create_tokens: u64,
    pub(super) cache_read_tokens: u64,
    pub(super) cost: f64,
    pub(super) success_requests: u64,
    pub(super) client_error_requests: u64,
    pub(super) server_error_requests: u64,
    pub(super) has_status: bool,
    pub(super) rate_sum: f64,
    pub(super) rate_count: u64,
    pub(super) rate_output_tokens: u64,
    pub(super) rate_duration_ms: u64,
    pub(super) min_rate: Option<f64>,
    pub(super) max_rate: Option<f64>,
    pub(super) ttft_sum: f64,
    pub(super) ttft_count: u64,
    pub(super) min_ttft_ms: Option<u64>,
    pub(super) max_ttft_ms: Option<u64>,
    pub(super) last_seen_ms: i64,
    pub(super) status_code_counts: HashMap<u16, u64>,
}

impl FactAccumulator {
    pub(super) fn add_tokens(
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
        self.total_tokens += input + output + cache_create + cache_read;
        self.cost += cost;
    }

    pub(super) fn add_fact(&mut self, fact: &MergedRequestFact) {
        self.add_tokens(
            fact.input_tokens,
            fact.output_tokens,
            fact.cache_create_tokens,
            fact.cache_read_tokens,
            1,
            fact.estimated_cost,
        );

        if matches!(fact.coverage_origin, CoverageOrigin::LocalOnly) {
            self.local_request_count += 1;
        } else {
            self.proxy_request_count += 1;
        }

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
            *self.status_code_counts.entry(status_code).or_insert(0) += 1;
        }

        if let (Some(duration_ms), Some(rate)) = (fact.duration_ms, fact.output_tokens_per_second) {
            if duration_ms > 0 && rate > 0.0 {
                self.rate_sum += rate;
                self.rate_count += 1;
                self.rate_output_tokens += fact.output_tokens;
                self.rate_duration_ms += duration_ms;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_fact(
        coverage_origin: CoverageOrigin,
        status_code: Option<u16>,
        duration_ms: Option<u64>,
        rate: Option<f64>,
        ttft_ms: Option<u64>,
    ) -> MergedRequestFact {
        MergedRequestFact {
            canonical_request_key: "k".to_string(),
            session_id: "s".to_string(),
            project_name: None,
            project_path: None,
            api_key_prefix: None,
            request_base_url: None,
            tool: "claude_code".to_string(),
            timestamp_sec: 1,
            timestamp_ms: 1234,
            model: "model-a".to_string(),
            input_tokens: 10,
            output_tokens: 20,
            cache_create_tokens: 3,
            cache_read_tokens: 2,
            total_tokens: 35,
            estimated_cost: 1.25,
            coverage_origin,
            status_code,
            duration_ms,
            output_tokens_per_second: rate,
            ttft_ms,
            source_label: None,
        }
    }

    #[test]
    fn add_fact_accumulates_basic_status_and_performance_fields() {
        let mut acc = FactAccumulator::default();
        acc.add_fact(&test_fact(
            CoverageOrigin::LocalOnly,
            Some(200),
            Some(500),
            Some(40.0),
            Some(300),
        ));
        acc.add_fact(&test_fact(
            CoverageOrigin::ProxyOnly,
            Some(500),
            Some(1000),
            Some(20.0),
            Some(700),
        ));

        assert_eq!(acc.request_count, 2);
        assert_eq!(acc.local_request_count, 1);
        assert_eq!(acc.proxy_request_count, 1);
        assert_eq!(acc.total_tokens, 70);
        assert_eq!(acc.cost, 2.5);
        assert_eq!(acc.success_requests, 1);
        assert_eq!(acc.server_error_requests, 1);
        assert!(acc.has_status);
        assert_eq!(acc.rate_count, 2);
        assert_eq!(acc.rate_output_tokens, 40);
        assert_eq!(acc.rate_duration_ms, 1500);
        assert_eq!(acc.min_rate, Some(20.0));
        assert_eq!(acc.max_rate, Some(40.0));
        assert_eq!(acc.ttft_count, 2);
        assert_eq!(acc.min_ttft_ms, Some(300));
        assert_eq!(acc.max_ttft_ms, Some(700));
        assert_eq!(acc.status_code_counts.get(&200), Some(&1));
        assert_eq!(acc.status_code_counts.get(&500), Some(&1));
    }
}
