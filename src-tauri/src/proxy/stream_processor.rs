//! 流处理器，用于实时透传并收集使用量
//!
//! 提供流式响应处理，在实时转发数据的同时在后台收集使用量统计

use super::collector::UsageCollector;
use super::sse::SseEventReader;
use super::types::{output_tokens_per_second, UsageRecord};
use async_stream::stream;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

// ============================================================================
// SSE 使用量收集器
// ============================================================================

const USAGE_MISSING_STATUS_CODE: u16 = 599;

/// 使用量完成回调类型。None 表示流正常结束，但未收到可解析的 Usage。
type UsageCallback = Arc<dyn Fn(Option<UsageData>) + Send + Sync + 'static>;

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
    /// HTTP 响应状态码
    #[allow(dead_code)]
    pub status_code: u16,
    /// 首 Token 生成时间（毫秒）
    pub ttft_ms: Option<u64>,
    pub generation_duration: Option<Duration>,
    has_final_usage: bool,
    /// API Key 前缀（用于来源识别）
    pub api_key_prefix: Option<String>,
    /// 实际请求目标 base_url
    pub request_base_url: Option<String>,
    /// 发起请求的客户端工具
    pub client_tool: String,
    /// 匹配到的客户端工具 profile ID
    pub proxy_profile_id: Option<String>,
    /// 工具识别方式
    pub client_detection_method: String,
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
    /// 首 Token 时间（检测到第一个非空输出 delta 的时间）
    first_token_time: Mutex<Option<Instant>>,
}

impl SseUsageCollector {
    /// 创建带有完成回调的新使用量收集器
    pub fn new(
        start_time: Instant,
        callback: impl Fn(Option<UsageData>) + Send + Sync + 'static,
    ) -> Self {
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
        if anthropic_first_output_candidate(&event) {
            let mut first_time = self.inner.first_token_time.lock().await;
            if first_time.is_none() {
                *first_time = Some(Instant::now());
            }
        }

        let mut events = self.inner.events.lock().await;
        events.push(event);
    }

    /// 完成收集并触发完成回调
    pub async fn finish(&self, completed_normally: bool) {
        if self.inner.finished.swap(true, Ordering::SeqCst) {
            return;
        }
        let finish_time = Instant::now();

        let events = {
            let mut guard = self.inner.events.lock().await;
            std::mem::take(&mut *guard)
        };

        // 计算首 Token 生成时间（TTFT）
        let first_token_time = {
            let first_time = self.inner.first_token_time.lock().await;
            *first_time
        };
        let ttft_ms = first_token_time.map(|t| {
            t.saturating_duration_since(self.inner.start_time)
                .as_millis()
                .max(1) as u64
        });

        // 从收集的事件中解析使用量。流正常结束但缺少 Usage 时仍触发回调，
        // 由持久化层记录 599 accounting error，避免静默漏统。
        let usage = parse_usage_from_events(&events).map(|mut usage| {
            usage.ttft_ms = ttft_ms;
            usage.generation_duration = if completed_normally && usage.has_final_usage {
                first_token_time.and_then(|first| finish_time.checked_duration_since(first))
            } else {
                None
            };
            usage
        });
        (self.inner.on_complete)(usage);
    }
}

