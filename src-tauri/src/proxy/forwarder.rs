//! 请求转发器，用于将请求代理到 Anthropic API

use super::collector::UsageCollector;
use super::request_body::ForwardRequestBody;
use super::stream_processor::{
    create_database_collector, create_passthrough_stream, StreamContext,
};
use super::types::{RequestContext, UsageRecord};
use crate::net::HttpClientFactory;
use bytes::Bytes;
use futures::TryStreamExt;
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use hyper::Method;
use reqwest::Client;
use std::sync::Arc;
use std::time::Duration;

/// UnsyncBoxBody 类型别名，用于响应体（不需要 Sync）
/// 错误类型为 std::io::Error，实现了 Into<Box<dyn StdError + Send + Sync>>
pub type BoxBody = http_body_util::combinators::UnsyncBoxBody<Bytes, std::io::Error>;
const USAGE_MISSING_STATUS_CODE: u16 = 599;

/// 请求转发器，将请求代理到 Anthropic API
pub struct RequestForwarder {
    /// HTTP 客户端
    client: Client,
    /// SSE 流式请求客户端：仅连接超时 + 读空闲超时，无总时长超时
    streaming_client: Client,
    /// 使用量收集器
    usage_collector: Arc<UsageCollector>,
    /// 目标基础 URL
    target_base_url: String,
    /// 流式空闲超时：`None` 表示关闭（不设超时）
    streaming_idle_timeout: Option<Duration>,
}

/// 转发请求的结果
pub enum ForwardResult {
    /// 流式响应（SSE）- 返回 BoxBody 用于实时透传
    Streaming {
        status_code: u16,
        headers: Vec<(String, String)>,
        body: BoxBody,
    },
    /// 非流式响应（JSON）
    NonStreaming {
        status_code: u16,
        headers: Vec<(String, String)>,
        content: Vec<u8>,
    },
}

fn anthropic_endpoint_url(target_base_url: &str, path: &str) -> String {
    let base = target_base_url.trim_end_matches('/');
    let raw_path = path.trim_start_matches('/');
    format!("{}/{}", base, raw_path)
}

impl RequestForwarder {
    /// 创建新的请求转发器
    pub fn new(
        usage_collector: Arc<UsageCollector>,
        target_base_url: String,
        request_timeout_secs: u64,
        streaming_idle_timeout_secs: u64,
    ) -> Result<Self, String> {
        let client = HttpClientFactory::global().long();
        let streaming_client = HttpClientFactory::global()
            .build_streaming(request_timeout_secs, streaming_idle_timeout_secs)?;
        let streaming_idle_timeout = if streaming_idle_timeout_secs > 0 {
            Some(Duration::from_secs(streaming_idle_timeout_secs))
        } else {
            None
        };

        Ok(Self {
            client,
            streaming_client,
            usage_collector,
            target_base_url,
            streaming_idle_timeout,
        })
    }

    /// 将请求转发到 Anthropic API
    pub async fn forward_with_usage(
        &self,
        method: Method,
        path: &str,
        body: ForwardRequestBody,
        context: RequestContext,
        headers: hyper::HeaderMap,
    ) -> Result<ForwardResult, String> {
        let mut context = context;
        let target_base_url = context
            .target_base_url
            .clone()
            .unwrap_or_else(|| self.target_base_url.clone());
        let url = anthropic_endpoint_url(&target_base_url, path);

        let (body, observation) = match body {
            ForwardRequestBody::Buffered(body) => {
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) {
                    context.model = json
                        .get("model")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    context.stream = json
                        .get("stream")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                }
                (body.into(), None)
            }
            ForwardRequestBody::Streaming { body, observation } => (body, observation),
        };

        let client = if observation.is_some() || context.stream {
            &self.streaming_client
        } else {
            &self.client
        };

        // 构建请求
        let method = reqwest::Method::from_bytes(method.as_str().as_bytes())
            .unwrap_or(reqwest::Method::POST);
        let mut request = client.request(method, &url);
        request = apply_passthrough_headers(request, &headers);
        request = apply_stream_content_length(request, &headers);
        request = request.body(body);

        // 发送请求
        let response = request
            .send()
            .await
            .map_err(|e| format!("Failed to send request: {}", e))?;
        if let Some(observation) = observation {
            observation.apply_to_context(&mut context);
        }

        let status_code = response.status().as_u16();
        let response_headers = collect_passthrough_response_headers(response.headers());
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let is_streaming = context.stream
            || content_type
                .as_deref()
                .map(|value| value.contains("text/event-stream"))
                .unwrap_or(false);

