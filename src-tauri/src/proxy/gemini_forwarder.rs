//! Google Generative Language API 代理转发与用量采集（Gemini CLI）。
//!
//! Gemini 响应在 `usageMetadata` 字段携带用量：
//! - `promptTokenCount`：输入（含缓存命中）
//! - `cachedContentTokenCount`：缓存命中（input 的子集）
//! - `candidatesTokenCount`：候选输出
//! - `thoughtsTokenCount`：思考/推理（计入输出）
//! - `totalTokenCount`：总数
//!
//! 归一化口径（与本地链一致）：
//! - `input = promptTokenCount - cachedContentTokenCount`
//! - `cache_read = cachedContentTokenCount`
//! - `output = candidatesTokenCount + thoughtsTokenCount`
//! - `reasoning = thoughtsTokenCount`
//! - `cache_create = 0`
//!
//! 流式（`:streamGenerateContent?alt=sse`）取最后一个携带 usageMetadata 的 chunk。

use super::collector::UsageCollector;
use super::sse::{append_utf8_safe, strip_sse_field, take_sse_block};
use super::types::{RequestContext, UsageRecord};
use crate::net::HttpClientFactory;
use async_stream::stream;
use bytes::Bytes;
use futures::StreamExt;
use http_body_util::StreamBody;
use hyper::body::Frame;
use hyper::{header, HeaderMap, Method};
use reqwest::Client;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub type BoxBody = http_body_util::combinators::UnsyncBoxBody<Bytes, std::io::Error>;
const USAGE_MISSING_STATUS_CODE: u16 = 599;

pub enum GeminiForwardResult {
    Streaming {
        status_code: u16,
        headers: Vec<(String, String)>,
        body: BoxBody,
    },
    NonStreaming {
        status_code: u16,
        headers: Vec<(String, String)>,
        content: Vec<u8>,
    },
    UpstreamError {
        status_code: u16,
        headers: Vec<(String, String)>,
        content: Vec<u8>,
    },
}

pub struct GeminiForwarder {
    client: Client,
    streaming_client: Client,
    usage_collector: Arc<UsageCollector>,
}

#[derive(Debug, Clone, Default)]
struct GeminiUsage {
    message_id: String,
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    reasoning_tokens: u64,
}

impl GeminiForwarder {
    pub fn new(
        usage_collector: Arc<UsageCollector>,
        request_timeout_secs: u64,
        streaming_idle_timeout_secs: u64,
    ) -> Result<Self, String> {
        let builder = Client::builder()
            .timeout(Duration::from_secs(request_timeout_secs))
            .http1_only()
            .http1_title_case_headers()
            .pool_max_idle_per_host(0)
            .no_gzip()
            .no_brotli()
            .no_deflate();
        let client = HttpClientFactory::global()
            .apply_proxy_to_builder(builder)
            .build()
            .map_err(|e| format!("Failed to create Gemini HTTP client: {}", e))?;
        let streaming_builder = Client::builder()
            .connect_timeout(Duration::from_secs(request_timeout_secs))
            .http1_only()
            .http1_title_case_headers()
            .pool_max_idle_per_host(0)
            .no_gzip()
            .no_brotli()
            .no_deflate();
        let streaming_builder = if streaming_idle_timeout_secs > 0 {
            streaming_builder.read_timeout(Duration::from_secs(streaming_idle_timeout_secs))
        } else {
            streaming_builder
        };
        let streaming_client = HttpClientFactory::global()
            .apply_proxy_to_builder(streaming_builder)
            .build()
            .map_err(|e| format!("Failed to create Gemini streaming HTTP client: {}", e))?;
        Ok(Self {
            client,
            streaming_client,
            usage_collector,
        })
    }

    pub async fn forward_with_usage(
        &self,
        method: Method,
        path: &str,
        headers: HeaderMap,
        body: bytes::Bytes,
        context: RequestContext,
    ) -> Result<GeminiForwardResult, String> {
        self.forward(method, path, headers, body, context, true)
            .await
    }

