//! 请求转发器，用于将请求代理到 Anthropic API

use super::collector::UsageCollector;
use super::stream_processor::{
    create_database_collector, create_passthrough_stream, StreamContext,
};
use super::types::{RequestContext, SseEvent, UsageRecord};
use bytes::Bytes;
use futures::TryStreamExt;
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use reqwest::Client;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// UnsyncBoxBody 类型别名，用于响应体（不需要 Sync）
/// 错误类型为 std::io::Error，实现了 Into<Box<dyn StdError + Send + Sync>>
pub type BoxBody = http_body_util::combinators::UnsyncBoxBody<Bytes, std::io::Error>;

/// 请求转发器，将请求代理到 Anthropic API
pub struct RequestForwarder {
    /// HTTP 客户端
    client: Client,
    /// 使用量收集器
    usage_collector: Arc<UsageCollector>,
    /// 目标基础 URL
    target_base_url: String,
    /// API 密钥（来自 Claude 设置）
    api_key: Option<String>,
}

/// 转发请求的结果
pub enum ForwardResult {
    /// 流式响应（SSE）- 返回 BoxBody 用于实时透传
    Streaming { body: BoxBody },
    /// 非流式响应（JSON）
    NonStreaming { content: Vec<u8> },
}

fn messages_endpoint_url(target_base_url: &str) -> String {
    let base = target_base_url.trim_end_matches('/');
    if let Some(prefix) = base.strip_suffix("/v1") {
        format!("{}/v1/messages", prefix)
    } else {
        format!("{}/v1/messages", base)
    }
}

impl RequestForwarder {
    /// 创建新的请求转发器
    pub fn new(
        usage_collector: Arc<UsageCollector>,
        target_base_url: String,
        api_key: Option<String>,
    ) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