        if status_code >= 400 {
            let error_body = response.bytes().await.unwrap_or_default();

            // 即使是错误响应，也记录请求（无 token 数据，但有状态码）
            let request_end_time = chrono::Utc::now().timestamp_millis();
            let duration_ms = context.start_time.elapsed().as_millis() as i64;

            let mut record = UsageRecord {
                timestamp: request_end_time,
                // 使用时间戳+状态码+随机数确保错误记录唯一性
                // 避免 message_id 为空时 UNIQUE 约束导致多条错误记录互相覆盖
                message_id: format!(
                    "error_{}_{}_{}",
                    request_end_time,
                    status_code,
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.subsec_nanos())
                        .unwrap_or(0)
                ),
                input_tokens: 0,
                output_tokens: 0,
                cache_create_tokens: 0,
                cache_read_tokens: 0,
                reasoning_tokens: 0,
                total_tokens: 0,
                model: context.model.clone().unwrap_or_default(),
                session_id: context.session_id.clone(),
                request_start_time: context.start_time_ms,
                request_end_time,
                duration_ms: duration_ms as u64,
                output_tokens_per_second: None,
                ttft_ms: None,
                status_code,
                estimated_cost: 0.0,
                pricing_snapshot_id: None,
                cost_locked: false,
                api_key_prefix: context.api_key_prefix.clone(),
                request_base_url: context.request_base_url.clone(),
                client_tool: context.client_tool.clone(),
                proxy_profile_id: context.proxy_profile_id.clone(),
                client_detection_method: context.client_detection_method.clone(),
                ..Default::default()
            };
            context.apply_provenance(&mut record);
            record.usage_source = super::types::default_usage_source();
            self.usage_collector.record(record).await;

