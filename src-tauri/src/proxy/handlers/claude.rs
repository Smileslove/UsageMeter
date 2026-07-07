use super::super::forwarder::RequestForwarder;
use super::super::request_common::{
    apply_request_identity, collect_body, json_error_response, resolve_registered_request_base_url,
    resolve_route_source, resolve_target_base_url, ClientRoute, HandlerResult,
};
use super::super::response_bridge::{forward_claude_passthrough, forward_claude_with_usage};
use super::super::source_registry::ProxySourceRegistry;
use super::super::types::{ProxyState, RequestContext};
use hyper::header::AUTHORIZATION;
use hyper::{Method, Request, StatusCode};
use std::sync::Arc;

pub(crate) async fn handle_claude_request(
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

    // Claude Code 用 ANTHROPIC_API_KEY 时走 x-api-key；用 ANTHROPIC_AUTH_TOKEN 时走
    // Authorization: Bearer（大量第三方 Anthropic 兼容供应商只接受后者）。只读 x-api-key
    // 会让这些请求捕获到 base_url 却拿不到 key 前缀，导致来源无法归因、落入「未归因」桶。
    let api_key_header = extract_claude_auth_token(req.headers());

    let (request_headers, body_bytes) = collect_body(req).await?;

    let active_source_id = state.active_source_id.read().await.clone();
    let source_handle = match resolve_route_source(
        client_route.source_id.as_deref(),
        active_source_id.as_deref(),
    ) {
        Ok(handle) => handle,
        Err(e) => {
            return Ok(json_error_response(
                StatusCode::BAD_GATEWAY,
                "proxy_source_not_found",
                &e,
            ));
        }
    };
    if let Some(ref handle) = source_handle {
        let _ = ProxySourceRegistry::new().touch_used(&handle.id);
    }

    let target_base_url = match resolve_target_base_url(
        source_handle
            .as_ref()
            .map(|handle| handle.real_base_url.as_str()),
        client_route.target_base_url.as_deref(),
        "Claude",
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
        resolve_registered_request_base_url(state, api_key_header.as_deref(), &target_base_url)
            .await;

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

    let capture_usage = path == "/v1/messages" && method == Method::POST;
    if capture_usage {
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

fn extract_claude_auth_token(headers: &hyper::HeaderMap) -> Option<String> {
    headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(extract_bearer_token)
                .map(str::to_string)
        })
}

fn extract_bearer_token(value: &str) -> Option<&str> {
    let mut parts = value.split_whitespace();
    let scheme = parts.next()?;
    let token = parts.next()?;
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::HeaderValue;
    use hyper::HeaderMap;

    #[test]
    fn extracts_claude_x_api_key() {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("sk-ant-primary"));

        assert_eq!(
            extract_claude_auth_token(&headers).as_deref(),
            Some("sk-ant-primary")
        );
    }

    #[test]
    fn falls_back_to_bearer_authorization_for_claude() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer sk-ant-bearer"),
        );

        assert_eq!(
            extract_claude_auth_token(&headers).as_deref(),
            Some("sk-ant-bearer")
        );
    }

    #[test]
    fn prefers_x_api_key_over_authorization_for_claude() {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("sk-ant-primary"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer sk-ant-secondary"),
        );

        assert_eq!(
            extract_claude_auth_token(&headers).as_deref(),
            Some("sk-ant-primary")
        );
    }

    #[test]
    fn ignores_non_bearer_authorization_for_claude() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Basic abc123"));

        assert_eq!(extract_claude_auth_token(&headers), None);
    }

    #[test]
    fn bearer_authorization_is_trimmed_and_case_insensitive() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("   bearer   sk-ant-trimmed   "),
        );

        assert_eq!(
            extract_claude_auth_token(&headers).as_deref(),
            Some("sk-ant-trimmed")
        );
    }
}
