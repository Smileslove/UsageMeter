//! OpenAI-compatible proxy forwarding and usage capture for Codex.

use super::collector::UsageCollector;
use super::sse::{append_utf8_safe, strip_sse_field, take_sse_block};
use super::types::{RequestContext, UsageRecord};
use async_stream::stream;
use bytes::Bytes;
use futures::StreamExt;
use http_body_util::StreamBody;
use hyper::body::Frame;
use hyper::{HeaderMap, Method};
use reqwest::Client;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub type BoxBody = http_body_util::combinators::UnsyncBoxBody<Bytes, std::io::Error>;
const USAGE_MISSING_STATUS_CODE: u16 = 599;

pub enum OpenAiForwardResult {
    Streaming {
        body: BoxBody,
    },
    NonStreaming {
        content: Vec<u8>,
    },
    UpstreamError {
        status_code: u16,
        content_type: Option<String>,
        headers: Vec<(String, String)>,
        content: Vec<u8>,
    },
}

pub struct OpenAiPassthroughResult {
    pub status_code: u16,
    pub content_type: Option<String>,
    pub headers: Vec<(String, String)>,
    pub content: Vec<u8>,
}

pub struct OpenAiForwarder {
    client: Client,
    usage_collector: Arc<UsageCollector>,
}

#[derive(Debug, Clone, Default)]
struct OpenAiUsage {
    message_id: String,
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_create_tokens: u64,
    reasoning_tokens: u64,
}

