use crate::models::{ApiSource, SubscriptionQuota};

pub(crate) fn make_source_config_error(
    source: &ApiSource,
    source_tool: Option<String>,
    tool: &str,
    msg: String,
) -> SubscriptionQuota {
    SubscriptionQuota {
        provider: "source-config".to_string(),
        tool: tool.to_string(),
        source_tool,
        credential_status: "queryFailed".to_string(),
        credential_message: source.display_name.clone(),
        success: false,
        tiers: Vec::new(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        from_cache: false,
        error: Some(msg),
        plan_label: None,
        account_label: source.display_name.clone(),
    }
}

pub(crate) fn value_to_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|v| v as f64))
        .or_else(|| value.as_u64().map(|v| v as f64))
        .or_else(|| value.as_str().and_then(|v| v.trim().parse::<f64>().ok()))
}

pub(crate) fn lookup_path<'a>(
    value: &'a serde_json::Value,
    path: &[&str],
) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    Some(current)
}

pub(crate) fn first_f64(body: &serde_json::Value, paths: &[&[&str]]) -> Option<f64> {
    paths
        .iter()
        .find_map(|path| lookup_path(body, path).and_then(value_to_f64))
}

pub(crate) fn first_string(body: &serde_json::Value, paths: &[&[&str]]) -> Option<String> {
    paths.iter().find_map(|path| {
        lookup_path(body, path)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    })
}

pub(crate) fn first_bool(body: &serde_json::Value, paths: &[&[&str]]) -> Option<bool> {
    paths
        .iter()
        .find_map(|path| lookup_path(body, path).and_then(|v| v.as_bool()))
}

pub(crate) fn normalize_base_url(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_string()
}
