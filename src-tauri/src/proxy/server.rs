//! HTTP 代理服务器，用于拦截 Claude API 请求

use super::codex_config::{CodexAuthMode, CodexConfigManager, CodexSourceRegistry};
use super::collector::UsageCollector;
use super::config_manager::ClaudeConfigManager;
use super::forwarder::{ForwardResult, RequestForwarder};
use super::openai_forwarder::{OpenAiForwardResult, OpenAiForwarder};
use super::source_detector::{detect_source_info, normalize_base_url, register_source_to_settings};
use super::source_registry::{ProxySourceHandle, ProxySourceRegistry};
use super::types::{ProxyConfig, ProxyState, ProxyStatus, RequestContext};
use crate::commands::{load_settings, save_settings};
use crate::models::{ClientToolProfile, DEFAULT_CLIENT_DETECTION_METHOD, DEFAULT_CLIENT_TOOL};
use http_body_util::BodyExt;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::Emitter;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, RwLock};

const CONFIG_MONITOR_POLL_INTERVAL: Duration = Duration::from_secs(5);

struct ClientRoute {
    normalized_path: String,
    client_tool: String,
    proxy_profile_id: Option<String>,
    detection_method: String,
    target_base_url: Option<String>,
    source_id: Option<String>,
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
    for profile in settings
        .client_tools
        .profiles
        .iter()
        .filter(|profile| profile.enabled)
    {
        if let Some(normalized_path) = strip_prefix_path(path, &profile.path_prefix) {
            let (normalized_path, source_id) = strip_source_handle_path(&normalized_path);
            return ClientRoute {
                normalized_path,
                client_tool: profile.tool.clone(),
                proxy_profile_id: Some(profile.id.clone()),
                detection_method: if source_id.is_some() {
                    "path_prefix_source".to_string()
                } else {
                    "path_prefix".to_string()
                },
                target_base_url: profile.target_base_url.clone(),
                source_id,
            };
        }
    }

    // CC Switch style Codex traffic may arrive without the configured /codex prefix.
    if is_codex_api_path(path) {
        // 找到 Codex profile
        if let Some(codex_profile) = settings
            .client_tools
            .profiles
            .iter()
            .find(|p| p.tool == "codex" && p.enabled)
        {
            return ClientRoute {
                normalized_path: path.to_string(),
                client_tool: "codex".to_string(),
                proxy_profile_id: Some(codex_profile.id.clone()),
                detection_method: "endpoint_match".to_string(),
                target_base_url: codex_profile.target_base_url.clone(),
                source_id: None,
            };
        }
    }

    let default_profile =
        default_profile_for_tool(&settings.client_tools.profiles, DEFAULT_CLIENT_TOOL)
            .filter(|profile| profile.enabled);
    ClientRoute {
        normalized_path: path.to_string(),
        client_tool: default_profile
            .map(|profile| profile.tool.clone())
            .unwrap_or_else(|| "unknown".to_string()),
        proxy_profile_id: default_profile.map(|profile| profile.id.clone()),
        detection_method: DEFAULT_CLIENT_DETECTION_METHOD.to_string(),
        target_base_url: None,
        source_id: None,
    }
}

fn strip_source_handle_path(path: &str) -> (String, Option<String>) {
    let clean_path = path.trim();
    let Some(rest) = clean_path.strip_prefix("/source/") else {
        return (path.to_string(), None);
    };

    let mut parts = rest.splitn(2, '/');
    let source_id = parts.next().unwrap_or_default().trim();
    if source_id.is_empty() {
        return (path.to_string(), None);
    }

    let normalized_path = parts
        .next()
        .map(|tail| format!("/{tail}"))
        .unwrap_or_else(|| "/".to_string());
    (normalized_path, Some(source_id.to_string()))
}

fn resolve_route_source(
    client_route_source_id: Option<&str>,
    active_source_id: Option<&str>,
) -> Result<Option<ProxySourceHandle>, String> {
    let registry = ProxySourceRegistry::new();
    if let Some(id) = client_route_source_id {
        return registry
            .get(id)
            .map(Some)
            .ok_or_else(|| format!("Proxy source handle '{}' was not found", id));
    }

    Ok(active_source_id.and_then(|id| registry.get(id)))
}

