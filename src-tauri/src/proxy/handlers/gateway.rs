//! Native-protocol local API gateway routing.
//!
//! A gateway profile is deliberately a route, not an authentication layer:
//! the client keeps its upstream credentials and this handler only selects the
//! configured upstream and forwards the request using the matching adapter.

use super::super::forwarder::RequestForwarder;
use super::super::request_common::{
    append_query, get_gemini_forwarder, get_openai_forwarder, json_error_response, HandlerResult,
};
use super::super::response_bridge::{
    forward_claude_with_usage, forward_codex_passthrough, forward_codex_with_usage,
    forward_gemini_with_usage,
};
use super::super::types::{ProxyState, RequestContext};
use crate::gateway::{
    is_expected_upstream_secret_ref, load_upstream_secret, report_upstream_result,
    select_upstream_key, select_upstream_key_excluding, validate_profile, verify_local_key,
};
use crate::models::{
    AppSettings, GatewayAuthMode, GatewayDispatchStrategy, GatewayProfile, GatewayProtocol,
};
use bytes::BytesMut;
use http_body_util::BodyExt;
use hyper::{
    header::{HeaderName, HeaderValue, AUTHORIZATION, CONTENT_LENGTH},
    Method, Request, StatusCode,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const GATEWAY_PREFIX: &str = "/gateway/";
const CLIENT_LABEL_HEADER: &str = "x-usagemeter-client";
const MAX_GATEWAY_REQUEST_BODY_BYTES: usize = 16 * 1024 * 1024;
static NEXT_GATEWAY_REQUEST: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GatewayForwardMode {
    OpenAiUsage,
    OpenAiPassthrough,
    AnthropicUsage,
    GeminiUsage,
}

#[derive(Debug, Clone, PartialEq)]
struct GatewayRoute {
    profile: GatewayProfile,
    path: String,
    mode: GatewayForwardMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GatewayRouteError {
    Malformed,
    ProfileNotFound,
    ProfileInvalid,
    ProfileDisabled,
    ProtocolMismatch,
    MethodNotAllowed,
    LocalKeyUnauthorized,
    UpstreamKeyUnavailable,
}

impl GatewayRouteError {
    fn response(self) -> hyper::Response<super::super::request_common::BoxBody> {
        let (status, error_type, message) = match self {
            Self::Malformed => (
                StatusCode::NOT_FOUND,
                "gateway_route_not_matched",
                "Gateway route must use /gateway/<profile-id>/<native-api-path>",
            ),
            Self::ProfileNotFound => (
                StatusCode::NOT_FOUND,
                "gateway_profile_not_found",
                "Gateway profile was not found",
            ),
            Self::ProfileInvalid => (
                StatusCode::FORBIDDEN,
                "gateway_profile_invalid",
                "Gateway profile is not valid for forwarding",
            ),
            Self::ProfileDisabled => (
                StatusCode::NOT_FOUND,
                "gateway_profile_disabled",
                "Gateway profile is disabled",
            ),
            Self::ProtocolMismatch => (
                StatusCode::BAD_REQUEST,
                "gateway_protocol_mismatch",
                "Request path does not match the configured native protocol",
            ),
            Self::MethodNotAllowed => (
                StatusCode::METHOD_NOT_ALLOWED,
                "gateway_method_not_allowed",
                "HTTP method is not supported for this native endpoint",
            ),
            Self::LocalKeyUnauthorized => (
                StatusCode::UNAUTHORIZED,
                "gateway_local_key_unauthorized",
                "A valid local gateway API key is required",
            ),
            Self::UpstreamKeyUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "gateway_no_upstream_key_available",
                "No usable upstream API key is configured",
            ),
        };
        json_error_response(status, error_type, message)
    }
}

pub(crate) fn is_gateway_path(path: &str) -> bool {
    path == "/gateway" || path.starts_with(GATEWAY_PREFIX)
}

fn split_gateway_path(path: &str) -> Result<(String, String), GatewayRouteError> {
    let rest = path
        .strip_prefix(GATEWAY_PREFIX)
        .ok_or(GatewayRouteError::Malformed)?;
    let mut parts = rest.splitn(2, '/');
    let profile_id = parts.next().unwrap_or_default();
    let tail = parts.next().unwrap_or_default();
    if profile_id.is_empty()
        || tail.is_empty()
        || profile_id == "."
        || profile_id == ".."
        || profile_id.contains('.')
        || profile_id.contains('%')
        || profile_id.contains('\\')
    {
        return Err(GatewayRouteError::Malformed);
    }
    Ok((profile_id.to_string(), format!("/{tail}")))
}

fn gemini_path_matches(path: &str) -> bool {
    let path = path.split_once('?').map(|(path, _)| path).unwrap_or(path);
    let Some(models) = path.strip_prefix("/v1beta/models/") else {
        return false;
    };
    let Some((model, action)) = models.rsplit_once(':') else {
        return false;
    };
    !model.is_empty()
        && !model.contains('/')
        && !model.contains("..")
        && matches!(action, "generateContent" | "streamGenerateContent")
}

fn mode_for_profile(
    profile: &GatewayProfile,
    method: &Method,
    path: &str,
) -> Result<GatewayForwardMode, GatewayRouteError> {
    match profile.protocol {
        GatewayProtocol::OpenAiChatCompletions => {
            if path == "/v1/chat/completions" {
                if *method == Method::POST {
                    Ok(GatewayForwardMode::OpenAiUsage)
                } else {
                    Err(GatewayRouteError::MethodNotAllowed)
                }
            } else if path == "/v1/models" {
                if *method == Method::GET {
                    Ok(GatewayForwardMode::OpenAiPassthrough)
                } else {
                    Err(GatewayRouteError::MethodNotAllowed)
                }
            } else {
                Err(GatewayRouteError::ProtocolMismatch)
            }
        }
        GatewayProtocol::OpenAiResponses => {
            if path == "/v1/responses" {
                if *method == Method::POST {
                    Ok(GatewayForwardMode::OpenAiUsage)
                } else {
                    Err(GatewayRouteError::MethodNotAllowed)
                }
            } else if path == "/v1/models" {
                if *method == Method::GET {
                    Ok(GatewayForwardMode::OpenAiPassthrough)
                } else {
                    Err(GatewayRouteError::MethodNotAllowed)
                }
            } else {
                Err(GatewayRouteError::ProtocolMismatch)
            }
        }
        GatewayProtocol::AnthropicMessages => {
            if path == "/v1/messages" {
                if *method == Method::POST {
                    Ok(GatewayForwardMode::AnthropicUsage)
                } else {
                    Err(GatewayRouteError::MethodNotAllowed)
                }
            } else {
                Err(GatewayRouteError::ProtocolMismatch)
            }
        }
        GatewayProtocol::GeminiGenerateContent => {
            if gemini_path_matches(path) {
                if *method == Method::POST {
                    Ok(GatewayForwardMode::GeminiUsage)
                } else {
                    Err(GatewayRouteError::MethodNotAllowed)
                }
            } else {
                Err(GatewayRouteError::ProtocolMismatch)
            }
        }
    }
}

fn resolve_route(
    path: &str,
    method: &Method,
    settings: &AppSettings,
) -> Result<GatewayRoute, GatewayRouteError> {
    let (profile_id, native_path) = split_gateway_path(path)?;
    let profile = settings
        .gateway
        .profiles
        .iter()
        .find(|profile| profile.id == profile_id)
        .cloned()
        .ok_or(GatewayRouteError::ProfileNotFound)?;
    // Existing settings can predate profile validation or have been edited
    // outside the app. Re-check before forwarding caller credentials.
    if validate_profile(&profile).is_err() {
        return Err(GatewayRouteError::ProfileInvalid);
    }
    if !profile.enabled {
        return Err(GatewayRouteError::ProfileDisabled);
    }
    let mode = mode_for_profile(&profile, method, &native_path)?;
    Ok(GatewayRoute {
        profile,
        path: native_path,
        mode,
    })
}

fn sanitize_client_label(value: Option<&HeaderValue>) -> Result<Option<String>, GatewayRouteError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| GatewayRouteError::Malformed)?
        .trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > 80 || value.chars().any(|ch| ch.is_control()) {
        return Err(GatewayRouteError::Malformed);
    }
    Ok(Some(value.to_string()))
}