    pub async fn forward_passthrough(
        &self,
        method: Method,
        path: &str,
        headers: HeaderMap,
        body: bytes::Bytes,
        context: RequestContext,
    ) -> Result<GeminiForwardResult, String> {
        self.forward(method, path, headers, body, context, false)
            .await
    }

    async fn forward(
        &self,
        method: Method,
        path: &str,
        headers: HeaderMap,
        body: bytes::Bytes,
        context: RequestContext,
        capture_usage: bool,
    ) -> Result<GeminiForwardResult, String> {
        let target_base_url = context
            .target_base_url
            .clone()
            .ok_or_else(|| "No target base URL found for Gemini provider".to_string())?;
        let url = gemini_endpoint_url(&target_base_url, path);

        let upstream_is_sse_request =
            super::gemini_api::is_gemini_streaming_path(path) || accepts_event_stream(&headers);
        let reqwest_method = reqwest::Method::from_bytes(method.as_str().as_bytes())
            .unwrap_or(reqwest::Method::POST);
        let client = if upstream_is_sse_request {
            &self.streaming_client
        } else {
            &self.client
        };
        let mut request = client.request(reqwest_method, &url);
        request = apply_passthrough_headers(request, &headers);
        let response = request
            .body(body)
            .send()
            .await
            .map_err(|e| format!("Failed to send Gemini request: {}", e))?;

        let status = response.status();
        let status_code = status.as_u16();
        let upstream_is_sse = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.contains("text/event-stream"))
            .unwrap_or(false);
        let response_headers = collect_passthrough_response_headers(response.headers());

        if !status.is_success() {
            let content = response
                .bytes()
                .await
                .map_err(|e| format!("Failed to read Gemini error response: {}", e))?
                .to_vec();
            if capture_usage {
                self.record_error(&context, status_code).await;
            }
            return Ok(GeminiForwardResult::UpstreamError {
                status_code,
                headers: response_headers,
                content,
            });
        }

