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
use crate::gateway::validate_profile;
use crate::models::{AppSettings, GatewayProfile, GatewayProtocol};
use bytes::BytesMut;
use http_body_util::BodyExt;
use hyper::{
    header::{HeaderName, HeaderValue, CONTENT_LENGTH},
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
    let mut headers = req.headers().clone();
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
        gateway_caller_label: caller_label,
        usage_source: "provider".to_string(),
        gateway_request_id: Some(format!(
            "gw-{:x}-{:x}",
            request_start_time_ms,
            NEXT_GATEWAY_REQUEST.fetch_add(1, Ordering::Relaxed)
        )),
        ..Default::default()
    };
    let forward_path = append_query(&route.path, raw_query);

    match route.mode {
        GatewayForwardMode::OpenAiUsage => {
            let forwarder = match get_openai_forwarder(state).await {
                Ok(forwarder) => forwarder,
                Err(response) => return Ok(*response),
            };
            forward_codex_with_usage(
                &forwarder,
                method,
                &forward_path,
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
                &forward_path,
                headers,
                body,
                context,
                state,
            )
            .await
        }
        GatewayForwardMode::AnthropicUsage => {
            forward_claude_with_usage(
                &forwarder,
                method,
                &forward_path,
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
                &forward_path,
                headers,
                body,
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
