//! 代理类型和数据结构定义

use super::collector::UsageCollector;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// 代理配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// 是否启用代理
    pub enabled: bool,
    /// 监听端口（默认：18765）
    pub port: u16,
    /// 目标 API 基础 URL
    pub target_base_url: String,
    /// 请求超时时间（秒）
    pub request_timeout: u64,
    /// 流式响应空闲超时时间（秒）
    pub streaming_idle_timeout: u64,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 18765,
            target_base_url: "https://api.anthropic.com".to_string(),
            request_timeout: 120,
            streaming_idle_timeout: 30,
        }
    }
}

/// 代理服务器状态
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProxyStatus {
    /// 代理是否运行中
    pub running: bool,
    /// 代理监听的端口
    pub port: u16,
    /// 运行时间（秒）
    pub uptime_seconds: u64,
    /// 已处理的总请求数
    pub total_requests: u64,
    /// 成功的请求数
    pub success_requests: u64,
    /// 失败的请求数
    pub failed_requests: u64,
    /// 当前活跃连接数
    pub active_connections: u64,
    /// Claude 配置是否已被接管
    pub config_taken_over: bool,
    /// 收集器中的记录数量
    pub record_count: usize,
    /// 2xx 状态码请求数
    #[serde(default)]
    pub status_2xx: u64,
    /// 4xx 状态码请求数
    #[serde(default)]
    pub status_4xx: u64,
    /// 5xx 状态码请求数
    #[serde(default)]
    pub status_5xx: u64,
}

/// 单次 API 请求的使用记录
///
/// 数据语义说明（与 Anthropic API 保持一致）：
/// - `input_tokens`: 原始输入 Token 数量（不含缓存相关 Token）
/// - `cache_create_tokens`: 用于创建新缓存的 Token 数量
/// - `cache_read_tokens`: 从已有缓存读取的 Token 数量
/// - `output_tokens`: 输出 Token 数量
/// - `total_tokens`: 实际处理量 = input_tokens + output_tokens（不含缓存）
///
/// 时间和速率字段：
/// - `request_start_time`: 请求开始时间（收到请求时）
/// - `request_end_time`: 请求结束时间（响应完成时）
/// - `duration_ms`: 请求总耗时（毫秒）
/// - `output_tokens_per_second`: 输出 Token 生成速率（tokens/s），仅当 duration > 0 时计算
///
/// 状态码字段：
/// - `status_code`: HTTP 响应状态码（如 200、400、500 等）
///
/// 注意：
/// 1. 四种 token 类型分开存储，用于成本计算（缓存 Token 价格不同）
/// 2. `total_tokens` 是实际处理量，用于使用量统计
/// 3. 不在数据库中存储冗余的 total_tokens，查询时动态计算
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    /// Unix 时间戳（毫秒）- 记录创建时间
    pub timestamp: i64,
    /// 消息 ID（用于去重）
    pub message_id: String,
    /// 输入 Token（不含缓存）
    pub input_tokens: u64,
    /// 输出 Token
    pub output_tokens: u64,
    /// 缓存创建 Token
    pub cache_create_tokens: u64,
    /// 缓存读取 Token
    pub cache_read_tokens: u64,
    /// 总 Token 数 = input + output（实际处理量，内存中计算）
    pub total_tokens: u64,
    /// 使用的模型
    pub model: String,
    /// 会话 ID（如果可用）
    pub session_id: Option<String>,
    /// 请求开始时间（Unix 毫秒）
    pub request_start_time: i64,
    /// 请求结束时间（Unix 毫秒）
    pub request_end_time: i64,
    /// 请求耗时（毫秒）
    pub duration_ms: u64,
    /// 输出 Token 生成速率（tokens/s）
    pub output_tokens_per_second: Option<f64>,
    /// 首 Token 生成时间（毫秒）
    /// 从请求开始到第一个输出 Token 生成的时间
    pub ttft_ms: Option<u64>,
    /// HTTP 响应状态码
    #[serde(default)]
    pub status_code: u16,
}

impl Default for UsageRecord {
    fn default() -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            timestamp: now,
            message_id: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            cache_create_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: 0,
            model: String::new(),
            session_id: None,
            request_start_time: now,
            request_end_time: now,
            duration_ms: 0,
            output_tokens_per_second: None,
            ttft_ms: None,
            status_code: 200,
        }
    }
}

/// 时间窗口统计
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WindowStats {
    /// 窗口名称："5h", "1d", "7d", "1m"
    pub window: String,
    /// 已使用的总 Token 数（实际处理量 = input + output）
    pub token_used: u64,
    /// 输入 Token（实际输入，不含缓存）
    pub input_tokens: u64,
    /// 输出 Token
    pub output_tokens: u64,
    /// 缓存创建 Token
    pub cache_create_tokens: u64,
    /// 缓存读取 Token
    pub cache_read_tokens: u64,
    /// 请求数量
    pub request_used: u64,
    /// 最后更新时间戳
    pub last_updated: i64,
    /// 成功请求数（2xx 状态码）
    #[serde(default)]
    pub success_requests: u64,
    /// 客户端错误请求数（4xx 状态码）
    #[serde(default)]
    pub client_error_requests: u64,
    /// 服务端错误请求数（5xx 状态码）
    #[serde(default)]
    pub server_error_requests: u64,
}