fn remove_client_label_header(headers: &mut hyper::HeaderMap) {
    headers.remove(HeaderName::from_static(CLIENT_LABEL_HEADER));
}

fn bearer_token(value: Option<&HeaderValue>) -> Option<&str> {
    value?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn query_key(raw_query: Option<&str>) -> Option<&str> {
    raw_query?
        .split('&')
        .find_map(|part| part.strip_prefix("key="))
        .filter(|value| !value.is_empty())
}

fn incoming_local_key<'a>(
    profile: &GatewayProfile,
    headers: &'a hyper::HeaderMap,
    raw_query: Option<&'a str>,
) -> Option<&'a str> {
    match profile.protocol {
        GatewayProtocol::OpenAiChatCompletions | GatewayProtocol::OpenAiResponses => {
            bearer_token(headers.get(AUTHORIZATION))
        }
        GatewayProtocol::AnthropicMessages => headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok()),
        GatewayProtocol::GeminiGenerateContent => headers
            .get("x-goog-api-key")
            .and_then(|value| value.to_str().ok())
            .or_else(|| query_key(raw_query)),
    }
}

fn strip_gateway_auth_headers(headers: &mut hyper::HeaderMap) {
    headers.remove(AUTHORIZATION);
    headers.remove("x-api-key");
    headers.remove("anthropic-api-key");
    headers.remove("x-goog-api-key");
}

