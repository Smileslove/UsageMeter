//! 流处理器，用于实时透传并收集使用量
//!
//! 提供流式响应处理，在实时转发数据的同时在后台收集使用量统计

use super::collector::UsageCollector;
use super::sse::strip_sse_field;
use super::types::UsageRecord;
use async_stream::stream;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

// ============================================================================
// SSE 使用量收集器
// ============================================================================

/// 使用量完成回调类型
type UsageCallback = Arc<dyn Fn(UsageData) + Send + Sync + 'static>;

/// 从 SSE 事件收集的使用量数据
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct UsageData {
    pub message_id: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub session_id: Option<String>,
    /// 请求开始时间（Unix 毫秒）
    pub request_start_time: i64,
    /// HTTP 响应状态码
    #[allow(dead_code)]
    pub status_code: u16,
    /// 首 Token 生成时间（毫秒）
    pub ttft_ms: Option<u64>,
}

/// SSE 使用量收集器，聚合事件并在完成时触发回调
#[derive(Clone)]
pub struct SseUsageCollector {
    inner: Arc<SseUsageCollectorInner>,
}

struct SseUsageCollectorInner {
    events: Mutex<Vec<Value>>,
    start_time: Instant,
    on_complete: UsageCallback,
    finished: AtomicBool,
    /// 首 Token 时间（检测到第一个 content_block_delta 的时间）
    first_token_time: Mutex<Option<Instant>>,
}

impl SseUsageCollector {
    /// 创建带有完成回调的新使用量收集器
    pub fn new(start_time: Instant, callback: impl Fn(UsageData) + Send + Sync + 'static) -> Self {
        let on_complete: UsageCallback = Arc::new(callback);
        Self {
            inner: Arc::new(SseUsageCollectorInner {
                events: Mutex::new(Vec::new()),
                start_time,
                on_complete,
                finished: AtomicBool::new(false),
                first_token_time: Mutex::new(None),
            }),
        }
    }

    /// 推送 SSE 事件以供后续处理
    pub async fn push(&self, event: Value) {
        // 检测首个 content_block_delta 事件（首 Token 生成）
        if event.get("type").and_then(|v| v.as_str()) == Some("content_block_delta") {
            let mut first_time = self.inner.first_token_time.lock().await;
            if first_time.is_none() {
                *first_time = Some(Instant::now());
            }
        }

        let mut events = self.inner.events.lock().await;
        events.push(event);
    }

    /// 完成收集并触发完成回调
    pub async fn finish(&self) {
        if self.inner.finished.swap(true, Ordering::SeqCst) {
            return;
        }

        let events = {
            let mut guard = self.inner.events.lock().await;
            std::mem::take(&mut *guard)
        };

        // 计算首 Token 生成时间（TTFT）
        let ttft_ms = {
            let first_time = self.inner.first_token_time.lock().await;
            first_time.map(|t| {
                let duration = t.duration_since(self.inner.start_time);
                duration.as_millis() as u64
            })
        };

        // 从收集的事件中解析使用量
        if let Some(mut usage) = parse_usage_from_events(&events) {
            usage.ttft_ms = ttft_ms;
            (self.inner.on_complete)(usage);
        }
    }
}