fn canonical_codex_api_path(path: &str) -> &str {
    let path = path.trim_start_matches('/');
    path.strip_prefix("v1/v1/").unwrap_or(path)
}

fn is_codex_api_path(path: &str) -> bool {
    let path = canonical_codex_api_path(path);
    path == "v1/chat/completions"
        || path == "chat/completions"
        || path == "v1/responses"
        || path == "responses"
        || path == "v1/responses/compact"
        || path == "responses/compact"
        || path.starts_with("v1/responses/")
        || path.starts_with("responses/")
}

fn is_codex_endpoint(path: &str, method: &Method) -> bool {
    *method == Method::POST && is_codex_api_path(path)
}

// 移除 is_codex_chatgpt_backend_endpoint 函数，因为新的代理实现不再拦截 ChatGPT 后端 API
// 只拦截标准 OpenAI 端点

fn is_codex_analytics_endpoint(path: &str) -> bool {
    path == "/codex/analytics-events"
        || path.starts_with("/codex/analytics-events/")
        || path == "/analytics-events"
        || path.starts_with("/analytics-events/")
}

fn append_query(path: &str, query: Option<&str>) -> String {
    match query {
        Some(query) if !query.is_empty() => format!("{path}?{query}"),
        _ => path.to_string(),
    }
}

struct ConfigChangeMonitor {
    interval: tokio::time::Interval,
}

impl ConfigChangeMonitor {
    fn new() -> Self {
        // Keep this as a small abstraction so the proxy loop is not coupled to
        // polling forever. A filesystem watcher can replace this later, but
        // polling is deliberately the default for now: it is cross-platform,
        // handles temp-file + rename writes from config switchers, and avoids
        // watcher debounce edge cases. Five seconds is enough because provider
        // switching is user-driven and not latency-sensitive.
        Self {
            interval: tokio::time::interval(CONFIG_MONITOR_POLL_INTERVAL),
        }
    }

    async fn changed(&mut self) {
        self.interval.tick().await;
    }
}

async fn sync_external_config_change(proxy_port: u16, state: Arc<ProxyState>) {
    let config_manager = ClaudeConfigManager::new();
    let Ok(settings) = config_manager.read_settings() else {
        return;
    };

    if let Some(base_url) = settings.get_base_url() {
        if ClaudeConfigManager::is_usagemeter_proxy_url_for_port(&base_url, proxy_port) {
            if let Some(source_id) =
                ClaudeConfigManager::extract_source_id_from_proxy_url(&base_url)
            {
                if ProxySourceRegistry::new().get(&source_id).is_some() {
                    *state.active_source_id.write().await = Some(source_id);
                }
            }
            return;
        }
    }

    let registry = ProxySourceRegistry::new();
    match registry.upsert_from_settings(&settings) {
        Ok(Some(handle)) => {
            *state.active_source_id.write().await = Some(handle.id.clone());
            if let Err(e) = config_manager.takeover_with_path_prefix_and_source(
                proxy_port,
                Some("claude-code"),
                Some(&handle.id),
            ) {
                eprintln!("[proxy] Failed to re-apply source-aware takeover: {}", e);
            }
        }
        Ok(None) => {}
        Err(e) => eprintln!(
            "[proxy] Failed to update source handle from settings: {}",
            e
        ),
    }
}