fn inject_upstream_auth(
    profile: &GatewayProfile,
    headers: &mut hyper::HeaderMap,
    secret: &str,
) -> Result<(), GatewayRouteError> {
    strip_gateway_auth_headers(headers);
    let value = match profile.protocol {
        GatewayProtocol::OpenAiChatCompletions | GatewayProtocol::OpenAiResponses => {
            HeaderValue::from_str(&format!("Bearer {secret}"))
        }
        GatewayProtocol::AnthropicMessages | GatewayProtocol::GeminiGenerateContent => {
            HeaderValue::from_str(secret)
        }
    }
    .map_err(|_| GatewayRouteError::UpstreamKeyUnavailable)?;
    let name = match profile.protocol {
        GatewayProtocol::OpenAiChatCompletions | GatewayProtocol::OpenAiResponses => AUTHORIZATION,
        GatewayProtocol::AnthropicMessages => HeaderName::from_static("x-api-key"),
        GatewayProtocol::GeminiGenerateContent => HeaderName::from_static("x-goog-api-key"),
    };
    headers.insert(name, value);
    Ok(())
}

fn strip_query_key(raw_query: Option<&str>) -> Option<String> {
    let items: Vec<&str> = raw_query?
        .split('&')
        .filter(|item| !item.starts_with("key="))
        .collect();
    (!items.is_empty()).then(|| items.join("&"))
}

#[derive(Debug)]
enum GatewayBodyError<E> {
    TooLarge,
    Read(E),
}

async fn collect_gateway_body<B>(
    headers: &hyper::HeaderMap,
    mut body: B,
) -> Result<bytes::Bytes, GatewayBodyError<B::Error>>
where
    B: hyper::body::Body<Data = bytes::Bytes> + Unpin,
{
    if headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_GATEWAY_REQUEST_BODY_BYTES)
    {
        return Err(GatewayBodyError::TooLarge);
    }

    let mut bytes = BytesMut::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(GatewayBodyError::Read)?;
        if let Ok(data) = frame.into_data() {
            if bytes.len().saturating_add(data.len()) > MAX_GATEWAY_REQUEST_BODY_BYTES {
                return Err(GatewayBodyError::TooLarge);
            }
            bytes.extend_from_slice(&data);
        }
    }

    Ok(bytes.freeze())
}