            return Ok(ForwardResult::NonStreaming {
                status_code,
                headers: response_headers,
                content: error_body.to_vec(),
            });
        }

        if is_streaming {
            // 处理流式响应
            self.handle_streaming_response(response, context, status_code, response_headers)
                .await
        } else {
            // 处理非流式响应
            self.handle_non_streaming_response(response, context, status_code, response_headers)
                .await
        }
    }

    /// 处理流式（SSE）响应 - 真正的流式透传
    ///
    /// 创建实时透传流，该流：
    /// 1. 立即将字节转发给客户端
    /// 2. 在后台收集使用量统计
    async fn handle_streaming_response(
        &self,
        response: reqwest::Response,
        context: RequestContext,
        status_code: u16,
        headers: Vec<(String, String)>,
    ) -> Result<ForwardResult, String> {
        // 使用请求入口的单调时钟，包含上游等待时间并与其它协议一致。
        let ttft_start_time = context.start_time;

        // 创建流上下文用于使用量收集
        let stream_context = StreamContext {
            cache_create_tokens: context.cache_create_tokens,
            cache_read_tokens: context.cache_read_tokens,
            session_id: context.session_id.clone(),
            request_start_time: context.start_time_ms,
            status_code,
            api_key_prefix: context.api_key_prefix,
            request_base_url: context.request_base_url,
            client_tool: context.client_tool,
            proxy_profile_id: context.proxy_profile_id,
            client_detection_method: context.client_detection_method,
            ingress_kind: context.ingress_kind,
            gateway_profile_id: context.gateway_profile_id,
            gateway_caller_label: context.gateway_caller_label,
            usage_source: context.usage_source,
            gateway_request_id: context.gateway_request_id,
        };

        // 创建使用量收集器，用于记录到数据库
        let collector = create_database_collector(
            self.usage_collector.clone(),
            stream_context,
            ttft_start_time,
        );

        // 获取响应的字节流
        let stream = response.bytes_stream();

        // 创建透传流，实时转发
        let passthrough_stream =
            create_passthrough_stream(stream, collector, self.streaming_idle_timeout);

        // 转换为 StreamBody 然后转为 UnsyncBoxBody
        // 保持 std::io::Error 作为错误类型（hyper 接受任何实现了
        // Into<Box<dyn StdError + Send + Sync>> 的错误）
        let stream_body = StreamBody::new(passthrough_stream.map_ok(Frame::data));

        Ok(ForwardResult::Streaming {
            status_code,
            headers,
            body: stream_body.boxed_unsync(),
        })
    }

    /// 处理非流式（JSON）响应
    ///
    /// 数据语义与流式响应保持一致：
    /// - input_tokens: 原始输入 Token（不含缓存）
    /// - total_tokens: input_tokens + cache_create_tokens + cache_read_tokens + output_tokens
    async fn handle_non_streaming_response(
        &self,
        response: reqwest::Response,
        context: RequestContext,
        status_code: u16,
        headers: Vec<(String, String)>,
    ) -> Result<ForwardResult, String> {
        let request_start_time = context.start_time_ms;

        let body = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read response body: {}", e))?;
        let request_end_time = chrono::Utc::now().timestamp_millis();
        let duration_ms = context.start_time.elapsed().as_millis() as u64;

        let parsed_usage = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(parse_anthropic_non_stream_usage);
        self.record_non_streaming_usage_optional(
            parsed_usage,
            context,
            request_start_time,
            request_end_time,
            duration_ms,
            status_code,
        )
        .await;

        Ok(ForwardResult::NonStreaming {
            status_code,
            headers,
            content: body.to_vec(),
        })
    }

    async fn record_non_streaming_usage_optional(
        &self,
        usage: Option<AnthropicNonStreamUsage>,
        context: RequestContext,
        request_start_time: i64,
        request_end_time: i64,
        duration_ms: u64,
        status_code: u16,
    ) {
        match usage {
            Some(usage) => {
                let mut record = UsageRecord {
                    timestamp: request_end_time,
                    message_id: usage.message_id,
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cache_create_tokens: usage.cache_create_tokens,
                    cache_read_tokens: usage.cache_read_tokens,
                    reasoning_tokens: 0,
                    total_tokens: usage.total_tokens,
                    model: usage.model,
                    session_id: context.session_id.clone(),
                    request_start_time,
                    request_end_time,
                    duration_ms,
                    output_tokens_per_second: None,
                    ttft_ms: None,
                    status_code,
                    estimated_cost: 0.0,
                    pricing_snapshot_id: None,
                    cost_locked: false,
                    api_key_prefix: context.api_key_prefix.clone(),
                    request_base_url: context.request_base_url.clone(),
                    client_tool: context.client_tool.clone(),
                    proxy_profile_id: context.proxy_profile_id.clone(),
                    client_detection_method: context.client_detection_method.clone(),
                    ..Default::default()
                };
                context.apply_provenance(&mut record);
                self.usage_collector.record(record).await;
            }
            None => {
                let mut record = UsageRecord {
                    timestamp: request_end_time,
                    message_id: format!(
                        "claude_usage_missing_{}_{}",
                        request_end_time, status_code
                    ),
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_create_tokens: 0,
                    cache_read_tokens: 0,
                    reasoning_tokens: 0,
                    total_tokens: 0,
                    model: context.model.clone().unwrap_or_default(),
                    session_id: context.session_id.clone(),
                    request_start_time,
                    request_end_time,
                    duration_ms,
                    output_tokens_per_second: None,
                    ttft_ms: None,
                    status_code: if (200..300).contains(&status_code) {
                        USAGE_MISSING_STATUS_CODE
                    } else {
                        status_code
                    },
                    estimated_cost: 0.0,
                    pricing_snapshot_id: None,
                    cost_locked: false,
                    api_key_prefix: context.api_key_prefix.clone(),
                    request_base_url: context.request_base_url.clone(),
                    client_tool: context.client_tool.clone(),
                    proxy_profile_id: context.proxy_profile_id.clone(),
                    client_detection_method: context.client_detection_method.clone(),
                    ..Default::default()
                };
                context.apply_provenance(&mut record);
                record.usage_source = super::types::default_usage_source();
                self.usage_collector.record(record).await;
            }
        }
    }

    pub async fn forward_passthrough(
        &self,
        method: Method,
        path: &str,
        body: bytes::Bytes,
        context: RequestContext,
        headers: hyper::HeaderMap,
    ) -> Result<ForwardResult, String> {
        self.forward_passthrough_stream(method, path, body.into(), context, headers)
            .await
    }

    pub async fn forward_passthrough_stream(
        &self,
        method: Method,
        path: &str,
        body: reqwest::Body,
        context: RequestContext,
        headers: hyper::HeaderMap,
    ) -> Result<ForwardResult, String> {
        let target_base_url = context
            .target_base_url
            .clone()
            .unwrap_or_else(|| self.target_base_url.clone());
        let url = anthropic_endpoint_url(&target_base_url, path);
        let method =
            reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
        let mut request = self.streaming_client.request(method, &url);
        request = apply_passthrough_headers(request, &headers);
        request = apply_stream_content_length(request, &headers);
        request = request.body(body);

        let response = request
            .send()
            .await
            .map_err(|e| format!("Failed to send request: {}", e))?;
        let status_code = response.status().as_u16();
        let response_headers = collect_passthrough_response_headers(response.headers());
        let stream = response
            .bytes_stream()
            .map_ok(Frame::data)
            .map_err(|e| std::io::Error::other(e.to_string()));
        Ok(ForwardResult::Streaming {
            status_code,
            headers: response_headers,
            body: StreamBody::new(stream).boxed_unsync(),
        })
    }
}

