use super::super::codex_api::is_codex_endpoint;
use super::super::forwarder::RequestForwarder;
use super::super::reasonix_config::{ReasonixConfigManager, ReasonixSourceRegistry};
use super::super::request_common::{
    apply_request_identity, build_request_base_url, collect_body, get_openai_forwarder,
    json_error_response, resolve_registry_source_handle, resolve_target_base_url, ClientRoute,
    HandlerResult,
};
use super::super::response_bridge::{
    forward_claude_passthrough, forward_claude_with_usage, forward_codex_passthrough,
    forward_codex_with_usage,
};
use super::super::types::{ProxyState, RequestContext};
use hyper::{Method, Request, StatusCode};
use std::sync::Arc;

/// Reasonix provider 协议：由 source handle 中保存的 `kind` 决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReasonixProtocol {
    /// DeepSeek / MiMo 等 OpenAI 兼容 chat completions。
    OpenAi,
    /// Anthropic messages。
    Anthropic,
}

fn reasonix_protocol_from_kind(kind: Option<&str>) -> ReasonixProtocol {
    match kind.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("anthropic") => ReasonixProtocol::Anthropic,
        // 默认 openai（Reasonix 未声明 kind 时按 openai 处理）。
        _ => ReasonixProtocol::OpenAi,
    }
}

fn is_reasonix_messages_endpoint(path: &str, method: &Method) -> bool {
    if *method != Method::POST {
        return false;
    }
    matches!(path.trim_start_matches('/'), "messages" | "v1/messages")
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_reasonix_request(
    method: Method,
    path: &str,
    forward_path: &str,
    client_route: ClientRoute,
    req: Request<hyper::body::Incoming>,
    forwarder: Arc<RequestForwarder>,
    state: &Arc<ProxyState>,
) -> HandlerResult {
    let request_start_time_ms = chrono::Utc::now().timestamp_millis();
    let request_start_instant = std::time::Instant::now();

    let (request_headers, body_bytes) = collect_body(req).await?;

    let source_id = client_route
        .source_id
        .clone()
        .or_else(|| ReasonixConfigManager::new().active_source_id());

    let source_handle = match resolve_registry_source_handle(
        source_id.as_deref(),
        "Reasonix",
        |id| ReasonixSourceRegistry::new().get(id),
        |id| {
            let _ = ReasonixSourceRegistry::new().touch_used(id);
        },
    ) {
        Ok(handle) => handle,
        Err(response) => return Ok(*response),
    };

    let protocol =
        reasonix_protocol_from_kind(source_handle.as_ref().map(|handle| handle.kind.as_str()));

    let auth_token = extract_reasonix_auth_token(protocol, &request_headers);

    let target_base_url = match resolve_target_base_url(
        source_handle.as_ref().map(|h| h.real_base_url.as_str()),
        client_route.target_base_url.as_deref(),
        "Reasonix",
    ) {
        Ok(url) => url,
        Err(message) => {
            return Ok(json_error_response(
                StatusCode::BAD_GATEWAY,
                "proxy_target_not_configured",
                &message,
            ));
        }
    };

    let (api_key_prefix, request_base_url) =
        build_request_base_url(state, auth_token.as_deref(), &target_base_url).await;

    let context = apply_request_identity(
        RequestContext {
            start_time: request_start_instant,
            start_time_ms: request_start_time_ms,
            ..Default::default()
        },
        &client_route,
        api_key_prefix,
        request_base_url,
        target_base_url,
    );

    match protocol {
        ReasonixProtocol::Anthropic => {
            if is_reasonix_messages_endpoint(path, &method) {
                return forward_claude_with_usage(
                    &forwarder,
                    method,
                    forward_path,
                    request_headers,
                    body_bytes,
                    context,
                    state,
                )
                .await;
            }
            forward_claude_passthrough(
                &forwarder,
                method,
                forward_path,
                request_headers,
                body_bytes,
                context,
                state,
            )
            .await
        }
        ReasonixProtocol::OpenAi => {
            let openai_forwarder = match get_openai_forwarder(state).await {
                Ok(forwarder) => forwarder,
                Err(response) => return Ok(*response),
            };
            if is_codex_endpoint(path, &method) {
                return forward_codex_with_usage(
                    &openai_forwarder,
                    method,
                    forward_path,
                    request_headers,
                    body_bytes,
                    context,
                    state,
                )
                .await;
            }
            forward_codex_passthrough(
                &openai_forwarder,
                method,
                forward_path,
                request_headers,
                body_bytes,
                context,
                state,
            )
            .await
        }
    }
}

fn extract_reasonix_auth_token(
    protocol: ReasonixProtocol,
    headers: &hyper::HeaderMap,
) -> Option<String> {
    match protocol {
        ReasonixProtocol::Anthropic => headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.to_string()),
        ReasonixProtocol::OpenAi => headers
            .get(hyper::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::{HeaderValue, AUTHORIZATION};
    use hyper::HeaderMap;

    #[test]
    fn protocol_defaults_to_openai() {
        assert_eq!(
            reasonix_protocol_from_kind(Some("openai")),
            ReasonixProtocol::OpenAi
        );
        assert_eq!(reasonix_protocol_from_kind(None), ReasonixProtocol::OpenAi);
        assert_eq!(
            reasonix_protocol_from_kind(Some("anthropic")),
            ReasonixProtocol::Anthropic
        );
    }

    #[test]
    fn messages_endpoint_detection() {
        assert!(is_reasonix_messages_endpoint("/v1/messages", &Method::POST));
        assert!(is_reasonix_messages_endpoint("/messages", &Method::POST));
        assert!(!is_reasonix_messages_endpoint("/v1/messages", &Method::GET));
    }

    #[test]
    fn auth_extraction_matches_protocol() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer sk-deepseek"),
        );
        headers.insert("x-api-key", HeaderValue::from_static("sk-ant"));
        assert_eq!(
            extract_reasonix_auth_token(ReasonixProtocol::OpenAi, &headers).as_deref(),
            Some("Bearer sk-deepseek")
        );
        assert_eq!(
            extract_reasonix_auth_token(ReasonixProtocol::Anthropic, &headers).as_deref(),
            Some("sk-ant")
        );
    }
}