async fn sync_codex_external_config_change(proxy_port: u16) {
    let settings = load_settings().unwrap_or_default();
    let codex_enabled = settings
        .client_tools
        .profiles
        .iter()
        .any(|profile| profile.tool == "codex" && profile.enabled);
    if !codex_enabled {
        return;
    }

    let config_manager = CodexConfigManager::new();

    // 检查 base_url 字段（在 model_providers 中），而不是 chatgpt_base_url
    if let Ok(is_active) = config_manager.is_takeover_active(proxy_port) {
        if is_active && config_manager.active_source_id().is_some() {
            if config_manager
                .is_chatgpt_http_provider_active(proxy_port)
                .unwrap_or(true)
            {
                return;
            }

            let registry = CodexSourceRegistry::new();
            if let Some(handle) = config_manager
                .active_source_id()
                .and_then(|source_id| registry.get(&source_id))
            {
                let _ = config_manager.takeover_with_source(proxy_port, &handle.id);
            }
            return;
        }
    }

    // 读取当前配置用于注册到 registry
    let snapshot = match config_manager.read_live_snapshot() {
        Ok(s) => s,
        Err(_) => return,
    };

    if CodexConfigManager::is_usagemeter_proxy_url_for_port(&snapshot.real_base_url, proxy_port) {
        let registry = CodexSourceRegistry::new();
        if let Some(handle) = config_manager
            .active_source_id()
            .and_then(|source_id| registry.get(&source_id))
            .or_else(|| registry.latest_for_provider(&snapshot.provider_id))
        {
            let _ = config_manager.takeover_with_source(proxy_port, &handle.id);
        }
        return;
    }

    if let Ok(handle) = CodexSourceRegistry::new().upsert_from_snapshot(snapshot) {
        let _ = config_manager.takeover_with_source(proxy_port, &handle.id);
    }
}

/// 辅助函数：创建完整响应体
fn full<T: Into<bytes::Bytes>>(chunk: T) -> BoxBody {
    http_body_util::Full::new(chunk.into())
        .map_err(|never| match never {})
        .boxed_unsync()
}

