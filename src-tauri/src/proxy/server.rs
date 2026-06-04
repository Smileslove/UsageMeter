//! HTTP 代理服务器，用于拦截客户端 API 请求

use super::codex_config::{CodexConfigManager, CodexSourceRegistry};
use super::collector::UsageCollector;
use super::config_manager::ClaudeConfigManager;
use super::forwarder::RequestForwarder;
use super::openai_forwarder::OpenAiForwarder;
use super::opencode_config::{OpenCodeConfigManager, OpenCodeSourceRegistry};
use super::request_common::{
    get_settings_snapshot, refresh_settings_snapshot_if_needed, settings_file_mtime,
};
use super::source_registry::ProxySourceRegistry;
use super::types::{ProxyConfig, ProxyState, ProxyStatus};
use crate::commands::load_settings;
use crate::net::HttpClientFactory;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::Emitter;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, RwLock};

const CONFIG_MONITOR_POLL_INTERVAL: Duration = Duration::from_secs(5);
const TAKEOVER_CONFLICT_WINDOW_MS: i64 = 30_000;
const TAKEOVER_CONFLICT_RECLAIM_THRESHOLD: usize = 3;

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

async fn emit_takeover_conflict_detected(
    state: &Arc<ProxyState>,
    tool: &str,
    config_path: String,
    external_base_url: String,
    reclaim_count: usize,
    window_ms: i64,
) {
    if let Some(ref app_handle) = *state.app_handle.read().await {
        let _ = app_handle.emit(
            "takeover_conflict_detected",
            TakeoverConflictDetectedPayload {
                tool: tool.to_string(),
                config_path,
                external_base_url,
                reclaim_count,
                window_ms,
            },
        );
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

async fn sync_opencode_external_config_change(proxy_port: u16, state: Arc<ProxyState>) {
    if is_takeover_conflict_paused(&state, "opencode").await {
        return;
    }

    let settings = get_settings_snapshot(&state).await;
    let opencode_enabled = settings
        .client_tools
        .profiles
        .iter()
        .any(|profile| profile.tool == "opencode" && profile.enabled);
    if !opencode_enabled {
        return;
    }

    let config_manager = OpenCodeConfigManager::new();
    if config_manager.ensure_config_exists().is_err() {
        return;
    }

    let route_state = match config_manager.read_live_snapshot() {
        Ok(state) => state,
        Err(_) => return,
    };

    if route_state.providers.is_empty() {
        return;
    }

    if route_state.providers.iter().all(|provider| {
        OpenCodeConfigManager::is_usagemeter_proxy_url_for_port(
            &provider.original_base_url,
            proxy_port,
        )
    }) {
        return;
    }

    let external_base_url = route_state
        .providers
        .iter()
        .find(|provider| {
            !OpenCodeConfigManager::is_usagemeter_proxy_url_for_port(
                &provider.original_base_url,
                proxy_port,
            )
        })
        .map(|provider| format!("{}: {}", provider.provider_id, provider.original_base_url))
        .unwrap_or_else(|| "external override detected".to_string());

    pause_takeover_conflict(&state, "opencode", Some(external_base_url.clone())).await;
    emit_takeover_conflict_detected(
        &state,
        "opencode",
        config_manager.config_path().display().to_string(),
        external_base_url,
        1,
        0,
    )
    .await;
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
        sync_opencode_external_config_change(self.config.port, self.state.clone()).await;

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
                        sync_opencode_external_config_change(proxy_port, state.clone()).await;
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
                        async move { super::routing::handle_request(req, forwarder, state).await }
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
            "opencode" => self.force_reclaim_opencode_takeover().await?,
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

    async fn force_reclaim_opencode_takeover(&self) -> Result<(), String> {
        let config_manager = OpenCodeConfigManager::new();
        config_manager.ensure_config_exists()?;

        let registry = OpenCodeSourceRegistry::new();
        let route_state = config_manager.read_live_snapshot()?;
        let handles = if route_state.providers.iter().any(|provider| {
            OpenCodeConfigManager::is_usagemeter_proxy_url(&provider.original_base_url)
        }) {
            let mut handles = Vec::new();
            for route in config_manager.active_routes() {
                let handle = registry
                    .get(&route.source_id)
                    .or_else(|| registry.latest_for_provider(&route.provider_id))
                    .ok_or_else(|| "Unable to resolve OpenCode takeover source".to_string())?;
                handles.push(handle);
            }
            handles
        } else {
            registry.upsert_from_state(&route_state)?
        };

        if handles.is_empty() {
            return Err(
                "No OpenCode providers with explicit baseURL were found. Configure a provider baseURL first, then enable takeover."
                    .to_string(),
            );
        }

        config_manager.takeover_with_handles(self.config.port, &handles)?;
        mark_takeover_config_write(&self.state, "opencode").await;
        Ok(())
    }

    /// 设置 Tauri 应用句柄（用于发送事件）
    #[allow(dead_code)]
    pub async fn set_app_handle(&self, handle: tauri::AppHandle) {
        *self.state.app_handle.write().await = Some(handle);
    }
}

impl Default for ProxyServer {
    fn default() -> Self {
        Self::new(ProxyConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use crate::models::AppSettings;
    use crate::proxy::request_common::{
        detect_client_route, resolve_route_source, resolve_target_base_url,
        strip_source_handle_path,
    };

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
    fn detect_client_route_strips_opencode_route_shell_only() {
        let mut settings = AppSettings::default();
        let opencode_profile = settings
            .client_tools
            .profiles
            .iter_mut()
            .find(|profile| profile.tool == "opencode")
            .expect("opencode profile");
        opencode_profile.enabled = true;

        let route = detect_client_route("/opencode/source/oc_123/messages", &settings);
        assert_eq!(route.client_tool, "opencode");
        assert_eq!(route.normalized_path, "/messages");
        assert_eq!(route.detection_method, "path_prefix_source");
        assert_eq!(route.source_id.as_deref(), Some("oc_123"));

        let route = detect_client_route("/opencode/source/oc_123/v1/messages", &settings);
        assert_eq!(route.normalized_path, "/v1/messages");
        assert_eq!(route.source_id.as_deref(), Some("oc_123"));
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
