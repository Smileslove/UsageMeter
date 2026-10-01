use super::super::codex_api::is_codex_endpoint;
use super::super::forwarder::RequestForwarder;
use super::super::gemini_api::is_gemini_endpoint;
use super::super::pi_config::{PiConfigManager, PiProviderApi, PiSourceRegistry};
use super::super::request_common::{
    apply_request_identity, build_request_base_url, collect_body, get_gemini_forwarder,
    get_openai_forwarder, json_error_response, resolve_registry_source_handle,
    resolve_target_base_url, ClientRoute, HandlerResult,
};
use super::super::response_bridge::{
    forward_claude_passthrough, forward_claude_with_usage, forward_codex_passthrough,
    forward_codex_with_usage, forward_gemini_passthrough, forward_gemini_with_usage,
};
use super::super::types::{ProxyState, RequestContext};
use hyper::{Method, Request, StatusCode};
use std::sync::Arc;

fn extract_auth_token(api: PiProviderApi, headers: &hyper::HeaderMap) -> Option<String> {
    match api {
        PiProviderApi::AnthropicMessages => headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| bearer_token(headers)),
        PiProviderApi::GoogleGenerativeAi | PiProviderApi::GoogleVertex => headers
            .get("x-goog-api-key")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| bearer_token(headers)),
        PiProviderApi::OpenaiCompletions | PiProviderApi::OpenaiResponses => bearer_token(headers)
            .or_else(|| {
                headers
                    .get("x-api-key")
                    .and_then(|value| value.to_str().ok())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            }),
    }
}

fn bearer_token(headers: &hyper::HeaderMap) -> Option<String> {
    headers
        .get(hyper::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let mut parts = value.split_whitespace();
            let scheme = parts.next()?;
            let token = parts.next()?;
            if !scheme.eq_ignore_ascii_case("bearer") || parts.next().is_some() {
                return None;
            }
            Some(token.to_string())
        })
}

pub(crate) async fn handle_pi_request(
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

    let source_id = if let Some(source_id) = client_route.source_id.clone() {
        Some(String::from(source_id))
    } else {
        // Legacy Pi URLs may omit the source path. Resolve only a source that
        // belongs to this listener; another UsageMeter instance may own a
        // different port and upstream.
        let proxy_port = state.config.read().await.port;
        PiConfigManager::new()
            .active_source_ids_for_port(proxy_port)
            .into_iter()
            .next()
    };
    let source_handle = match resolve_registry_source_handle(
        source_id.as_deref(),
        "Pi",
        |id| PiSourceRegistry::new().get(id),
        |id| {
            let _ = PiSourceRegistry::new().touch_used(id);
        },
    ) {
        Ok(handle) => handle,
        Err(response) => return Ok(*response),
    };

    let Some(api) = source_handle.as_ref().map(|handle| handle.api) else {
        return Ok(json_error_response(
            StatusCode::BAD_GATEWAY,
            "proxy_provider_api_unknown",
            "Pi provider API is not configured or is unsupported",
        ));
    };
    let auth_token = extract_auth_token(api, &request_headers);
    let target_base_url = match resolve_target_base_url(
        source_handle
            .as_ref()
            .map(|handle| handle.real_base_url.as_str()),
        client_route.target_base_url.as_deref(),
        "Pi",
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

    match api {
        PiProviderApi::AnthropicMessages => {
            if method == Method::POST
                && matches!(path.trim_start_matches('/'), "messages" | "v1/messages")
            {
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
        PiProviderApi::OpenaiCompletions | PiProviderApi::OpenaiResponses => {
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
        PiProviderApi::GoogleGenerativeAi | PiProviderApi::GoogleVertex => {
            let gemini_forwarder = match get_gemini_forwarder(state).await {
                Ok(forwarder) => forwarder,
                Err(response) => return Ok(*response),
            };
            if is_gemini_endpoint(path, &method) {
                return forward_gemini_with_usage(
                    &gemini_forwarder,
                    method,
                    forward_path,
                    request_headers,
                    body_bytes,
                    context,
                    state,
                )
                .await;
            }
            forward_gemini_passthrough(
                &gemini_forwarder,
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

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::{HeaderValue, AUTHORIZATION};

    #[test]
    fn extracts_protocol_specific_auth_without_logging_secret() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("sk-ant"));
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer sk-openai"));
        assert_eq!(
            extract_auth_token(PiProviderApi::AnthropicMessages, &headers).as_deref(),
            Some("sk-ant")
        );
        assert_eq!(
            extract_auth_token(PiProviderApi::OpenaiCompletions, &headers).as_deref(),
            Some("sk-openai")
        );
    }
}