/// 从收集的 SSE 事件中解析使用量数据
///
/// ## SSE 事件顺序与数据语义
///
/// Anthropic API 的 SSE 流事件顺序：
/// 1. `message_start` - 流开始，包含 Message 对象和初始 usage
/// 2. `content_block_start/delta/stop` - 内容块（可能多个）
/// 3. `message_delta` - 流结束前的最终数据，usage 为累积值
/// 4. `message_stop` - 流结束信号
///
/// ## Token 统计策略
///
/// **累积值覆盖原则**：`message_delta` 中的 usage 字段为累积值，优先级最高
///
/// - `input_tokens`: `message_delta` 有则用其值，否则保留 `message_start` 的值
/// - `output_tokens`: 从 `message_delta` 获取（最终累积值）
/// - `cache_create_tokens`: `message_delta` 有则用其值，否则保留 `message_start` 的值
/// - `cache_read_tokens`: `message_delta` 有则用其值，否则保留 `message_start` 的值
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
                        // 提取初始 usage 数据（message_start 中的值可能被 message_delta 覆盖）
                        if let Some(msg_usage) = message.get("usage") {
                            usage.input_tokens = msg_usage
                                .get("input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            usage.cache_read_tokens = msg_usage
                                .get("cache_read_input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            usage.cache_create_tokens = msg_usage
                                .get("cache_creation_input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                        }
                    }
                }
                "message_delta" => {
                    // 最终 usage 数据（累积值，最精确）
                    if let Some(delta_usage) = event.get("usage") {
                        usage.input_tokens = delta_usage
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(usage.input_tokens);
                        if let Some(output_tokens) = delta_usage
                            .get("output_tokens")
                            .and_then(|value| value.as_u64())
                        {
                            usage.output_tokens = output_tokens;
                            usage.has_final_usage = true;
                        }
                        // 缓存字段在 message_delta 中也是累积值，覆盖 message_start 的初始值
                        usage.cache_create_tokens = delta_usage
                            .get("cache_creation_input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(usage.cache_create_tokens);
                        usage.cache_read_tokens = delta_usage
                            .get("cache_read_input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(usage.cache_read_tokens);
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

fn anthropic_first_output_candidate(event: &Value) -> bool {
    match event.get("type").and_then(|value| value.as_str()) {
        Some("content_block_delta") => event
            .get("delta")
            .and_then(Value::as_object)
            .map(|delta| {
                ["text", "thinking", "partial_json", "data"]
                    .iter()
                    .any(|key| {
                        delta
                            .get(*key)
                            .and_then(Value::as_str)
                            .map(|value| !value.is_empty())
                            .unwrap_or(false)
                    })
            })
            .unwrap_or(false),
        _ => false,
    }
}

// ============================================================================
// 透传流创建器
// ============================================================================

/// 创建透传流，实时转发数据并收集使用量
///
/// 这是真正流式传输的核心函数：立即 yield 字节，
/// 同时在后台解析 SSE 事件以收集使用量。
///
/// `streaming_idle_timeout` 控制上游 SSE 静默断流后的回收：
/// `None` 表示关闭（不设空闲超时）；`Some(d)` 表示两次数据块之间
/// 超过 `d` 没有新数据时，yield `Err` 并结束流。该超时不依赖
/// reqwest 客户端的 `read_timeout` 配置，对半开连接同样有效。
pub fn create_passthrough_stream(
    stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + Sync + 'static,
    collector: SseUsageCollector,
    streaming_idle_timeout: Option<Duration>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + Sync {
    stream! {
        let mut reader = SseEventReader::new();
        let mut stream = std::pin::pin!(stream);
        let mut completed_normally = false;

        loop {
            let chunk_result = match streaming_idle_timeout {
                Some(timeout) => match tokio::time::timeout(timeout, stream.next()).await {
                    Ok(Some(next)) => next,
                    Ok(None) => {
                        completed_normally = true;
                        break;
                    }
                    Err(_) => {
                        yield Err(std::io::Error::other(
                            "SSE stream idle timeout: no data from upstream",
                        ));
                        break;
                    }
                },
                None => match stream.next().await {
                    Some(next) => next,
                    None => {
                        completed_normally = true;
                        break;
                    }
                },
            };

            match chunk_result {
                Ok(bytes) => {
                    // 解析完整的 SSE 事件以收集使用量（驱动公共实现）
                    reader
                        .push_bytes(&bytes, |json_value| {
                            let collector = collector.clone();
                            async move {
                                collector.push(json_value).await;
                            }
                        })
                        .await;

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

        // 流尾未以空行结束的最后一个事件
        reader
            .finish(|json_value| {
                let collector = collector.clone();
                async move {
                    collector.push(json_value).await;
                }
            })
            .await;

        // 流结束，完成使用量收集
        collector.finish(completed_normally).await;
    }
}

// ============================================================================
// 创建收集器的辅助函数
// ============================================================================

/// 创建记录到数据库的使用量收集器
///
/// 统一的计算逻辑：
/// - input_tokens: 原始输入 Token（不含缓存）
/// - total_tokens: input_tokens + cache_create_tokens + cache_read_tokens + output_tokens
/// - output_tokens_per_second: 完整流式请求中 output_tokens / 首个非空输出至流结束时长
pub fn create_database_collector(
    usage_collector: Arc<UsageCollector>,
    context: StreamContext,
    start_time: Instant,
) -> SseUsageCollector {
    // 使用 StreamContext 中的真实开始时间，确保 duration_ms 计算准确
    let request_start_time = context.request_start_time;
    let status_code = context.status_code;
    let api_key_prefix = context.api_key_prefix.clone();
    let request_base_url = context.request_base_url.clone();
    let client_tool = context.client_tool.clone();
    let proxy_profile_id = context.proxy_profile_id.clone();
    let client_detection_method = context.client_detection_method.clone();
    let ingress_kind = context.ingress_kind.clone();
    let gateway_profile_id = context.gateway_profile_id.clone();
    let gateway_caller_label = context.gateway_caller_label.clone();
    let usage_source = context.usage_source.clone();
    let gateway_request_id = context.gateway_request_id.clone();

    SseUsageCollector::new(start_time, move |usage| {
        // 计算请求结束时间和耗时
        let request_end_time = chrono::Utc::now().timestamp_millis();
        // 使用 StreamContext 中传递的真实开始时间
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let usage = usage.unwrap_or_default();
        let has_usage = !usage.message_id.is_empty()
            || !usage.model.is_empty()
            || usage.input_tokens > 0
            || usage.output_tokens > 0
            || usage.cache_create_tokens > 0
            || usage.cache_read_tokens > 0;
        let usage_complete = usage.has_final_usage;

        let total_tokens = usage.input_tokens
            + usage.cache_create_tokens
            + usage.cache_read_tokens
            + usage.output_tokens;

        let output_tokens_per_second = if has_usage {
            output_tokens_per_second(usage.output_tokens, usage.generation_duration)
        } else {
            None
        };

        let effective_status_code =
            if (!has_usage || !usage_complete) && (200..300).contains(&status_code) {
                USAGE_MISSING_STATUS_CODE
            } else {
                status_code
            };
        let message_id = if usage.message_id.is_empty() {
            format!(
                "anthropic_usage_missing_{}_{}",
                request_end_time, effective_status_code
            )
        } else {
            usage.message_id.clone()
        };

        let mut record = UsageRecord {
            timestamp: request_end_time,
            message_id,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_create_tokens: usage.cache_create_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            reasoning_tokens: 0,
            total_tokens,
            model: usage.model.clone(),
            session_id: usage.session_id.clone(),
            request_start_time,
            request_end_time,
            duration_ms,
            output_tokens_per_second,
            ttft_ms: usage.ttft_ms,
            status_code: effective_status_code,
            estimated_cost: 0.0,
            pricing_snapshot_id: None,
            cost_locked: false,
            api_key_prefix: api_key_prefix.clone(),
            request_base_url: request_base_url.clone(),
            client_tool: client_tool.clone(),
            proxy_profile_id: proxy_profile_id.clone(),
            client_detection_method: client_detection_method.clone(),
            ..Default::default()
        };
        record.ingress_kind = ingress_kind.clone();
        record.gateway_profile_id = gateway_profile_id.clone();
        record.gateway_caller_label = gateway_caller_label.clone();
        record.usage_source = usage_source.clone();
        record.gateway_request_id = gateway_request_id.clone();
        if !has_usage || !usage_complete {
            record.usage_source = crate::proxy::types::default_usage_source();
        }

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
    /// API Key 前缀（用于来源识别）
    pub api_key_prefix: Option<String>,
    /// 实际请求目标 base_url
    pub request_base_url: Option<String>,
    /// 发起请求的客户端工具
    pub client_tool: String,
    /// 匹配到的客户端工具 profile ID
    pub proxy_profile_id: Option<String>,
    /// 工具识别方式
    pub client_detection_method: String,
    /// 请求进入 UsageMeter 的方式。
    pub ingress_kind: String,
    /// Gateway profile ID。
    pub gateway_profile_id: Option<String>,
    /// Gateway 调用方标签。
    pub gateway_caller_label: Option<String>,
    /// usage 的可信来源。
    pub usage_source: String,
    /// Gateway 本地请求 ID。
    pub gateway_request_id: Option<String>,
}

impl Default for StreamContext {
    fn default() -> Self {
        Self {
            cache_create_tokens: 0,
            cache_read_tokens: 0,
            session_id: None,
            request_start_time: chrono::Utc::now().timestamp_millis(),
            status_code: 200,
            api_key_prefix: None,
            request_base_url: None,
            client_tool: crate::models::DEFAULT_CLIENT_TOOL.to_string(),
            proxy_profile_id: None,
            client_detection_method: crate::models::DEFAULT_CLIENT_DETECTION_METHOD.to_string(),
            ingress_kind: crate::proxy::types::default_ingress_kind(),
            gateway_profile_id: None,
            gateway_caller_label: None,
            usage_source: crate::proxy::types::default_usage_source(),
            gateway_request_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_usage_from_events() {
        // 模拟 API 响应流程：
        // message_start 提供初始 usage，message_delta 用累积值覆盖
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
    fn test_parse_usage_delta_overrides_cache() {
        // message_delta 中的缓存字段应覆盖 message_start 的值
        // 模拟 web search 场景：message_delta 返回完整累积 usage
        let events = vec![
            serde_json::json!({
                "type": "message_start",
                "message": {
                    "id": "msg_789",
                    "model": "claude-opus-4-7",
                    "usage": {
                        "input_tokens": 2679,
                        "output_tokens": 3,
                        "cache_creation_input_tokens": 0,
                        "cache_read_input_tokens": 0
                    }
                }
            }),
            serde_json::json!({
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                "usage": {
                    "input_tokens": 10682,
                    "cache_creation_input_tokens": 500,
                    "cache_read_input_tokens": 200,
                    "output_tokens": 510
                }
            }),
        ];

        let usage = parse_usage_from_events(&events).unwrap();
        assert_eq!(usage.input_tokens, 10682); // message_delta 覆盖
        assert_eq!(usage.output_tokens, 510);
        assert_eq!(usage.cache_create_tokens, 500); // message_delta 覆盖 message_start 的 0
        assert_eq!(usage.cache_read_tokens, 200); // message_delta 覆盖 message_start 的 0
    }

    #[test]
    fn test_parse_usage_empty_events() {
        let events: Vec<Value> = vec![];
        assert!(parse_usage_from_events(&events).is_none());
    }

    #[tokio::test]
    async fn finish_reports_missing_usage_once() {
        let results = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = results.clone();
        let collector = SseUsageCollector::new(Instant::now(), move |usage| {
            captured.lock().unwrap().push(usage);
        });

        collector.finish(true).await;
        collector.finish(true).await;

        let results = results.lock().unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].is_none());
    }

    #[tokio::test]
    async fn finish_reports_parsed_usage() {
        let results = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = results.clone();
        let collector = SseUsageCollector::new(Instant::now(), move |usage| {
            captured.lock().unwrap().push(usage);
        });
        collector
            .push(serde_json::json!({
                "type": "message_start",
                "message": {
                    "id": "msg_usage",
                    "model": "claude-sonnet-4",
                    "usage": { "input_tokens": 12 }
                }
            }))
            .await;
        collector
            .push(serde_json::json!({
                "type": "message_delta",
                "usage": { "output_tokens": 8 }
            }))
            .await;

        collector.finish(true).await;

        let results = results.lock().unwrap();
        let usage = results[0].as_ref().expect("parsed usage");
        assert_eq!(usage.message_id, "msg_usage");
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 8);
        assert!(usage.has_final_usage);
    }

    #[tokio::test]
    async fn incomplete_anthropic_stream_keeps_ttft_but_has_no_rate_duration() {
        let results = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = results.clone();
        let collector = SseUsageCollector::new(Instant::now(), move |usage| {
            captured.lock().unwrap().push(usage);
        });
        collector
            .push(serde_json::json!({
                "type": "content_block_delta",
                "delta": {"type": "text_delta", "text": "hello"}
            }))
            .await;
        collector
            .push(serde_json::json!({
                "type": "message_delta",
                "usage": {"output_tokens": 8}
            }))
            .await;

        collector.finish(false).await;

        let results = results.lock().unwrap();
        let usage = results[0].as_ref().expect("parsed usage");
        assert!(usage.ttft_ms.is_some());
        assert_eq!(usage.generation_duration, None);
    }

    #[test]
    fn anthropic_usage_without_final_message_delta_is_incomplete() {
        let usage = parse_usage_from_events(&[serde_json::json!({
            "type": "message_start",
            "message": {
                "id": "msg_partial",
                "model": "claude-sonnet",
                "usage": {"input_tokens": 12}
            }
        })])
        .expect("message_start usage is retained for accounting");
        assert_eq!(usage.input_tokens, 12);
        assert!(!usage.has_final_usage);
    }

    #[test]
    fn anthropic_first_output_detection_ignores_empty_and_metadata_deltas() {
        assert!(!anthropic_first_output_candidate(&serde_json::json!({
            "type": "content_block_start",
            "content_block": {"type": "tool_use", "name": "tool"}
        })));
        assert!(!anthropic_first_output_candidate(&serde_json::json!({
            "type": "content_block_delta",
            "delta": {"type": "text_delta", "text": ""}
        })));
        assert!(!anthropic_first_output_candidate(&serde_json::json!({
            "type": "content_block_delta",
            "delta": {"type": "signature_delta", "signature": "sig"}
        })));
        assert!(anthropic_first_output_candidate(&serde_json::json!({
            "type": "content_block_delta",
            "delta": {"type": "thinking_delta", "thinking": "reasoning"}
        })));
    }
}
