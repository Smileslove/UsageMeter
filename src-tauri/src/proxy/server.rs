//! HTTP 代理服务器，用于拦截 Claude API 请求

use super::codex_config::{CodexConfigManager, CodexSourceRegistry};
use super::collector::UsageCollector;
use super::config_manager::ClaudeConfigManager;
use super::forwarder::{ForwardResult, RequestForwarder};
use super::openai_forwarder::{OpenAiForwardResult, OpenAiForwarder};
use super::source_detector::{
    detect_source_info, normalize_base_url, register_source_to_settings, SourceRegistrationResult,
};
use super::source_registry::{ProxySourceHandle, ProxySourceRegistry};
use super::types::{ProxyConfig, ProxyState, ProxyStatus, RequestContext};
use crate::commands::{load_settings, save_settings_internal};
use crate::models::AppSettings;
use crate::net::HttpClientFactory;
use http_body_util::BodyExt;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use std::fs;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::Emitter;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, RwLock};

const CONFIG_MONITOR_POLL_INTERVAL: Duration = Duration::from_secs(5);
const TAKEOVER_CONFLICT_WINDOW_MS: i64 = 30_000;
const TAKEOVER_CONFLICT_RECLAIM_THRESHOLD: usize = 3;

struct ClientRoute {
    normalized_path: String,
    client_tool: String,
    proxy_profile_id: Option<String>,
    detection_method: String,
    target_base_url: Option<String>,
    source_id: Option<String>,
}