impl OpenAiForwarder {
    pub fn new(usage_collector: Arc<UsageCollector>) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .http1_only()
            .http1_title_case_headers()
            .pool_max_idle_per_host(0)
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .build()
            .map_err(|e| format!("Failed to create OpenAI HTTP client: {}", e))?;
        Ok(Self {
            client,
            usage_collector,
        })
    }

    pub async fn forward_with_headers(
        &self,
        method: Method,
        path: &str,
        headers: HeaderMap,
        body: bytes::Bytes,
        mut context: RequestContext,
    ) -> Result<OpenAiForwardResult, String> {
        let api_key = context
            .target_api_key
            .clone()
            .or_else(|| context.inbound_api_key.clone())
            .ok_or_else(|| "No API key found for Codex provider".to_string())?;
        let target_base_url = context
            .target_base_url
            .clone()
            .ok_or_else(|| "No target base URL found for Codex provider".to_string())?;
        let url = openai_endpoint_url(&target_base_url, path);

        if let Ok(json) = serde_json::from_slice::<Value>(&body) {
            context.model = json
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            context.stream = json
                .get("stream")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
        }

        let method = reqwest::Method::from_bytes(method.as_str().as_bytes())
            .unwrap_or(reqwest::Method::POST);
        let mut request = self.client.request(method, &url);
        request = apply_passthrough_headers(request, &headers);
        request = apply_chatgpt_account_header(request, context.chatgpt_account_id.as_deref());
        if !body.is_empty() && !headers.contains_key("content-type") {
            request = request.header("Content-Type", "application/json");
        }
        let response = request
            .bearer_auth(&api_key)
            .body(body)
            .send()
            .await
            .map_err(|e| format!("Failed to send Codex request: {}", e))?;

        let status = response.status();
        let status_code = status.as_u16();
        let upstream_is_sse = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.contains("text/event-stream"))
            .unwrap_or(false);
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);

        if !status.is_success() {
            let response_headers = collect_passthrough_response_headers(response.headers());
            let content = response
                .bytes()
                .await
                .map_err(|e| format!("Failed to read Codex error response: {}", e))?
                .to_vec();
            self.record_error(&context, status_code).await;
            return Ok(OpenAiForwardResult::UpstreamError {
                status_code,
                content_type,
                headers: response_headers,
                content,
            });
        }

        if context.stream || upstream_is_sse {
            Ok(OpenAiForwardResult::Streaming {
                body: self.handle_streaming(response, context).await?,
            })
        } else {
            let bytes = response
                .bytes()
                .await
                .map_err(|e| format!("Failed to read Codex response: {}", e))?;
            self.record_json_response(&bytes, context, status_code)
                .await;
            Ok(OpenAiForwardResult::NonStreaming {
                content: bytes.to_vec(),
            })
        }
    }

    pub async fn forward_passthrough(
        &self,
        method: Method,
        path: &str,
        headers: HeaderMap,
        body: bytes::Bytes,
        context: RequestContext,
    ) -> Result<OpenAiPassthroughResult, String> {
        let api_key = context
            .target_api_key
            .clone()
            .or_else(|| context.inbound_api_key.clone())
            .ok_or_else(|| "No API key found for Codex provider".to_string())?;
        let target_base_url = context
            .target_base_url
            .clone()
            .ok_or_else(|| "No target base URL found for Codex provider".to_string())?;
        let url = openai_endpoint_url(&target_base_url, path);

        let method =
            reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
        let mut request = self.client.request(method, &url);
        request = apply_passthrough_headers(request, &headers);
        request = apply_chatgpt_account_header(request, context.chatgpt_account_id.as_deref());
        if !body.is_empty() && !headers.contains_key("content-type") {
            request = request.header("Content-Type", "application/json");
        }

        let response = request
            .bearer_auth(&api_key)
            .body(body)
            .send()
            .await
            .map_err(|e| format!("Failed to send Codex passthrough request: {}", e))?;
        let status_code = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let response_headers = collect_passthrough_response_headers(response.headers());
        let content = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Codex passthrough response: {}", e))?
            .to_vec();

        Ok(OpenAiPassthroughResult {
            status_code,
            content_type,
            headers: response_headers,
            content,
        })
    }

    async fn record_error(&self, context: &RequestContext, status_code: u16) {
        let now = chrono::Utc::now().timestamp_millis();
        let duration_ms = now.saturating_sub(context.start_time_ms) as u64;
        let record = UsageRecord {
            timestamp: now,
            message_id: format!("codex_error_{}_{}", now, status_code),
            model: context.model.clone().unwrap_or_default(),
            session_id: context.session_id.clone(),
            request_start_time: context.start_time_ms,
            request_end_time: now,
            duration_ms,
            status_code,
            api_key_prefix: context.api_key_prefix.clone(),
            request_base_url: context.request_base_url.clone(),
            client_tool: context.client_tool.clone(),
            proxy_profile_id: context.proxy_profile_id.clone(),
            client_detection_method: context.client_detection_method.clone(),
            ..Default::default()
        };
        self.usage_collector.record(record).await;
    }

    async fn record_json_response(&self, bytes: &[u8], context: RequestContext, status_code: u16) {
        let usage = serde_json::from_slice::<Value>(bytes)
            .ok()
            .and_then(|value| parse_openai_usage(&value));
        self.record_usage_optional(usage, context, status_code)
            .await;
    }

    async fn record_usage_optional(
        &self,
        usage: Option<OpenAiUsage>,
        context: RequestContext,
        status_code: u16,
    ) {
        record_usage_with_collector_optional(
            self.usage_collector.clone(),
            usage,
            context,
            status_code,
            None,
        )
        .await;
    }

    async fn handle_streaming(
        &self,
        response: reqwest::Response,
        context: RequestContext,
    ) -> Result<BoxBody, String> {
        let status_code = response.status().as_u16();
        let collector = self.usage_collector.clone();
        let usage_candidate = Arc::new(Mutex::new(None::<OpenAiUsage>));
        let first_token_time = Arc::new(Mutex::new(None::<Instant>));
        // TTFT 从收到上游响应头开始计时
        let ttft_start = std::time::Instant::now();
        let context_for_finish = context.clone();
        let stream = response.bytes_stream();

        let passthrough = stream! {
            let mut buffer = String::new();
            let mut utf8_remainder = Vec::new();
            let mut stream = std::pin::pin!(stream);

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        append_utf8_safe(&mut buffer, &mut utf8_remainder, &bytes);
                        while let Some(event_text) = take_sse_block(&mut buffer) {
                            for line in event_text.lines() {
                                if let Some(data) = strip_sse_field(line, "data") {
                                    if data.trim() != "[DONE]" {
                                        if let Ok(value) = serde_json::from_str::<Value>(data) {
                                            if first_token_candidate(&value) {
                                                let mut first = first_token_time.lock().await;
                                                if first.is_none() {
                                                    *first = Some(Instant::now());
                                                }
                                            }
                                            if let Some(usage) = parse_openai_stream_usage_event(&value) {
                                                *usage_candidate.lock().await = Some(usage);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        yield Ok(Frame::data(bytes));
                    }
                    Err(e) => {
                        yield Err(std::io::Error::other(e.to_string()));
                        break;
                    }
                }
            }

            let usage = usage_candidate.lock().await.take();
            let ttft_ms = first_token_time
                .lock()
                .await
                .map(|instant| instant.duration_since(ttft_start).as_millis() as u64);
            record_usage_with_collector_optional(
                collector,
                usage,
                context_for_finish,
                status_code,
                ttft_ms,
            )
            .await;
        };

        Ok(http_body_util::BodyExt::boxed_unsync(StreamBody::new(
            passthrough,
        )))
    }
}

async fn record_usage_with_collector(
    collector: Arc<UsageCollector>,
    usage: OpenAiUsage,
    context: RequestContext,
    status_code: u16,
    ttft_ms: Option<u64>,
) {
    let now = chrono::Utc::now().timestamp_millis();
    let duration_ms = now.saturating_sub(context.start_time_ms) as u64;
    let output_tokens_per_second = if duration_ms > 0 {
        Some(usage.output_tokens as f64 / (duration_ms as f64 / 1000.0))
    } else {
        None
    };
    let message_id = if usage.message_id.is_empty() {
        format!(
            "codex_{}_{}",
            now,
            std::time::Instant::now().elapsed().as_nanos()
        )
    } else {
        usage.message_id.clone()
    };
    let total_tokens = usage.input_tokens
        + usage.cache_create_tokens
        + usage.cache_read_tokens
        + usage.output_tokens;
    let record = UsageRecord {
        timestamp: now,
        message_id,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_create_tokens: usage.cache_create_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        total_tokens,
        model: if usage.model.is_empty() {
            context.model.clone().unwrap_or_default()
        } else {
            usage.model
        },
        session_id: context.session_id.clone(),
        request_start_time: context.start_time_ms,
        request_end_time: now,
        duration_ms,
        output_tokens_per_second,
        ttft_ms,
        status_code,
        estimated_cost: 0.0,
        pricing_snapshot_id: None,
        cost_locked: false,
        api_key_prefix: context.api_key_prefix.clone(),
        request_base_url: context.request_base_url.clone(),
        client_tool: context.client_tool.clone(),
        proxy_profile_id: context.proxy_profile_id.clone(),
        client_detection_method: context.client_detection_method.clone(),
    };
    collector.record(record).await;
}

/// 记录使用量（可选 usage），即使解析失败也创建带状态码的记录
async fn record_usage_with_collector_optional(
    collector: Arc<UsageCollector>,
    usage: Option<OpenAiUsage>,
    context: RequestContext,
    status_code: u16,
    ttft_ms: Option<u64>,
) {
    match usage {
        Some(usage) => {
            record_usage_with_collector(collector, usage, context, status_code, ttft_ms).await;
        }
        None => {
            let now = chrono::Utc::now().timestamp_millis();
            let duration_ms = now.saturating_sub(context.start_time_ms) as u64;
            let record = UsageRecord {
                timestamp: now,
                message_id: format!("codex_usage_missing_{}_{}", now, status_code),
                input_tokens: 0,
                output_tokens: 0,
                cache_create_tokens: 0,
                cache_read_tokens: 0,
                reasoning_tokens: 0,
                total_tokens: 0,
                model: context.model.clone().unwrap_or_default(),
                session_id: context.session_id.clone(),
                request_start_time: context.start_time_ms,
                request_end_time: now,
                duration_ms,
                output_tokens_per_second: None,
                ttft_ms,
                // The upstream request returned a success status, but without
                // parseable usage the proxy cannot account for tokens
                // truthfully. Mark it as a proxy-side accounting error so
                // default success-only summaries do not silently undercount.
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
            };
            collector.record(record).await;
        }
    }
}

fn apply_passthrough_headers(
    mut request: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        let name = name.as_str();
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "host"
                | "content-length"
                | "connection"
                | "accept-encoding"
                | "authorization"
                | "proxy-authorization"
        ) {
            continue;
        }
        if let Ok(value) = value.to_str() {
            request = request.header(name, value);
        }
    }
    request
}

