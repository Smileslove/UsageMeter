//! SSE 工具函数，用于处理 Server-Sent Events
//!
//! 提供 UTF-8 安全的缓冲区处理和 SSE 字段解析

use serde_json::Value;
use std::future::Future;

/// 从一行中剥离 SSE 字段前缀
///
/// 支持 "field: value" 和 "field:value" 两种格式（空格可选）
#[inline]
pub fn strip_sse_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    line.strip_prefix(&format!("{field}: "))
        .or_else(|| line.strip_prefix(&format!("{field}:")))
}

/// 将原始字节安全地追加到 UTF-8 `String` 缓冲区，
/// 正确处理跨块边界的多字节字符。
///
/// `remainder` 累积上一块中形成不完整 UTF-8 序列的尾随字节
/// （正常运行时最多 3 字节）。每次调用时，将 remainder 预置到 `new_bytes`，
/// 将最长的有效 UTF-8 前缀追加到 `buffer`，并将任何尾随不完整字节保存
/// 回 `remainder` 以备下次调用。
///
/// 防御性保护：如果 `remainder` 曾经超过 3 字节，则通过丢失转换丢弃它，
/// 这在格式良好的 UTF-8 流中不可能发生。
pub fn append_utf8_safe(buffer: &mut String, remainder: &mut Vec<u8>, new_bytes: &[u8]) {
    // 构建要解码的字节切片：预置上一块的剩余字节
    let (owned, bytes): (Option<Vec<u8>>, &[u8]) = if remainder.is_empty() {
        (None, new_bytes)
    } else {
        // 防御性保护：remainder 永远不应超过 3 字节（最大的不完整
        // UTF-8 序列是 3 字节：一个 4 字节字符缺少最后一个字节）。如果
        // 超过，说明流产生了真正无效的字节；以丢失方式刷新它们
        // 并重新开始。
        if remainder.len() > 3 {
            buffer.push_str(&String::from_utf8_lossy(remainder));
            remainder.clear();
            (None, new_bytes)
        } else {
            let mut combined = std::mem::take(remainder);
            combined.extend_from_slice(new_bytes);
            (Some(combined), &[])
        }
    };
    let input = owned.as_deref().unwrap_or(bytes);

    // 解码循环：消耗所有有效 UTF-8 和任何真正无效的字节，
    // 只留下尾随的不完整序列在 remainder 中。
    let mut pos = 0;
    loop {
        match std::str::from_utf8(&input[pos..]) {
            Ok(s) => {
                buffer.push_str(s);
                // 所有内容已消耗 - remainder 保持为空
                return;
            }
            Err(e) => {
                let valid_up_to = pos + e.valid_up_to();
                buffer.push_str(
                    // 安全性：from_utf8 保证 [pos..valid_up_to] 是有效的 UTF-8
                    std::str::from_utf8(&input[pos..valid_up_to]).unwrap(),
                );
                if let Some(invalid_len) = e.error_len() {
                    // 真正无效的字节 - 发出 U+FFFD 并继续
                    buffer.push('\u{FFFD}');
                    pos = valid_up_to + invalid_len;
                } else {
                    // 不完整的尾随序列 - 暂存以备下一块
                    *remainder = input[valid_up_to..].to_vec();
                    return;
                }
            }
        }
    }
}

/// Take one complete SSE block from the front of the buffer.
///
/// The SSE spec uses a blank line as the event delimiter. In practice streams
/// may delimit with either `\n\n` or `\r\n\r\n`; supporting both is important
/// because upstream HTTP stacks often normalize line endings differently.
pub fn take_sse_block(buffer: &mut String) -> Option<String> {
    let lf_pos = buffer.find("\n\n");
    let crlf_pos = buffer.find("\r\n\r\n");

    let (pos, delimiter_len) = match (lf_pos, crlf_pos) {
        (Some(lf), Some(crlf)) if crlf <= lf => (crlf, 4),
        (Some(lf), _) => (lf, 2),
        (None, Some(crlf)) => (crlf, 4),
        (None, None) => return None,
    };

    let block = buffer[..pos].to_string();
    buffer.drain(..pos + delimiter_len);
    Some(block)
}