        if upstream_is_sse {
            Ok(GeminiForwardResult::Streaming {
                status_code,
                headers: response_headers,
                body: self
                    .handle_streaming(response, context, capture_usage)
                    .await?,
            })
        } else {
            let bytes = response
                .bytes()
                .await
                .map_err(|e| format!("Failed to read Gemini response: {}", e))?;
            if capture_usage {
                let usage = serde_json::from_slice::<Value>(&bytes)
                    .ok()
                    .and_then(|value| parse_gemini_usage(&value));
                record_usage_optional(
                    self.usage_collector.clone(),
                    usage,
                    context,
                    status_code,
                    None,
                )
                .await;
            }
            Ok(GeminiForwardResult::NonStreaming {
                status_code,
                headers: response_headers,
                content: bytes.to_vec(),
            })
        }
    }

    async fn record_error(&self, context: &RequestContext, status_code: u16) {
        let now = chrono::Utc::now().timestamp_millis();
        let duration_ms = now.saturating_sub(context.start_time_ms) as u64;
        let record = UsageRecord {
            timestamp: now,
            message_id: format!("gemini_error_{}_{}", now, status_code),
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

    async fn handle_streaming(
        &self,
        response: reqwest::Response,
        context: RequestContext,
        capture_usage: bool,
    ) -> Result<BoxBody, String> {
        let status_code = response.status().as_u16();
        let collector = self.usage_collector.clone();
        let usage_candidate = Arc::new(Mutex::new(None::<GeminiUsage>));
        let first_token_time = Arc::new(Mutex::new(None::<Instant>));
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
                        if capture_usage {
                            append_utf8_safe(&mut buffer, &mut utf8_remainder, &bytes);
                            while let Some(event_text) = take_sse_block(&mut buffer) {
                                for line in event_text.lines() {
                                    if let Some(data) = strip_sse_field(line, "data") {
                                        if data.trim() != "[DONE]" {
                                            if let Ok(value) = serde_json::from_str::<Value>(data) {
                                                if gemini_first_token_candidate(&value) {
                                                    let mut first = first_token_time.lock().await;
                                                    if first.is_none() {
                                                        *first = Some(Instant::now());
                                                    }
                                                }
                                                if let Some(usage) = parse_gemini_usage(&value) {
                                                    *usage_candidate.lock().await = Some(usage);
                                                }
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

            if capture_usage {
                let usage = usage_candidate.lock().await.take();
                let ttft_ms = first_token_time.lock().await.map(|instant| {
                    let elapsed_ms = instant.duration_since(ttft_start).as_millis() as u64;
                    elapsed_ms.max(1)
                });
                record_usage_optional(collector, usage, context_for_finish, status_code, ttft_ms)
                    .await;
            }
        };

        Ok(http_body_util::BodyExt::boxed_unsync(StreamBody::new(
            passthrough,
        )))
    }
}

async fn record_usage_optional(
    collector: Arc<UsageCollector>,
    usage: Option<GeminiUsage>,
    context: RequestContext,
    status_code: u16,
    ttft_ms: Option<u64>,
) {
    let now = chrono::Utc::now().timestamp_millis();
    let duration_ms = now.saturating_sub(context.start_time_ms) as u64;
    match usage {
        Some(usage) => {
            let output_tokens_per_second = if duration_ms > 0 {
                Some(usage.output_tokens as f64 / (duration_ms as f64 / 1000.0))
            } else {
                None
            };
            let message_id = if usage.message_id.is_empty() {
                format!("gemini_{}_{}", now, duration_ms)
            } else {
                usage.message_id.clone()
            };
            let total_tokens = usage.input_tokens + usage.cache_read_tokens + usage.output_tokens;
            let record = UsageRecord {
                timestamp: now,
                message_id,
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_create_tokens: 0,
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
                api_key_prefix: context.api_key_prefix.clone(),
                request_base_url: context.request_base_url.clone(),
                client_tool: context.client_tool.clone(),
                proxy_profile_id: context.proxy_profile_id.clone(),
                client_detection_method: context.client_detection_method.clone(),
                ..Default::default()
            };
            collector.record(record).await;
        }
        None => {
            let record = UsageRecord {
                timestamp: now,
                message_id: format!("gemini_usage_missing_{}_{}", now, status_code),
                model: context.model.clone().unwrap_or_default(),
                session_id: context.session_id.clone(),
                request_start_time: context.start_time_ms,
                request_end_time: now,
                duration_ms,
                ttft_ms,
                status_code: if (200..300).contains(&status_code) {
                    USAGE_MISSING_STATUS_CODE
                } else {
                    status_code
                },
                api_key_prefix: context.api_key_prefix.clone(),
                request_base_url: context.request_base_url.clone(),
                client_tool: context.client_tool.clone(),
                proxy_profile_id: context.proxy_profile_id.clone(),
                client_detection_method: context.client_detection_method.clone(),
                ..Default::default()
            };
            collector.record(record).await;
        }
    }
}

fn accepts_event_stream(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains("text/event-stream"))
        .unwrap_or(false)
}

fn apply_passthrough_headers(
    mut request: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        let name = name.as_str();
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "host" | "content-length" | "connection" | "accept-encoding" | "proxy-authorization"
        ) {
            continue;
        }
        if let Ok(value) = value.to_str() {
            request = request.header(name, value);
        }
    }
    request
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

fn gemini_endpoint_url(target_base_url: &str, path: &str) -> String {
    let base = target_base_url.trim_end_matches('/');
    let raw_path = path.trim_start_matches('/');
    format!("{}/{}", base, raw_path)
}

fn gemini_first_token_candidate(value: &Value) -> bool {
    value
        .get("candidates")
        .and_then(|v| v.as_array())
        .map(|candidates| {
            candidates.iter().any(|candidate| {
                candidate
                    .get("content")
                    .and_then(|content| content.get("parts"))
                    .and_then(|parts| parts.as_array())
                    .map(|parts| {
                        parts.iter().any(|part| {
                            part.get("text")
                                .and_then(|t| t.as_str())
                                .map(|t| !t.is_empty())
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn parse_gemini_usage(value: &Value) -> Option<GeminiUsage> {
    let usage = value.get("usageMetadata")?;

    let prompt_tokens = usage
        .get("promptTokenCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cached_tokens = usage
        .get("cachedContentTokenCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let candidates_tokens = usage
        .get("candidatesTokenCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let thoughts_tokens = usage
        .get("thoughtsTokenCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    // 全零（且无 total）视为无有效 usage。
    let total = usage
        .get("totalTokenCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if prompt_tokens == 0 && candidates_tokens == 0 && thoughts_tokens == 0 && total == 0 {
        return None;
    }

    let cache_read = cached_tokens.min(prompt_tokens);
    let input = prompt_tokens.saturating_sub(cache_read);
    let output = candidates_tokens + thoughts_tokens;

    let message_id = value
        .get("responseId")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let model = value
        .get("modelVersion")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    Some(GeminiUsage {
        message_id,
        model,
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        reasoning_tokens: thoughts_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_non_stream_usage_metadata() {
        let value = serde_json::json!({
            "responseId": "resp_abc",
            "modelVersion": "gemini-2.5-pro",
            "candidates": [{"content": {"parts": [{"text": "hi"}]}}],
            "usageMetadata": {
                "promptTokenCount": 100,
                "cachedContentTokenCount": 40,
                "candidatesTokenCount": 30,
                "thoughtsTokenCount": 10,
                "totalTokenCount": 140
            }
        });
        let usage = parse_gemini_usage(&value).unwrap();
        assert_eq!(usage.message_id, "resp_abc");
        assert_eq!(usage.model, "gemini-2.5-pro");
        assert_eq!(usage.input_tokens, 60); // 100 - 40
        assert_eq!(usage.cache_read_tokens, 40);
        assert_eq!(usage.output_tokens, 40); // 30 + 10
        assert_eq!(usage.reasoning_tokens, 10);
    }

    #[test]
    fn parses_usage_without_cache_or_thoughts() {
        let value = serde_json::json!({
            "modelVersion": "gemini-2.5-flash",
            "usageMetadata": {
                "promptTokenCount": 12,
                "candidatesTokenCount": 8,
                "totalTokenCount": 20
            }
        });
        let usage = parse_gemini_usage(&value).unwrap();
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.output_tokens, 8);
        assert_eq!(usage.reasoning_tokens, 0);
        assert_eq!(usage.message_id, "");
    }

    #[test]
    fn returns_none_when_no_usage_metadata() {
        let value = serde_json::json!({
            "candidates": [{"content": {"parts": [{"text": "hi"}]}}]
        });
        assert!(parse_gemini_usage(&value).is_none());
    }

    #[test]
    fn returns_none_on_all_zero_usage() {
        let value = serde_json::json!({
            "usageMetadata": {
                "promptTokenCount": 0,
                "candidatesTokenCount": 0,
                "totalTokenCount": 0
            }
        });
        assert!(parse_gemini_usage(&value).is_none());
    }

    #[test]
    fn first_token_candidate_detects_text_parts() {
        let with_text = serde_json::json!({
            "candidates": [{"content": {"parts": [{"text": "hello"}]}}]
        });
        assert!(gemini_first_token_candidate(&with_text));

        let empty = serde_json::json!({
            "candidates": [{"content": {"parts": [{"text": ""}]}}]
        });
        assert!(!gemini_first_token_candidate(&empty));
    }

    #[test]
    fn builds_endpoint_url() {
        assert_eq!(
            gemini_endpoint_url(
                "https://generativelanguage.googleapis.com",
                "/v1beta/models/gemini-2.5-pro:generateContent"
            ),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:generateContent"
        );
    }
}