struct AnthropicNonStreamUsage {
    message_id: String,
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_create_tokens: u64,
    cache_read_tokens: u64,
    total_tokens: u64,
}

fn parse_anthropic_non_stream_usage(json: serde_json::Value) -> Option<AnthropicNonStreamUsage> {
    let usage = json.get("usage")?;
    let input_tokens = usage.get("input_tokens")?.as_u64()?;
    let output_tokens = usage.get("output_tokens")?.as_u64()?;
    let cache_create_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_read_tokens = usage
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    Some(AnthropicNonStreamUsage {
        message_id: json
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        model: json
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        input_tokens,
        output_tokens,
        cache_create_tokens,
        cache_read_tokens,
        total_tokens: input_tokens + cache_create_tokens + cache_read_tokens + output_tokens,
    })
}

fn apply_passthrough_headers(
    mut request: reqwest::RequestBuilder,
    headers: &hyper::HeaderMap,
) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        let name = name.as_str();
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "host"
                | "content-length"
                | "connection"
                | "accept-encoding"
                | "proxy-authorization"
                | "transfer-encoding"
                | "te"
                | "trailer"
                | "upgrade"
        ) {
            continue;
        }
        if let Ok(value) = value.to_str() {
            request = request.header(name, value);
        }
    }
    request
}

fn apply_stream_content_length(
    request: reqwest::RequestBuilder,
    headers: &hyper::HeaderMap,
) -> reqwest::RequestBuilder {
    if let Some(value) = headers
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
    {
        request.header(reqwest::header::CONTENT_LENGTH, value)
    } else {
        request
    }
}

fn collect_passthrough_response_headers(
    headers: &reqwest::header::HeaderMap,
) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            let lower = name.as_str().to_ascii_lowercase();
            if is_hop_by_hop_response_header(&lower) {
                return None;
            }
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect()
}

fn is_hop_by_hop_response_header(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header;

    #[test]
    fn test_anthropic_endpoint_url_preserves_raw_request_path() {
        assert_eq!(
            anthropic_endpoint_url("https://api.example.com", "/v1/messages"),
            "https://api.example.com/v1/messages"
        );
        assert_eq!(
            anthropic_endpoint_url("https://api.example.com/v1", "/v1/messages"),
            "https://api.example.com/v1/v1/messages"
        );
        assert_eq!(
            anthropic_endpoint_url("https://api.example.com/api/v1/", "/foo/bar"),
            "https://api.example.com/api/v1/foo/bar"
        );
    }

    #[test]
    fn parse_anthropic_non_stream_usage_requires_usage_fields() {
        let parsed = parse_anthropic_non_stream_usage(serde_json::json!({
            "id": "msg_123",
            "model": "claude-sonnet-4"
        }));
        assert!(parsed.is_none());
    }

    #[test]
    fn parse_anthropic_non_stream_usage_extracts_tokens() {
        let parsed = parse_anthropic_non_stream_usage(serde_json::json!({
            "id": "msg_123",
            "model": "claude-sonnet-4",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 20,
                "cache_creation_input_tokens": 3,
                "cache_read_input_tokens": 4
            }
        }))
        .expect("usage should parse");

        assert_eq!(parsed.message_id, "msg_123");
        assert_eq!(parsed.model, "claude-sonnet-4");
        assert_eq!(parsed.input_tokens, 10);
        assert_eq!(parsed.output_tokens, 20);
        assert_eq!(parsed.cache_create_tokens, 3);
        assert_eq!(parsed.cache_read_tokens, 4);
        assert_eq!(parsed.total_tokens, 37);
    }

    #[test]
    fn collect_passthrough_response_headers_keeps_upstream_metadata() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        headers.insert("retry-after", "120".parse().unwrap());
        headers.insert("request-id", "req_123".parse().unwrap());
        headers.insert(header::CONNECTION, "keep-alive".parse().unwrap());
        headers.insert(header::CONTENT_LENGTH, "42".parse().unwrap());

        let collected = collect_passthrough_response_headers(&headers);

        assert!(collected
            .iter()
            .any(|(name, value)| name == "content-type" && value == "application/json"));
        assert!(collected
            .iter()
            .any(|(name, value)| name == "retry-after" && value == "120"));
        assert!(collected
            .iter()
            .any(|(name, value)| name == "request-id" && value == "req_123"));
        assert!(!collected.iter().any(|(name, _)| name == "connection"));
        assert!(!collected.iter().any(|(name, _)| name == "content-length"));
    }
}