/// 会话统计信息
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    /// 会话 ID
    pub session_id: String,
    /// 总请求数
    pub total_requests: u64,
    /// 总输入 Token
    pub total_input_tokens: u64,
    /// 总输出 Token
    pub total_output_tokens: u64,
    /// 总缓存创建 Token
    pub total_cache_create_tokens: u64,
    /// 总缓存读取 Token
    pub total_cache_read_tokens: u64,
    /// 总耗时（毫秒）
    pub total_duration_ms: u64,
    /// 平均输出 Token 生成速率（tokens/s）
    pub avg_output_tokens_per_second: f64,
    /// 第一个请求时间
    pub first_request_time: i64,
    /// 最后一个请求时间
    pub last_request_time: i64,
    /// 使用的模型列表（去重）
    pub models: Vec<String>,
    /// 平均 TTFT（首 Token 生成时间，毫秒）
    #[serde(default)]
    pub avg_ttft_ms: f64,
    /// 成功请求数（status < 400）
    #[serde(default)]
    pub success_requests: u64,
    /// 错误请求数（status >= 400）
    #[serde(default)]
    pub error_requests: u64,
    /// 估算费用（美元）
    #[serde(default)]
    pub estimated_cost: f64,
    /// 是否为估算费用
    #[serde(default)]
    pub is_cost_estimated: bool,
    // === JSONL 元信息（可选） ===
    /// 工作目录
    #[serde(default)]
    pub cwd: Option<String>,
    /// 项目名称（从 cwd 提取）
    #[serde(default)]
    pub project_name: Option<String>,
    /// 会话主题
    #[serde(default)]
    pub topic: Option<String>,
    /// 最后用户提示
    #[serde(default)]
    pub last_prompt: Option<String>,
    /// 自定义会话名称
    #[serde(default)]
    pub session_name: Option<String>,
}

/// 项目统计信息（聚合多个会话）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStats {
    /// 项目名称
    pub name: String,
    /// 会话数量
    pub session_count: u64,
    /// 总输入 Token
    pub total_input_tokens: u64,
    /// 总输出 Token
    pub total_output_tokens: u64,
    /// 总费用
    pub total_cost: f64,
    /// 最后活跃时间（Unix 时间戳）
    pub last_active: i64,
}

/// 代理状态（在处理器之间共享）
#[allow(dead_code)]
pub struct ProxyState {
    /// 使用量收集器
    pub usage_collector: Arc<UsageCollector>,
    /// 用于转发的 HTTP 客户端
    #[allow(dead_code)]
    pub client: reqwest::Client,
    /// 代理配置
    #[allow(dead_code)]
    pub config: Arc<RwLock<ProxyConfig>>,
    /// 代理状态
    pub status: Arc<RwLock<ProxyStatus>>,
    /// 启动时间（Unix 时间戳，秒）
    pub start_time: Arc<RwLock<Option<i64>>>,
}

/// Claude API 的 SSE 事件类型
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum SseEvent {
    MessageStart {
        message_id: String,
        model: String,
        input_tokens: u64,
    },
    MessageDelta {
        output_tokens: u64,
    },
    MessageStop {
        message_id: String,
    },
    ContentBlockDelta {
        delta_text: String,
    },
    Error {
        error_type: String,
        message: String,
    },
}

/// 请求上下文（用于追踪）
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RequestContext {
    /// 请求中的消息 ID
    #[allow(dead_code)]
    pub message_id: Option<String>,
    /// 请求的模型
    pub model: Option<String>,
    /// 是否启用流式响应
    pub stream: bool,
    /// 会话 ID（如果可用）
    pub session_id: Option<String>,
    /// 请求中的输入 Token
    pub input_tokens: u64,
    /// 请求中的缓存创建 Token
    pub cache_create_tokens: u64,
    /// 请求中的缓存读取 Token
    pub cache_read_tokens: u64,
    /// 请求开始时间（Instant，用于计时）
    #[allow(dead_code)]
    pub start_time: std::time::Instant,
    /// 请求开始时间（Unix 毫秒）
    pub start_time_ms: i64,
}

impl Default for RequestContext {
    fn default() -> Self {
        Self {
            message_id: None,
            model: None,
            stream: false,
            session_id: None,
            input_tokens: 0,
            cache_create_tokens: 0,
            cache_read_tokens: 0,
            start_time: std::time::Instant::now(),
            start_time_ms: chrono::Utc::now().timestamp_millis(),
        }
    }
}