/// 从收集的 SSE 事件中解析使用量数据
///
/// ## SSE 事件顺序与数据语义
///
/// Anthropic API 的 SSE 流事件顺序：
/// 1. `message_start` - 流开始，包含初始占位 usage（input_tokens 通常为 0 或占位值）
/// 2. `content_block_start` - 内容块开始
/// 3. `content_block_delta` - 内容增量（多次）
/// 4. `message_delta` - **流结束前最后一个数据事件，包含最终 usage**
/// 5. `message_stop` - 流结束信号
///
/// ## Token 统计策略
///
/// **精确统计原则**：优先使用 `message_delta` 中的最终值
///
/// - `input_tokens`: 从 `message_delta.usage.input_tokens` 获取（最终真实值）
/// - `output_tokens`: 从 `message_delta.usage.output_tokens` 获取（最终真实值）
/// - `cache_create_tokens`: 从 `message_start.message.usage.cache_creation_input_tokens` 获取
/// - `cache_read_tokens`: 从 `message_start.message.usage.cache_read_input_tokens` 获取
///
/// 注意：缓存相关 Token 只在 `message_start` 中返回，`message_delta` 中不包含
fn parse_usage_from_events(events: &[Value]) -> Option<UsageData> {
    let mut usage = UsageData::default();

    for event in events {
        if let Some(event_type) = event.get("type").and_then(|v| v.as_str()) {
            match event_type {
                "message_start" => {
                    if let Some(message) = event.get("message") {
                        // 提取消息 ID（唯一标识）
                        if let Some(id) = message.get("id").and_then(|v| v.as_str()) {
                            usage.message_id = id.to_string();
                        }
                        // 提取模型
                        if let Some(m) = message.get("model").and_then(|v| v.as_str()) {
                            usage.model = m.to_string();
                        }
                        // 提取缓存相关 Token（只在 message_start 中返回）
                        if let Some(msg_usage) = message.get("usage") {
                            // 缓存读取 Token
                            usage.cache_read_tokens = msg_usage
                                .get("cache_read_input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            // 缓存创建 Token
                            usage.cache_create_tokens = msg_usage
                                .get("cache_creation_input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            // 注意：不使用 message_start 中的 input_tokens
                            // 因为它通常是 0 或占位值，真实值在 message_delta 中
                        }
                    }
                }
                "message_delta" => {
                    // 最终 usage 数据（最精确）
                    if let Some(delta_usage) = event.get("usage") {
                        // 输入 Token（最终真实值）
                        usage.input_tokens = delta_usage
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        // 输出 Token（最终真实值）
                        usage.output_tokens = delta_usage
                            .get("output_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                    }
                }
                _ => {}
            }
        }
    }

    // 只要有任何 token 使用就认为是有效记录
    if usage.input_tokens > 0
        || usage.output_tokens > 0
        || usage.cache_create_tokens > 0
        || usage.cache_read_tokens > 0
    {
        Some(usage)
    } else {
        None
    }
}

// ============================================================================
// 透传流创建器
// ============================================================================

/// 创建透传流，实时转发数据并收集使用量
///
/// 这是真正流式传输的核心函数：立即 yield 字节，
/// 同时在后台解析 SSE 事件以收集使用量。
pub fn create_passthrough_stream(
    stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + Sync + 'static,
    collector: SseUsageCollector,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + Sync {
    stream! {
        let mut buffer = String::new();
        let mut utf8_remainder: Vec<u8> = Vec::new();

        let mut stream = std::pin::pin!(stream);

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(bytes) => {
                    // 安全处理 UTF-8 边界（仅用于 SSE 解析）
                    append_utf8_safe(&mut buffer, &mut utf8_remainder, &bytes);

                    // 解析完整的 SSE 事件以收集使用量
                    while let Some(pos) = buffer.find("\n\n") {
                        let event_text = buffer[..pos].to_string();
                        buffer = buffer[pos + 2..].to_string();

                        // 提取并解析 SSE 数据
                        for line in event_text.lines() {
                            if let Some(data) = strip_sse_field(line, "data") {
                                if data.trim() != "[DONE]" {
                                    if let Ok(json_value) = serde_json::from_str::<Value>(data) {
                                        collector.push(json_value).await;
                                    }
                                }
                            }
                        }
                    }

                    // 立即转发原始字节（实时透传）
                    yield Ok(bytes);
                }
                Err(e) => {
                    let io_error = std::io::Error::other(e.to_string());
                    yield Err(io_error);
                    break;
                }
            }
        }

        // 流结束，完成使用量收集
        collector.finish().await;
    }
}

/// 安全追加 UTF-8 字节，处理多字节字符边界
fn append_utf8_safe(buffer: &mut String, remainder: &mut Vec<u8>, new_bytes: &[u8]) {
    // 使用 sse 模块的实现
    super::sse::append_utf8_safe(buffer, remainder, new_bytes);
}

// ============================================================================
// 创建收集器的辅助函数
// ============================================================================

/// 创建记录到数据库的使用量收集器
///
/// 统一的计算逻辑：
/// - input_tokens: 原始输入 Token（不含缓存）
/// - total_tokens: input_tokens + cache_create_tokens + cache_read_tokens + output_tokens
/// - duration_ms: 请求耗时（从 start_time 到当前时间）
/// - output_tokens_per_second: output_tokens / (duration_ms / 1000)
pub fn create_database_collector(
    usage_collector: Arc<UsageCollector>,
    context: StreamContext,
    start_time: Instant,
) -> SseUsageCollector {
    // 记录请求开始时间
    let request_start_time = chrono::Utc::now().timestamp_millis();
    let status_code = context.status_code;

    SseUsageCollector::new(start_time, move |usage| {
        // 计算请求结束时间和耗时
        let request_end_time = chrono::Utc::now().timestamp_millis();
        let duration_ms = if usage.request_start_time > 0 {
            request_end_time - usage.request_start_time
        } else {
            request_end_time - request_start_time
        };

        // 计算总 Token：input + cache_create + cache_read + output（含缓存）
        let total_tokens = usage.input_tokens
            + usage.cache_create_tokens
            + usage.cache_read_tokens
            + usage.output_tokens;

        // 计算输出 Token 生成速率（tokens/s）
        let output_tokens_per_second = if duration_ms > 0 {
            Some((usage.output_tokens as f64) / (duration_ms as f64 / 1000.0))
        } else {
            None
        };

        // 使用记录的开始时间，如果没有则使用当前测量的开始时间
        let actual_start_time = if usage.request_start_time > 0 {
            usage.request_start_time
        } else {
            request_start_time
        };

        let record = UsageRecord {
            timestamp: request_end_time,
            message_id: usage.message_id.clone(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_create_tokens: usage.cache_create_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            total_tokens,
            model: usage.model.clone(),
            session_id: usage.session_id.clone(),
            request_start_time: actual_start_time,
            request_end_time,
            duration_ms: duration_ms as u64,
            output_tokens_per_second,
            ttft_ms: usage.ttft_ms,
            status_code,
        };

        let collector = usage_collector.clone();
        tokio::spawn(async move {
            collector.record(record).await;
        });
    })
}

/// 流式请求的上下文
#[derive(Clone)]
#[allow(dead_code)]
pub struct StreamContext {
    #[allow(dead_code)]
    pub cache_create_tokens: u64,
    #[allow(dead_code)]
    pub cache_read_tokens: u64,
    #[allow(dead_code)]
    pub session_id: Option<String>,
    /// 请求开始时间（Unix 毫秒）
    #[allow(dead_code)]
    pub request_start_time: i64,
    /// HTTP 响应状态码
    pub status_code: u16,
}

impl Default for StreamContext {
    fn default() -> Self {
        Self {
            cache_create_tokens: 0,
            cache_read_tokens: 0,
            session_id: None,
            request_start_time: chrono::Utc::now().timestamp_millis(),
            status_code: 200,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_usage_from_events() {
        // 模拟真实的 API 响应流程：
        // message_start 中 input_tokens 为 0（占位值）
        // message_delta 中包含最终的 input_tokens 和 output_tokens
        let events = vec![
            serde_json::json!({
                "type": "message_start",
                "message": {
                    "id": "msg_123",
                    "model": "claude-sonnet-4",
                    "usage": {
                        "input_tokens": 0,  // 占位值
                        "output_tokens": 1,  // 占位值
                        "cache_read_input_tokens": 20,
                        "cache_creation_input_tokens": 10
                    }
                }
            }),
            serde_json::json!({
                "type": "message_delta",
                "usage": {
                    "input_tokens": 100,  // 最终真实值
                    "output_tokens": 50   // 最终真实值
                }
            }),
        ];

        let usage = parse_usage_from_events(&events).unwrap();
        assert_eq!(usage.message_id, "msg_123");
        assert_eq!(usage.model, "claude-sonnet-4");
        assert_eq!(usage.input_tokens, 100); // 来自 message_delta
        assert_eq!(usage.output_tokens, 50); // 来自 message_delta
        assert_eq!(usage.cache_read_tokens, 20); // 来自 message_start
        assert_eq!(usage.cache_create_tokens, 10); // 来自 message_start
    }

    #[test]
    fn test_parse_usage_with_only_cache() {
        // 测试仅有缓存 Token 的情况也应该被记录
        let events = vec![
            serde_json::json!({
                "type": "message_start",
                "message": {
                    "id": "msg_456",
                    "model": "claude-sonnet-4",
                    "usage": {
                        "input_tokens": 0,
                        "cache_read_input_tokens": 100,
                        "cache_creation_input_tokens": 0
                    }
                }
            }),
            serde_json::json!({
                "type": "message_delta",
                "usage": {
                    "input_tokens": 0,
                    "output_tokens": 0
                }
            }),
        ];

        let usage = parse_usage_from_events(&events).unwrap();
        assert_eq!(usage.cache_read_tokens, 100);
    }

    #[test]
    fn test_parse_usage_empty_events() {
        let events: Vec<Value> = vec![];
        assert!(parse_usage_from_events(&events).is_none());
    }
}
