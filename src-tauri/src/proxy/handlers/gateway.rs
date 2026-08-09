//! Native-protocol local API gateway routing.
//!
//! A gateway profile is deliberately a route, not an authentication layer:
//! the client keeps its upstream credentials and this handler only selects the
//! configured upstream and forwards the request using the matching adapter.

use super::super::forwarder::{ForwardResult, RequestForwarder};
use super::super::gemini_forwarder::GeminiForwardResult;
use super::super::openai_forwarder::OpenAiForwardResult;
use super::super::request_body::ForwardRequestBody;
use super::super::request_common::{
    append_query, full, get_gemini_forwarder, get_openai_forwarder, json_error_response, BoxBody,
    HandlerResult,
};
use super::super::types::{ProxyState, RequestContext};
use crate::gateway::{
    report_upstream_outcome, select_upstream_key, validate_profile, UpstreamOutcome,
};
use crate::models::{AppSettings, GatewayAuthMode, GatewayProfile, GatewayProtocol};
use bytes::{Buf, Bytes, BytesMut};
use futures::TryStreamExt;
use http_body_util::BodyDataStream;
use hyper::{
    body::Incoming,
    header::{
        HeaderName, HeaderValue, AUTHORIZATION, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, UPGRADE,
    },
    Method, Request, StatusCode,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const GATEWAY_PREFIX: &str = "/gateway/";
const CLIENT_LABEL_HEADER: &str = "x-usagemeter-client";
static NEXT_GATEWAY_REQUEST: AtomicU64 = AtomicU64::new(1);

/// Maximum request body size accepted for protocol validation. Bodies larger
/// than this are forwarded unchanged without field validation so large uploads
/// keep their streaming passthrough behaviour.
const MAX_VALIDATION_BODY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GatewayForwardMode {
    OpenAiUsage,
    OpenAiPassthrough,
    AnthropicUsage,
    AnthropicPassthrough,
    GeminiUsage,
    GeminiPassthrough,
}

impl GatewayForwardMode {
    fn captures_usage(self) -> bool {
        matches!(
            self,
            Self::OpenAiUsage | Self::AnthropicUsage | Self::GeminiUsage
        )
    }
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
    MethodNotAllowed,
    LocalKeyUnauthorized,
    UpstreamKeyUnavailable,
    ProtocolMismatch,
}

impl GatewayRouteError {
    fn response(self) -> hyper::Response<super::super::request_common::BoxBody> {
        // Unified error responses to avoid leaking gateway configuration details.
        // Specific error variants are logged internally but not exposed to clients.
        let (status, error_type, message) = match self {
            Self::Malformed | Self::ProfileNotFound | Self::ProfileDisabled => (
                StatusCode::NOT_FOUND,
                "gateway_not_found",
                "Gateway endpoint not found",
            ),
            Self::ProfileInvalid => (
                StatusCode::FORBIDDEN,
                "gateway_forbidden",
                "Gateway request forbidden",
            ),
            Self::MethodNotAllowed => (
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "HTTP method not allowed",
            ),
            Self::LocalKeyUnauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Valid authentication required",
            ),
            Self::UpstreamKeyUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "service_unavailable",
                "Service temporarily unavailable",
            ),
            Self::ProtocolMismatch => (
                StatusCode::BAD_REQUEST,
                "protocol_mismatch",
                "Request body or content type does not match the gateway profile protocol",
            ),
        };

        // Log the actual error for debugging
        if !matches!(self, Self::LocalKeyUnauthorized) {
            log::debug!("Gateway route error: {:?}", self);
        }

        json_error_response(status, error_type, message)
    }
}

pub(crate) fn is_gateway_path(path: &str) -> bool {
    path == "/gateway" || path.starts_with(GATEWAY_PREFIX)
}

/// Whether the HTTP method is expected to carry a request body.
fn has_request_body(method: &Method) -> bool {
    matches!(*method, Method::POST | Method::PUT | Method::PATCH)
}

/// Validates that a gateway request matches the profile's native protocol:
/// the request-side Content-Type must not be `text/event-stream` (SSE is only
/// valid on the response side), JSON bodies must declare `application/json`,
/// and the body must contain the key fields expected by the profile protocol.
///
/// The check is deliberately shallow (field-name presence only, no deep schema
/// validation). Passthrough modes keep their existing transparent behaviour.
fn validate_gateway_request(
    mode: GatewayForwardMode,
    protocol: GatewayProtocol,
    method: &Method,
    headers: &hyper::HeaderMap,
    body: &[u8],
) -> Result<(), GatewayRouteError> {
    // Request-side `text/event-stream` is never valid: streaming responses are
    // produced by the upstream, not requested as an upload content type.
    if let Some(value) = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()) {
        let media_type = value
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if media_type == "text/event-stream" {
            return Err(GatewayRouteError::ProtocolMismatch);
        }
    }

    // Requests without a body (e.g. GET /v1/models) carry nothing to validate.
    if !has_request_body(method) {
        return Ok(());
    }

    // Passthrough modes forward arbitrary endpoints (uploads, embeddings,
    // non-standard paths) and must keep their existing transparent behaviour.
    if !mode.captures_usage() {
        return Ok(());
    }

    // Usage modes speak JSON: a declared non-JSON content type is a protocol
    // mismatch. A missing content type is tolerated and falls back to the body
    // field check below (some clients omit the header).
    if let Some(value) = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()) {
        let media_type = value
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if media_type != "application/json" {
            return Err(GatewayRouteError::ProtocolMismatch);
        }
    }

    validate_body_fields(protocol, body)
}

