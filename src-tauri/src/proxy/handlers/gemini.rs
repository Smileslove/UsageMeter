use super::super::gemini_api::{extract_gemini_model_from_path, is_gemini_endpoint};
use super::super::gemini_config::{GeminiConfigManager, GeminiSourceRegistry};
use super::super::request_common::{
    apply_request_identity, collect_body, get_gemini_forwarder, json_error_response,
    resolve_registered_request_base_url, resolve_registry_source_handle, resolve_target_base_url,
    ClientRoute, HandlerResult,
};
use super::super::response_bridge::{forward_gemini_passthrough, forward_gemini_with_usage};
use super::super::types::{ProxyState, RequestContext};
use hyper::{Method, Request, StatusCode};
use std::sync::Arc;

pub(crate) async fn handle_gemini_request(
    method: Method,
    path: &str,
    forward_path: &str,
    client_route: ClientRoute,
    req: Request<hyper::body::Incoming>,
    state: &Arc<ProxyState>,
) -> HandlerResult {
    let capture_usage = is_gemini_endpoint(path, &method);
    let request_start_time_ms = chrono::Utc::now().timestamp_millis();
    let request_start_instant = std::time::Instant::now();

    // Gemini CLI 用 x-goog-api-key 传 API Key；也兼容 Authorization: Bearer。
    let auth_header = req
        .headers()
        .get("x-goog-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| {
            req.headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|value| value.strip_prefix("Bearer ").or(Some(value)))
                .map(str::to_string)
        });

    let (request_headers, body_bytes) = collect_body(req).await?;

    // Gemini 模型写在路径里；body 里没有稳定 session_id。
    let gemini_model = extract_gemini_model_from_path(path);

    let source_id = client_route
        .source_id
        .clone()
        .or_else(|| GeminiConfigManager::new().active_source_id());
    let source_handle = match resolve_registry_source_handle(
        source_id.as_deref(),
        "Gemini",
        |id| GeminiSourceRegistry::new().get(id),
        |id| {
            let _ = GeminiSourceRegistry::new().touch_used(id);
        },
    ) {
        Ok(handle) => handle,
        Err(response) => return Ok(*response),
    };

    let target_base_url = match resolve_target_base_url(
        source_handle
            .as_ref()
            .map(|handle| handle.real_base_url.as_str()),
        client_route.target_base_url.as_deref(),
        "Gemini",
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
            model: gemini_model,
            session_id: None,
            ..Default::default()
        },
        &client_route,
        api_key_prefix,
        request_base_url,
        target_base_url,
    );

    let forwarder = match get_gemini_forwarder(state).await {
        Ok(forwarder) => forwarder,
        Err(response) => return Ok(*response),
    };

    if !capture_usage {
        return forward_gemini_passthrough(
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

    forward_gemini_with_usage(
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
