use super::forwarder::{ForwardResult, RequestForwarder};
use super::gemini_forwarder::{GeminiForwardResult, GeminiForwarder};
use super::openai_forwarder::{OpenAiForwardResult, OpenAiForwarder};
use super::request_common::{full, json_error_response, BoxBody, HandlerResult};
use super::types::{ProxyState, RequestContext};
use hyper::{Method, Response, StatusCode};
use std::sync::Arc;

async fn record_proxy_status(state: &Arc<ProxyState>, status_code: u16) {
    let mut status = state.status.write().await;
    if status_code < 400 {
        status.success_requests += 1;
    } else {
        status.failed_requests += 1;
    }
}

async fn record_proxy_failure(state: &Arc<ProxyState>) {
    let mut status = state.status.write().await;
    status.failed_requests += 1;
}

fn build_upstream_response(
    status_code: u16,
    headers: Vec<(String, String)>,
    body: BoxBody,
) -> Response<BoxBody> {
    let mut builder = Response::builder()
        .status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    builder.body(body).unwrap()
}

pub(crate) async fn forward_codex_passthrough(
    forwarder: &OpenAiForwarder,
    method: Method,
    forward_path: &str,
    request_headers: hyper::HeaderMap,
    body_bytes: bytes::Bytes,
    context: RequestContext,
    state: &Arc<ProxyState>,
) -> HandlerResult {
    match forwarder
        .forward_passthrough(method, forward_path, request_headers, body_bytes, context)
        .await
    {
        Ok(OpenAiForwardResult::Streaming {
            status_code,
            headers,
            body,
        }) => {
            record_proxy_status(state, status_code).await;
            Ok(build_upstream_response(status_code, headers, body))
        }
        Ok(OpenAiForwardResult::NonStreaming {
            status_code,
            headers,
            content,
        }) => {
            record_proxy_status(state, status_code).await;
            Ok(build_upstream_response(status_code, headers, full(content)))
        }
        Ok(OpenAiForwardResult::UpstreamError {
            status_code,
            headers,
            content,
        }) => {
            record_proxy_failure(state).await;
            Ok(build_upstream_response(status_code, headers, full(content)))
        }
        Err(e) => {
            record_proxy_failure(state).await;
            Ok(json_error_response(
                StatusCode::BAD_GATEWAY,
                "proxy_error",
                &e,
            ))
        }
    }
}

pub(crate) async fn forward_codex_with_usage(
    forwarder: &OpenAiForwarder,
    method: Method,
    forward_path: &str,
    request_headers: hyper::HeaderMap,
    body_bytes: bytes::Bytes,
    context: RequestContext,
    state: &Arc<ProxyState>,
) -> HandlerResult {
    match forwarder
        .forward_with_headers(method, forward_path, request_headers, body_bytes, context)
        .await
    {
        Ok(result) => match result {
            OpenAiForwardResult::Streaming {
                status_code,
                headers,
                body,
            } => {
                record_proxy_status(state, status_code).await;
                Ok(build_upstream_response(status_code, headers, body))
            }
            OpenAiForwardResult::NonStreaming {
                status_code,
                headers,
                content,
            } => {
                record_proxy_status(state, status_code).await;
                Ok(build_upstream_response(status_code, headers, full(content)))
            }
            OpenAiForwardResult::UpstreamError {
                status_code,
                headers,
                content,
            } => {
                record_proxy_failure(state).await;
                Ok(build_upstream_response(status_code, headers, full(content)))
            }
        },
        Err(e) => {
            record_proxy_failure(state).await;
            Ok(json_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "proxy_error",
                &e,
            ))
        }
    }
}