/// Claude settings.json 结构
/// 使用 flatten 保留所有未知字段，确保序列化/反序列化时不会丢失配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClaudeSettings {
    #[serde(default)]
    pub env: serde_json::Map<String, serde_json::Value>,
    /// 权限配置，仅当存在时才序列化
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<serde_json::Value>,
    /// 是否包含 co-authored-by，仅当存在时才序列化
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_co_authored_by: Option<bool>,
    /// 保留所有其他字段（hooks、autoUpdaterStatus 等）
    /// 确保读写时不会丢失任何配置
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

impl ClaudeSettings {
    /// 获取 API 密钥
    pub fn get_api_key(&self) -> Option<String> {
        self.env
            .get("ANTHROPIC_API_KEY")
            .or_else(|| self.env.get("ANTHROPIC_AUTH_TOKEN"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// 获取基础 URL
    pub fn get_base_url(&self) -> Option<String> {
        self.env
            .get("ANTHROPIC_BASE_URL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// 设置基础 URL
    pub fn set_base_url(&mut self, url: &str) {
        self.env.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            serde_json::Value::String(url.to_string()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claude_settings_preserves_unknown_fields() {
        // 模拟真实的 settings.json，包含 hooks 和其他字段
        let json = r#"{
            "autoUpdaterStatus": "disabled",
            "env": {
                "ANTHROPIC_API_KEY": "test-key"
            },
            "hooks": {
                "PostToolUse": [{
                    "hooks": [{ "type": "command", "command": "test.sh" }],
                    "matcher": ""
                }]
            },
            "customField": "someValue"
        }"#;

        // 解析
        let settings: ClaudeSettings = serde_json::from_str(json).unwrap();

        // 验证已知字段
        assert_eq!(settings.env.get("ANTHROPIC_API_KEY").unwrap().as_str().unwrap(), "test-key");

        // 验证未知字段被保留在 'other' 中
        assert!(settings.other.contains_key("hooks"));
        assert!(settings.other.contains_key("customField"));
        assert!(settings.other.contains_key("autoUpdaterStatus"));

        // 序列化回去并验证没有丢失任何内容
        let serialized = serde_json::to_string(&settings).unwrap();
        let reparsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();

        assert!(reparsed.get("hooks").is_some());
        assert!(reparsed.get("customField").is_some());
        assert_eq!(reparsed.get("customField").unwrap().as_str().unwrap(), "someValue");
    }

    #[test]
    fn test_set_base_url_preserves_other_fields() {
        let json = r#"{
            "env": { "ANTHROPIC_API_KEY": "key" },
            "hooks": { "PostToolUse": [] },
            "customField": "value"
        }"#;

        let mut settings: ClaudeSettings = serde_json::from_str(json).unwrap();
        settings.set_base_url("http://127.0.0.1:18765");

        let serialized = serde_json::to_string(&settings).unwrap();
        let reparsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();

        // 验证 BASE_URL 已设置
        assert_eq!(
            reparsed["env"]["ANTHROPIC_BASE_URL"].as_str().unwrap(),
            "http://127.0.0.1:18765"
        );
        // 验证其他字段被保留
        assert!(reparsed.get("hooks").is_some());
        assert_eq!(reparsed["customField"].as_str().unwrap(), "value");
    }

    #[test]
    fn test_optional_fields_not_serialized_when_none() {
        // 当原始 JSON 中不包含 permissions 和 include_co_authored_by 时，
        // 序列化后也不应该出现这些字段（不应该输出 null）
        let json = r#"{
            "env": { "ANTHROPIC_API_KEY": "test-key" }
        }"#;

        let settings: ClaudeSettings = serde_json::from_str(json).unwrap();

        // 验证这些字段是 None
        assert!(settings.permissions.is_none());
        assert!(settings.include_co_authored_by.is_none());

        // 序列化后不应该包含 null 值
        let serialized = serde_json::to_string(&settings).unwrap();
        let reparsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();

        // 不应该包含 permissions 或 include_co_authored_by 字段
        assert!(!reparsed.as_object().unwrap().contains_key("permissions"));
        assert!(!reparsed.as_object().unwrap().contains_key("include_co_authored_by"));
    }

    #[test]
    fn test_optional_fields_preserved_when_present() {
        // 当原始 JSON 中包含 permissions 和 include_co_authored_by 时，
        // 序列化后应该保留这些字段的值
        let json = r#"{
            "env": { "ANTHROPIC_API_KEY": "test-key" },
            "permissions": { "allow": ["*"], "deny": [] },
            "include_co_authored_by": true
        }"#;

        let settings: ClaudeSettings = serde_json::from_str(json).unwrap();

        // 验证这些字段已被解析
        assert!(settings.permissions.is_some());
        assert!(settings.include_co_authored_by.is_some());

        // 序列化后应该保留这些字段
        let serialized = serde_json::to_string(&settings).unwrap();
        let reparsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();

        // 应该包含这些字段且值正确
        assert!(reparsed.get("permissions").is_some());
        assert!(reparsed.get("include_co_authored_by").is_some());
        assert!(reparsed["include_co_authored_by"].as_bool().unwrap());
    }
}
