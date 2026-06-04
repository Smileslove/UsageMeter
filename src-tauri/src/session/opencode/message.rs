use crate::session::opencode_reader::OpenCodeMessageSnapshot;
use serde_json::Value;

pub(in crate::session) fn parse_message_snapshot(
    raw_session_id: &str,
    raw_message_id: &str,
    data: &Value,
    fallback_time_updated_ms: i64,
    source_kind: &'static str,
) -> Option<OpenCodeMessageSnapshot> {
    let role = data
        .get("role")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if role != "assistant" {
        return None;
    }

    let canonical_session_id = canonical_opencode_session_id(raw_session_id);
    let message_id = if raw_message_id.is_empty() {
        data.get("id")
            .or_else(|| data.get("messageID"))
            .or_else(|| data.get("messageId"))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        raw_message_id.trim().to_string()
    };
    if canonical_session_id.trim().is_empty() || message_id.trim().is_empty() {
        return None;
    }

    let tokens = data.get("tokens")?.as_object()?;
    let input_tokens = tokens.get("input").map(to_non_negative_u64).unwrap_or(0);
    let output_tokens = tokens.get("output").map(to_non_negative_u64).unwrap_or(0);
    let reasoning_tokens = tokens
        .get("reasoning")
        .map(to_non_negative_u64)
        .unwrap_or(0);
    let cache_read_tokens = tokens
        .get("cache")
        .and_then(|cache| cache.get("read"))
        .map(to_non_negative_u64)
        .unwrap_or(0);
    let cache_create_tokens = tokens
        .get("cache")
        .and_then(|cache| cache.get("write"))
        .map(to_non_negative_u64)
        .unwrap_or(0);
    let total_tokens =
        input_tokens + output_tokens + reasoning_tokens + cache_read_tokens + cache_create_tokens;
    if total_tokens == 0 {
        return None;
    }

    let timestamp_ms = extract_opencode_timestamp_ms(data).unwrap_or(fallback_time_updated_ms);
    if timestamp_ms <= 0 {
        return None;
    }
    let timestamp_sec = timestamp_ms / 1000;

    let provider_id = data
        .get("providerID")
        .or_else(|| data.get("providerId"))
        .or_else(|| data.get("provider"))
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let model_id = data
        .get("modelID")
        .or_else(|| data.get("modelId"))
        .or_else(|| data.get("model"))
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let model = normalize_model_string(provider_id.as_deref(), model_id.as_deref());
    let cwd = data
        .pointer("/path/cwd")
        .or_else(|| data.pointer("/path/cwdPath"))
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let title = data
        .get("title")
        .or_else(|| data.get("sessionTitle"))
        .and_then(|value| value.as_str())
        .map(str::to_string);

    Some(OpenCodeMessageSnapshot {
        canonical_session_id,
        raw_message_id: message_id,
        timestamp_sec,
        model,
        cwd,
        title,
        input_tokens,
        output_tokens,
        reasoning_tokens,
        cache_create_tokens,
        cache_read_tokens,
        total_tokens,
        source_kind,
    })
}

pub(in crate::session) fn normalize_model_string(
    provider_id: Option<&str>,
    model_id: Option<&str>,
) -> String {
    match (provider_id, model_id) {
        (_, Some(model)) if !model.is_empty() => model.to_string(),
        (Some(provider), None) if !provider.is_empty() => provider.to_string(),
        _ => "unknown".to_string(),
    }
}

pub(in crate::session) fn canonical_opencode_session_id(raw_session_id: &str) -> String {
    if raw_session_id.starts_with("opencode::") {
        raw_session_id.to_string()
    } else {
        format!("opencode::{}", raw_session_id)
    }
}

fn extract_opencode_timestamp_ms(data: &Value) -> Option<i64> {
    let completed = data
        .pointer("/time/completed")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    if completed > 0 {
        return Some(completed);
    }
    let created = data
        .pointer("/time/created")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    (created > 0).then_some(created)
}

fn to_non_negative_u64(value: &Value) -> u64 {
    value
        .as_i64()
        .map(|v| v.max(0) as u64)
        .or_else(|| value.as_u64())
        .unwrap_or(0)
}