        Ok(Self {
            client,
            usage_collector,
            target_base_url,
            api_key,
        })
    }

    /// 获取目标 base_url
    pub fn get_target_base_url(&self) -> String {
        self.target_base_url.clone()
    }

    /// 获取要使用的 API 密钥
    pub(crate) fn get_api_key(
        &self,
        inbound_api_key: Option<&str>,
        target_api_key: Option<&str>,
    ) -> Result<String, String> {
        inbound_api_key
            .map(|key| key.to_string())
            .or_else(|| target_api_key.map(|key| key.to_string()))
            .or_else(|| self.api_key.clone())
            .or_else(|| {
                // 尝试从 Claude 设置获取
                let manager = super::config_manager::ClaudeConfigManager::new();
                manager.get_api_key()
            })
            .ok_or_else(|| {
                "No API key found. Please configure ANTHROPIC_API_KEY in Claude settings."
                    .to_string()
            })
    }

    /// 将请求转发到 Anthropic API
    pub async fn forward_messages(
        &self,
        body: bytes::Bytes,
        mut context: RequestContext,
    ) -> Result<ForwardResult, String> {
        let api_key = match self.get_api_key(
            context.inbound_api_key.as_deref(),
            context.target_api_key.as_deref(),
        ) {
            Ok(key) => key,
            Err(e) => {
                // 记录无 key 的错误，确保请求被统计
                let now = chrono::Utc::now().timestamp_millis();
                let record = UsageRecord {
                    timestamp: now,
                    message_id: format!("no_key_{}", now),
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_create_tokens: 0,
                    cache_read_tokens: 0,
                    total_tokens: 0,
                    model: context.model.clone().unwrap_or_default(),
                    session_id: context.session_id.clone(),
                    request_start_time: context.start_time_ms,
                    request_end_time: now,
                    duration_ms: 0,
                    output_tokens_per_second: None,
                    ttft_ms: None,
                    status_code: 502,
                    estimated_cost: 0.0,
                    pricing_snapshot_id: None,
                    cost_locked: false,
                    api_key_prefix: context.api_key_prefix,
                    request_base_url: context.request_base_url,
                    client_tool: context.client_tool,
                    proxy_profile_id: context.proxy_profile_id,
                    client_detection_method: context.client_detection_method,
                };
                self.usage_collector.record(record).await;
                return Err(e);
            }
        };
        let target_base_url = context
            .target_base_url
            .clone()
            .unwrap_or_else(|| self.target_base_url.clone());
        let url = messages_endpoint_url(&target_base_url);

        // 解析请求体以提取元数据
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) {
            context.model = json
                .get("model")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            context.stream = json
                .get("stream")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            // 如果可用，从请求中提取使用量信息
            if let Some(usage) = json.get("usage") {
                context.input_tokens = usage
                    .get("input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                context.cache_create_tokens = usage
                    .get("cache_creation_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                context.cache_read_tokens = usage
                    .get("cache_read_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
            }
        }

        // 构建请求
        let mut request = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .body(body);

        // 添加 anthropic-dangerous-direct-browser-access 头，支持浏览器式访问
        request = request.header("anthropic-dangerous-direct-browser-access", "true");

        // 发送请求
        let response = request
            .send()
            .await
            .map_err(|e| format!("Failed to send request: {}", e))?;

        let status = response.status();
        let is_streaming = context.stream;
        let status_code = status.as_u16();

        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();

            // 即使是错误响应，也记录请求（无 token 数据，但有状态码）
            let request_end_time = chrono::Utc::now().timestamp_millis();
            let duration_ms = request_end_time - context.start_time_ms;

            let record = UsageRecord {
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
                api_key_prefix: context.api_key_prefix,
                request_base_url: context.request_base_url,
                client_tool: context.client_tool,
                proxy_profile_id: context.proxy_profile_id,
                client_detection_method: context.client_detection_method,
            };
            self.usage_collector.record(record).await;

            return Err(format!("API error ({}): {}", status, error_body));
        }

        if is_streaming {
            // 处理流式响应
            self.handle_streaming_response(response, context).await
        } else {
            // 处理非流式响应
            self.handle_non_streaming_response(response, context).await
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
    ) -> Result<ForwardResult, String> {
        let start_time = Instant::now();
        let status_code = response.status().as_u16();

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
        };

        // 创建使用量收集器，用于记录到数据库
        let collector =
            create_database_collector(self.usage_collector.clone(), stream_context, start_time);

        // 获取响应的字节流
        let stream = response.bytes_stream();

        // 创建透传流，实时转发
        let passthrough_stream = create_passthrough_stream(stream, collector);

        // 转换为 StreamBody 然后转为 UnsyncBoxBody
        // 保持 std::io::Error 作为错误类型（hyper 接受任何实现了
        // Into<Box<dyn StdError + Send + Sync>> 的错误）
        let stream_body = StreamBody::new(passthrough_stream.map_ok(Frame::data));

        Ok(ForwardResult::Streaming {
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
    ) -> Result<ForwardResult, String> {
        let request_end_time = chrono::Utc::now().timestamp_millis();
        let request_start_time = context.start_time_ms;
        let duration_ms = request_end_time - request_start_time;
        let status_code = response.status().as_u16();

        let body = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read response body: {}", e))?;

        // 解析响应以提取使用量
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) {
            let message_id = json
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();

            let model = json
                .get("model")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();

            // 从响应的 usage 中提取各项 Token
            let input_tokens = json
                .get("usage")
                .and_then(|u| u.get("input_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            let output_tokens = json
                .get("usage")
                .and_then(|u| u.get("output_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            let cache_create = json
                .get("usage")
                .and_then(|u| u.get("cache_creation_input_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            let cache_read = json
                .get("usage")
                .and_then(|u| u.get("cache_read_input_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            // 计算总 Token：原始输入 + 缓存创建 + 缓存读取 + 输出
            let total_tokens = input_tokens + cache_create + cache_read + output_tokens;

            // 计算输出 Token 生成速率（tokens/s）
            let output_tokens_per_second = if duration_ms > 0 {
                Some((output_tokens as f64) / (duration_ms as f64 / 1000.0))
            } else {
                None
            };

            // 记录使用量
            let record = UsageRecord {
                timestamp: request_end_time,
                message_id,
                input_tokens,
                output_tokens,
                cache_create_tokens: cache_create,
                cache_read_tokens: cache_read,
                total_tokens,
                model,
                session_id: context.session_id,
                request_start_time,
                request_end_time,
                duration_ms: duration_ms as u64,
                output_tokens_per_second,
                ttft_ms: None, // 非流式请求无法计算 TTFT
                status_code,
                estimated_cost: 0.0,
                pricing_snapshot_id: None,
                cost_locked: false,
                api_key_prefix: context.api_key_prefix,
                request_base_url: context.request_base_url,
                client_tool: context.client_tool,
                proxy_profile_id: context.proxy_profile_id,
                client_detection_method: context.client_detection_method,
            };
            self.usage_collector.record(record).await;
        }

        Ok(ForwardResult::NonStreaming {
            content: body.to_vec(),
        })
    }
}

/// 从文本解析 SSE 事件
#[allow(dead_code)]
fn parse_sse_event(text: &str) -> Option<SseEvent> {
    for line in text.lines() {
        if let Some(json_str) = line.strip_prefix("data: ") {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(json_str) {
                let event_type = json.get("type")?.as_str()?;

                match event_type {
                    "message_start" => {
                        let message = json.get("message")?;
                        let message_id = message.get("id")?.as_str()?.to_string();
                        let model = message.get("model")?.as_str()?.to_string();
                        let usage = message.get("usage")?;
                        let input_tokens = usage.get("input_tokens")?.as_u64()?;

                        return Some(SseEvent::MessageStart {
                            message_id,
                            model,
                            input_tokens,
                        });
                    }
                    "message_delta" => {
                        let usage = json.get("usage")?;
                        let output_tokens = usage.get("output_tokens")?.as_u64()?;

                        return Some(SseEvent::MessageDelta { output_tokens });
                    }
                    "message_stop" => {
                        // message_stop 事件本身没有 message_id
                        // 我们使用之前收集的 message_id
                        return Some(SseEvent::MessageStop {
                            message_id: String::new(),
                        });
                    }
                    "content_block_delta" => {
                        let delta = json.get("delta")?;
                        let delta_text = delta
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        return Some(SseEvent::ContentBlockDelta { delta_text });
                    }
                    "error" => {
                        let error = json.get("error")?;
                        let error_type = error
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let message = error
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Unknown error")
                            .to_string();
                        return Some(SseEvent::Error {
                            error_type,
                            message,
                        });
                    }
                    _ => {}
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_messages_endpoint_url_normalizes_v1_suffix() {
        assert_eq!(
            messages_endpoint_url("https://api.example.com"),
            "https://api.example.com/v1/messages"
        );
        assert_eq!(
            messages_endpoint_url("https://api.example.com/v1"),
            "https://api.example.com/v1/messages"
        );
        assert_eq!(
            messages_endpoint_url("https://api.example.com/api/v1/"),
            "https://api.example.com/api/v1/messages"
        );
    }

    #[test]
    fn test_parse_sse_message_start() {
        let event = r#"data: {"type":"message_start","message":{"id":"msg_123","model":"claude-sonnet-4","usage":{"input_tokens":100}}}"#;
        let result = parse_sse_event(event);
        assert!(matches!(result, Some(SseEvent::MessageStart { .. })));
    }

    #[test]
    fn test_parse_sse_message_delta() {
        let event = r#"data: {"type":"message_delta","usage":{"output_tokens":50}}"#;
        let result = parse_sse_event(event);
        assert!(matches!(
            result,
            Some(SseEvent::MessageDelta { output_tokens: 50 })
        ));
    }
}