/// 代理服务器
pub struct ProxyServer {
    /// 代理配置
    config: ProxyConfig,
    /// 是否在启动/停止代理时接管 Claude Code 配置。
    takeover_claude: bool,
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
        Self::new_with_claude_takeover(config, true)
    }

    /// 创建不自动接管 Claude Code 配置的代理服务器。
    pub fn new_without_claude_takeover(config: ProxyConfig) -> Self {
        Self::new_with_claude_takeover(config, false)
    }

    fn new_with_claude_takeover(config: ProxyConfig, takeover_claude: bool) -> Self {
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
            active_source_id: Arc::new(RwLock::new(None)),
            openai_forwarder: Arc::new(RwLock::new(None)),
        });

        Self {
            config,
            takeover_claude,
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

        let config_manager = ClaudeConfigManager::new();
        let (source_handle, api_key, target_base_url) = if self.takeover_claude {
            // 从 Claude 配置获取 API 密钥和目标 URL
            let registry = ProxySourceRegistry::new();
            let current_settings = config_manager.read_settings()?;
            let source_handle = current_settings
                .get_base_url()
                .and_then(|base_url| {
                    ClaudeConfigManager::extract_source_id_from_proxy_url(&base_url)
                })
                .and_then(|source_id| registry.get(&source_id))
                .or_else(|| {
                    registry
                        .upsert_from_settings(&current_settings)
                        .ok()
                        .flatten()
                });

            let api_key = source_handle
                .as_ref()
                .and_then(|handle| handle.api_key.clone())
                .or_else(|| config_manager.get_api_key());
            let target_base_url = source_handle
                .as_ref()
                .map(|handle| handle.real_base_url.clone())
                .or_else(|| config_manager.get_original_base_url())
                .unwrap_or_else(|| "https://api.anthropic.com".to_string());
            let source_id = source_handle.as_ref().map(|handle| handle.id.clone());

            *self.state.active_source_id.write().await = source_id.clone();

            // 接管 Claude 配置。新配置使用路径前缀；服务端仍保留 /v1/messages 兼容旧配置。
            config_manager.takeover_with_path_prefix_and_source(
                self.config.port,
                Some("claude-code"),
                source_id.as_deref(),
            )?;

            (source_handle, api_key, target_base_url)
        } else {
            *self.state.active_source_id.write().await = None;
            (None, None, "https://api.anthropic.com".to_string())
        };

        // 创建转发器
        let forwarder = Arc::new(
            RequestForwarder::new(self.state.usage_collector.clone(), target_base_url, api_key)
                .map_err(|e| format!("Failed to create forwarder: {}", e))?,
        );

        // 创建 OpenAI-compatible 转发器（Codex 等），一次性创建后复用
        let openai_forwarder = Arc::new(
            OpenAiForwarder::new(self.state.usage_collector.clone())
                .map_err(|e| format!("Failed to create OpenAI forwarder: {}", e))?,
        );
        *self.state.openai_forwarder.write().await = Some(openai_forwarder);

        // 代理启动时先登记当前有效来源。这样即使 Claude Code 的入站请求不带
        // x-api-key，设置页也能立即看到本次接管对应的 API 来源。
        if self.takeover_claude {
            if let Ok(api_key) = forwarder.get_api_key(
                None,
                source_handle.as_ref().and_then(|h| h.api_key.as_deref()),
            ) {
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

        sync_codex_external_config_change(self.config.port).await;

        // 创建关闭通道
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        *self.shutdown_tx.write().await = Some(shutdown_tx);

        // 克隆状态用于服务器任务
        let state = self.state.clone();
        let proxy_port = self.config.port;
        let takeover_claude = self.takeover_claude;

        // 启动服务器任务
        let handle = tokio::spawn(async move {
            let mut config_monitor = ConfigChangeMonitor::new();
            loop {
                // 接受新连接
                let accepted = tokio::select! {
                    result = listener.accept() => {
                        match result {
                            Ok(conn) => Some(conn),
                            Err(e) => {
                                eprintln!("Accept error: {}", e);
                                None
                            }
                        }
                    }
                    _ = config_monitor.changed() => {
                        if takeover_claude {
                            sync_external_config_change(proxy_port, state.clone()).await;
                        }
                        sync_codex_external_config_change(proxy_port).await;
                        None
                    }
                    _ = &mut shutdown_rx => {
                        // 收到关闭信号
                        break;
                    }
                };

                let Some((stream, _remote_addr)) = accepted else {
                    continue;
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

        if self.takeover_claude {
            // 恢复 Claude 配置。source-aware URL 优先恢复对应来源的原始配置；
            // 如果用户/外部工具已经写回真实配置，则不覆盖。
            let config_manager = ClaudeConfigManager::new();
            let current_settings = config_manager.read_settings()?;
            if let Some(base_url) = current_settings.get_base_url() {
                if ClaudeConfigManager::is_usagemeter_proxy_url_for_port(
                    &base_url,
                    self.config.port,
                ) {
                    if !config_manager.restore_from_active_source_handle()? {
                        config_manager.restore()?;
                    }
                } else {
                    config_manager.clear_backup()?;
                }
            } else {
                config_manager.clear_backup()?;
            }
        }

        // 更新状态
        {
            let mut status = self.state.status.write().await;
            status.running = false;
            status.active_connections = 0;
        }

        // 清除启动时间
        *self.state.start_time.write().await = None;
        *self.state.active_source_id.write().await = None;
        *self.state.openai_forwarder.write().await = None;

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

type BoxBody = http_body_util::combinators::UnsyncBoxBody<bytes::Bytes, std::io::Error>;
type HandlerResult = Result<Response<BoxBody>, hyper::Error>;

/// 处理 Codex / OpenAI-compatible API 请求
async fn handle_codex_request(
    method: Method,
    path: &str,
    forward_path: &str,
    client_route: ClientRoute,
    req: Request<hyper::body::Incoming>,
    state: &Arc<ProxyState>,
) -> HandlerResult {
    // Analytics 端点直接返回 204
    if is_codex_analytics_endpoint(path) {
        {
            let mut status = state.status.write().await;
            status.success_requests += 1;
        }
        return Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(full(bytes::Bytes::new()))
            .unwrap());
    }

    let known_codex_api_path = is_codex_api_path(path);

    // Only known Codex/OpenAI API paths can be handled without an explicit source
    // handle. Source-scoped ChatGPT OAuth requests may still need passthrough for
    // backend auxiliary endpoints.
    if !known_codex_api_path && client_route.source_id.is_none() {
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header("Content-Type", "application/json")
            .body(full(
                r#"{"error":{"type":"not_found","message":"Endpoint not found"}}"#,
            ))
            .unwrap());
    }

    let capture_usage = is_codex_endpoint(path, &method);
    let request_start_time_ms = chrono::Utc::now().timestamp_millis();
    let request_start_instant = std::time::Instant::now();

    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer ").or(Some(value)))
        .map(str::to_string);
    let request_headers = req.headers().clone();

    let body_bytes = req.collect().await?.to_bytes();

    let (codex_session_id, codex_model) = serde_json::from_slice::<Value>(&body_bytes)
        .ok()
        .map(|json| {
            let session_id = json
                .get("session_id")
                .or_else(|| json.pointer("/metadata/session_id"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let model = json
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            (session_id, model)
        })
        .unwrap_or((None, None));

    // 解析 source handle
    let source_id = client_route
        .source_id
        .clone()
        .or_else(|| CodexConfigManager::new().active_source_id());
    let source_handle = match source_id.as_deref() {
        Some(id) => match CodexSourceRegistry::new().get(id) {
            Some(handle) => {
                let _ = CodexSourceRegistry::new().touch_used(id);
                Some(handle)
            }
            None => {
                return Ok(json_error_response(
                    StatusCode::BAD_GATEWAY,
                    "proxy_source_not_found",
                    &format!("Codex proxy source handle '{}' was not found", id),
                ));
            }
        },
        None => None,
    };

    // 解析目标 URL 和认证模式
    let target_base_url = source_handle
        .as_ref()
        .map(|handle| handle.real_base_url.clone())
        .or_else(|| client_route.target_base_url.clone())
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let source_auth_mode = source_handle
        .as_ref()
        .map(|handle| handle.original_snapshot.auth_mode);

    // 解析 API Key：ChatGPT OAuth 优先使用请求中的 token，API Key 模式优先使用保存的 key
    let target_api_key = if source_handle
        .as_ref()
        .map(|handle| handle.original_snapshot.auth_mode == CodexAuthMode::ChatGpt)
        .unwrap_or(false)
    {
        auth_header.clone().or_else(|| {
            source_handle
                .as_ref()
                .and_then(|handle| handle.api_key.clone())
        })
    } else {
        source_handle
            .as_ref()
            .and_then(|handle| handle.api_key.clone())
            .or(auth_header.clone())
    };
    let chatgpt_account_id = source_handle
        .as_ref()
        .filter(|handle| handle.original_snapshot.auth_mode == CodexAuthMode::ChatGpt)
        .and_then(|handle| extract_chatgpt_account_id(handle.original_snapshot.auth_json.as_ref()));

    // 来源检测
    let key_for_source_detection = target_api_key.as_deref();
    let (api_key_prefix, request_base_url) = if let Some(api_key) = key_for_source_detection {
        let settings = load_settings().unwrap_or_default();
        let sources = settings.source_aware.sources;
        let (prefix, base_url, _) = detect_source_info(api_key, &target_base_url, &sources);
        let (is_new, updated_settings) = register_source_to_settings(api_key, &target_base_url);
        let _ = save_settings(updated_settings);
        if is_new {
            if let Some(ref app_handle) = *state.app_handle.read().await {
                let _ = app_handle.emit("source_detected", ());
            }
        }
        (prefix, base_url)
    } else {
        (String::new(), normalize_base_url(&target_base_url))
    };

    let context = RequestContext {
        start_time: request_start_instant,
        start_time_ms: request_start_time_ms,
        model: codex_model,
        session_id: codex_session_id,
        api_key_prefix: if api_key_prefix.is_empty() {
            None
        } else {
            Some(api_key_prefix)
        },
        request_base_url,
        client_tool: client_route.client_tool,
        proxy_profile_id: client_route.proxy_profile_id,
        client_detection_method: client_route.detection_method,
        inbound_api_key: auth_header,
        target_base_url: Some(target_base_url.clone()),
        target_api_key: target_api_key.clone(),
        chatgpt_account_id,
        ..Default::default()
    };

    // 获取共享的 OpenAI 转发器
    let openai_forwarder = {
        let guard = state.openai_forwarder.read().await;
        guard.clone()
    };
    let Some(openai_forwarder) = openai_forwarder else {
        return Ok(json_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "proxy_error",
            "OpenAI forwarder not initialized",
        ));
    };

    // Non-usage endpoints:
    // - API-key providers may use Responses lifecycle endpoints such as
    //   GET /v1/responses/{id}; pass known OpenAI-compatible paths through.
    // - Source-scoped arbitrary backend paths remain limited to ChatGPT OAuth.
    if !capture_usage {
        if !known_codex_api_path && source_auth_mode != Some(CodexAuthMode::ChatGpt) {
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("Content-Type", "application/json")
                .body(full(
                    r#"{"error":{"type":"not_found","message":"Endpoint not found"}}"#,
                ))
                .unwrap());
        }
        return forward_codex_passthrough(
            &openai_forwarder,
            method,
            forward_path,
            request_headers,
            body_bytes,
            context,
            state,
        )
        .await;
    }

    // 标准 API 端点：转发并捕获 usage
    forward_codex_with_usage(
        &openai_forwarder,
        method,
        forward_path,
        request_headers,
        body_bytes,
        context,
        state,
    )
    .await
}

/// Codex 非 usage 端点透传
async fn forward_codex_passthrough(
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
        Ok(result) => {
            {
                let mut status = state.status.write().await;
                if result.status_code < 400 {
                    status.success_requests += 1;
                } else {
                    status.failed_requests += 1;
                }
            }
            let mut builder = Response::builder().status(
                StatusCode::from_u16(result.status_code).unwrap_or(StatusCode::BAD_GATEWAY),
            );
            for (name, value) in result.headers {
                builder = builder.header(name, value);
            }
            if let Some(content_type) = result.content_type {
                builder = builder.header("Content-Type", content_type);
            }
            Ok(builder.body(full(result.content)).unwrap())
        }
        Err(e) => {
            {
                let mut status = state.status.write().await;
                status.failed_requests += 1;
            }
            Ok(json_error_response(
                StatusCode::BAD_GATEWAY,
                "proxy_error",
                &e,
            ))
        }
    }
}

/// Codex 标准 API 端点转发（捕获 usage）
async fn forward_codex_with_usage(
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
            OpenAiForwardResult::Streaming { body } => {
                {
                    let mut status = state.status.write().await;
                    status.success_requests += 1;
                }
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "text/event-stream")
                    .header("Cache-Control", "no-cache")
                    .header("Connection", "keep-alive")
                    .body(body)
                    .unwrap())
            }
            OpenAiForwardResult::NonStreaming { content } => {
                {
                    let mut status = state.status.write().await;
                    status.success_requests += 1;
                }
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(full(content))
                    .unwrap())
            }
            OpenAiForwardResult::UpstreamError {
                status_code,
                content_type,
                headers,
                content,
            } => {
                {
                    let mut status = state.status.write().await;
                    status.failed_requests += 1;
                }
                let mut builder = Response::builder()
                    .status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
                for (name, value) in headers {
                    builder = builder.header(name, value);
                }
                if let Some(content_type) = content_type {
                    builder = builder.header("Content-Type", content_type);
                }
                Ok(builder.body(full(content)).unwrap())
            }
        },
        Err(e) => {
            {
                let mut status = state.status.write().await;
                status.failed_requests += 1;
            }
            Ok(json_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "proxy_error",
                &e,
            ))
        }
    }
}