/// Shallow key-field presence check for the profile's native protocol.
fn validate_body_fields(protocol: GatewayProtocol, body: &[u8]) -> Result<(), GatewayRouteError> {
    if body.is_empty() {
        return Ok(());
    }
    let json: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| GatewayRouteError::ProtocolMismatch)?;
    let ok = match protocol {
        GatewayProtocol::AnthropicMessages => json.get("messages").is_some(),
        GatewayProtocol::OpenAiChatCompletions => {
            json.get("model").is_some()
                && (json.get("messages").is_some() || json.get("stream").is_some())
        }
        GatewayProtocol::OpenAiResponses => {
            json.get("input").is_some() || json.get("model").is_some()
        }
        GatewayProtocol::GeminiGenerateContent => json.get("contents").is_some(),
    };
    if ok {
        Ok(())
    } else {
        Err(GatewayRouteError::ProtocolMismatch)
    }
}

/// A gateway request body that has either been buffered and validated, or is
/// left as the incoming streaming body for passthrough/oversized requests.
enum ValidatedBody {
    Buffered(Bytes),
    Streaming(Incoming),
}

/// Reads the request body when it can be validated, checks it against the
/// profile protocol, and returns a body ready for forwarding. Oversized bodies
/// (larger than `MAX_VALIDATION_BODY_BYTES`) or chunked bodies without a
/// `Content-Length` are forwarded unchanged with the original streaming body so
/// large uploads keep their behaviour.
async fn read_and_validate_gateway_body(
    mode: GatewayForwardMode,
    protocol: GatewayProtocol,
    method: &Method,
    headers: &hyper::HeaderMap,
    body: Incoming,
) -> Result<ValidatedBody, GatewayRouteError> {
    // Passthrough modes forward arbitrary endpoints (uploads, embeddings,
    // non-standard paths). Only the cheap header checks apply; the body keeps
    // streaming so the transparent behaviour is unchanged.
    if !mode.captures_usage() {
        validate_gateway_request(mode, protocol, method, headers, &[])?;
        return Ok(ValidatedBody::Streaming(body));
    }

    let content_length = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    match content_length {
        Some(len) if len <= MAX_VALIDATION_BODY_BYTES as u64 => {
            let bytes = read_body_limited(body, len as usize).await?;
            validate_gateway_request(mode, protocol, method, headers, &bytes)?;
            Ok(ValidatedBody::Buffered(bytes))
        }
        _ => {
            // Missing or oversized Content-Length: keep the streaming body and
            // only apply the cheap header-based checks.
            validate_gateway_request(mode, protocol, method, headers, &[])?;
            Ok(ValidatedBody::Streaming(body))
        }
    }
}

async fn read_body_limited<B>(body: B, limit: usize) -> Result<Bytes, GatewayRouteError>
where
    B: hyper::body::Body + Unpin,
    B::Data: bytes::Buf,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let mut bytes = BytesMut::new();
    let mut stream = BodyDataStream::new(body);
    while let Some(chunk) = stream
        .try_next()
        .await
        .map_err(|_| GatewayRouteError::ProtocolMismatch)?
    {
        let chunk_len = chunk.remaining();
        if bytes.len().saturating_add(chunk_len) > limit {
            return Err(GatewayRouteError::ProtocolMismatch);
        }
        bytes.extend_from_slice(chunk.chunk());
    }
    // EOF 时实际读取字节数必须等于声明的 Content-Length；若客户端提前断开
    // （CL 大但实际字节少），直接转发会带着原 CL 头让上游挂起至超时，
    // 因此这里显式拒绝。
    if bytes.len() != limit {
        return Err(GatewayRouteError::ProtocolMismatch);
    }
    Ok(bytes.freeze())
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
    if matches!(*method, Method::CONNECT | Method::TRACE) {
        return Err(GatewayRouteError::MethodNotAllowed);
    }

    match profile.protocol {
        GatewayProtocol::OpenAiChatCompletions
            if *method == Method::POST && path == "/v1/chat/completions" =>
        {
            Ok(GatewayForwardMode::OpenAiUsage)
        }
        GatewayProtocol::OpenAiResponses if *method == Method::POST && path == "/v1/responses" => {
            Ok(GatewayForwardMode::OpenAiUsage)
        }
        GatewayProtocol::OpenAiChatCompletions | GatewayProtocol::OpenAiResponses => {
            Ok(GatewayForwardMode::OpenAiPassthrough)
        }
        GatewayProtocol::AnthropicMessages if *method == Method::POST && path == "/v1/messages" => {
            Ok(GatewayForwardMode::AnthropicUsage)
        }
        GatewayProtocol::AnthropicMessages => Ok(GatewayForwardMode::AnthropicPassthrough),
        GatewayProtocol::GeminiGenerateContent => {
            if *method == Method::POST && gemini_path_matches(path) {
                Ok(GatewayForwardMode::GeminiUsage)
            } else {
                Ok(GatewayForwardMode::GeminiPassthrough)
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
    // Reject control characters and characters that could break log parsing or inject commands
    if value.chars().count() > 80
        || value.chars().any(|ch| {
            ch.is_control()
                || matches!(ch, '\n' | '\r' | '\t' | '"' | '\'' | '<' | '>' | '\\' | '`')
        })
    {
        return Err(GatewayRouteError::Malformed);
    }
    Ok(Some(value.to_string()))
}

fn remove_client_label_header(headers: &mut hyper::HeaderMap) {
    headers.remove(HeaderName::from_static(CLIENT_LABEL_HEADER));
}

fn is_upgrade_request(headers: &hyper::HeaderMap) -> bool {
    headers.contains_key(UPGRADE)
        || headers
            .get(CONNECTION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(',')
                    .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
            })
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
    // Delegate to the single source of truth shared with the connectivity
    // probe so the forwarder and the test path can never diverge.
    let (name, value) = crate::gateway::upstream_auth_header(profile.protocol, secret);
    let header_name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| GatewayRouteError::UpstreamKeyUnavailable)?;
    let header_value =
        HeaderValue::from_str(&value).map_err(|_| GatewayRouteError::UpstreamKeyUnavailable)?;
    headers.insert(header_name, header_value);
    Ok(())
}