/// 外部配置管理器持续抢写时发送给前端的事件载荷。
#[derive(serde::Serialize, Clone)]
struct TakeoverConflictDetectedPayload {
    tool: String,
    config_path: String,
    external_base_url: String,
    reclaim_count: usize,
    window_ms: i64,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

async fn mark_takeover_config_write(state: &Arc<ProxyState>, tool: &str) {
    let mut conflicts = state.takeover_conflicts.write().await;
    let tool_state = conflicts.tools.entry(tool.to_string()).or_default();
    tool_state.last_usagemeter_write_ms = Some(now_ms());
}

async fn clear_takeover_conflict(state: &Arc<ProxyState>, tool: &str) {
    let mut conflicts = state.takeover_conflicts.write().await;
    let tool_state = conflicts.tools.entry(tool.to_string()).or_default();
    tool_state.reclaim_events.clear();
    tool_state.paused_conflict = false;
    tool_state.paused_external_base_url = None;
}

async fn pause_takeover_conflict(
    state: &Arc<ProxyState>,
    tool: &str,
    external_base_url: Option<String>,
) {
    let mut conflicts = state.takeover_conflicts.write().await;
    let tool_state = conflicts.tools.entry(tool.to_string()).or_default();
    tool_state.paused_conflict = true;
    tool_state.paused_external_base_url = external_base_url;
}

async fn is_takeover_conflict_paused(state: &Arc<ProxyState>, tool: &str) -> bool {
    state
        .takeover_conflicts
        .read()
        .await
        .tools
        .get(tool)
        .map(|tool_state| tool_state.paused_conflict)
        .unwrap_or(false)
}

async fn takeover_conflict_external_base_url(
    state: &Arc<ProxyState>,
    tool: &str,
) -> Option<String> {
    state
        .takeover_conflicts
        .read()
        .await
        .tools
        .get(tool)
        .and_then(|tool_state| tool_state.paused_external_base_url.clone())
}

async fn should_reclaim_external_config(
    state: &Arc<ProxyState>,
    tool: &str,
    config_path: String,
    external_base_url: String,
) -> bool {
    let now = now_ms();
    let mut emit_payload = None;
    let should_reclaim = {
        let mut conflicts = state.takeover_conflicts.write().await;
        let tool_state = conflicts.tools.entry(tool.to_string()).or_default();
        if tool_state.paused_conflict {
            return false;
        }

        let recent_self_write = tool_state
            .last_usagemeter_write_ms
            .map(|last_write| now.saturating_sub(last_write) <= TAKEOVER_CONFLICT_WINDOW_MS)
            .unwrap_or(false);
        if !recent_self_write {
            true
        } else {
            tool_state.reclaim_events.push_back(now);
            while tool_state
                .reclaim_events
                .front()
                .map(|event| now.saturating_sub(*event) > TAKEOVER_CONFLICT_WINDOW_MS)
                .unwrap_or(false)
            {
                tool_state.reclaim_events.pop_front();
            }

            if tool_state.reclaim_events.len() >= TAKEOVER_CONFLICT_RECLAIM_THRESHOLD {
                tool_state.paused_conflict = true;
                tool_state.paused_external_base_url = Some(external_base_url.clone());
                emit_payload = Some(TakeoverConflictDetectedPayload {
                    tool: tool.to_string(),
                    config_path,
                    external_base_url,
                    reclaim_count: tool_state.reclaim_events.len(),
                    window_ms: TAKEOVER_CONFLICT_WINDOW_MS,
                });
                false
            } else {
                true
            }
        }
    };

    if let Some(payload) = emit_payload {
        if let Some(ref app_handle) = *state.app_handle.read().await {
            let _ = app_handle.emit("takeover_conflict_detected", payload);
        }
    }

    should_reclaim
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

fn settings_file_mtime() -> Option<SystemTime> {
    let path = AppSettings::settings_path().ok()?;
    fs::metadata(path).ok()?.modified().ok()
}

fn detect_client_route(path: &str, settings: &AppSettings) -> ClientRoute {
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

    ClientRoute {
        normalized_path: path.to_string(),
        client_tool: "unknown".to_string(),
        proxy_profile_id: None,
        detection_method: "unmatched_path".to_string(),
        target_base_url: None,
        source_id: None,
    }
}

async fn get_settings_snapshot(state: &Arc<ProxyState>) -> AppSettings {
    state.settings_snapshot.read().await.clone()
}

async fn store_settings_snapshot(
    state: &Arc<ProxyState>,
    settings: AppSettings,
    mtime: Option<SystemTime>,
) {
    *state.settings_snapshot.write().await = settings;
    *state.settings_file_mtime.write().await = mtime;
}

async fn persist_proxy_settings(
    state: &Arc<ProxyState>,
    settings: AppSettings,
) -> Result<(), String> {
    save_settings_internal(settings.clone()).map_err(String::from)?;
    store_settings_snapshot(state, settings, settings_file_mtime()).await;
    Ok(())
}

async fn refresh_settings_snapshot_if_needed(state: &Arc<ProxyState>) {
    let current_mtime = settings_file_mtime();
    {
        let known_mtime = state.settings_file_mtime.read().await;
        if *known_mtime == current_mtime {
            return;
        }
    }

    match load_settings() {
        Ok(settings) => {
            store_settings_snapshot(state, settings, current_mtime).await;
        }
        Err(err) => {
            eprintln!("[proxy] Failed to reload settings snapshot: {}", err);
        }
    }
}

async fn register_source_for_runtime(
    state: &Arc<ProxyState>,
    api_key: &str,
    target_base_url: &str,
) -> SourceRegistrationResult {
    let mut settings = state.settings_snapshot.write().await;
    let result = register_source_to_settings(&mut settings, api_key, target_base_url);
    let settings_to_persist = if result.is_new {
        Some(settings.clone())
    } else {
        None
    };
    drop(settings);

    if let Some(settings) = settings_to_persist {
        if let Err(err) = persist_proxy_settings(state, settings).await {
            eprintln!("[proxy] Failed to persist source state: {}", err);
        }
    }

    result
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

fn resolve_target_base_url(
    source_base_url: Option<&str>,
    route_target_base_url: Option<&str>,
    tool: &str,
) -> Result<String, String> {
    source_base_url
        .or(route_target_base_url)
        .map(str::to_string)
        .ok_or_else(|| {
            format!(
                "{tool} proxy target base URL is not configured; refusing to guess an upstream target"
            )
        })
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

/// 外部工具（如 cc switch）修改配置时发送给前端的事件载荷
#[derive(serde::Serialize, Clone)]
struct ProxyConfigChangedPayload {
    new_real_base_url: String,
    source_id: String,
}

async fn sync_external_config_change(proxy_port: u16, state: Arc<ProxyState>) {
    if is_takeover_conflict_paused(&state, "claude_code").await {
        return;
    }

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
            if !should_reclaim_external_config(
                &state,
                "claude_code",
                config_manager.settings_path().display().to_string(),
                handle.real_base_url.clone(),
            )
            .await
            {
                return;
            }

            *state.active_source_id.write().await = Some(handle.id.clone());
            if let Err(e) = config_manager.takeover_with_path_prefix_and_source(
                proxy_port,
                Some("claude-code"),
                Some(&handle.id),
            ) {
                eprintln!("[proxy] Failed to re-apply source-aware takeover: {}", e);
            } else {
                mark_takeover_config_write(&state, "claude_code").await;
                // 通知前端：外部工具修改了配置，代理已自动切换目标
                if let Some(ref app_handle) = *state.app_handle.read().await {
                    let _ = app_handle.emit(
                        "proxy_config_changed",
                        ProxyConfigChangedPayload {
                            new_real_base_url: handle.real_base_url.clone(),
                            source_id: handle.id.clone(),
                        },
                    );
                }
            }
        }
        Ok(None) => {}
        Err(e) => eprintln!(
            "[proxy] Failed to update source handle from settings: {}",
            e
        ),
    }
}

async fn sync_codex_external_config_change(proxy_port: u16, state: Arc<ProxyState>) {
    if is_takeover_conflict_paused(&state, "codex").await {
        return;
    }

    let settings = get_settings_snapshot(&state).await;
    let codex_enabled = settings
        .client_tools
        .profiles
        .iter()
        .any(|profile| profile.tool == "codex" && profile.enabled);
    if !codex_enabled {
        return;
    }

    let config_manager = CodexConfigManager::new();

    // config.toml 已正确指向代理且携带有效 source ID，无需重写。
    // 过去此处会无条件调用 takeover_with_source，每 5 秒覆写一次文件，
    // 与 Codex 读取配置产生竞争，导致 experimental_bearer_token 短暂丢失。
    if let Ok(is_active) = config_manager.is_takeover_active(proxy_port) {
        if is_active && config_manager.active_source_id().is_some() {
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
            mark_takeover_config_write(&state, "codex").await;
        }
        return;
    }

    if let Ok(handle) = CodexSourceRegistry::new().upsert_from_snapshot(snapshot) {
        if !should_reclaim_external_config(
            &state,
            "codex",
            config_manager.config_path().display().to_string(),
            handle.real_base_url.clone(),
        )
        .await
        {
            return;
        }
        if config_manager
            .takeover_with_source(proxy_port, &handle.id)
            .is_ok()
        {
            mark_takeover_config_write(&state, "codex").await;
        }
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
        let initial_settings = load_settings().unwrap_or_default();
        let initial_settings_mtime = settings_file_mtime();

        let client = HttpClientFactory::global()
            .apply_proxy_to_builder(
                reqwest::Client::builder().timeout(Duration::from_secs(config.request_timeout)),
            )
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
            settings_snapshot: Arc::new(RwLock::new(initial_settings)),
            settings_file_mtime: Arc::new(RwLock::new(initial_settings_mtime)),
            takeover_conflicts: Arc::new(RwLock::new(Default::default())),
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
        let (_source_handle, target_base_url) = if self.takeover_claude {
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
            mark_takeover_config_write(&self.state, "claude_code").await;

            (source_handle, target_base_url)
        } else {
            *self.state.active_source_id.write().await = None;
            (None, "https://api.anthropic.com".to_string())
        };

        // 创建转发器
        let forwarder = Arc::new(
            RequestForwarder::new(
                self.state.usage_collector.clone(),
                target_base_url,
                self.config.request_timeout,
                self.config.streaming_idle_timeout,
            )
            .map_err(|e| format!("Failed to create forwarder: {}", e))?,
        );

        // 创建 OpenAI-compatible 转发器（Codex 等），一次性创建后复用
        let openai_forwarder = Arc::new(
            OpenAiForwarder::new(
                self.state.usage_collector.clone(),
                self.config.request_timeout,
                self.config.streaming_idle_timeout,
            )
            .map_err(|e| format!("Failed to create OpenAI forwarder: {}", e))?,
        );
        *self.state.openai_forwarder.write().await = Some(openai_forwarder);

        // 代理启动时先登记当前有效来源。这样即使 Claude Code 的入站请求不带
        // x-api-key，设置页也能立即看到本次接管对应的 API 来源。
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

        sync_codex_external_config_change(self.config.port, self.state.clone()).await;

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
                        refresh_settings_snapshot_if_needed(&state).await;
                        if takeover_claude {
                            sync_external_config_change(proxy_port, state.clone()).await;
                        }
                        sync_codex_external_config_change(proxy_port, state.clone()).await;
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

    /// 指定工具是否因外部配置管理器持续抢写而暂停自动接管。
    pub async fn is_takeover_conflict_paused(&self, tool: &str) -> bool {
        is_takeover_conflict_paused(&self.state, tool).await
    }

    /// 获取冲突暂停时观察到的真实上游地址。
    pub async fn takeover_conflict_external_base_url(&self, tool: &str) -> Option<String> {
        takeover_conflict_external_base_url(&self.state, tool).await
    }

    /// 将指定工具标记为冲突暂停。
    pub async fn pause_takeover_conflict(&self, tool: &str) {
        pause_takeover_conflict(&self.state, tool, None).await;
    }

    /// 清除冲突暂停并立即重新接管当前配置。
    pub async fn force_reclaim_takeover(&self, tool: &str) -> Result<(), String> {
        match tool {
            "claude_code" | "claude" => self.force_reclaim_claude_takeover().await?,
            "codex" => self.force_reclaim_codex_takeover().await?,
            other => return Err(format!("Unsupported takeover tool: {}", other)),
        };
        clear_takeover_conflict(&self.state, tool).await;
        Ok(())
    }

    async fn force_reclaim_claude_takeover(&self) -> Result<(), String> {
        let config_manager = ClaudeConfigManager::new();
        let registry = ProxySourceRegistry::new();
        let settings = config_manager.read_settings()?;
        let active_source_id = self.state.active_source_id.read().await.clone();
        let source_handle = match settings.get_base_url() {
            Some(base_url) if ClaudeConfigManager::is_usagemeter_proxy_url(&base_url) => {
                ClaudeConfigManager::extract_source_id_from_proxy_url(&base_url)
                    .and_then(|source_id| registry.get(&source_id))
                    .or_else(|| {
                        active_source_id
                            .as_deref()
                            .and_then(|source_id| registry.get(source_id))
                    })
            }
            _ => registry.upsert_from_settings(&settings).ok().flatten(),
        };
        let Some(handle) = source_handle else {
            return Err("Unable to resolve Claude takeover source".to_string());
        };

        *self.state.active_source_id.write().await = Some(handle.id.clone());
        config_manager.takeover_with_path_prefix_and_source(
            self.config.port,
            Some("claude-code"),
            Some(&handle.id),
        )?;
        mark_takeover_config_write(&self.state, "claude_code").await;
        Ok(())
    }

    async fn force_reclaim_codex_takeover(&self) -> Result<(), String> {
        let config_manager = CodexConfigManager::new();
        let registry = CodexSourceRegistry::new();
        let snapshot = config_manager.read_live_snapshot()?;
        let handle = if CodexConfigManager::is_usagemeter_proxy_url_for_port(
            &snapshot.real_base_url,
            self.config.port,
        ) {
            config_manager
                .active_source_id()
                .and_then(|source_id| registry.get(&source_id))
                .or_else(|| registry.latest_for_provider(&snapshot.provider_id))
                .ok_or_else(|| "Unable to resolve Codex takeover source".to_string())?
        } else {
            registry.upsert_from_snapshot(snapshot)?
        };

        config_manager.takeover_with_source(self.config.port, &handle.id)?;
        mark_takeover_config_write(&self.state, "codex").await;
        Ok(())
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
    let target_base_url = match resolve_target_base_url(
        source_handle
            .as_ref()
            .map(|handle| handle.real_base_url.as_str()),
        client_route.target_base_url.as_deref(),
        "Codex",
    ) {
        Ok(url) => url,
        Err(message) => {
            return Ok(json_error_response(
                StatusCode::BAD_GATEWAY,
                "proxy_target_not_configured",
                &message,
            ));
        }
    };
    // 来源检测
    let key_for_source_detection = auth_header.as_deref();
    let (api_key_prefix, request_base_url) = if let Some(api_key) = key_for_source_detection {
        let result = register_source_for_runtime(state, api_key, &target_base_url).await;
        if result.is_new {
            if let Some(ref app_handle) = *state.app_handle.read().await {
                let _ = app_handle.emit("source_detected", ());
            }
        }
        (result.prefix, result.base_url)
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
        target_base_url: Some(target_base_url.clone()),
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

    // Non-usage endpoints are still transparently proxied; they are only excluded
    // from usage capture.
    if !capture_usage {
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
        Ok(OpenAiForwardResult::Streaming {
            status_code,
            headers,
            body,
        }) => {
            {
                let mut status = state.status.write().await;
                if status_code < 400 {
                    status.success_requests += 1;
                } else {
                    status.failed_requests += 1;
                }
            }
            let mut builder = Response::builder()
                .status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
            for (name, value) in headers {
                builder = builder.header(name, value);
            }
            Ok(builder.body(body).unwrap())
        }
        Ok(OpenAiForwardResult::NonStreaming {
            status_code,
            headers,
            content,
        }) => {
            {
                let mut status = state.status.write().await;
                if status_code < 400 {
                    status.success_requests += 1;
                } else {
                    status.failed_requests += 1;
                }
            }
            let mut builder = Response::builder()
                .status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
            for (name, value) in headers {
                builder = builder.header(name, value);
            }
            Ok(builder.body(full(content)).unwrap())
        }
        Ok(OpenAiForwardResult::UpstreamError {
            status_code,
            headers,
            content,
        }) => {
            {
                let mut status = state.status.write().await;
                status.failed_requests += 1;
            }
            let mut builder = Response::builder()
                .status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
            for (name, value) in headers {
                builder = builder.header(name, value);
            }
            Ok(builder.body(full(content)).unwrap())
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
            OpenAiForwardResult::Streaming {
                status_code,
                headers,
                body,
            } => {
                {
                    let mut status = state.status.write().await;
                    if status_code < 400 {
                        status.success_requests += 1;
                    } else {
                        status.failed_requests += 1;
                    }
                }
                let mut builder = Response::builder()
                    .status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
                for (name, value) in headers {
                    builder = builder.header(name, value);
                }
                Ok(builder.body(body).unwrap())
            }
            OpenAiForwardResult::NonStreaming {
                status_code,
                headers,
                content,
            } => {
                {
                    let mut status = state.status.write().await;
                    if status_code < 400 {
                        status.success_requests += 1;
                    } else {
                        status.failed_requests += 1;
                    }
                }
                let mut builder = Response::builder()
                    .status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
                for (name, value) in headers {
                    builder = builder.header(name, value);
                }
                Ok(builder.body(full(content)).unwrap())
            }
            OpenAiForwardResult::UpstreamError {
                status_code,
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

async fn forward_claude_passthrough(
    forwarder: &RequestForwarder,
    method: Method,
    forward_path: &str,
    request_headers: hyper::HeaderMap,
    body_bytes: bytes::Bytes,
    context: RequestContext,
    state: &Arc<ProxyState>,
) -> HandlerResult {
    match forwarder
        .forward_passthrough(method, forward_path, body_bytes, context, request_headers)
        .await
    {
        Ok(ForwardResult::Streaming {
            status_code,
            headers,
            body,
        }) => {
            {
                let mut status = state.status.write().await;
                if status_code < 400 {
                    status.success_requests += 1;
                } else {
                    status.failed_requests += 1;
                }
            }
            let mut builder = Response::builder()
                .status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
            for (name, value) in headers {
                builder = builder.header(name, value);
            }
            Ok(builder.body(body).unwrap())
        }
        Ok(ForwardResult::NonStreaming {
            status_code,
            headers,
            content,
        }) => {
            {
                let mut status = state.status.write().await;
                if status_code < 400 {
                    status.success_requests += 1;
                } else {
                    status.failed_requests += 1;
                }
            }
            let mut builder = Response::builder()
                .status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
            for (name, value) in headers {
                builder = builder.header(name, value);
            }
            Ok(builder.body(full(content)).unwrap())
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

async fn forward_claude_with_usage(
    forwarder: &RequestForwarder,
    method: Method,
    forward_path: &str,
    request_headers: hyper::HeaderMap,
    body_bytes: bytes::Bytes,
    context: RequestContext,
    state: &Arc<ProxyState>,
) -> HandlerResult {
    match forwarder
        .forward_with_usage(method, forward_path, body_bytes, context, request_headers)
        .await
    {
        Ok(ForwardResult::Streaming {
            status_code,
            headers,
            body,
        }) => {
            {
                let mut status = state.status.write().await;
                if status_code < 400 {
                    status.success_requests += 1;
                } else {
                    status.failed_requests += 1;
                }
            }
            let mut builder = Response::builder()
                .status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
            for (name, value) in headers {
                builder = builder.header(name, value);
            }
            Ok(builder.body(body).unwrap())
        }
        Ok(ForwardResult::NonStreaming {
            status_code,
            headers,
            content,
        }) => {
            {
                let mut status = state.status.write().await;
                if status_code < 400 {
                    status.success_requests += 1;
                } else {
                    status.failed_requests += 1;
                }
            }
            let mut builder = Response::builder()
                .status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
            for (name, value) in headers {
                builder = builder.header(name, value);
            }
            Ok(builder.body(full(content)).unwrap())
        }
        Err(e) => {
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
    let settings = get_settings_snapshot(&state).await;
    let client_route = detect_client_route(&raw_path, &settings);
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
                r#"{"error":{"type":"proxy_route_not_matched","message":"Request path did not match any enabled proxy route"}}"#,
            ))
            .unwrap());
    }

    if client_route.client_tool == "claude_code" {
        // 在收集请求体之前立即记录开始时间，确保端到端时间准确
        let request_start_time_ms = chrono::Utc::now().timestamp_millis();
        let request_start_instant = std::time::Instant::now();

        // 提取 x-api-key 头用于来源识别。
        let api_key_header = req
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let request_headers = req.headers().clone();

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

        // 获取目标 base_url（source 句柄优先，其次工具 profile；目标不明确时直接报错）
        let target_base_url = match resolve_target_base_url(
            source_handle
                .as_ref()
                .map(|handle| handle.real_base_url.as_str()),
            client_route.target_base_url.as_deref(),
            "Claude",
        ) {
            Ok(url) => url,
            Err(message) => {
                return Ok(json_error_response(
                    StatusCode::BAD_GATEWAY,
                    "proxy_target_not_configured",
                    &message,
                ));
            }
        };

        // 检测并注册来源
        let is_new_source = if let Some(ref api_key) = api_key_header {
            let result = register_source_for_runtime(&state, api_key, &target_base_url).await;
            if result.is_new {
                if let Some(ref app_handle) = *state.app_handle.read().await {
                    let _ = app_handle.emit("source_detected", ());
                }
            }
            result.is_new
        } else {
            false
        };

        // 获取来源信息用于 RequestContext
        let (api_key_prefix, request_base_url) = if let Some(ref api_key) = api_key_header {
            let settings = get_settings_snapshot(&state).await;
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
            target_base_url: Some(target_base_url),
            ..Default::default()
        };
        let _ = is_new_source;
        let capture_usage = path == "/v1/messages" && method == Method::POST;
        if capture_usage {
            return forward_claude_with_usage(
                &forwarder,
                method,
                &forward_path,
                request_headers,
                body_bytes,
                context,
                &state,
            )
            .await;
        }

        return forward_claude_passthrough(
            &forwarder,
            method,
            &forward_path,
            request_headers,
            body_bytes,
            context,
            &state,
        )
        .await;
    }

    // 对于未知端点返回 404
    Ok(Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header("Content-Type", "application/json")
        .body(full(
            r#"{"error":{"type":"not_found","message":"Endpoint not found"}}"#,
        ))
        .unwrap())
}

impl Default for ProxyServer {
    fn default() -> Self {
        Self::new(ProxyConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AppSettings;

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
    fn codex_endpoint_detection_uses_shared_path_classifier() {
        assert!(is_codex_api_path("/v1/responses"));
        assert!(is_codex_api_path("/v1/v1/responses"));
        assert!(is_codex_api_path("/responses/resp_123"));
        assert!(is_codex_endpoint("/v1/v1/chat/completions", &Method::POST));
        assert!(!is_codex_endpoint("/v1/responses", &Method::GET));
        assert!(!is_codex_api_path("/v1/models"));
    }

    #[test]
    fn detect_client_route_keeps_prefixed_codex_unknown_paths() {
        let mut settings = AppSettings::default();
        let codex_profile = settings
            .client_tools
            .profiles
            .iter_mut()
            .find(|profile| profile.tool == "codex")
            .expect("codex profile");
        codex_profile.enabled = true;

        let route = detect_client_route("/codex/connectors/directory/list", &settings);
        assert_eq!(route.client_tool, "codex");
        assert_eq!(route.normalized_path, "/connectors/directory/list");
        assert_eq!(route.detection_method, "path_prefix");
        assert!(route.source_id.is_none());
    }

    #[test]
    fn detect_client_route_keeps_source_scoped_codex_unknown_paths() {
        let mut settings = AppSettings::default();
        let codex_profile = settings
            .client_tools
            .profiles
            .iter_mut()
            .find(|profile| profile.tool == "codex")
            .expect("codex profile");
        codex_profile.enabled = true;

        let route =
            detect_client_route("/codex/source/src_123/connectors/directory/list", &settings);
        assert_eq!(route.client_tool, "codex");
        assert_eq!(route.normalized_path, "/connectors/directory/list");
        assert_eq!(route.detection_method, "path_prefix_source");
        assert_eq!(route.source_id.as_deref(), Some("src_123"));
    }

    #[test]
    fn detect_client_route_does_not_guess_unprefixed_codex_paths() {
        let mut settings = AppSettings::default();
        let codex_profile = settings
            .client_tools
            .profiles
            .iter_mut()
            .find(|profile| profile.tool == "codex")
            .expect("codex profile");
        codex_profile.enabled = true;

        let route = detect_client_route("/v1/responses", &settings);
        assert_eq!(route.client_tool, "unknown");
        assert_eq!(route.normalized_path, "/v1/responses");
        assert_eq!(route.detection_method, "unmatched_path");
    }

    #[test]
    fn detect_client_route_keeps_prefixed_claude_unknown_paths() {
        let settings = AppSettings::default();
        let route = detect_client_route("/claude-code/foo/bar", &settings);
        assert_eq!(route.client_tool, "claude_code");
        assert_eq!(route.normalized_path, "/foo/bar");
        assert_eq!(route.detection_method, "path_prefix");
    }

    #[test]
    fn detect_client_route_does_not_guess_unprefixed_claude_paths() {
        let settings = AppSettings::default();
        let route = detect_client_route("/v1/messages", &settings);
        assert_eq!(route.client_tool, "unknown");
        assert_eq!(route.normalized_path, "/v1/messages");
        assert_eq!(route.detection_method, "unmatched_path");
        assert!(route.proxy_profile_id.is_none());
    }

    #[test]
    fn resolve_target_base_url_prefers_source_over_route() {
        let result = resolve_target_base_url(
            Some("https://source.example/v1"),
            Some("https://route.example/v1"),
            "Codex",
        )
        .unwrap();
        assert_eq!(result, "https://source.example/v1");
    }

    #[test]
    fn resolve_target_base_url_uses_route_when_source_missing() {
        let result =
            resolve_target_base_url(None, Some("https://route.example/v1"), "Codex").unwrap();
        assert_eq!(result, "https://route.example/v1");
    }

    #[test]
    fn resolve_target_base_url_refuses_to_guess() {
        let err = resolve_target_base_url(None, None, "Codex").unwrap_err();
        assert!(err.contains("refusing to guess an upstream target"));
    }
}