fn json_error_response(status: StatusCode, error_type: &str, message: &str) -> Response<BoxBody> {
    let body = serde_json::json!({ "error": { "type": error_type, "message": message } });
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(full(serde_json::to_string(&body).unwrap_or_default()))
        .unwrap()
}

/// 处理传入的 HTTP 请求
async fn handle_request(
    req: Request<hyper::body::Incoming>,
    forwarder: Arc<RequestForwarder>,
    state: Arc<ProxyState>,
) -> HandlerResult {
    let method = req.method().clone();
    let raw_path = req.uri().path().to_string();
    let raw_query = req.uri().query().map(str::to_string);
    let client_route = detect_client_route(&raw_path);
    let path = client_route.normalized_path.clone();
    let forward_path = append_query(&path, raw_query.as_deref());

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

    // 处理所有 Codex 相关请求
    if client_route.client_tool == "codex" {
        return handle_codex_request(method, &path, &forward_path, client_route, req, &state).await;
    }

    if client_route.client_tool == "unknown" {
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header("Content-Type", "application/json")
            .body(full(
                r#"{"error":{"type":"tool_not_enabled","message":"Client tool is not enabled for proxy takeover"}}"#,
            ))
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

        let active_source_id = state.active_source_id.read().await.clone();
        let source_handle = match resolve_route_source(
            client_route.source_id.as_deref(),
            active_source_id.as_deref(),
        ) {
            Ok(handle) => handle,
            Err(e) => {
                let error_body = serde_json::json!({
                    "error": {
                        "type": "proxy_source_not_found",
                        "message": e
                    }
                });
                return Ok(Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .header("Content-Type", "application/json")
                    .body(full(serde_json::to_string(&error_body).unwrap_or_default()))
                    .unwrap());
            }
        };
        if let Some(ref handle) = source_handle {
            let _ = ProxySourceRegistry::new().touch_used(&handle.id);
        }

        // 获取目标 base_url（source 句柄优先，其次工具 profile，最后回退到启动默认值）
        let target_base_url = source_handle
            .as_ref()
            .map(|handle| handle.real_base_url.clone())
            .or_else(|| client_route.target_base_url.clone())
            .unwrap_or_else(|| forwarder.get_target_base_url());

        let source_api_key = source_handle
            .as_ref()
            .and_then(|handle| handle.api_key.as_deref());
        let effective_api_key = forwarder
            .get_api_key(api_key_header.as_deref(), source_api_key)
            .ok();

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
            target_api_key: source_handle.and_then(|handle| handle.api_key),
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

fn extract_chatgpt_account_id(auth_json: Option<&Value>) -> Option<String> {
    let auth = auth_json?;
    auth.pointer("/tokens/account_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            auth.pointer("/tokens/access_token")
                .and_then(|value| value.as_str())
                .and_then(extract_chatgpt_account_id_from_jwt)
        })
        .or_else(|| {
            auth.pointer("/tokens/id_token")
                .and_then(|value| value.as_str())
                .and_then(extract_chatgpt_account_id_from_jwt)
        })
}

fn extract_chatgpt_account_id_from_jwt(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64_url_decode(payload)?;
    let json: Value = serde_json::from_slice(&decoded).ok()?;
    json.pointer("/https://api.openai.com/auth/chatgpt_account_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    use base64::Engine;

    let mut value = input.replace('-', "+").replace('_', "/");
    while !value.len().is_multiple_of(4) {
        value.push('=');
    }
    base64::engine::general_purpose::STANDARD
        .decode(value.as_bytes())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_route_source_rejects_missing_explicit_source() {
        let result = resolve_route_source(Some("src_missing"), Some("src_active"));
        assert!(result.is_err());
    }

    #[test]
    fn test_strip_source_handle_path() {
        let (path, source_id) = strip_source_handle_path("/source/src_abc/v1/messages");
        assert_eq!(path, "/v1/messages");
        assert_eq!(source_id.as_deref(), Some("src_abc"));

        let (path, source_id) = strip_source_handle_path("/v1/messages");
        assert_eq!(path, "/v1/messages");
        assert_eq!(source_id, None);
    }

    #[test]
    fn test_codex_analytics_endpoint() {
        assert!(is_codex_analytics_endpoint(
            "/codex/analytics-events/events"
        ));
        assert!(is_codex_analytics_endpoint("/codex/analytics-events"));
        assert!(is_codex_analytics_endpoint("/analytics-events"));
        assert!(is_codex_analytics_endpoint("/analytics-events/events"));
        assert!(!is_codex_analytics_endpoint("/v1/responses"));
    }

    #[test]
    fn codex_endpoint_detection_uses_shared_path_classifier() {
        assert!(is_codex_api_path("/v1/responses"));
        assert!(is_codex_api_path("/v1/v1/responses"));
        assert!(is_codex_api_path("/responses/resp_123"));
        assert!(is_codex_endpoint("/v1/v1/chat/completions", &Method::POST));
        assert!(!is_codex_endpoint("/v1/responses", &Method::GET));
        assert!(!is_codex_api_path("/v1/models"));
    }

    #[test]
    fn extracts_chatgpt_account_id_from_auth_json() {
        let auth = serde_json::json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "account_id": "acct_test"
            }
        });

        assert_eq!(
            extract_chatgpt_account_id(Some(&auth)).as_deref(),
            Some("acct_test")
        );
    }
}
