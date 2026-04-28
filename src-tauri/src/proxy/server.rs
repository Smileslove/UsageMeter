//! HTTP 代理服务器，用于拦截 Claude API 请求

use super::collector::UsageCollector;
use super::config_manager::ClaudeConfigManager;
use super::forwarder::{ForwardResult, RequestForwarder};
use super::source_detector::{detect_source_info, normalize_base_url, register_source_to_settings};
use super::types::{ProxyConfig, ProxyState, ProxyStatus, RequestContext};
use crate::commands::{load_settings, save_settings};
use crate::models::{ClientToolProfile, DEFAULT_CLIENT_DETECTION_METHOD, DEFAULT_CLIENT_TOOL};
use http_body_util::BodyExt;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::Emitter;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, RwLock};

struct ClientRoute {
    normalized_path: String,
    client_tool: String,
    proxy_profile_id: Option<String>,
    detection_method: String,
    target_base_url: Option<String>,
}

fn trim_path_prefix(prefix: &str) -> String {
    prefix.trim().trim_matches('/').to_string()
}

fn strip_prefix_path(path: &str, prefix: &str) -> Option<String> {
    let clean_prefix = trim_path_prefix(prefix);
    if clean_prefix.is_empty() {
        return None;
    }
    let full_prefix = format!("/{clean_prefix}");
    if path == full_prefix {
        return Some("/".to_string());
    }
    path.strip_prefix(&(full_prefix + "/"))
        .map(|rest| format!("/{rest}"))
}

fn default_profile_for_tool<'a>(
    profiles: &'a [ClientToolProfile],
    tool: &str,
) -> Option<&'a ClientToolProfile> {
    profiles.iter().find(|profile| profile.tool == tool)
}

fn detect_client_route(path: &str) -> ClientRoute {
    let settings = load_settings().unwrap_or_default();
    for profile in &settings.client_tools.profiles {
        if let Some(normalized_path) = strip_prefix_path(path, &profile.path_prefix) {
            return ClientRoute {
                normalized_path,
                client_tool: profile.tool.clone(),
                proxy_profile_id: Some(profile.id.clone()),
                detection_method: "path_prefix".to_string(),
                target_base_url: profile.target_base_url.clone(),
            };
        }
    }

    let proxy_profile_id =
        default_profile_for_tool(&settings.client_tools.profiles, DEFAULT_CLIENT_TOOL)
            .map(|profile| profile.id.clone());
    ClientRoute {
        normalized_path: path.to_string(),
        client_tool: DEFAULT_CLIENT_TOOL.to_string(),
        proxy_profile_id,
        detection_method: DEFAULT_CLIENT_DETECTION_METHOD.to_string(),
        target_base_url: None,
    }
}

/// 辅助函数：创建完整响应体
fn full<T: Into<bytes::Bytes>>(
    chunk: T,
) -> http_body_util::combinators::UnsyncBoxBody<bytes::Bytes, std::io::Error> {
    http_body_util::Full::new(chunk.into())
        .map_err(|never| match never {})
        .boxed_unsync()
}

/// 代理服务器
pub struct ProxyServer {
    /// 代理配置
    config: ProxyConfig,
    /// 共享状态
    state: Arc<ProxyState>,
    /// 关闭信号发送端
    shutdown_tx: Arc<RwLock<Option<oneshot::Sender<()>>>>,
    /// 服务器任务句柄
    server_handle: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
}

