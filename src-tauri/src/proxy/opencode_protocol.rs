use super::codex_api::is_codex_endpoint;
use serde_json::Value;

const OPENCODE_SESSION_DEBUG_ENV: &str = "USAGEMETER_DEBUG_OPENCODE_SESSION";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OpenCodeProviderProtocol {
    Anthropic,
    OpenAiCompatible,
    OpenAi,
    Unknown,
}

pub(super) fn is_openai_usage_endpoint(path: &str, method: &hyper::Method) -> bool {
    is_codex_endpoint(path, method)
}

pub(super) fn is_opencode_messages_endpoint(path: &str, method: &hyper::Method) -> bool {
    if *method != hyper::Method::POST {
        return false;
    }

    matches!(path.trim_start_matches('/'), "messages" | "v1/messages")
}

pub(super) fn opencode_provider_protocol(
    provider_id: Option<&str>,
    provider_npm: Option<&str>,
) -> OpenCodeProviderProtocol {
    match provider_npm {
        Some("@ai-sdk/anthropic") => OpenCodeProviderProtocol::Anthropic,
        Some("@ai-sdk/openai-compatible") => OpenCodeProviderProtocol::OpenAiCompatible,
        Some("@ai-sdk/openai") => OpenCodeProviderProtocol::OpenAi,
        Some(_) => OpenCodeProviderProtocol::Unknown,
        None => match provider_id {
            Some("anthropic") => OpenCodeProviderProtocol::Anthropic,
            Some("openai") => OpenCodeProviderProtocol::OpenAi,
            _ => OpenCodeProviderProtocol::Unknown,
        },
    }
}

pub(super) fn extract_opencode_auth_token(
    protocol: OpenCodeProviderProtocol,
    headers: &hyper::HeaderMap,
) -> Option<String> {
    match protocol {
        OpenCodeProviderProtocol::Anthropic => headers
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()),
        _ => headers
            .get(hyper::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                headers
                    .get("x-api-key")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
            }),
    }
}

pub(super) fn is_opencode_session_debug_enabled() -> bool {
    matches!(
        std::env::var(OPENCODE_SESSION_DEBUG_ENV).ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("on")
    )
}

pub(super) fn observe_opencode_request_shape(
    method: &hyper::Method,
    path: &str,
    body_bytes: &bytes::Bytes,
) {
    if !is_opencode_session_debug_enabled() {
        return;
    }

    let body_len = body_bytes.len();
    let parsed = serde_json::from_slice::<Value>(body_bytes);
    let json = match parsed {
        Ok(json) => json,
        Err(err) => {
            eprintln!(
                "[opencode-observe] method={} path={} body_len={} parse_error={}",
                method, path, body_len, err
            );
            return;
        }
    };

    let top_level_keys = json
        .as_object()
        .map(|obj| {
            let mut keys: Vec<String> = obj.keys().cloned().collect();
            keys.sort();
            keys
        })
        .unwrap_or_default();
    let metadata_keys = json
        .get("metadata")
        .and_then(|value| value.as_object())
        .map(|obj| {
            let mut keys: Vec<String> = obj.keys().cloned().collect();
            keys.sort();
            keys
        })
        .unwrap_or_default();

    let mut session_hits = Vec::new();
    collect_session_field_hits("$", &json, &mut session_hits, 24);

    eprintln!(
        "[opencode-observe] method={} path={} body_len={} top_level_keys={:?} metadata_keys={:?} session_hits={:?}",
        method,
        path,
        body_len,
        top_level_keys,
        metadata_keys,
        session_hits
    );
}

fn collect_session_field_hits(path: &str, value: &Value, hits: &mut Vec<String>, max_hits: usize) {
    if hits.len() >= max_hits {
        return;
    }

    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if hits.len() >= max_hits {
                    break;
                }
                let child_path = format!("{path}.{key}");
                if matches!(key.as_str(), "session_id" | "sessionId" | "sessionID") {
                    hits.push(format!(
                        "{}={}",
                        child_path,
                        summarize_session_debug_value(child)
                    ));
                }
                collect_session_field_hits(&child_path, child, hits, max_hits);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                if hits.len() >= max_hits {
                    break;
                }
                let child_path = format!("{path}[{index}]");
                collect_session_field_hits(&child_path, child, hits, max_hits);
            }
        }
        _ => {}
    }
}

fn summarize_session_debug_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => v.to_string(),
        Value::String(v) => {
            let trimmed = v.trim();
            if trimmed.is_empty() {
                "\"\"".to_string()
            } else if trimmed.chars().count() <= 96 {
                format!("{trimmed:?}")
            } else {
                let prefix: String = trimmed.chars().take(96).collect();
                format!("{prefix:?}…")
            }
        }
        Value::Array(items) => format!("[array:{}]", items.len()),
        Value::Object(map) => {
            let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
            keys.sort();
            if keys.len() > 8 {
                keys.truncate(8);
            }
            format!("{{keys:{keys:?}}}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::{HeaderValue, AUTHORIZATION};
    use hyper::{HeaderMap, Method};

    #[test]
    fn opencode_usage_capture_accepts_transparent_message_paths() {
        assert!(is_opencode_messages_endpoint("/messages", &Method::POST));
        assert!(is_opencode_messages_endpoint("/v1/messages", &Method::POST));
        assert!(!is_opencode_messages_endpoint("/messages", &Method::GET));
        assert!(!is_opencode_messages_endpoint("/v1/models", &Method::POST));
    }

    #[test]
    fn extracts_auth_header_for_openai_protocols() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer token-123"));
        assert_eq!(
            extract_opencode_auth_token(OpenCodeProviderProtocol::OpenAi, &headers).as_deref(),
            Some("Bearer token-123")
        );
    }

    #[test]
    fn prefers_x_api_key_for_anthropic_protocol() {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("sk-ant-123"));
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer ignored"));
        assert_eq!(
            extract_opencode_auth_token(OpenCodeProviderProtocol::Anthropic, &headers).as_deref(),
            Some("sk-ant-123")
        );
    }
}
