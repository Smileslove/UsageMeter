use super::super::codex_api::is_codex_endpoint;
use super::super::codex_config::{CodexConfigManager, CodexSourceRegistry};
use super::super::request_common::{
    apply_request_identity, collect_body, get_openai_forwarder, json_error_response,
    resolve_registered_request_base_url, resolve_registry_source_handle, resolve_target_base_url,
    ClientRoute, HandlerResult,
};
use super::super::response_bridge::{forward_codex_passthrough, forward_codex_with_usage};
use super::super::types::{ProxyState, RequestContext};
use hyper::{Method, Request, StatusCode};
use serde_json::Value;
use std::sync::Arc;

pub(crate) async fn handle_codex_request(
    method: Method,
    path: &str,
    forward_path: &str,
    client_route: ClientRoute,
    req: Request<hyper::body::Incoming>,
    state: &Arc<ProxyState>,
) -> HandlerResult {
    let capture_usage = is_codex_endpoint(path, &method);
    let request_start_time_ms = chrono::Utc::now().timestamp_millis();
    let request_start_instant = std::time::Instant::now();

    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer ").or(Some(value)))
        .map(str::to_string);

    let (request_headers, body_bytes) = collect_body(req).await?;

    let (codex_session_id, codex_model) = serde_json::from_slice::<Value>(&body_bytes)
        .ok()
        .map(|json| {
            let session_id = json
                .get("session_id")
                .or_else(|| json.pointer("/metadata/session_id"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let model = json
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            (session_id, model)
        })
        .unwrap_or((None, None));

    let source_id = client_route
        .source_id
        .clone()
        .or_else(|| CodexConfigManager::new().active_source_id());
    let source_handle = match resolve_registry_source_handle(
        source_id.as_deref(),
        "Codex",
        |id| CodexSourceRegistry::new().get(id),
        |id| {
            let _ = CodexSourceRegistry::new().touch_used(id);
        },
    ) {
        Ok(handle) => handle,
        Err(response) => return Ok(response),
    };

    let target_base_url = match resolve_target_base_url(
        source_handle
            .as_ref()
            .map(|handle| handle.real_base_url.as_str()),
        client_route.target_base_url.as_deref(),
        "Codex",
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
        resolve_registered_request_base_url(state, auth_header.as_deref(), &target_base_url).await;

    let context = apply_request_identity(
        RequestContext {
            start_time: request_start_instant,
            start_time_ms: request_start_time_ms,
            model: codex_model,
            session_id: codex_session_id,
            ..Default::default()
        },
        &client_route,
        api_key_prefix,
        request_base_url,
        target_base_url,
    );

    let openai_forwarder = match get_openai_forwarder(state).await {
        Ok(forwarder) => forwarder,
        Err(response) => return Ok(response),
    };

    if !capture_usage {
        return forward_codex_passthrough(
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

    forward_codex_with_usage(
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