impl ProxyServer {
    /// 创建新的代理服务器
    pub fn new(config: ProxyConfig) -> Self {
        let usage_collector = Arc::new(UsageCollector::new());

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.request_timeout))
            .build()
            .expect("Failed to create HTTP client");

        let state = Arc::new(ProxyState {
            usage_collector,
            client,
            config: Arc::new(RwLock::new(config.clone())),
            status: Arc::new(RwLock::new(ProxyStatus::default())),
            start_time: Arc::new(RwLock::new(None)),
            app_handle: Arc::new(RwLock::new(None)),
        });

        Self {
            config,
            state,
            shutdown_tx: Arc::new(RwLock::new(None)),
            server_handle: Arc::new(RwLock::new(None)),
        }
    }

    /// 启动代理服务器
    pub async fn start(&self) -> Result<(), String> {
        // 检查是否已在运行
        if self.is_running().await {
            return Err("Proxy is already running".to_string());
        }

        // 从 Claude 配置获取 API 密钥和目标 URL
        let config_manager = ClaudeConfigManager::new();
        let api_key = config_manager.get_api_key();
        let target_base_url = config_manager
            .get_original_base_url()
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());

        // 接管 Claude 配置。新配置使用路径前缀；服务端仍保留 /v1/messages 兼容旧配置。
        config_manager.takeover_with_path_prefix(self.config.port, Some("claude-code"))?;

        // 创建转发器
        let forwarder = Arc::new(
            RequestForwarder::new(self.state.usage_collector.clone(), target_base_url, api_key)
                .map_err(|e| format!("Failed to create forwarder: {}", e))?,
        );

        // 代理启动时先登记当前有效来源。这样即使 Claude Code 的入站请求不带
        // x-api-key，设置页也能立即看到本次接管对应的 API 来源。
        if let Ok(api_key) = forwarder.get_api_key(None) {
            let target_base_url = forwarder.get_target_base_url();
            let (is_new, updated_settings) =
                register_source_to_settings(&api_key, &target_base_url);
            if let Err(e) = save_settings(updated_settings) {
                eprintln!("[proxy] Failed to save source state on startup: {}", e);
            }
            if is_new {
                if let Some(ref app_handle) = *self.state.app_handle.read().await {
                    let _ = app_handle.emit("source_detected", ());
                }
            }
        }

        // 绑定地址
        let addr: SocketAddr = format!("127.0.0.1:{}", self.config.port)
            .parse()
            .map_err(|e| format!("Invalid address: {}", e))?;

        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| format!("Failed to bind to {}: {}", addr, e))?;

        // 记录启动时间
        *self.state.start_time.write().await = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        );

        // 更新状态
        {
            let mut status = self.state.status.write().await;
            status.running = true;
            status.port = self.config.port;
        }

        // 创建关闭通道
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        *self.shutdown_tx.write().await = Some(shutdown_tx);

        // 克隆状态用于服务器任务
        let state = self.state.clone();

        // 启动服务器任务
        let handle = tokio::spawn(async move {
            loop {
                // 接受新连接
                let (stream, _remote_addr) = tokio::select! {
                    result = listener.accept() => {
                        match result {
                            Ok(conn) => conn,
                            Err(e) => {
                                eprintln!("Accept error: {}", e);
                                continue;
                            }
                        }
                    }
                    _ = &mut shutdown_rx => {
                        // 收到关闭信号
                        break;
                    }
                };

                // 增加活跃连接数
                {
                    let mut status = state.status.write().await;
                    status.active_connections += 1;
                }

                // 为此连接克隆转发器
                let forwarder = forwarder.clone();
                let state_for_conn = state.clone();
                let state_for_decrement = state.clone();

                // 生成任务处理此连接
                tokio::spawn(async move {
                    let io = TokioIo::new(stream);
                    let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                        let forwarder = forwarder.clone();
                        let state = state_for_conn.clone();
                        async move { handle_request(req, forwarder, state).await }
                    });

                    if let Err(e) = http1::Builder::new().serve_connection(io, service).await {
                        eprintln!("Connection error: {}", e);
                    }

                    // 减少活跃连接数
                    {
                        let mut status = state_for_decrement.status.write().await;
                        status.active_connections = status.active_connections.saturating_sub(1);
                    }
                });
            }
        });

        *self.server_handle.write().await = Some(handle);

        Ok(())
    }

    /// 停止代理服务器
    pub async fn stop(&self) -> Result<(), String> {
        if !self.is_running().await {
            return Ok(());
        }

        // 发送关闭信号
        if let Some(tx) = self.shutdown_tx.write().await.take() {
            let _ = tx.send(());
        }

        // 等待服务器任务结束
        if let Some(handle) = self.server_handle.write().await.take() {
            let _ = handle.await;
        }

        // 恢复 Claude 配置
        let config_manager = ClaudeConfigManager::new();
        config_manager.restore()?;

        // 更新状态
        {
            let mut status = self.state.status.write().await;
            status.running = false;
            status.active_connections = 0;
        }

        // 清除启动时间
        *self.state.start_time.write().await = None;

        Ok(())
    }

    /// 检查代理是否运行中
    pub async fn is_running(&self) -> bool {
        self.state.status.read().await.running
    }

    /// 获取代理状态
    pub async fn get_status(&self) -> ProxyStatus {
        let mut status = self.state.status.read().await.clone();

        // 计算运行时间
        if let Some(start_time) = *self.state.start_time.read().await {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            status.uptime_seconds = (now - start_time) as u64;
        }

        // 检查配置是否被接管
        let config_manager = ClaudeConfigManager::new();
        status.config_taken_over = config_manager.is_takeover_active();

        // 从收集器获取记录数
        status.record_count = self.state.usage_collector.record_count().await;

        status
    }

    /// 获取使用量收集器
    pub fn get_collector(&self) -> Arc<UsageCollector> {
        self.state.usage_collector.clone()
    }

    /// 设置 Tauri 应用句柄（用于发送事件）
    #[allow(dead_code)]
    pub async fn set_app_handle(&self, handle: tauri::AppHandle) {
        *self.state.app_handle.write().await = Some(handle);
    }
}