async fn forward_gateway(
    mode: GatewayForwardMode,
    forwarder: &Arc<RequestForwarder>,
    state: &Arc<ProxyState>,
    method: Method,
    forward_path: &str,
    headers: hyper::HeaderMap,
    body: bytes::Bytes,
    context: RequestContext,
) -> HandlerResult {
    match mode {
        GatewayForwardMode::OpenAiUsage => {
            let forwarder = match get_openai_forwarder(state).await {
                Ok(forwarder) => forwarder,
                Err(response) => return Ok(*response),
            };
            forward_codex_with_usage(
                &forwarder,
                method,
                forward_path,
                headers,
                body,
                context,
                state,
            )
            .await
        }
        GatewayForwardMode::OpenAiPassthrough => {
            let forwarder = match get_openai_forwarder(state).await {
                Ok(forwarder) => forwarder,
                Err(response) => return Ok(*response),
            };
            forward_codex_passthrough(
                &forwarder,
                method,
                forward_path,
                headers,
                body,
                context,
                state,
            )
            .await
        }
        GatewayForwardMode::AnthropicUsage => {
            forward_claude_with_usage(
                forwarder,
                method,
                forward_path,
                headers,
                body,
                context,
                state,
            )
            .await
        }
        GatewayForwardMode::GeminiUsage => {
            let forwarder = match get_gemini_forwarder(state).await {
                Ok(forwarder) => forwarder,
                Err(response) => return Ok(*response),
            };
            forward_gemini_with_usage(
                &forwarder,
                method,
                forward_path,
                headers,
                body,
                context,
                state,
            )
            .await
        }
    }
}

fn should_failover(status: Option<u16>) -> bool {
    matches!(status, Some(401 | 402 | 403 | 429))
}

