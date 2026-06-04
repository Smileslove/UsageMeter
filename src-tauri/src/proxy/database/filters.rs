use super::ProxyDatabase;
use crate::models::{SourceFilter, ToolFilter, UsageQueryFilter};

impl ProxyDatabase {
    pub(super) fn build_source_filter_sql(source_filter: &SourceFilter) -> (String, Vec<String>) {
        match source_filter {
            SourceFilter::All => (String::new(), vec![]),
            SourceFilter::Source {
                api_key_prefixes,
                base_url,
            } => {
                if api_key_prefixes.is_empty() {
                    return ("AND 1 = 0".to_string(), vec![]);
                }
                let placeholders: Vec<String> =
                    api_key_prefixes.iter().map(|_| "?".to_string()).collect();
                let mut params: Vec<String> = api_key_prefixes.clone();
                params.push(base_url.clone().unwrap_or_default());
                (
                    format!(
                        "AND api_key_prefix IN ({}) AND COALESCE(request_base_url, '') = ?",
                        placeholders.join(",")
                    ),
                    params,
                )
            }
            SourceFilter::Unknown { known_pairs } => {
                if known_pairs.is_empty() {
                    (String::new(), vec![])
                } else {
                    let mut clauses = Vec::new();
                    let mut params = Vec::new();
                    for (prefix, base_url) in known_pairs {
                        clauses.push(
                            "(api_key_prefix = ? AND COALESCE(request_base_url, '') = ?)"
                                .to_string(),
                        );
                        params.push(prefix.clone());
                        params.push(base_url.clone().unwrap_or_default());
                    }
                    (
                        format!(
                            "AND (api_key_prefix IS NULL OR NOT ({}))",
                            clauses.join(" OR ")
                        ),
                        params,
                    )
                }
            }
        }
    }

    pub(super) fn build_tool_filter_sql(tool_filter: &ToolFilter) -> (String, Vec<String>) {
        match tool_filter {
            ToolFilter::All => (String::new(), vec![]),
            ToolFilter::Tool(tool) if tool.trim().is_empty() => (String::new(), vec![]),
            ToolFilter::Tool(tool) => ("AND client_tool = ?".to_string(), vec![tool.clone()]),
        }
    }

    pub(super) fn build_usage_filter_sql(usage_filter: &UsageQueryFilter) -> (String, Vec<String>) {
        let (source_where, mut params) = Self::build_source_filter_sql(&usage_filter.source);
        let (tool_where, tool_params) = Self::build_tool_filter_sql(&usage_filter.tool);
        params.extend(tool_params);
        let where_clause = match (source_where.is_empty(), tool_where.is_empty()) {
            (true, true) => String::new(),
            (false, true) => source_where,
            (true, false) => tool_where,
            (false, false) => format!("{source_where} {tool_where}"),
        };
        (where_clause, params)
    }
}
