use super::types::{canonical_request_key_for_local, canonical_request_key_for_proxy};
use crate::models::ToolFilter;
use crate::proxy::UsageRecord;
use crate::session::{LocalRequestRecord, SessionMeta};
use std::collections::HashMap;

pub(super) fn request_key_for_local(record: &LocalRequestRecord) -> String {
    canonical_request_key_for_local(record)
}

pub(super) fn request_key_for_proxy(record: &UsageRecord) -> String {
    canonical_request_key_for_proxy(record)
}

pub(super) fn local_tool_matches(record: &LocalRequestRecord, tool_filter: &ToolFilter) -> bool {
    match tool_filter {
        ToolFilter::All => true,
        ToolFilter::Tool(tool) if tool.trim().is_empty() => true,
        ToolFilter::Tool(tool) => record.tool == *tool,
        ToolFilter::AnyOf(tools) => tools.contains(&record.tool),
    }
}

pub(super) fn session_meta_matches(meta: &SessionMeta, tool_filter: &ToolFilter) -> bool {
    match tool_filter {
        ToolFilter::All => true,
        ToolFilter::Tool(tool) if tool.trim().is_empty() => true,
        ToolFilter::Tool(tool) => meta.tool == *tool,
        ToolFilter::AnyOf(tools) => tools.contains(&meta.tool),
    }
}

pub(super) fn build_local_meta_index(sessions: &[SessionMeta]) -> HashMap<String, SessionMeta> {
    sessions
        .iter()
        .cloned()
        .map(|meta| (meta.session_id.clone(), meta))
        .collect()
}

pub(super) fn build_message_to_session_index(
    local_records: &[LocalRequestRecord],
) -> HashMap<String, String> {
    let mut message_to_session = HashMap::new();
    for record in local_records {
        if !record.message_id.trim().is_empty() {
            message_to_session.insert(record.message_id.clone(), record.session_id.clone());
        }
    }
    message_to_session
}

pub(super) fn attach_proxy_session_ids(
    proxy_records: &mut [UsageRecord],
    message_to_session: &HashMap<String, String>,
) {
    for record in proxy_records.iter_mut() {
        let needs_fill = record
            .session_id
            .as_ref()
            .map(|id| id.trim().is_empty())
            .unwrap_or(true);
        if needs_fill {
            if let Some(session_id) = message_to_session.get(&record.message_id) {
                record.session_id = Some(session_id.clone());
            }
        }
    }
}

pub(super) fn compute_local_request_cost_cached(
    record: &LocalRequestRecord,
    pricings: &[crate::models::ModelPricingConfig],
    match_mode: &str,
    pricing_cache: &mut HashMap<String, crate::models::ModelPricing>,
) -> f64 {
    let pricing = pricing_cache
        .entry(record.model.clone())
        .or_insert_with(|| crate::models::get_pricing(&record.model, pricings, match_mode));

    let input_cost = (record.input_tokens as f64 / 1_000_000.0) * pricing.input;
    let output_cost = (record.output_tokens as f64 / 1_000_000.0) * pricing.output;
    let cache_read_cost = (record.cache_read_tokens as f64 / 1_000_000.0) * pricing.cache_read;
    let cache_create_cost =
        (record.cache_create_tokens as f64 / 1_000_000.0) * pricing.cache_write_1h;

    input_cost + output_cost + cache_read_cost + cache_create_cost
}

#[cfg(test)]
mod tests {
    use super::local_tool_matches;
    use crate::models::ToolFilter;
    use crate::session::LocalRequestRecord;

    #[test]
    fn empty_tool_filter_matches_local_records() {
        let record = LocalRequestRecord {
            tool: "codex".to_string(),
            ..Default::default()
        };
        assert!(local_tool_matches(
            &record,
            &ToolFilter::Tool(String::new())
        ));
    }
}