/// 统一的 SSE 事件流读取器，驱动底层解析工具，向调用方暴露事件回调。
///
/// 三个转发器（Anthropic / OpenAI / Gemini）的流式消费循环都基于
/// `append_utf8_safe` + `take_sse_block` + `strip_sse_field` 的同一套骨架。
/// 本结构把这段骨架收敛为公共实现：`push_bytes` 处理增量字节，
/// `finish` 处理流尾未以空行结束的最后一个事件。[DONE] 过滤与
/// `serde_json::from_str` 都在公共层完成，调用方只保留消费回调。
pub struct SseEventReader {
    buffer: String,
    remainder: Vec<u8>,
}

impl Default for SseEventReader {
    fn default() -> Self {
        Self::new()
    }
}

impl SseEventReader {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            remainder: Vec::new(),
        }
    }

    /// 追加一段字节并回调解析出的每个 SSE 事件（已过滤 [DONE]、已是 JSON）。
    ///
    /// 回调可返回 Future 以便调用方在消费逻辑中 await（例如异步锁、
    /// 异步收集器），公共层按顺序逐事件驱动回调。
    pub async fn push_bytes<F, Fut>(&mut self, new_bytes: &[u8], mut on_event: F)
    where
        F: FnMut(Value) -> Fut,
        Fut: Future<Output = ()>,
    {
        append_utf8_safe(&mut self.buffer, &mut self.remainder, new_bytes);
        while let Some(event_text) = take_sse_block(&mut self.buffer) {
            self.emit_event_text(&event_text, &mut on_event).await;
        }
    }

    /// 处理流尾：若缓冲区中残留未以空行结束的最后一个事件，同样回调。
    ///
    /// 尾随的不完整 UTF-8 字节（`remainder`）无法构成合法事件，直接丢弃。
    /// 调用后读取器回到初始状态，可安全复用。
    pub async fn finish<F, Fut>(&mut self, mut on_event: F)
    where
        F: FnMut(Value) -> Fut,
        Fut: Future<Output = ()>,
    {
        self.remainder.clear();
        if self.buffer.is_empty() {
            return;
        }
        let trailing = std::mem::take(&mut self.buffer);
        self.emit_event_text(&trailing, &mut on_event).await;
    }

    async fn emit_event_text<F, Fut>(&mut self, event_text: &str, on_event: &mut F)
    where
        F: FnMut(Value) -> Fut,
        Fut: Future<Output = ()>,
    {
        for line in event_text.lines() {
            if let Some(data) = strip_sse_field(line, "data") {
                if data.trim() != "[DONE]" {
                    if let Ok(value) = serde_json::from_str::<Value>(data) {
                        on_event(value).await;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn strip_sse_field_accepts_optional_space() {
        assert_eq!(
            strip_sse_field("data: {\"ok\":true}", "data"),
            Some("{\"ok\":true}")
        );
        assert_eq!(
            strip_sse_field("data:{\"ok\":true}", "data"),
            Some("{\"ok\":true}")
        );
        assert_eq!(
            strip_sse_field("event: message_start", "event"),
            Some("message_start")
        );
        assert_eq!(
            strip_sse_field("event:message_start", "event"),
            Some("message_start")
        );
        assert_eq!(strip_sse_field("id:1", "data"), None);
    }

    #[test]
    fn ascii_passthrough() {
        let mut buf = String::new();
        let mut rem = Vec::new();
        append_utf8_safe(&mut buf, &mut rem, b"hello world");
        assert_eq!(buf, "hello world");
        assert!(rem.is_empty());
    }

    #[test]
    fn complete_multibyte_in_single_chunk() {
        let mut buf = String::new();
        let mut rem = Vec::new();
        append_utf8_safe(&mut buf, &mut rem, "你好世界".as_bytes());
        assert_eq!(buf, "你好世界");
        assert!(rem.is_empty());
    }

    #[test]
    fn split_multibyte_across_two_chunks() {
        // "你" = E4 BD A0 (3 字节)
        let bytes = "你".as_bytes();
        assert_eq!(bytes.len(), 3);

        let mut buf = String::new();
        let mut rem = Vec::new();

        // 块 1：前 2 字节（不完整）
        append_utf8_safe(&mut buf, &mut rem, &bytes[..2]);
        assert_eq!(buf, "");
        assert_eq!(rem.len(), 2);

        // 块 2：最后一字节完成该字符
        append_utf8_safe(&mut buf, &mut rem, &bytes[2..]);
        assert_eq!(buf, "你");
        assert!(rem.is_empty());
    }

    #[test]
    fn split_four_byte_char_across_chunks() {
        // 😀 = F0 9F 98 80 (4 字节)
        let bytes = "😀".as_bytes();
        assert_eq!(bytes.len(), 4);

        let mut buf = String::new();
        let mut rem = Vec::new();

        // 每次发送 1 字节
        append_utf8_safe(&mut buf, &mut rem, &bytes[..1]);
        assert_eq!(buf, "");
        assert_eq!(rem.len(), 1);

        append_utf8_safe(&mut buf, &mut rem, &bytes[1..2]);
        assert_eq!(buf, "");
        assert_eq!(rem.len(), 2);

        append_utf8_safe(&mut buf, &mut rem, &bytes[2..3]);
        assert_eq!(buf, "");
        assert_eq!(rem.len(), 3);

        append_utf8_safe(&mut buf, &mut rem, &bytes[3..]);
        assert_eq!(buf, "😀");
        assert!(rem.is_empty());
    }

    #[test]
    fn mixed_ascii_and_split_multibyte() {
        // "hi你" = 68 69 E4 BD A0
        let all = "hi你".as_bytes();
        assert_eq!(all.len(), 5);

        let mut buf = String::new();
        let mut rem = Vec::new();

        // 块 1："hi" + "你" 的第一个字节
        append_utf8_safe(&mut buf, &mut rem, &all[..3]);
        assert_eq!(buf, "hi");
        assert_eq!(rem.len(), 1);

        // 块 2："你" 的剩余 2 字节
        append_utf8_safe(&mut buf, &mut rem, &all[3..]);
        assert_eq!(buf, "hi你");
        assert!(rem.is_empty());
    }

    #[test]
    fn sse_json_with_chinese_split_at_boundary() {
        // 模拟 SSE 数据行在边界处分割中文内容
        let json_line = "data: {\"text\":\"你好\"}\n\n";
        let bytes = json_line.as_bytes();

        // 找到 "你" 在字节流中的起始位置并在那里分割
        let ni_start = bytes.windows(3).position(|w| w == "你".as_bytes()).unwrap();
        let split_point = ni_start + 1; // 在 "你" 内部分割

        let mut buf = String::new();
        let mut rem = Vec::new();

        append_utf8_safe(&mut buf, &mut rem, &bytes[..split_point]);
        append_utf8_safe(&mut buf, &mut rem, &bytes[split_point..]);

        assert_eq!(buf, json_line);
        assert!(rem.is_empty());

        // 验证缓冲区可以解析为带有有效 JSON 的 SSE
        let data = strip_sse_field(buf.lines().next().unwrap(), "data").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(data).unwrap();
        assert_eq!(parsed["text"], "你好");
    }

    #[test]
    fn take_sse_block_accepts_lf_and_crlf_delimiters() {
        let mut lf = "data: {\"a\":1}\n\ndata: {\"b\":2}\n\n".to_string();
        assert_eq!(take_sse_block(&mut lf).as_deref(), Some("data: {\"a\":1}"));
        assert_eq!(take_sse_block(&mut lf).as_deref(), Some("data: {\"b\":2}"));
        assert!(take_sse_block(&mut lf).is_none());

        let mut crlf = "event: done\r\ndata: {\"ok\":true}\r\n\r\n".to_string();
        assert_eq!(
            take_sse_block(&mut crlf).as_deref(),
            Some("event: done\r\ndata: {\"ok\":true}")
        );
        assert!(take_sse_block(&mut crlf).is_none());
    }

    #[tokio::test]
    async fn reader_emits_event_split_across_chunk_boundary() {
        // 一个事件被拆成两块：第一块只到事件中间，第二块以空行结尾
        let first = "data: {\"type\":\"delta\",\"t";
        let second = "ext\":\"hi\"}\n\ndata: {\"type\":\"stop\"}\n\n";
        let mut reader = SseEventReader::new();
        let events = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        reader
            .push_bytes(first.as_bytes(), |value| {
                let events = events.clone();
                async move {
                    events.lock().await.push(value);
                }
            })
            .await;
        assert!(events.lock().await.is_empty());
        reader
            .push_bytes(second.as_bytes(), |value| {
                let events = events.clone();
                async move {
                    events.lock().await.push(value);
                }
            })
            .await;
        let events = events.lock().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["text"], "hi");
        assert_eq!(events[1]["type"], "stop");
    }

    #[tokio::test]
    async fn reader_emits_utf8_char_split_across_chunk_boundary() {
        // "你" = E4 BD A0，把它拆在两个块之间，事件仍应被完整解析
        let full = "data: {\"text\":\"你好\"}\n\n";
        let split_point = full.find("你").unwrap() + 1;
        let mut reader = SseEventReader::new();
        let events = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        reader
            .push_bytes(&full.as_bytes()[..split_point], |value| {
                let events = events.clone();
                async move {
                    events.lock().await.push(value);
                }
            })
            .await;
        assert!(events.lock().await.is_empty());
        reader
            .push_bytes(&full.as_bytes()[split_point..], |value| {
                let events = events.clone();
                async move {
                    events.lock().await.push(value);
                }
            })
            .await;
        let events = events.lock().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["text"], "你好");
    }

    #[tokio::test]
    async fn reader_finish_emits_trailing_event_without_blank_line() {
        // 流尾没有空行：push_bytes 阶段不触发，finish 阶段触发
        let trailing = "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":5}}";
        let mut reader = SseEventReader::new();
        let events = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        reader
            .push_bytes(trailing.as_bytes(), |value| {
                let events = events.clone();
                async move {
                    events.lock().await.push(value);
                }
            })
            .await;
        assert!(events.lock().await.is_empty());
        reader
            .finish(|value| {
                let events = events.clone();
                async move {
                    events.lock().await.push(value);
                }
            })
            .await;
        assert_eq!(events.lock().await.len(), 1);
        assert_eq!(events.lock().await[0]["type"], "message_delta");

        // finish 之后读取器已清空，再次 finish 不会重复回调
        reader
            .finish(|value| {
                let events = events.clone();
                async move {
                    events.lock().await.push(value);
                }
            })
            .await;
        assert_eq!(events.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn reader_filters_done_marker() {
        let chunk = "data: {\"type\":\"delta\"}\n\ndata: [DONE]\n\n";
        let mut reader = SseEventReader::new();
        let events = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        reader
            .push_bytes(chunk.as_bytes(), |value| {
                let events = events.clone();
                async move {
                    events.lock().await.push(value);
                }
            })
            .await;
        let events = events.lock().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "delta");
    }

    #[tokio::test]
    async fn reader_ignores_non_json_and_non_data_lines() {
        let chunk = "event: ping\ndata: not-json\n\n: comment\n\ndata: {\"a\":1}\n\n";
        let mut reader = SseEventReader::new();
        let events = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        reader
            .push_bytes(chunk.as_bytes(), |value| {
                let events = events.clone();
                async move {
                    events.lock().await.push(value);
                }
            })
            .await;
        let events = events.lock().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["a"], 1);
    }
}