pub(crate) async fn forward_gemini_passthrough(
    forwarder: &GeminiForwarder,
    method: Method,
    forward_path: &str,
    request_headers: hyper::HeaderMap,
    body_bytes: bytes::Bytes,
    context: RequestContext,
    state: &Arc<ProxyState>,
) -> HandlerResult {
    match forwarder
        .forward_passthrough(method, forward_path, request_headers, body_bytes, context)
        .await
    {
        Ok(GeminiForwardResult::Streaming {
            status_code,
            headers,
            body,
        }) => {
            record_proxy_status(state, status_code).await;
            Ok(build_upstream_response(status_code, headers, body))
        }
        Ok(GeminiForwardResult::NonStreaming {
            status_code,
            headers,
            content,
        }) => {
            record_proxy_status(state, status_code).await;
            Ok(build_upstream_response(status_code, headers, full(content)))
        }
        Ok(GeminiForwardResult::UpstreamError {
            status_code,
            headers,
            content,
        }) => {
            record_proxy_failure(state).await;
            Ok(build_upstream_response(status_code, headers, full(content)))
        }
        Err(e) => {
            record_proxy_failure(state).await;
            Ok(json_error_response(
                StatusCode::BAD_GATEWAY,
                "proxy_error",
                &e,
            ))
        }
    }
}

pub(crate) async fn forward_gemini_with_usage(
    forwarder: &GeminiForwarder,
    method: Method,
    forward_path: &str,
    request_headers: hyper::HeaderMap,
    body_bytes: bytes::Bytes,
    context: RequestContext,
    state: &Arc<ProxyState>,
) -> HandlerResult {
    match forwarder
        .forward_with_usage(method, forward_path, request_headers, body_bytes, context)
        .await
    {
        Ok(GeminiForwardResult::Streaming {
            status_code,
            headers,
            body,
        }) => {
            record_proxy_status(state, status_code).await;
            Ok(build_upstream_response(status_code, headers, body))
        }
        Ok(GeminiForwardResult::NonStreaming {
            status_code,
            headers,
            content,
        }) => {
            record_proxy_status(state, status_code).await;
            Ok(build_upstream_response(status_code, headers, full(content)))
        }
        Ok(GeminiForwardResult::UpstreamError {
            status_code,
            headers,
            content,
        }) => {
            record_proxy_failure(state).await;
            Ok(build_upstream_response(status_code, headers, full(content)))
        }
        Err(e) => {
            record_proxy_failure(state).await;
            Ok(json_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "proxy_error",
                &e,
            ))
        }
    }
}

pub(crate) async fn forward_claude_passthrough(
    forwarder: &RequestForwarder,
    method: Method,
    forward_path: &str,
    request_headers: hyper::HeaderMap,
    body_bytes: bytes::Bytes,
    context: RequestContext,
    state: &Arc<ProxyState>,
) -> HandlerResult {
    match forwarder
        .forward_passthrough(method, forward_path, body_bytes, context, request_headers)
        .await
    {
        Ok(ForwardResult::Streaming {
            status_code,
            headers,
            body,
        }) => {
            record_proxy_status(state, status_code).await;
            Ok(build_upstream_response(status_code, headers, body))
        }
        Ok(ForwardResult::NonStreaming {
            status_code,
            headers,
            content,
        }) => {
            record_proxy_status(state, status_code).await;
            Ok(build_upstream_response(status_code, headers, full(content)))
        }
        Err(e) => {
            record_proxy_failure(state).await;
            Ok(json_error_response(
                StatusCode::BAD_GATEWAY,
                "proxy_error",
                &e,
            ))
        }
    }
}

pub(crate) async fn forward_claude_with_usage(
    forwarder: &RequestForwarder,
    method: Method,
    forward_path: &str,
    request_headers: hyper::HeaderMap,
    body_bytes: bytes::Bytes,
    context: RequestContext,
    state: &Arc<ProxyState>,
) -> HandlerResult {
    match forwarder
        .forward_with_usage(method, forward_path, body_bytes, context, request_headers)
        .await
    {
        Ok(ForwardResult::Streaming {
            status_code,
            headers,
            body,
        }) => {
            record_proxy_status(state, status_code).await;
            Ok(build_upstream_response(status_code, headers, body))
        }
        Ok(ForwardResult::NonStreaming {
            status_code,
            headers,
            content,
        }) => {
            record_proxy_status(state, status_code).await;
            Ok(build_upstream_response(status_code, headers, full(content)))
        }
        Err(e) => {
            record_proxy_failure(state).await;
            let error_body = serde_json::json!({
                "error": {
                    "type": "proxy_error",
                    "message": e
                }
            });

            Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("Content-Type", "application/json")
                .body(full(serde_json::to_string(&error_body).unwrap_or_default()))
                .unwrap())
        }
    }
}