fn apply_chatgpt_account_header(
    request: reqwest::RequestBuilder,
    account_id: Option<&str>,
) -> reqwest::RequestBuilder {
    let Some(account_id) = account_id.filter(|value| !value.trim().is_empty()) else {
        return request;
    };

    request.header("ChatGPT-Account-Id", account_id)
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

fn openai_endpoint_url(target_base_url: &str, path: &str) -> String {
    let base = target_base_url.trim_end_matches('/');
    let normalized_path = normalize_openai_path(path);

    if is_chatgpt_codex_base(base) {
        if is_codex_responses_path(&normalized_path) {
            return format!("{}/{}", base, normalized_path.trim_start_matches("v1/"));
        }

        let backend_base = base.trim_end_matches("/codex");
        if normalized_path == "api/codex/apps" {
            return format!("{backend_base}/wham/apps");
        }
        return format!(
            "{}/{}",
            backend_base,
            normalized_path.trim_start_matches("v1/")
        );
    }

    if base.ends_with("/v1") && normalized_path.starts_with("v1/") {
        format!("{}/{}", base.trim_end_matches("/v1"), normalized_path)
    } else {
        format!("{}/{}", base, normalized_path)
    }
}

fn is_chatgpt_codex_base(base: &str) -> bool {
    base.contains("chatgpt.com/backend-api/codex")
}

fn is_codex_responses_path(normalized_path: &str) -> bool {
    let path = normalized_path.trim_start_matches("v1/");
    path == "responses" || path == "responses/compact" || path.starts_with("responses/")
}

fn normalize_openai_path(path: &str) -> String {
    let normalized = path.trim_start_matches('/');
    // Defensive normalization for configs where the upstream base URL already
    // ends in /v1 while the client also sends a /v1/... path. The proxy should
    // forward a single OpenAI API version segment, not propagate /v1/v1.
    normalized
        .strip_prefix("v1/v1/")
        .map(|rest| format!("v1/{rest}"))
        .unwrap_or_else(|| normalized.to_string())
}

fn first_token_candidate(value: &Value) -> bool {
    value.get("choices").is_some()
        || value
            .get("type")
            .and_then(|v| v.as_str())
            .map(|t| t.contains("delta") || t.contains("output"))
            .unwrap_or(false)
}

fn parse_openai_stream_usage_event(event: &Value) -> Option<OpenAiUsage> {
    if event.get("type").and_then(|v| v.as_str()) == Some("response.completed") {
        return event.get("response").and_then(parse_openai_usage);
    }

    if let Some(usage) = event.get("usage") {
        if !usage.is_null() {
            return parse_openai_usage(event);
        }
    }

    None
}

#[cfg(test)]
fn parse_openai_stream_usage(events: &[Value]) -> Option<OpenAiUsage> {
    // 阶段 1: Codex Responses API — 按 type == "response.completed" 显式查找
    for event in events {
        if event.get("type").and_then(|v| v.as_str()) == Some("response.completed") {
            if let Some(response) = event.get("response") {
                if let Some(usage) = parse_openai_usage(response) {
                    return Some(usage);
                }
            }
        }
    }
    // 阶段 2: OpenAI Chat Completions — 最后一个有 usage 的 chunk
    for event in events.iter().rev() {
        if let Some(usage) = event.get("usage") {
            if !usage.is_null() {
                if let Some(usage) = parse_openai_usage(event) {
                    return Some(usage);
                }
            }
        }
    }
    None
}

fn parse_openai_usage(value: &Value) -> Option<OpenAiUsage> {
    if let Some(response) = value.get("response").filter(|v| v.is_object()) {
        if let Some(mut parsed) = parse_openai_usage(response) {
            if parsed.message_id.is_empty() {
                parsed.message_id = value
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
            }
            return Some(parsed);
        }
    }

    if let Some(data) = value.get("data") {
        if data.is_object() {
            if let Some(parsed) = parse_openai_usage(data) {
                return Some(parsed);
            }
        } else if let Some(text) = data.as_str() {
            if let Ok(data_value) = serde_json::from_str::<Value>(text) {
                if let Some(parsed) = parse_openai_usage(&data_value) {
                    return Some(parsed);
                }
            }
        }
    }

    if let Some(item) = value.get("item").filter(|v| v.is_object()) {
        if let Some(parsed) = parse_openai_usage(item) {
            return Some(parsed);
        }
    }

    let usage = value.get("usage")?;
    let token_details = usage
        .get("prompt_tokens_details")
        .or_else(|| usage.get("input_tokens_details"));
    let cached_tokens = token_details
        .and_then(|details| details.get("cached_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_create_tokens = token_details
        .and_then(|details| details.get("cache_creation"))
        .and_then(|cc| cc.get("cache_creation_input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("output_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let raw_input = usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = usage
        .get("completion_tokens")
        .or_else(|| usage.get("output_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let input = raw_input.saturating_sub(cached_tokens);
    Some(OpenAiUsage {
        message_id: value
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        model: value
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cached_tokens,
        cache_create_tokens,
        reasoning_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chat_completion_usage() {
        let value = serde_json::json!({
            "id": "chatcmpl_1",
            "model": "gpt-5.1",
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "prompt_tokens_details": { "cached_tokens": 20 }
            }
        });
        let usage = parse_openai_usage(&value).unwrap();
        assert_eq!(usage.message_id, "chatcmpl_1");
        assert_eq!(usage.input_tokens, 80);
        assert_eq!(usage.cache_read_tokens, 20);
        assert_eq!(usage.cache_create_tokens, 0);
        assert_eq!(usage.output_tokens, 50);
    }

    #[test]
    fn parses_chat_completion_with_cache_creation() {
        let value = serde_json::json!({
            "id": "chatcmpl_2",
            "model": "qwen-plus",
            "usage": {
                "prompt_tokens": 3019,
                "completion_tokens": 104,
                "total_tokens": 3123,
                "prompt_tokens_details": {
                    "cached_tokens": 2048,
                    "cache_creation": {
                        "cache_creation_input_tokens": 500,
                        "cache_type": "ephemeral"
                    }
                }
            }
        });
        let usage = parse_openai_usage(&value).unwrap();
        assert_eq!(usage.message_id, "chatcmpl_2");
        assert_eq!(usage.input_tokens, 971); // 3019 - 2048
        assert_eq!(usage.cache_read_tokens, 2048);
        assert_eq!(usage.cache_create_tokens, 500);
        assert_eq!(usage.output_tokens, 104);
    }

    #[test]
    fn parses_responses_usage() {
        let value = serde_json::json!({
            "id": "resp_1",
            "model": "gpt-5.1",
            "usage": {
                "input_tokens": 200,
                "output_tokens": 75,
                "input_tokens_details": { "cached_tokens": 30 }
            }
        });
        let usage = parse_openai_usage(&value).unwrap();
        assert_eq!(usage.input_tokens, 170);
        assert_eq!(usage.cache_read_tokens, 30);
        assert_eq!(usage.cache_create_tokens, 0);
        assert_eq!(usage.output_tokens, 75);
    }

    #[test]
    fn parses_nested_responses_usage() {
        let value = serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": "resp_nested",
                "model": "gpt-5.4",
                "usage": {
                    "input_tokens": 120,
                    "output_tokens": 40,
                    "input_tokens_details": { "cached_tokens": 20 }
                }
            }
        });
        let usage = parse_openai_usage(&value).unwrap();
        assert_eq!(usage.message_id, "resp_nested");
        assert_eq!(usage.model, "gpt-5.4");
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.cache_read_tokens, 20);
        assert_eq!(usage.cache_create_tokens, 0);
        assert_eq!(usage.output_tokens, 40);
    }

    #[test]
    fn parses_websocket_data_enveloped_usage() {
        let value = serde_json::json!({
            "type": "event",
            "data": {
                "type": "response.completed",
                "response": {
                    "id": "resp_ws",
                    "model": "gpt-5.4",
                    "usage": {
                        "input_tokens": 10,
                        "output_tokens": 5
                    }
                }
            }
        });
        let usage = parse_openai_usage(&value).unwrap();
        assert_eq!(usage.message_id, "resp_ws");
        assert_eq!(usage.model, "gpt-5.4");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
    }

    // ========================================================================
    // parse_openai_stream_usage 两阶段搜索测试
    // ========================================================================

    #[test]
    fn stream_codex_responses_api() {
        // Codex Responses API: response.completed 事件包含 usage
        let events = vec![
            serde_json::json!({
                "type": "response.created",
                "response": { "id": "resp_1" }
            }),
            serde_json::json!({
                "type": "response.output_item.done",
                "item": { "type": "message", "role": "assistant" }
            }),
            serde_json::json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_1",
                    "model": "gpt-5.4",
                    "usage": {
                        "input_tokens": 15054,
                        "output_tokens": 5504,
                        "input_tokens_details": { "cached_tokens": 1200 }
                    }
                }
            }),
        ];
        let usage = parse_openai_stream_usage(&events).unwrap();
        assert_eq!(usage.message_id, "resp_1");
        assert_eq!(usage.model, "gpt-5.4");
        assert_eq!(usage.input_tokens, 13854); // 15054 - 1200
        assert_eq!(usage.output_tokens, 5504);
        assert_eq!(usage.cache_read_tokens, 1200);
    }

    #[test]
    fn stream_openai_chat_completions() {
        // OpenAI Chat Completions: 最后一个 chunk 包含 usage
        let events = vec![
            serde_json::json!({
                "id": "chatcmpl-123",
                "model": "gpt-4o",
                "choices": [{"delta": {"content": "Hello"}}]
            }),
            serde_json::json!({
                "id": "chatcmpl-123",
                "model": "gpt-4o",
                "choices": [{"delta": {}}],
                "usage": {
                    "prompt_tokens": 100,
                    "completion_tokens": 50,
                    "prompt_tokens_details": { "cached_tokens": 20 }
                }
            }),
        ];
        let usage = parse_openai_stream_usage(&events).unwrap();
        assert_eq!(usage.message_id, "chatcmpl-123");
        assert_eq!(usage.model, "gpt-4o");
        assert_eq!(usage.input_tokens, 80); // 100 - 20
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_tokens, 20);
    }

    #[test]
    fn stream_usage_event_parser_ignores_delta_noise() {
        let delta = serde_json::json!({
            "type": "response.output_text.delta",
            "delta": "hello"
        });
        assert!(parse_openai_stream_usage_event(&delta).is_none());

        let completed = serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": "resp_event",
                "model": "gpt-5.4",
                "usage": {
                    "input_tokens": 20,
                    "output_tokens": 7
                }
            }
        });
        let usage = parse_openai_stream_usage_event(&completed).unwrap();
        assert_eq!(usage.message_id, "resp_event");
        assert_eq!(usage.input_tokens, 20);
        assert_eq!(usage.output_tokens, 7);
    }

    #[test]
    fn stream_response_completed_not_last() {
        // response.completed 不是最后一个事件（后面有其他事件）
        let events = vec![
            serde_json::json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_2",
                    "model": "o3",
                    "usage": {
                        "input_tokens": 200,
                        "output_tokens": 100
                    }
                }
            }),
            serde_json::json!({
                "type": "response.exhausted",
                "reason": "max_tokens"
            }),
        ];
        let usage = parse_openai_stream_usage(&events).unwrap();
        assert_eq!(usage.message_id, "resp_2");
        assert_eq!(usage.model, "o3");
        assert_eq!(usage.input_tokens, 200);
        assert_eq!(usage.output_tokens, 100);
    }

    #[test]
    fn stream_no_usage_events() {
        // 没有 usage 信息的事件
        let events = vec![
            serde_json::json!({
                "type": "response.created",
                "response": { "id": "resp_3" }
            }),
            serde_json::json!({
                "type": "response.output_item.done",
                "item": { "type": "message" }
            }),
        ];
        assert!(parse_openai_stream_usage(&events).is_none());
    }

    #[test]
    fn stream_empty_events() {
        let events: Vec<Value> = vec![];
        assert!(parse_openai_stream_usage(&events).is_none());
    }

    #[test]
    fn stream_ignores_null_usage_chunks() {
        // 中间 chunk 的 "usage": null 不应被解析为有效 usage
        let events = vec![
            serde_json::json!({
                "id": "chatcmpl-null",
                "model": "qwen-plus",
                "choices": [{"delta": {"content": "hello"}}],
                "usage": null
            }),
            serde_json::json!({
                "id": "chatcmpl-null",
                "model": "qwen-plus",
                "choices": [],
                "usage": {
                    "prompt_tokens": 100,
                    "completion_tokens": 20,
                    "total_tokens": 120,
                    "prompt_tokens_details": { "cached_tokens": 0 }
                }
            }),
        ];
        let usage = parse_openai_stream_usage(&events).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 20);
    }

    #[test]
    fn stream_all_null_usage_returns_none() {
        // 所有 chunk 的 usage 都是 null，不应产生记录
        let events = vec![
            serde_json::json!({
                "id": "chatcmpl-all-null",
                "model": "qwen-plus",
                "choices": [{"delta": {"content": "hi"}}],
                "usage": null
            }),
            serde_json::json!({
                "id": "chatcmpl-all-null",
                "model": "qwen-plus",
                "choices": [],
                "usage": null
            }),
        ];
        assert!(parse_openai_stream_usage(&events).is_none());
    }

    #[test]
    fn parses_response_api_with_reasoning_tokens() {
        // Response API 非流式响应，包含 output_tokens_details.reasoning_tokens
        let value = serde_json::json!({
            "id": "c9f9c06b-032d-4525-a422-ac8ab5eccxxx",
            "model": "qwen3.6-plus",
            "object": "response",
            "status": "completed",
            "usage": {
                "input_tokens": 55,
                "input_tokens_details": { "cached_tokens": 10 },
                "output_tokens": 43,
                "output_tokens_details": { "reasoning_tokens": 15 },
                "total_tokens": 98
            }
        });
        let usage = parse_openai_usage(&value).unwrap();
        assert_eq!(usage.message_id, "c9f9c06b-032d-4525-a422-ac8ab5eccxxx");
        assert_eq!(usage.model, "qwen3.6-plus");
        assert_eq!(usage.input_tokens, 45); // 55 - 10
        assert_eq!(usage.output_tokens, 43);
        assert_eq!(usage.cache_read_tokens, 10);
        assert_eq!(usage.cache_create_tokens, 0);
        assert_eq!(usage.reasoning_tokens, 15);
    }

    #[test]
    fn parses_response_api_without_output_tokens_details() {
        // Response API 无 output_tokens_details 时 reasoning_tokens 默认为 0
        let value = serde_json::json!({
            "id": "resp_no_details",
            "model": "gpt-4o",
            "usage": {
                "input_tokens": 100,
                "input_tokens_details": { "cached_tokens": 20 },
                "output_tokens": 50
            }
        });
        let usage = parse_openai_usage(&value).unwrap();
        assert_eq!(usage.input_tokens, 80);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.reasoning_tokens, 0);
    }

    #[test]
    fn stream_response_completed_with_reasoning_tokens() {
        // 流式 response.completed 事件包含 reasoning_tokens
        let events = vec![serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": "resp_reasoning",
                "model": "qwen3.6-plus",
                "usage": {
                    "input_tokens": 200,
                    "output_tokens": 100,
                    "input_tokens_details": { "cached_tokens": 50 },
                    "output_tokens_details": { "reasoning_tokens": 30 }
                }
            }
        })];
        let usage = parse_openai_stream_usage(&events).unwrap();
        assert_eq!(usage.message_id, "resp_reasoning");
        assert_eq!(usage.input_tokens, 150); // 200 - 50
        assert_eq!(usage.output_tokens, 100);
        assert_eq!(usage.cache_read_tokens, 50);
        assert_eq!(usage.reasoning_tokens, 30);
    }

    #[test]
    fn normalizes_duplicate_v1_paths() {
        assert_eq!(normalize_openai_path("/v1/v1/responses"), "v1/responses");
        assert_eq!(
            openai_endpoint_url("https://api.openai.com/v1", "/v1/v1/responses"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            openai_endpoint_url("https://chatgpt.com/backend-api/codex", "/v1/responses"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            openai_endpoint_url(
                "https://chatgpt.com/backend-api/codex",
                "/responses/compact"
            ),
            "https://chatgpt.com/backend-api/codex/responses/compact"
        );
        assert_eq!(
            openai_endpoint_url("https://chatgpt.com/backend-api/codex", "/api/codex/apps"),
            "https://chatgpt.com/backend-api/wham/apps"
        );
        assert_eq!(
            openai_endpoint_url(
                "https://chatgpt.com/backend-api/codex",
                "/connectors/directory/list?external_logos=true"
            ),
            "https://chatgpt.com/backend-api/connectors/directory/list?external_logos=true"
        );
    }
}
