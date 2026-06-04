use super::super::forwarder::RequestForwarder;
use super::super::opencode_config::{OpenCodeConfigManager, OpenCodeSourceRegistry};
use super::super::opencode_protocol::{
    extract_opencode_auth_token, is_openai_usage_endpoint, is_opencode_messages_endpoint,
    observe_opencode_request_shape, opencode_provider_protocol, OpenCodeProviderProtocol,
};
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

pub(crate) async fn handle_opencode_request(
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
    observe_opencode_request_shape(&method, path, &body_bytes);

    let source_id = client_route
        .source_id
        .clone()
        .or_else(|| OpenCodeConfigManager::new().active_source_id());

    let source_handle = match resolve_registry_source_handle(
        source_id.as_deref(),
        "OpenCode",
        |id| OpenCodeSourceRegistry::new().get(id),
        |id| {
            let _ = OpenCodeSourceRegistry::new().touch_used(id);
        },
    ) {
        Ok(handle) => handle,
        Err(response) => return Ok(response),
    };

    let provider_protocol = opencode_provider_protocol(
        client_route.provider_id.as_deref().or_else(|| {
            source_handle
                .as_ref()
                .map(|handle| handle.provider_id.as_str())
        }),
        source_handle
            .as_ref()
            .and_then(|handle| handle.provider_npm.as_deref()),
    );
    let auth_token = extract_opencode_auth_token(provider_protocol, &request_headers);

    let target_base_url = match resolve_target_base_url(
        source_handle.as_ref().map(|h| h.real_base_url.as_str()),
        client_route.target_base_url.as_deref(),
        "OpenCode",
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

    match provider_protocol {
        OpenCodeProviderProtocol::Anthropic => {
            let capture_usage = is_opencode_messages_endpoint(path, &method);
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
        OpenCodeProviderProtocol::OpenAiCompatible | OpenCodeProviderProtocol::OpenAi => {
            let openai_forwarder = match get_openai_forwarder(state).await {
                Ok(forwarder) => forwarder,
                Err(response) => return Ok(response),
            };
            let capture_usage = is_openai_usage_endpoint(path, &method);
            if capture_usage {
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
        OpenCodeProviderProtocol::Unknown => {
            let openai_forwarder = match get_openai_forwarder(state).await {
                Ok(forwarder) => forwarder,
                Err(response) => return Ok(response),
            };
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