/// 处理传入的 HTTP 请求
async fn handle_request(
    req: Request<hyper::body::Incoming>,
    forwarder: Arc<RequestForwarder>,
    state: Arc<ProxyState>,
) -> Result<
    Response<http_body_util::combinators::UnsyncBoxBody<bytes::Bytes, std::io::Error>>,
    hyper::Error,
> {
    let method = req.method().clone();
    let raw_path = req.uri().path().to_string();
    let client_route = detect_client_route(&raw_path);
    let path = client_route.normalized_path.clone();

    // 增加总请求数
    {
        let mut status = state.status.write().await;
        status.total_requests += 1;
    }

    // 处理健康检查
    if path == "/health" && method == Method::GET {
        let status = state.status.read().await.clone();
        let body = serde_json::to_string(&status).unwrap_or_default();
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(full(body))
            .unwrap());
    }

    // 处理状态端点
    if path == "/status" && method == Method::GET {
        let status = state.status.read().await.clone();
        let body = serde_json::to_string(&status).unwrap_or_default();
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(full(body))
            .unwrap());
    }

    // 处理 Claude Messages API
    if path == "/v1/messages" && method == Method::POST {
        // 在收集请求体之前立即记录开始时间，确保端到端时间准确
        let request_start_time_ms = chrono::Utc::now().timestamp_millis();
        let request_start_instant = std::time::Instant::now();

        // 提取 x-api-key 头用于来源识别。Claude Code 在被接管 base_url 后，
        // 可能不会把 key 作为入站请求头带给本地代理，因此下面会回退到转发器解析到的 key。
        let api_key_header = req
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // 收集请求体（这一步的耗时现在会被计入）
        let body_bytes = req.collect().await?.to_bytes();

        // 获取目标 base_url（从 forwarder 获取）
        let target_base_url = client_route
            .target_base_url
            .clone()
            .unwrap_or_else(|| forwarder.get_target_base_url());

        let effective_api_key = forwarder.get_api_key(api_key_header.as_deref()).ok();

        // 检测并注册来源
        let is_new_source = if let Some(ref api_key) = effective_api_key {
            let (is_new, updated_settings) = register_source_to_settings(api_key, &target_base_url);
            if let Err(e) = save_settings(updated_settings) {
                eprintln!("[proxy] Failed to save source state: {}", e);
            }
            if is_new {
                if let Some(ref app_handle) = *state.app_handle.read().await {
                    let _ = app_handle.emit("source_detected", ());
                }
            }
            is_new
        } else {
            false
        };

        // 获取来源信息用于 RequestContext
        let (api_key_prefix, request_base_url) = if let Some(ref api_key) = effective_api_key {
            let settings = load_settings().unwrap_or_default();
            let sources = settings.source_aware.sources;
            let (prefix, base_url, _) = detect_source_info(api_key, &target_base_url, &sources);
            (prefix, base_url)
        } else {
            (String::new(), normalize_base_url(&target_base_url))
        };

        // 创建请求上下文，使用预先记录的时间
        let context = RequestContext {
            start_time: request_start_instant,
            start_time_ms: request_start_time_ms,
            api_key_prefix: if api_key_prefix.is_empty() {
                None
            } else {
                Some(api_key_prefix)
            },
            request_base_url: request_base_url.clone(),
            client_tool: client_route.client_tool,
            proxy_profile_id: client_route.proxy_profile_id,
            client_detection_method: client_route.detection_method,
            inbound_api_key: api_key_header.clone(),
            target_base_url: Some(target_base_url),
            ..Default::default()
        };

        // 转发请求
        match forwarder.forward_messages(body_bytes, context).await {
            Ok(result) => {
                // 增加成功请求数
                {
                    let mut status = state.status.write().await;
                    status.success_requests += 1;
                }

                let _ = is_new_source;

                match result {
                    ForwardResult::Streaming { body } => {
                        // 流式响应，实时透传
                        Ok(Response::builder()
                            .status(StatusCode::OK)
                            .header("Content-Type", "text/event-stream")
                            .header("Cache-Control", "no-cache")
                            .header("Connection", "keep-alive")
                            .body(body)
                            .unwrap())
                    }
                    ForwardResult::NonStreaming { content } => Ok(Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "application/json")
                        .body(full(content))
                        .unwrap()),
                }
            }
            Err(e) => {
                // 增加失败请求数
                {
                    let mut status = state.status.write().await;
                    status.failed_requests += 1;
                }

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
    } else {
        // 对于未知端点返回 404
        Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header("Content-Type", "application/json")
            .body(full(
                r#"{"error":{"type":"not_found","message":"Endpoint not found"}}"#,
            ))
            .unwrap())
    }
}

impl Default for ProxyServer {
    fn default() -> Self {
        Self::new(ProxyConfig::default())
    }
}