pub(crate) async fn handle_gateway_request(
    method: Method,
    raw_path: &str,
    raw_query: Option<&str>,
    req: Request<hyper::body::Incoming>,
    forwarder: Arc<RequestForwarder>,
    state: &Arc<ProxyState>,
) -> HandlerResult {
    let route = match resolve_route(
        raw_path,
        &method,
        &super::super::request_common::get_settings_snapshot(state).await,
    ) {
        Ok(route) => route,
        Err(error) => return Ok(error.response()),
    };
    let caller_label = match sanitize_client_label(req.headers().get(CLIENT_LABEL_HEADER)) {
        Ok(label) => label.or_else(|| {
            (!route.profile.client_label.trim().is_empty())
                .then(|| route.profile.client_label.trim().to_string())
        }),
        Err(error) => return Ok(error.response()),
    };
    let request_headers = req.headers().clone();
    let mut headers = request_headers.clone();
    let mut selected_upstream_key_id = None;
    let managed_key_remark = if route.profile.auth_mode == GatewayAuthMode::ManagedKeys {
        let local_key = incoming_local_key(&route.profile, &headers, raw_query)
            .and_then(|value| {
                route
                    .profile
                    .local_keys
                    .iter()
                    .find(|key| verify_local_key(value, key))
            })
            .ok_or(GatewayRouteError::LocalKeyUnauthorized);
        let local_key = match local_key {
            Ok(key) => key,
            Err(error) => return Ok(error.response()),
        };
        let upstream_key = match select_upstream_key(&route.profile) {
            Some(key) => key,
            None => return Ok(GatewayRouteError::UpstreamKeyUnavailable.response()),
        };
        if !is_expected_upstream_secret_ref(&route.profile.id, upstream_key) {
            return Ok(GatewayRouteError::UpstreamKeyUnavailable.response());
        }
        let secret = match load_upstream_secret(&upstream_key.secret_ref) {
            Ok(secret) => secret,
            Err(_) => return Ok(GatewayRouteError::UpstreamKeyUnavailable.response()),
        };
        if let Err(error) = inject_upstream_auth(&route.profile, &mut headers, &secret) {
            return Ok(error.response());
        }
        selected_upstream_key_id = Some(upstream_key.id.clone());
        (!local_key.remark.is_empty()).then(|| local_key.remark.clone())
    } else {
        None
    };
    let body = match collect_gateway_body(&headers, req.into_body()).await {
        Ok(body) => body,
        Err(GatewayBodyError::TooLarge) => {
            return Ok(json_error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "gateway_request_too_large",
                "Gateway request body exceeds the configured limit",
            ));
        }
        Err(GatewayBodyError::Read(error)) => return Err(error),
    };
    remove_client_label_header(&mut headers);

    let request_start_time_ms = chrono::Utc::now().timestamp_millis();
    let request_start_instant = std::time::Instant::now();
    let context = RequestContext {
        start_time: request_start_instant,
        start_time_ms: request_start_time_ms,
        client_tool: "api_gateway".to_string(),
        proxy_profile_id: None,
        client_detection_method: "gateway_profile".to_string(),
        request_base_url: Some(route.profile.base_url.clone()),
        target_base_url: Some(route.profile.base_url.clone()),
        ingress_kind: "gateway".to_string(),
        gateway_profile_id: Some(route.profile.id.clone()),
        gateway_caller_label: managed_key_remark.or(caller_label),
        usage_source: "provider".to_string(),
        gateway_request_id: Some(format!(
            "gw-{:x}-{:x}",
            request_start_time_ms,
            NEXT_GATEWAY_REQUEST.fetch_add(1, Ordering::Relaxed)
        )),
        ..Default::default()
    };
    let forward_path = if route.profile.auth_mode == GatewayAuthMode::ManagedKeys {
        append_query(&route.path, strip_query_key(raw_query).as_deref())
    } else {
        append_query(&route.path, raw_query)
    };

    let mut result = forward_gateway(
        route.mode,
        &forwarder,
        state,
        method.clone(),
        &forward_path,
        headers,
        body.clone(),
        context.clone(),
    )
    .await;
    let mut attempted_key_ids = selected_upstream_key_id
        .clone()
        .into_iter()
        .collect::<Vec<_>>();
    loop {
        let status = result
            .as_ref()
            .ok()
            .map(|response| response.status().as_u16());
        if let Some(key_id) = selected_upstream_key_id.as_deref() {
            report_upstream_result(&route.profile.id, key_id, status);
        }
        if route.profile.auth_mode != GatewayAuthMode::ManagedKeys
            || route.profile.dispatch_strategy != GatewayDispatchStrategy::PriorityFailover
            || !should_failover(status)
        {
            break;
        }
        let Some(upstream_key) = select_upstream_key_excluding(&route.profile, &attempted_key_ids)
        else {
            break;
        };
        if !is_expected_upstream_secret_ref(&route.profile.id, upstream_key) {
            break;
        }
        let Ok(secret) = load_upstream_secret(&upstream_key.secret_ref) else {
            break;
        };
        let mut retry_headers = request_headers.clone();
        remove_client_label_header(&mut retry_headers);
        if inject_upstream_auth(&route.profile, &mut retry_headers, &secret).is_err() {
            break;
        }
        selected_upstream_key_id = Some(upstream_key.id.clone());
        attempted_key_ids.push(upstream_key.id.clone());
        result = forward_gateway(
            route.mode,
            &forwarder,
            state,
            method.clone(),
            &forward_path,
            retry_headers,
            body.clone(),
            context.clone(),
        )
        .await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{GatewayProfile, GatewayProtocol, GatewaySettings};
    use futures::stream;
    use hyper::body::Frame;
    use hyper::header::HeaderValue;
    use std::convert::Infallible;

    fn profile(id: &str, protocol: GatewayProtocol) -> GatewayProfile {
        GatewayProfile {
            id: id.to_string(),
            name: id.to_string(),
            protocol,
            base_url: "https://example.test".to_string(),
            enabled: true,
            client_label: String::new(),
            auth_mode: crate::models::GatewayAuthMode::ClientPassthrough,
            dispatch_strategy: crate::models::GatewayDispatchStrategy::RoundRobin,
            upstream_keys: Vec::new(),
            local_keys: Vec::new(),
        }
    }

    fn settings(profile: GatewayProfile) -> AppSettings {
        AppSettings {
            gateway: GatewaySettings {
                profiles: vec![profile],
            },
            ..AppSettings::default()
        }
    }

    #[test]
    fn matches_native_paths_without_protocol_conversion() {
        let cases = [
            (
                GatewayProtocol::OpenAiChatCompletions,
                Method::POST,
                "/gateway/chat/v1/chat/completions",
                GatewayForwardMode::OpenAiUsage,
            ),
            (
                GatewayProtocol::OpenAiResponses,
                Method::POST,
                "/gateway/responses/v1/responses",
                GatewayForwardMode::OpenAiUsage,
            ),
            (
                GatewayProtocol::AnthropicMessages,
                Method::POST,
                "/gateway/anthropic/v1/messages",
                GatewayForwardMode::AnthropicUsage,
            ),
            (
                GatewayProtocol::GeminiGenerateContent,
                Method::POST,
                "/gateway/gemini/v1beta/models/deepseek-v4-flash:generateContent",
                GatewayForwardMode::GeminiUsage,
            ),
        ];
        for (protocol, method, path, expected) in cases {
            let id = path.split('/').nth(2).unwrap();
            let route = resolve_route(path, &method, &settings(profile(id, protocol))).unwrap();
            assert_eq!(route.mode, expected);
        }
    }

    #[test]
    fn rejects_disabled_missing_and_mismatched_routes() {
        let mut disabled = profile("disabled", GatewayProtocol::OpenAiResponses);
        disabled.enabled = false;
        assert_eq!(
            resolve_route(
                "/gateway/disabled/v1/responses",
                &Method::POST,
                &settings(disabled)
            ),
            Err(GatewayRouteError::ProfileDisabled)
        );
        assert_eq!(
            resolve_route(
                "/gateway/missing/v1/responses",
                &Method::POST,
                &settings(profile("other", GatewayProtocol::OpenAiResponses))
            ),
            Err(GatewayRouteError::ProfileNotFound)
        );
        assert_eq!(
            resolve_route(
                "/gateway/other/v1/chat/completions",
                &Method::POST,
                &settings(profile("other", GatewayProtocol::OpenAiResponses))
            ),
            Err(GatewayRouteError::ProtocolMismatch)
        );
    }

    #[test]
    fn rejects_invalid_profiles_loaded_from_existing_settings() {
        let mut invalid = profile("invalid", GatewayProtocol::OpenAiResponses);
        invalid.base_url = "http://127.0.0.1".to_string();
        assert_eq!(
            resolve_route(
                "/gateway/invalid/v1/responses",
                &Method::POST,
                &settings(invalid)
            ),
            Err(GatewayRouteError::ProfileInvalid)
        );
    }

    #[test]
    fn rejects_traversal_and_wrong_method() {
        assert_eq!(
            resolve_route(
                "/gateway/../v1/messages",
                &Method::POST,
                &settings(profile("..", GatewayProtocol::AnthropicMessages))
            ),
            Err(GatewayRouteError::Malformed)
        );
        assert_eq!(
            resolve_route(
                "/gateway/anthropic/v1/messages",
                &Method::GET,
                &settings(profile("anthropic", GatewayProtocol::AnthropicMessages))
            ),
            Err(GatewayRouteError::MethodNotAllowed)
        );
    }

    #[test]
    fn client_label_is_bounded_and_header_is_not_forwarded() {
        let value = HeaderValue::from_static("Cursor");
        assert_eq!(
            sanitize_client_label(Some(&value)).unwrap().as_deref(),
            Some("Cursor")
        );
        let long = HeaderValue::from_str(&"x".repeat(81)).unwrap();
        assert_eq!(
            sanitize_client_label(Some(&long)),
            Err(GatewayRouteError::Malformed)
        );
        let mut headers = hyper::HeaderMap::new();
        headers.insert(CLIENT_LABEL_HEADER, value);
        remove_client_label_header(&mut headers);
        assert!(headers.get(CLIENT_LABEL_HEADER).is_none());
    }

    #[tokio::test]
    async fn rejects_a_body_larger_than_the_declared_content_length_limit() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            CONTENT_LENGTH,
            HeaderValue::from_str(&(MAX_GATEWAY_REQUEST_BODY_BYTES + 1).to_string()).unwrap(),
        );

        let result =
            collect_gateway_body(&headers, http_body_util::Full::new(bytes::Bytes::new())).await;
        assert!(matches!(result, Err(GatewayBodyError::TooLarge)));
    }

    #[tokio::test]
    async fn rejects_chunked_body_that_exceeds_the_limit() {
        let body = http_body_util::StreamBody::new(stream::iter(vec![
            Ok::<_, Infallible>(Frame::data(bytes::Bytes::from(
                vec![0; MAX_GATEWAY_REQUEST_BODY_BYTES],
            ))),
            Ok(Frame::data(bytes::Bytes::from_static(&[1]))),
        ]));

        let result = collect_gateway_body(&hyper::HeaderMap::new(), body).await;
        assert!(matches!(result, Err(GatewayBodyError::TooLarge)));
    }

    #[tokio::test]
    async fn accepts_small_body_and_preserves_its_content() {
        let body = http_body_util::Full::new(bytes::Bytes::from_static(b"small request"));

        let bytes = collect_gateway_body(&hyper::HeaderMap::new(), body)
            .await
            .expect("small body should be accepted");
        assert_eq!(bytes.as_ref(), b"small request");
    }
}