fn strip_query_key(raw_query: Option<&str>) -> Option<String> {
    let items: Vec<&str> = raw_query?
        .split('&')
        .filter(|item| !item.starts_with("key="))
        .collect();
    (!items.is_empty()).then(|| items.join("&"))
}

fn forward_query(profile: &GatewayProfile, raw_query: Option<&str>) -> Option<String> {
    if profile.auth_mode == GatewayAuthMode::ManagedKeys
        && profile.protocol == GatewayProtocol::GeminiGenerateContent
    {
        strip_query_key(raw_query)
    } else {
        raw_query.map(str::to_string)
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GatewayTransportFailure {
    Connect,
    Timeout,
    Other,
}

impl GatewayTransportFailure {
    fn classify(error: &str) -> Self {
        let error = error.to_ascii_lowercase();
        if error.contains("timed out") || error.contains("timeout") {
            Self::Timeout
        } else if error.contains("failed to send")
            || error.contains("connect")
            || error.contains("connection")
            || error.contains("dns")
            || error.contains("tls")
        {
            Self::Connect
        } else {
            Self::Other
        }
    }

    fn retryable(self) -> bool {
        matches!(self, Self::Connect | Self::Timeout)
    }
}

enum GatewayAttemptResult {
    Response {
        status_code: u16,
        headers: Vec<(String, String)>,
        body: BoxBody,
        #[allow(dead_code)]
        retryable_before_response: bool,
    },
    TransportError(String),
}

impl GatewayAttemptResult {
    fn status_code(&self) -> Option<u16> {
        match self {
            Self::Response { status_code, .. } => Some(*status_code),
            Self::TransportError(_) => None,
        }
    }

    fn outcome(&self) -> UpstreamOutcome {
        match self.status_code() {
            Some(200..=399) => UpstreamOutcome::Success,
            Some(401) => UpstreamOutcome::InvalidCredentials,
            Some(402) => UpstreamOutcome::PaymentRequired,
            Some(403) => UpstreamOutcome::PermissionDenied,
            Some(429) => UpstreamOutcome::RateLimited,
            Some(400..=499) => UpstreamOutcome::ClientError,
            Some(500..=599) => UpstreamOutcome::ServerError,
            _ => UpstreamOutcome::TransportError,
        }
    }

    #[allow(dead_code)]
    fn should_failover(&self) -> bool {
        match self {
            Self::TransportError(error) => GatewayTransportFailure::classify(error).retryable(),
            Self::Response {
                status_code,
                retryable_before_response,
                ..
            } => should_failover_status(*status_code, *retryable_before_response),
        }
    }

    fn into_response(self) -> hyper::Response<BoxBody> {
        match self {
            Self::Response {
                status_code,
                headers,
                body,
                ..
            } => build_gateway_response(status_code, headers, body),
            Self::TransportError(error) => {
                json_error_response(StatusCode::BAD_GATEWAY, "gateway_upstream_error", &error)
            }
        }
    }
}

#[allow(dead_code)]
fn should_failover_status(status_code: u16, retryable_before_response: bool) -> bool {
    retryable_before_response && matches!(status_code, 401 | 402 | 429 | 500 | 502 | 503 | 504)
}

fn build_gateway_response(
    status_code: u16,
    headers: Vec<(String, String)>,
    body: BoxBody,
) -> hyper::Response<BoxBody> {
    let mut builder = hyper::Response::builder()
        .status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    builder.body(body).unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn forward_gateway_attempt(
    mode: GatewayForwardMode,
    forwarder: &Arc<RequestForwarder>,
    state: &Arc<ProxyState>,
    method: Method,
    forward_path: &str,
    headers: hyper::HeaderMap,
    body: ForwardRequestBody,
    context: RequestContext,
) -> GatewayAttemptResult {
    match mode {
        GatewayForwardMode::OpenAiUsage => {
            let forwarder = match get_openai_forwarder(state).await {
                Ok(forwarder) => forwarder,
                Err(response) => {
                    return GatewayAttemptResult::Response {
                        status_code: response.status().as_u16(),
                        headers: Vec::new(),
                        body: response.into_body(),
                        retryable_before_response: true,
                    }
                }
            };
            match forwarder
                .forward_with_headers(method, forward_path, headers, body, context)
                .await
            {
                Ok(OpenAiForwardResult::Streaming {
                    status_code,
                    headers,
                    body,
                }) => GatewayAttemptResult::Response {
                    status_code,
                    headers,
                    body,
                    retryable_before_response: false,
                },
                Ok(
                    OpenAiForwardResult::NonStreaming {
                        status_code,
                        headers,
                        content,
                    }
                    | OpenAiForwardResult::UpstreamError {
                        status_code,
                        headers,
                        content,
                    },
                ) => GatewayAttemptResult::Response {
                    status_code,
                    headers,
                    body: full(content),
                    retryable_before_response: true,
                },
                Err(error) => GatewayAttemptResult::TransportError(error),
            }
        }
        GatewayForwardMode::OpenAiPassthrough => {
            let forwarder = match get_openai_forwarder(state).await {
                Ok(forwarder) => forwarder,
                Err(response) => {
                    return GatewayAttemptResult::Response {
                        status_code: response.status().as_u16(),
                        headers: Vec::new(),
                        body: response.into_body(),
                        retryable_before_response: true,
                    }
                }
            };
            let (body, _) = body.into_parts();
            match forwarder
                .forward_passthrough_stream(method, forward_path, headers, body, context)
                .await
            {
                Ok(OpenAiForwardResult::Streaming {
                    status_code,
                    headers,
                    body,
                }) => GatewayAttemptResult::Response {
                    status_code,
                    headers,
                    body,
                    retryable_before_response: false,
                },
                Ok(
                    OpenAiForwardResult::NonStreaming {
                        status_code,
                        headers,
                        content,
                    }
                    | OpenAiForwardResult::UpstreamError {
                        status_code,
                        headers,
                        content,
                    },
                ) => GatewayAttemptResult::Response {
                    status_code,
                    headers,
                    body: full(content),
                    retryable_before_response: true,
                },
                Err(error) => GatewayAttemptResult::TransportError(error),
            }
        }
        GatewayForwardMode::AnthropicUsage | GatewayForwardMode::AnthropicPassthrough => {
            let result = if mode == GatewayForwardMode::AnthropicUsage {
                forwarder
                    .forward_with_usage(method, forward_path, body, context, headers)
                    .await
            } else {
                let (body, _) = body.into_parts();
                forwarder
                    .forward_passthrough_stream(method, forward_path, body, context, headers)
                    .await
            };
            match result {
                Ok(ForwardResult::Streaming {
                    status_code,
                    headers,
                    body,
                }) => GatewayAttemptResult::Response {
                    status_code,
                    headers,
                    body,
                    retryable_before_response: false,
                },
                Ok(ForwardResult::NonStreaming {
                    status_code,
                    headers,
                    content,
                }) => GatewayAttemptResult::Response {
                    status_code,
                    headers,
                    body: full(content),
                    retryable_before_response: true,
                },
                Err(error) => GatewayAttemptResult::TransportError(error),
            }
        }
        GatewayForwardMode::GeminiUsage | GatewayForwardMode::GeminiPassthrough => {
            let forwarder = match get_gemini_forwarder(state).await {
                Ok(forwarder) => forwarder,
                Err(response) => {
                    return GatewayAttemptResult::Response {
                        status_code: response.status().as_u16(),
                        headers: Vec::new(),
                        body: response.into_body(),
                        retryable_before_response: true,
                    }
                }
            };
            let result = if mode == GatewayForwardMode::GeminiUsage {
                forwarder
                    .forward_with_usage(method, forward_path, headers, body, context)
                    .await
            } else {
                let (body, _) = body.into_parts();
                forwarder
                    .forward_passthrough_stream(method, forward_path, headers, body, context)
                    .await
            };
            match result {
                Ok(GeminiForwardResult::Streaming {
                    status_code,
                    headers,
                    body,
                }) => GatewayAttemptResult::Response {
                    status_code,
                    headers,
                    body,
                    retryable_before_response: false,
                },
                Ok(
                    GeminiForwardResult::NonStreaming {
                        status_code,
                        headers,
                        content,
                    }
                    | GeminiForwardResult::UpstreamError {
                        status_code,
                        headers,
                        content,
                    },
                ) => GatewayAttemptResult::Response {
                    status_code,
                    headers,
                    body: full(content),
                    retryable_before_response: true,
                },
                Err(error) => GatewayAttemptResult::TransportError(error),
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn forward_gateway_passthrough_attempt(
    mode: GatewayForwardMode,
    forwarder: &Arc<RequestForwarder>,
    state: &Arc<ProxyState>,
    method: Method,
    forward_path: &str,
    headers: hyper::HeaderMap,
    body: reqwest::Body,
    context: RequestContext,
) -> GatewayAttemptResult {
    match mode {
        GatewayForwardMode::OpenAiPassthrough => {
            let forwarder = match get_openai_forwarder(state).await {
                Ok(forwarder) => forwarder,
                Err(response) => {
                    return GatewayAttemptResult::Response {
                        status_code: response.status().as_u16(),
                        headers: Vec::new(),
                        body: response.into_body(),
                        retryable_before_response: true,
                    }
                }
            };
            match forwarder
                .forward_passthrough_stream(method, forward_path, headers, body, context)
                .await
            {
                Ok(OpenAiForwardResult::Streaming {
                    status_code,
                    headers,
                    body,
                }) => GatewayAttemptResult::Response {
                    status_code,
                    headers,
                    body,
                    retryable_before_response: false,
                },
                Ok(
                    OpenAiForwardResult::NonStreaming {
                        status_code,
                        headers,
                        content,
                    }
                    | OpenAiForwardResult::UpstreamError {
                        status_code,
                        headers,
                        content,
                    },
                ) => GatewayAttemptResult::Response {
                    status_code,
                    headers,
                    body: full(content),
                    retryable_before_response: false,
                },
                Err(error) => GatewayAttemptResult::TransportError(error),
            }
        }
        GatewayForwardMode::AnthropicPassthrough => match forwarder
            .forward_passthrough_stream(method, forward_path, body, context, headers)
            .await
        {
            Ok(ForwardResult::Streaming {
                status_code,
                headers,
                body,
            }) => GatewayAttemptResult::Response {
                status_code,
                headers,
                body,
                retryable_before_response: false,
            },
            Ok(ForwardResult::NonStreaming {
                status_code,
                headers,
                content,
            }) => GatewayAttemptResult::Response {
                status_code,
                headers,
                body: full(content),
                retryable_before_response: false,
            },
            Err(error) => GatewayAttemptResult::TransportError(error),
        },
        GatewayForwardMode::GeminiPassthrough => {
            let forwarder = match get_gemini_forwarder(state).await {
                Ok(forwarder) => forwarder,
                Err(response) => {
                    return GatewayAttemptResult::Response {
                        status_code: response.status().as_u16(),
                        headers: Vec::new(),
                        body: response.into_body(),
                        retryable_before_response: true,
                    }
                }
            };
            match forwarder
                .forward_passthrough_stream(method, forward_path, headers, body, context)
                .await
            {
                Ok(GeminiForwardResult::Streaming {
                    status_code,
                    headers,
                    body,
                }) => GatewayAttemptResult::Response {
                    status_code,
                    headers,
                    body,
                    retryable_before_response: false,
                },
                Ok(
                    GeminiForwardResult::NonStreaming {
                        status_code,
                        headers,
                        content,
                    }
                    | GeminiForwardResult::UpstreamError {
                        status_code,
                        headers,
                        content,
                    },
                ) => GatewayAttemptResult::Response {
                    status_code,
                    headers,
                    body: full(content),
                    retryable_before_response: false,
                },
                Err(error) => GatewayAttemptResult::TransportError(error),
            }
        }
        _ => GatewayAttemptResult::TransportError(
            "Gateway passthrough mode classification failed".to_string(),
        ),
    }
}

async fn record_final_proxy_status(state: &Arc<ProxyState>, status_code: u16) {
    let mut status = state.status.write().await;
    if status_code < 400 {
        status.success_requests += 1;
    } else {
        status.failed_requests += 1;
    }
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
    if is_upgrade_request(req.headers()) {
        return Ok(GatewayRouteError::MethodNotAllowed.response());
    }
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
                route.profile.local_keys.iter().find(|key| {
                    match crate::gateway::verify_local_key_with_expiry(value, key) {
                        Ok(()) => true,
                        Err(crate::gateway::LocalKeyVerifyError::Expired) => {
                            log::debug!("Local key {} has expired", key.id);
                            false
                        }
                        Err(crate::gateway::LocalKeyVerifyError::QuotaExceeded) => {
                            log::debug!("Local key {} quota exceeded", key.id);
                            false
                        }
                        Err(_) => false,
                    }
                })
            })
            .ok_or(GatewayRouteError::LocalKeyUnauthorized);
        let local_key = match local_key {
            Ok(key) => key,
            Err(error) => return Ok(error.response()),
        };

        // Rate limiting check
        if let Err(rate_limit_error) = state.gateway_rate_limiter.check() {
            log::debug!(
                "Rate limit exceeded for local key {}: {:?}",
                local_key.id,
                rate_limit_error
            );

            // Audit log for rate limit
            crate::gateway::audit::log_audit(crate::gateway::audit::GatewayAuditEvent {
                timestamp_ms: chrono::Utc::now().timestamp_millis(),
                event_type: crate::gateway::audit::GatewayAuditEventType::RateLimitExceeded,
                profile_id: route.profile.id.clone(),
                actor: Some(local_key.id.clone()),
                details: serde_json::json!({
                    "limit_type": format!("{:?}", rate_limit_error),
                }),
                result: crate::gateway::audit::AuditResult::Success,
            });

            return Ok(json_error_response(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_exceeded",
                "Too many requests",
            ));
        }

        let upstream_key = match select_upstream_key(&route.profile) {
            Some(key) => key,
            None => return Ok(GatewayRouteError::UpstreamKeyUnavailable.response()),
        };
        let secret = upstream_key.secret.trim();
        if secret.is_empty() {
            return Ok(GatewayRouteError::UpstreamKeyUnavailable.response());
        }
        if let Err(error) = inject_upstream_auth(&route.profile, &mut headers, secret) {
            return Ok(error.response());
        }
        selected_upstream_key_id = Some(upstream_key.id.clone());
        (!local_key.remark.is_empty()).then(|| local_key.remark.clone())
    } else {
        None
    };
    remove_client_label_header(&mut headers);

    let request_start_time_ms = chrono::Utc::now().timestamp_millis();
    let request_start_instant = std::time::Instant::now();
    let mut context = RequestContext {
        start_time: request_start_instant,
        start_time_ms: request_start_time_ms,
        client_tool: "api_gateway".to_string(),
        proxy_profile_id: None,
        client_detection_method: "gateway_profile".to_string(),
        request_base_url: Some(route.profile.base_url.clone()),
        // Clients usually carry the protocol base path inside their request
        // path (e.g. `/v1/messages`); in that case a base URL that already
        // ends with `/v1` must be stripped to avoid `.../v1/v1/messages`.
        // When the client path has no base path, the base URL is kept so the
        // profile still supplies it.
        target_base_url: Some(crate::gateway::probe::forwarding_base_url(
            &route.profile.base_url,
            route.profile.protocol,
            &route.path,
        )),
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
    // In managed mode the local Gemini query key must not reach the upstream.
    // Client-passthrough profiles preserve the caller's query string verbatim.
    let query = forward_query(&route.profile, raw_query);
    let forward_path = append_query(&route.path, query.as_deref());

    // Validate the request body against the profile protocol before touching
    // the upstream: mismatched content types or body shapes get a clear 400
    // instead of being forwarded to an opaque upstream error.
    let body = match read_and_validate_gateway_body(
        route.mode,
        route.profile.protocol,
        &method,
        &headers,
        req.into_body(),
    )
    .await
    {
        Ok(body) => body,
        Err(error) => return Ok(error.response()),
    };

    let result = if route.mode.captures_usage() {
        let body = match body {
            ValidatedBody::Buffered(bytes) => {
                // Preserve the model/stream extraction that the streaming
                // observation used to provide for buffered (validated) bodies.
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    if context.model.is_none() {
                        context.model = json
                            .get("model")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                    }
                    context.stream = json
                        .get("stream")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(context.stream);
                }
                ForwardRequestBody::Buffered(bytes)
            }
            ValidatedBody::Streaming(incoming) => ForwardRequestBody::observed_stream(incoming),
        };
        forward_gateway_attempt(
            route.mode,
            &forwarder,
            state,
            method.clone(),
            &forward_path,
            headers,
            body,
            context.clone(),
        )
        .await
    } else {
        let body = match body {
            ValidatedBody::Buffered(bytes) => ForwardRequestBody::Buffered(bytes).into_parts().0,
            ValidatedBody::Streaming(incoming) => {
                ForwardRequestBody::passthrough_stream(incoming)
                    .into_parts()
                    .0
            }
        };
        forward_gateway_passthrough_attempt(
            route.mode,
            &forwarder,
            state,
            method.clone(),
            &forward_path,
            headers,
            body,
            context.clone(),
        )
        .await
    };
    if let Some(key_id) = selected_upstream_key_id.as_deref() {
        report_upstream_outcome(&route.profile.id, key_id, result.outcome());
    }

    let response = result.into_response();
    record_final_proxy_status(state, response.status().as_u16()).await;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{GatewayProfile, GatewayProtocol, GatewaySettings};
    use hyper::header::HeaderValue;

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
            upstream_models: Vec::new(),
        }
    }

    fn settings(profile: GatewayProfile) -> AppSettings {
        AppSettings {
            gateway: GatewaySettings {
                storage_version: 2,
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
    fn rejects_disabled_and_missing_routes() {
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
    }

    #[test]
    fn forwards_unknown_and_non_usage_endpoints_with_the_protocol_adapter() {
        let cases = [
            (
                GatewayProtocol::OpenAiChatCompletions,
                Method::GET,
                "/gateway/openai/v1/models",
                GatewayForwardMode::OpenAiPassthrough,
            ),
            (
                GatewayProtocol::OpenAiResponses,
                Method::GET,
                "/gateway/openai/dashboard/billing/credit_grants",
                GatewayForwardMode::OpenAiPassthrough,
            ),
            (
                GatewayProtocol::AnthropicMessages,
                Method::GET,
                "/gateway/anthropic/v1/models",
                GatewayForwardMode::AnthropicPassthrough,
            ),
            (
                GatewayProtocol::GeminiGenerateContent,
                Method::GET,
                "/gateway/gemini/v1beta/models",
                GatewayForwardMode::GeminiPassthrough,
            ),
            (
                GatewayProtocol::OpenAiResponses,
                Method::POST,
                "/gateway/openai/v1/chat/completions",
                GatewayForwardMode::OpenAiPassthrough,
            ),
        ];

        for (protocol, method, path, expected) in cases {
            let id = path.split('/').nth(2).unwrap();
            let route = resolve_route(path, &method, &settings(profile(id, protocol))).unwrap();
            assert_eq!(route.mode, expected, "{path}");
        }
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
    fn rejects_traversal_and_unsupported_methods() {
        assert_eq!(
            resolve_route(
                "/gateway/../v1/messages",
                &Method::POST,
                &settings(profile("..", GatewayProtocol::AnthropicMessages))
            ),
            Err(GatewayRouteError::Malformed)
        );
        for method in [Method::CONNECT, Method::TRACE] {
            assert_eq!(
                resolve_route(
                    "/gateway/anthropic/v1/messages",
                    &method,
                    &settings(profile("anthropic", GatewayProtocol::AnthropicMessages))
                ),
                Err(GatewayRouteError::MethodNotAllowed)
            );
        }
    }

    #[test]
    fn rejects_upgrade_requests_without_forwarding_them() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
        assert!(is_upgrade_request(&headers));

        let mut connection_only = hyper::HeaderMap::new();
        connection_only.insert(CONNECTION, HeaderValue::from_static("keep-alive, Upgrade"));
        assert!(is_upgrade_request(&connection_only));

        let ordinary = hyper::HeaderMap::new();
        assert!(!is_upgrade_request(&ordinary));
    }

    #[test]
    fn only_strips_gemini_query_key_for_managed_profiles() {
        let mut managed = profile("gemini", GatewayProtocol::GeminiGenerateContent);
        managed.auth_mode = GatewayAuthMode::ManagedKeys;
        assert_eq!(
            forward_query(&managed, Some("key=umg_local&alt=sse")),
            Some("alt=sse".to_string())
        );
        assert_eq!(forward_query(&managed, Some("key=umg_local")), None);

        let passthrough = profile("gemini", GatewayProtocol::GeminiGenerateContent);
        assert_eq!(
            forward_query(&passthrough, Some("key=upstream&alt=sse")),
            Some("key=upstream&alt=sse".to_string())
        );

        let mut managed_openai = profile("openai", GatewayProtocol::OpenAiResponses);
        managed_openai.auth_mode = GatewayAuthMode::ManagedKeys;
        assert_eq!(
            forward_query(&managed_openai, Some("key=filter-value")),
            Some("key=filter-value".to_string())
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

    #[test]
    fn transport_retry_policy_rejects_ambiguous_body_read_failures() {
        assert_eq!(
            GatewayTransportFailure::classify("Failed to send Codex request: connection refused"),
            GatewayTransportFailure::Connect
        );
        assert_eq!(
            GatewayTransportFailure::classify("Failed to send request: operation timed out"),
            GatewayTransportFailure::Timeout
        );
        assert_eq!(
            GatewayTransportFailure::classify("Failed to read response body: reset by peer"),
            GatewayTransportFailure::Other
        );
        assert!(GatewayTransportFailure::Connect.retryable());
        assert!(GatewayTransportFailure::Timeout.retryable());
        assert!(!GatewayTransportFailure::Other.retryable());
    }

    #[test]
    fn failover_status_policy_is_bounded_and_excludes_permission_denied() {
        for status in [401, 402, 429, 500, 502, 503, 504] {
            assert!(should_failover_status(status, true), "{status}");
        }
        for status in [200, 400, 403, 404, 422, 501] {
            assert!(!should_failover_status(status, true), "{status}");
        }
        assert!(!should_failover_status(503, false));
    }

    fn json_headers() -> hyper::HeaderMap {
        let mut headers = hyper::HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers
    }

    #[test]
    fn protocol_validation_accepts_matching_json_bodies() {
        let mode = GatewayForwardMode::OpenAiUsage;
        let method = Method::POST;
        let headers = json_headers();

        // OpenAI chat: model + messages
        assert!(validate_gateway_request(
            mode,
            GatewayProtocol::OpenAiChatCompletions,
            &method,
            &headers,
            br#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#
        )
        .is_ok());

        // OpenAI chat: model + stream
        assert!(validate_gateway_request(
            mode,
            GatewayProtocol::OpenAiChatCompletions,
            &method,
            &headers,
            br#"{"model":"gpt-4o","stream":true}"#
        )
        .is_ok());

        // OpenAI responses: input or model
        assert!(validate_gateway_request(
            mode,
            GatewayProtocol::OpenAiResponses,
            &method,
            &headers,
            br#"{"input":"tell me a joke"}"#
        )
        .is_ok());
        assert!(validate_gateway_request(
            mode,
            GatewayProtocol::OpenAiResponses,
            &method,
            &headers,
            br#"{"model":"gpt-4o","input":"hi"}"#
        )
        .is_ok());

        // Anthropic messages: messages present
        assert!(validate_gateway_request(
            mode,
            GatewayProtocol::AnthropicMessages,
            &method,
            &headers,
            br#"{"model":"claude","messages":[{"role":"user","content":"hi"}]}"#
        )
        .is_ok());

        // Gemini: contents present
        assert!(validate_gateway_request(
            mode,
            GatewayProtocol::GeminiGenerateContent,
            &method,
            &headers,
            br#"{"contents":[{"parts":[{"text":"hi"}]}]}"#
        )
        .is_ok());
    }

    #[test]
    fn protocol_validation_rejects_mismatched_body_shapes() {
        let mode = GatewayForwardMode::OpenAiUsage;
        let method = Method::POST;
        let headers = json_headers();

        // OpenAI Responses body (input) sent to Anthropic profile: no messages
        // field, which Anthropic Messages requires.
        assert_eq!(
            validate_gateway_request(
                mode,
                GatewayProtocol::AnthropicMessages,
                &method,
                &headers,
                br#"{"input":"tell me a joke"}"#
            ),
            Err(GatewayRouteError::ProtocolMismatch)
        );

        // OpenAI chat: model missing.
        assert_eq!(
            validate_gateway_request(
                mode,
                GatewayProtocol::OpenAiChatCompletions,
                &method,
                &headers,
                br#"{"messages":[{"role":"user","content":"hi"}]}"#
            ),
            Err(GatewayRouteError::ProtocolMismatch)
        );

        // OpenAI chat: neither messages nor stream.
        assert_eq!(
            validate_gateway_request(
                mode,
                GatewayProtocol::OpenAiChatCompletions,
                &method,
                &headers,
                br#"{"model":"gpt-4o","temperature":0.5}"#
            ),
            Err(GatewayRouteError::ProtocolMismatch)
        );

        // OpenAI responses: neither input nor model.
        assert_eq!(
            validate_gateway_request(
                mode,
                GatewayProtocol::OpenAiResponses,
                &method,
                &headers,
                br#"{"temperature":0.5}"#
            ),
            Err(GatewayRouteError::ProtocolMismatch)
        );

        // Gemini: contents missing.
        assert_eq!(
            validate_gateway_request(
                mode,
                GatewayProtocol::GeminiGenerateContent,
                &method,
                &headers,
                br#"{"model":"gemini-2.0-flash"}"#
            ),
            Err(GatewayRouteError::ProtocolMismatch)
        );

        // Not valid JSON at all.
        assert_eq!(
            validate_gateway_request(
                mode,
                GatewayProtocol::GeminiGenerateContent,
                &method,
                &headers,
                b"this is not json"
            ),
            Err(GatewayRouteError::ProtocolMismatch)
        );
    }

    #[test]
    fn protocol_validation_rejects_request_side_event_stream_content_type() {
        let mode = GatewayForwardMode::OpenAiUsage;
        let method = Method::POST;
        let mut headers = hyper::HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
        assert_eq!(
            validate_gateway_request(
                mode,
                GatewayProtocol::OpenAiChatCompletions,
                &method,
                &headers,
                br#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#
            ),
            Err(GatewayRouteError::ProtocolMismatch)
        );
    }

    #[test]
    fn protocol_validation_rejects_non_json_content_type_for_usage_modes() {
        let mode = GatewayForwardMode::OpenAiUsage;
        let method = Method::POST;
        let mut headers = hyper::HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        assert_eq!(
            validate_gateway_request(
                mode,
                GatewayProtocol::OpenAiChatCompletions,
                &method,
                &headers,
                br#"{"model":"gpt-4o","messages":[]}"#
            ),
            Err(GatewayRouteError::ProtocolMismatch)
        );

        // application/json with a charset suffix is accepted.
        let mut json_charset = hyper::HeaderMap::new();
        json_charset.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        assert!(validate_gateway_request(
            mode,
            GatewayProtocol::OpenAiChatCompletions,
            &method,
            &json_charset,
            br#"{"model":"gpt-4o","messages":[]}"#
        )
        .is_ok());
    }

    #[test]
    fn protocol_validation_ignores_body_for_get_and_passthrough_modes() {
        let method_get = Method::GET;
        assert!(validate_gateway_request(
            GatewayForwardMode::OpenAiUsage,
            GatewayProtocol::OpenAiChatCompletions,
            &method_get,
            &hyper::HeaderMap::new(),
            b""
        )
        .is_ok());

        // Passthrough mode keeps its transparent behaviour even for non-JSON
        // content types and bodies that would fail usage-mode validation.
        let mut headers = hyper::HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        assert!(validate_gateway_request(
            GatewayForwardMode::OpenAiPassthrough,
            GatewayProtocol::OpenAiResponses,
            &Method::POST,
            &headers,
            b"anything"
        )
        .is_ok());
    }

    #[test]
    fn protocol_mismatch_error_maps_to_bad_request_json() {
        let response = GatewayRouteError::ProtocolMismatch.response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response
                .headers()
                .get(hyper::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("application/json")
        );
    }

    #[test]
    fn oversized_body_skips_field_validation_and_keeps_streaming() {
        // A body larger than the validation cap is forwarded without buffering;
        // the header checks still apply.
        let mode = GatewayForwardMode::OpenAiUsage;
        let method = Method::POST;
        let mut headers = hyper::HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            CONTENT_LENGTH,
            HeaderValue::from((MAX_VALIDATION_BODY_BYTES + 1) as u64),
        );
        // Since the body is not read, only the header checks run and a plain
        // non-JSON body cannot fail the field check. We just assert the header
        // path accepts oversized bodies for matching content types.
        assert!(validate_gateway_request(
            mode,
            GatewayProtocol::OpenAiChatCompletions,
            &method,
            &headers,
            b""
        )
        .is_ok());
    }

    #[tokio::test]
    async fn read_body_limited_rejects_short_body_against_content_length() {
        // Client declares CL=8 but sends only 4 bytes (premature EOF): the
        // buffered forward would otherwise carry the original CL header and
        // hang the upstream. It must be rejected instead.
        let body = http_body_util::Full::new(bytes::Bytes::from_static(b"{\"a\":")); // 5 bytes, declared 8
        assert!(matches!(
            read_body_limited(body, 8).await,
            Err(GatewayRouteError::ProtocolMismatch)
        ));
    }

    #[tokio::test]
    async fn read_body_limited_accepts_exact_content_length() {
        let body = http_body_util::Full::new(bytes::Bytes::from_static(b"{\"a\":1}")); // 7 bytes
        let bytes = read_body_limited(body, 7).await.unwrap();
        assert_eq!(&bytes[..], b"{\"a\":1}");
    }
}
