use super::super::forwarder::RequestForwarder;
use super::super::request_common::{
    apply_request_identity, collect_body, json_error_response, resolve_registered_request_base_url,
    resolve_route_source, resolve_target_base_url, ClientRoute, HandlerResult,
};
use super::super::response_bridge::{forward_claude_passthrough, forward_claude_with_usage};
use super::super::source_registry::ProxySourceRegistry;
use super::super::types::{ProxyState, RequestContext};
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

    let api_key_header = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

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
