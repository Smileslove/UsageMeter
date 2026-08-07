use crate::models::{AppSettings, ModelPricingConfig, SourceFilter, ToolFilter};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub(super) fn normalized_day_boundary_mode(settings: &AppSettings) -> String {
    crate::utils::business_time::normalize_day_boundary_mode(&settings.day_boundary_mode)
}

pub(super) fn cache_key_for_source_filter(filter: &SourceFilter) -> String {
    match filter {
        SourceFilter::All => "all".to_string(),
        SourceFilter::Unknown { known_pairs } => format!("unknown:{known_pairs:?}"),
        SourceFilter::Source {
            api_key_prefixes,
            base_url,
        } => format!("source:{api_key_prefixes:?}:{base_url:?}"),
    }
}

pub(super) fn cache_key_for_tool_filter(filter: &ToolFilter) -> String {
    match filter {
        ToolFilter::All => "all".to_string(),
        ToolFilter::Tool(tool) => format!("tool:{tool}"),
        ToolFilter::AnyOf(tools) => {
            let mut sorted = tools.clone();
            sorted.sort();
            format!("anyof:{}", sorted.join(","))
        }
    }
}

pub(super) fn fingerprint_pricings(pricings: &[ModelPricingConfig]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for pricing in pricings {
        pricing.model_id.hash(&mut hasher);
        pricing.display_name.hash(&mut hasher);
        pricing.input_price.to_bits().hash(&mut hasher);
        pricing.output_price.to_bits().hash(&mut hasher);
        pricing.cache_read_price.map(f64::to_bits).hash(&mut hasher);
        pricing
            .cache_write_price
            .map(f64::to_bits)
            .hash(&mut hasher);
        pricing.source.hash(&mut hasher);
        pricing.last_updated.hash(&mut hasher);
    }
    hasher.finish()
}

pub(super) fn normalize_range_bounds(
    start_epoch: Option<i64>,
    end_epoch: Option<i64>,
) -> (i64, i64) {
    let start = start_epoch.unwrap_or(0).max(0);
    let end = end_epoch.unwrap_or(i64::MAX).max(start);
    (start, end)
}

/// Stabilize open-ended cache ranges within the current minute.
pub(super) fn normalize_open_ended_range_end(range_end: i64, now: i64) -> i64 {
    if range_end < now {
        return range_end;
    }
    ((now / 60) + 1) * 60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_filter_key_is_order_independent() {
        let first = ToolFilter::AnyOf(vec!["codex".to_string(), "claude_code".to_string()]);
        let second = ToolFilter::AnyOf(vec!["claude_code".to_string(), "codex".to_string()]);
        assert_eq!(
            cache_key_for_tool_filter(&first),
            cache_key_for_tool_filter(&second)
        );
    }

    #[test]
    fn range_bounds_never_move_before_zero_or_before_start() {
        assert_eq!(normalize_range_bounds(Some(-10), Some(-5)), (0, 0));
        assert_eq!(normalize_range_bounds(Some(20), Some(10)), (20, 20));
    }

    #[test]
    fn open_ended_range_uses_next_minute_boundary() {
        assert_eq!(normalize_open_ended_range_end(125, 120), 180);
        assert_eq!(normalize_open_ended_range_end(119, 120), 119);
    }
}
