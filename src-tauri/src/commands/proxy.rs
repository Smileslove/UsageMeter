//! 代理相关的 Tauri 命令

use crate::proxy::{
    codex_snapshot_uses_official_provider, request_common, server, ClaudeConfigManager,
    CodexConfigManager, CodexSourceRegistry, OpenCodeConfigManager, OpenCodeSourceRegistry,
    ProxyConfig, ProxyServer, ProxyStatus, ReasonixConfigManager, ReasonixSourceRegistry,
};
use tauri::State;

use super::usage::ProxyState;
use super::{load_settings, save_settings_internal};

pub async fn ensure_passive_proxy_monitor_started(state: &ProxyState) {
    if state.passive_monitor_handle.read().await.is_some() {
        return;
    }

    let port = load_settings().unwrap_or_default().proxy.port;
    let proxy_state = std::sync::Arc::new(crate::proxy::ProxyState {
        usage_collector: std::sync::Arc::new(crate::proxy::UsageCollector::new()),
        client: reqwest::Client::new(),
        config: std::sync::Arc::new(tokio::sync::RwLock::new(ProxyConfig {
            port,
            ..ProxyConfig::default()
        })),
        status: std::sync::Arc::new(tokio::sync::RwLock::new(ProxyStatus::default())),
        start_time: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        app_handle: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        active_source_id: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        openai_forwarder: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        settings_snapshot: std::sync::Arc::new(tokio::sync::RwLock::new(
            load_settings().unwrap_or_default(),
        )),
        settings_file_mtime: std::sync::Arc::new(tokio::sync::RwLock::new(
            request_common::settings_file_mtime(),
        )),
        takeover_conflicts: std::sync::Arc::new(tokio::sync::RwLock::new(Default::default())),
        passive_recovery_enabled: std::sync::Arc::new(tokio::sync::RwLock::new(true)),
    });
    let server_state = state.server.clone();

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
    let handle = tauri::async_runtime::spawn(async move {
        tokio::select! {
            _ = async {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                loop {
                    interval.tick().await;
                    if server_state.read().await.is_some() {
                        continue;
                    }
                    request_common::refresh_settings_snapshot_if_needed(&proxy_state).await;
                    server::sync_external_config_change(
                        port,
                        proxy_state.clone(),
                        server::ExternalConfigSyncMode::PassiveRecoveryOnly,
                    )
                    .await;
                    server::sync_codex_external_config_change(
                        port,
                        proxy_state.clone(),
                        server::ExternalConfigSyncMode::PassiveRecoveryOnly,
                    )
                    .await;
                    server::sync_opencode_external_config_change(
                        port,
                        proxy_state.clone(),
                        server::ExternalConfigSyncMode::PassiveRecoveryOnly,
                    )
                    .await;
                    server::sync_reasonix_external_config_change(
                        port,
                        proxy_state.clone(),
                        server::ExternalConfigSyncMode::PassiveRecoveryOnly,
                    )
                    .await;
                }
            } => {}
            _ = &mut shutdown_rx => {}
        }
    });

    *state.passive_monitor_shutdown.write().await = Some(shutdown_tx);
    *state.passive_monitor_handle.write().await = Some(handle);
}

/// 启动代理服务器
#[tauri::command]
pub async fn start_proxy(
    port: u16,
    state: State<'_, ProxyState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut server_guard = state.server.write().await;

    // 检查是否已在运行
    if server_guard.is_some() {
        return Err("Proxy is already running".to_string());
    }

    let settings = load_settings().unwrap_or_default();
    let takeover_claude = is_client_tool_enabled(&settings, "claude_code");
    let has_non_claude_tool = settings
        .client_tools
        .profiles
        .iter()
        .any(|profile| profile.tool != "claude_code" && profile.enabled);
    let config = proxy_config_from_settings(port, &settings);

    let server = if takeover_claude {
        ProxyServer::new(config.clone())
    } else {
        ProxyServer::new_without_claude_takeover(config.clone())
    };
    server.set_app_handle(app.clone()).await;
    if takeover_claude {
        if let Err(e) = server.start().await {
            if !has_non_claude_tool {
                return Err(e);
            }

            mark_client_tool_enabled("claude_code", false)?;
            let fallback_server = ProxyServer::new_without_claude_takeover(config);
            fallback_server.set_app_handle(app).await;
            fallback_server.start().await.map_err(|fallback_error| {
                format!(
                    "Failed to start Claude takeover: {}; fallback proxy also failed: {}",
                    e, fallback_error
                )
            })?;
            *server_guard = Some(fallback_server);
            return Ok(());
        }
    }
    if !takeover_claude {
        let fallback_server = ProxyServer::new_without_claude_takeover(config);
        fallback_server.set_app_handle(app).await;
        fallback_server.start().await?;
        *server_guard = Some(fallback_server);
        return Ok(());
    }

    *server_guard = Some(server);

    Ok(())
}

/// 停止代理服务器
#[tauri::command]
pub async fn stop_proxy(state: State<'_, ProxyState>) -> Result<(), String> {
    stop_proxy_runtime_only_inner(&state).await?;
    mark_all_client_tools_enabled(false)?;
    Ok(())
}

/// 仅停止当前运行中的代理并恢复外部工具配置，不修改用户保存的开启意图。
///
/// 该命令用于应用退出、更新重启等场景：
/// - 需要确保 Claude/Codex 不再指向本地代理
/// - 但下次打开应用时仍应按用户上次偏好自动恢复代理/接管
#[tauri::command]
pub async fn stop_proxy_runtime_only(state: State<'_, ProxyState>) -> Result<(), String> {
    stop_proxy_runtime_only_inner(&state).await
}

/// 共享的运行时停机逻辑：停止本地服务并恢复外部配置，但不改用户偏好。
pub async fn stop_proxy_runtime_only_inner(state: &State<'_, ProxyState>) -> Result<(), String> {
    let settings = load_settings().unwrap_or_default();
    let port = settings.proxy.port;
    let mut server_guard = state.server.write().await;

    if let Some(server) = server_guard.take() {
        server.stop().await?;
    }

    restore_codex_takeover_if_active(port)?;
    restore_opencode_takeover_if_active(port)?;

    Ok(())
}

/// 获取代理状态
#[tauri::command]
pub async fn get_proxy_status(state: State<'_, ProxyState>) -> Result<ProxyStatus, String> {
    let server_guard = state.server.read().await;

    if let Some(server) = server_guard.as_ref() {
        Ok(server.get_status().await)
    } else {
        Ok(ProxyStatus::default())
    }
}

/// 检查代理是否运行中
#[tauri::command]
pub async fn is_proxy_running(state: State<'_, ProxyState>) -> Result<bool, String> {
    let server_guard = state.server.read().await;

    if let Some(server) = server_guard.as_ref() {
        Ok(server.is_running().await)
    } else {
        Ok(false)
    }
}

/// 单独接管或恢复指定客户端工具配置。
///
/// 当前支持：
/// - `codex`: 修改 `~/.codex/config.toml` 和 `~/.codex/auth.json`
/// - `opencode`: 修改 `~/.config/opencode/opencode.jsonc`（provider.anthropic.options.baseURL）
#[tauri::command]
pub async fn set_takeover_for_app(
    app: String,
    enabled: bool,
    state: State<'_, ProxyState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    match app.as_str() {
        "codex" => set_codex_takeover(enabled, state, app_handle).await,
        "claude_code" | "claude" => set_claude_takeover(enabled, state, app_handle).await,
        "opencode" => set_opencode_takeover(enabled, state, app_handle).await,
        "reasonix" => set_reasonix_takeover(enabled, state, app_handle).await,
        other => Err(format!("Unsupported takeover app: {}", other)),
    }
}

async fn set_claude_takeover(
    enabled: bool,
    state: State<'_, ProxyState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    mark_client_tool_enabled("claude_code", enabled)?;

    let settings = load_settings().unwrap_or_default();
    let port = settings.proxy.port;
    let should_keep_server = settings
        .client_tools
        .profiles
        .iter()
        .any(|profile| profile.enabled);

    {
        let mut server_guard = state.server.write().await;
        if let Some(server) = server_guard.take() {
            server.stop().await?;
        }
    }

    if should_keep_server {
        start_proxy(port, state, app_handle).await?;
    }

    Ok(())
}

pub(crate) fn restore_codex_takeover_if_active(port: u16) -> Result<(), String> {
    let manager = CodexConfigManager::new();
    if !manager.is_takeover_active(port).unwrap_or(false) {
        return Ok(());
    }

    let registry = CodexSourceRegistry::new();
    let current_snapshot = manager.read_live_snapshot()?;
    let handle = manager
        .active_source_id()
        .and_then(|source_id| registry.get(&source_id))
        .or_else(|| registry.latest_for_provider(&current_snapshot.provider_id))
        .or_else(|| {
            registry
                .list_handles()
                .into_iter()
                .max_by_key(|handle| handle.last_used_at_ms.max(handle.last_seen_at_ms))
        });

    let Some(handle) = handle else {
        return Err(
            "Codex is pointed at UsageMeter, but no restorable source handle was found. Restore ~/.codex/config.toml manually or re-enable takeover after fixing the source registry."
                .to_string(),
        );
    };

    let _ = manager.restore_from_source(&handle.id)?;

    Ok(())
}

pub(crate) fn restore_claude_takeover_if_proxy_url_present(port: u16) -> Result<bool, String> {
    let manager = ClaudeConfigManager::new();
    let current_settings = match manager.read_settings() {
        Ok(settings) => settings,
        Err(_) => return Ok(false),
    };

    let Some(base_url) = current_settings.get_base_url() else {
        return Ok(false);
    };

    if !ClaudeConfigManager::is_usagemeter_proxy_url_for_port(&base_url, port) {
        return Ok(false);
    }

    if manager.restore_from_active_source_handle()? {
        return Ok(true);
    }

    if manager.has_backup() {
        manager.restore()?;
        let restored_settings = manager.read_settings()?;
        let restored_base_url = restored_settings.get_base_url();
        let still_proxy = restored_base_url
            .as_deref()
            .map(|url| ClaudeConfigManager::is_usagemeter_proxy_url_for_port(url, port))
            .unwrap_or(false);
        return Ok(!still_proxy);
    }

    Ok(false)
}

pub(crate) fn restore_codex_takeover_if_proxy_url_present(port: u16) -> Result<bool, String> {
    let manager = CodexConfigManager::new();
    let snapshot = match manager.read_live_snapshot() {
        Ok(snapshot) => snapshot,
        Err(_) => return Ok(false),
    };

    if !CodexConfigManager::is_usagemeter_proxy_url_for_port(&snapshot.real_base_url, port) {
        return Ok(false);
    }

    restore_codex_takeover_if_active(port)?;
    Ok(true)
}

async fn set_codex_takeover(
    enabled: bool,
    state: State<'_, ProxyState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let settings = load_settings().unwrap_or_default();
    let port = settings.proxy.port;
    let manager = CodexConfigManager::new();

    if enabled {
        {
            let server_guard = state.server.read().await;
            if server_guard.is_none() {
                drop(server_guard);
                start_proxy(port, state.clone(), app_handle).await?;
            }
        }

        // 读取当前配置并注册到 source registry
        let registry = CodexSourceRegistry::new();
        let mut snapshot = manager.read_live_snapshot()?;
        if CodexConfigManager::is_usagemeter_proxy_url_for_port(&snapshot.real_base_url, port) {
            if let Some(handle) = manager
                .active_source_id()
                .and_then(|source_id| registry.get(&source_id))
                .or_else(|| registry.latest_for_provider(&snapshot.provider_id))
            {
                let _ = manager.restore_from_source(&handle.id)?;
                snapshot = manager.read_live_snapshot()?;
            } else {
                return Err(
                    "Codex is already pointed at UsageMeter, but no original source handle was found. Disable takeover once or restore ~/.codex/config.toml manually, then enable it again."
                        .to_string(),
                );
            }
        }
        let handle = registry.upsert_from_snapshot(snapshot)?;

        // 执行接管：修改 config.toml 的 base_url
        manager.takeover_with_source(port, &handle.id)?;
        mark_client_tool_enabled("codex", true)?;
    } else {
        // 仅当当前 Codex 配置仍指向 UsageMeter 时才恢复快照，避免覆盖用户手动改回的真实配置。
        restore_codex_takeover_if_active(port)?;
        mark_client_tool_enabled("codex", false)?;

        let settings = load_settings().unwrap_or_default();
        if !is_client_tool_enabled(&settings, "claude_code") {
            let mut server_guard = state.server.write().await;
            if let Some(server) = server_guard.take() {
                server.stop().await?;
            }
        }
    }

    Ok(())
}

pub(crate) fn restore_opencode_takeover_if_active(port: u16) -> Result<(), String> {
    let manager = OpenCodeConfigManager::new();
    if !manager.is_takeover_active(port).unwrap_or(false) {
        return Ok(());
    }

    let registry = OpenCodeSourceRegistry::new();
    let active_source_ids: Vec<String> = manager
        .active_routes()
        .into_iter()
        .map(|route| route.source_id)
        .collect();
    let source_ids = if !active_source_ids.is_empty() {
        active_source_ids
    } else {
        registry
            .list_handles()
            .into_iter()
            .map(|handle| handle.id)
            .collect()
    };

    if source_ids.is_empty() {
        return Err(
            "OpenCode is pointed at UsageMeter, but no restorable source handle was found. \
             Restore ~/.config/opencode/opencode.jsonc manually or re-enable takeover."
                .to_string(),
        );
    }

    let _ = manager.restore_from_sources(&source_ids)?;
    Ok(())
}

pub(crate) fn restore_opencode_takeover_if_proxy_url_present(port: u16) -> Result<bool, String> {
    let manager = OpenCodeConfigManager::new();
    let snapshot = match manager.read_live_snapshot() {
        Ok(snapshot) => snapshot,
        Err(_) => return Ok(false),
    };

    if !snapshot.providers.iter().any(|provider| {
        OpenCodeConfigManager::is_usagemeter_proxy_url_for_port(&provider.original_base_url, port)
    }) {
        return Ok(false);
    }

    restore_opencode_takeover_if_active(port)?;
    Ok(true)
}

async fn set_opencode_takeover(
    enabled: bool,
    state: State<'_, ProxyState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let settings = load_settings().unwrap_or_default();
    let port = settings.proxy.port;
    let manager = OpenCodeConfigManager::new();

    if enabled {
        manager.ensure_config_exists()?;
        let registry = OpenCodeSourceRegistry::new();
        let mut route_state = manager.read_live_snapshot()?;
        if route_state.providers.is_empty() {
            return Err(
                "No OpenCode providers with explicit baseURL were found. Configure a provider baseURL first, then enable takeover."
                    .to_string(),
            );
        }

        if route_state.providers.iter().any(|provider| {
            OpenCodeConfigManager::is_usagemeter_proxy_url_for_port(
                &provider.original_base_url,
                port,
            )
        }) {
            let active_source_ids: Vec<String> = manager
                .active_routes()
                .into_iter()
                .map(|route| route.source_id)
                .collect();
            if active_source_ids.is_empty() {
                return Err(
                    "OpenCode is already pointed at UsageMeter, but no original source handle was found. \
                     Disable takeover once or restore the config manually."
                        .to_string(),
                );
            }
            let _ = manager.restore_from_sources(&active_source_ids)?;
            route_state = manager.read_live_snapshot()?;
        }

        let handles = registry.upsert_from_state(&route_state)?;
        if handles.is_empty() {
            return Err(
                "No OpenCode providers with explicit baseURL were found. Configure a provider baseURL first, then enable takeover."
                    .to_string(),
            );
        }

        {
            let server_guard = state.server.read().await;
            if server_guard.is_none() {
                drop(server_guard);
                start_proxy(port, state.clone(), app_handle).await?;
            }
        }

        manager.takeover_with_handles(port, &handles)?;
        mark_client_tool_enabled("opencode", true)?;
    } else {
        restore_opencode_takeover_if_active(port)?;
        mark_client_tool_enabled("opencode", false)?;

        let settings = load_settings().unwrap_or_default();
        let any_tool_enabled = settings
            .client_tools
            .profiles
            .iter()
            .any(|profile| profile.enabled);
        if !any_tool_enabled {
            let mut server_guard = state.server.write().await;
            if let Some(server) = server_guard.take() {
                server.stop().await?;
            }
        }
    }

    Ok(())
}

pub(crate) fn restore_reasonix_takeover_if_active(port: u16) -> Result<(), String> {
    let manager = ReasonixConfigManager::new();
    if !manager.is_takeover_active(port).unwrap_or(false) {
        return Ok(());
    }

    let active_source_ids = manager.active_source_ids();
    if active_source_ids.is_empty() {
        return Err(
            "Reasonix is pointed at UsageMeter, but no restorable source handle was found. \
             Restore the Reasonix config.toml manually or re-enable takeover."
                .to_string(),
        );
    }

    let _ = manager.restore_from_sources(&active_source_ids)?;
    Ok(())
}

pub(crate) fn restore_reasonix_takeover_if_proxy_url_present(port: u16) -> Result<bool, String> {
    let manager = ReasonixConfigManager::new();
    let snapshot = match manager.read_live_snapshot() {
        Ok(snapshot) => snapshot,
        Err(_) => return Ok(false),
    };

    if !snapshot.providers.iter().any(|provider| {
        ReasonixConfigManager::is_usagemeter_proxy_url_for_port(&provider.original_base_url, port)
    }) {
        return Ok(false);
    }

    restore_reasonix_takeover_if_active(port)?;
    Ok(true)
}

async fn set_reasonix_takeover(
    enabled: bool,
    state: State<'_, ProxyState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let settings = load_settings().unwrap_or_default();
    let port = settings.proxy.port;
    let manager = ReasonixConfigManager::new();

    if enabled {
        manager.ensure_config_exists()?;
        let registry = ReasonixSourceRegistry::new();
        let mut route_state = manager.read_live_snapshot()?;
        if route_state.providers.is_empty() {
            return Err(
                "No Reasonix providers with a base_url were found. Run `reasonix setup` first, then enable takeover."
                    .to_string(),
            );
        }

        // 若配置已指向 UsageMeter，先恢复再重新接管，避免把代理地址当成真实上游保存。
        if route_state.providers.iter().any(|provider| {
            ReasonixConfigManager::is_usagemeter_proxy_url_for_port(
                &provider.original_base_url,
                port,
            )
        }) {
            let active_source_ids = manager.active_source_ids();
            if active_source_ids.is_empty() {
                return Err(
                    "Reasonix is already pointed at UsageMeter, but no original source handle was found. \
                     Disable takeover once or restore the config manually, then enable it again."
                        .to_string(),
                );
            }
            let _ = manager.restore_from_sources(&active_source_ids)?;
            route_state = manager.read_live_snapshot()?;
        }

        let handles = registry.upsert_from_state(&route_state)?;
        if handles.is_empty() {
            return Err("No Reasonix providers with a base_url were found.".to_string());
        }

        {
            let server_guard = state.server.read().await;
            if server_guard.is_none() {
                drop(server_guard);
                start_proxy(port, state.clone(), app_handle).await?;
            }
        }

        manager.takeover_with_handles(port, &handles)?;
        mark_client_tool_enabled("reasonix", true)?;
    } else {
        restore_reasonix_takeover_if_active(port)?;
        mark_client_tool_enabled("reasonix", false)?;

        let settings = load_settings().unwrap_or_default();
        let any_tool_enabled = settings
            .client_tools
            .profiles
            .iter()
            .any(|profile| profile.enabled);
        if !any_tool_enabled {
            let mut server_guard = state.server.write().await;
            if let Some(server) = server_guard.take() {
                server.stop().await?;
            }
        }
    }

    Ok(())
}

fn proxy_config_from_settings(port: u16, settings: &crate::models::AppSettings) -> ProxyConfig {
    ProxyConfig {
        enabled: true,
        port,
        target_base_url: "https://api.anthropic.com".to_string(),
        request_timeout: settings.proxy.request_timeout_seconds,
        streaming_idle_timeout: settings.proxy.streaming_idle_timeout_seconds,
    }
}

fn is_client_tool_enabled(settings: &crate::models::AppSettings, tool: &str) -> bool {
    settings
        .client_tools
        .profiles
        .iter()
        .any(|profile| profile.tool == tool && profile.enabled)
}

fn mark_client_tool_enabled(tool: &str, enabled: bool) -> Result<(), String> {
    let mut settings = load_settings().unwrap_or_default();
    let now = chrono::Utc::now().timestamp_millis();
    if let Some(profile) = settings
        .client_tools
        .profiles
        .iter_mut()
        .find(|profile| profile.tool == tool)
    {
        profile.enabled = enabled;
        profile.last_seen_ms = now;
    }
    settings.proxy.enabled = settings
        .client_tools
        .profiles
        .iter()
        .any(|profile| profile.enabled);
    save_settings_internal(settings).map_err(String::from)
}

fn mark_all_client_tools_enabled(enabled: bool) -> Result<(), String> {
    let mut settings = load_settings().unwrap_or_default();
    let now = chrono::Utc::now().timestamp_millis();
    for profile in &mut settings.client_tools.profiles {
        profile.enabled = enabled;
        profile.last_seen_ms = now;
    }
    settings.proxy.enabled = enabled && settings.client_tools.profiles.iter().any(|p| p.enabled);
    save_settings_internal(settings).map_err(String::from)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolTakeoverStatus {
    pub tool: String,
    pub enabled: bool,
    pub takeover_active: bool,
    pub conflict_paused: bool,
    pub config_path: Option<String>,
    pub auth_path: Option<String>,
    pub auth_mode: Option<String>,
    pub official_provider: bool,
    pub active_source_id: Option<String>,
    pub managed_provider_ids: Option<Vec<String>>,
    pub conflict_external_base_url: Option<String>,
    pub scope_warning_key: Option<String>,
    pub last_error: Option<String>,
}

#[tauri::command]
pub async fn get_takeover_statuses(
    state: State<'_, ProxyState>,
) -> Result<Vec<ToolTakeoverStatus>, String> {
    let settings = load_settings().unwrap_or_default();
    let port = settings.proxy.port;
    let server_guard = state.server.read().await;
    let server = server_guard.as_ref();
    let claude_conflict_paused = match server {
        Some(server) => server.is_takeover_conflict_paused("claude_code").await,
        None => false,
    };
    let claude_conflict_external_base_url = match server {
        Some(server) => {
            server
                .takeover_conflict_external_base_url("claude_code")
                .await
        }
        None => None,
    };
    let codex_conflict_paused = match server {
        Some(server) => server.is_takeover_conflict_paused("codex").await,
        None => false,
    };
    let codex_conflict_external_base_url = match server {
        Some(server) => server.takeover_conflict_external_base_url("codex").await,
        None => None,
    };
    let opencode_conflict_paused = match server {
        Some(server) => server.is_takeover_conflict_paused("opencode").await,
        None => false,
    };
    let opencode_conflict_external_base_url = match server {
        Some(server) => server.takeover_conflict_external_base_url("opencode").await,
        None => None,
    };
    let reasonix_conflict_paused = match server {
        Some(server) => server.is_takeover_conflict_paused("reasonix").await,
        None => false,
    };
    let reasonix_conflict_external_base_url = match server {
        Some(server) => server.takeover_conflict_external_base_url("reasonix").await,
        None => None,
    };
    drop(server_guard);

    let codex_manager = CodexConfigManager::new();
    let codex_snapshot = codex_manager.read_live_snapshot().ok();
    let codex_auth_mode = codex_snapshot
        .as_ref()
        .map(|snapshot| match snapshot.auth_mode {
            crate::proxy::CodexAuthMode::ChatGpt => "chat_gpt".to_string(),
            crate::proxy::CodexAuthMode::ApiKey => "api_key".to_string(),
        });
    let codex_official_provider = codex_snapshot
        .as_ref()
        .map(codex_snapshot_uses_official_provider)
        .unwrap_or(false);
    let (codex_active, codex_source, codex_error) = match codex_manager.is_takeover_active(port) {
        Ok(active) => (active, codex_manager.active_source_id(), None),
        Err(e) => (false, None, Some(e)),
    };
    let codex_enabled = settings
        .client_tools
        .profiles
        .iter()
        .find(|profile| profile.tool == "codex")
        .map(|profile| profile.enabled)
        .unwrap_or(false);
    let claude_manager = ClaudeConfigManager::new();
    let claude_enabled = settings
        .client_tools
        .profiles
        .iter()
        .find(|profile| profile.tool == "claude_code")
        .map(|profile| profile.enabled)
        .unwrap_or(false);

    let opencode_manager = OpenCodeConfigManager::new();
    let opencode_snapshot = opencode_manager.read_live_snapshot().unwrap_or_default();
    let opencode_managed_provider_ids: Vec<String> = opencode_snapshot
        .providers
        .iter()
        .map(|provider| provider.provider_id.clone())
        .collect();
    let opencode_official_provider = opencode_snapshot
        .providers
        .iter()
        .any(|provider| matches!(provider.provider_id.as_str(), "anthropic" | "openai"));
    let opencode_enabled = settings
        .client_tools
        .profiles
        .iter()
        .find(|profile| profile.tool == "opencode")
        .map(|profile| profile.enabled)
        .unwrap_or(false);
    let (opencode_active, opencode_source, opencode_error) =
        match opencode_manager.is_takeover_active(port) {
            Ok(active) => (active, opencode_manager.active_source_id(), None),
            Err(e) => (false, None, Some(e)),
        };

    let reasonix_manager = ReasonixConfigManager::new();
    let reasonix_snapshot = reasonix_manager.read_live_snapshot().unwrap_or_default();
    let reasonix_managed_provider_ids: Vec<String> = reasonix_snapshot
        .providers
        .iter()
        .map(|provider| provider.provider_name.clone())
        .collect();
    let reasonix_enabled = settings
        .client_tools
        .profiles
        .iter()
        .find(|profile| profile.tool == "reasonix")
        .map(|profile| profile.enabled)
        .unwrap_or(false);
    let (reasonix_active, reasonix_source, reasonix_error) =
        match reasonix_manager.is_takeover_active(port) {
            Ok(active) => (active, reasonix_manager.active_source_id(), None),
            Err(e) => (false, None, Some(e)),
        };

    Ok(vec![
        ToolTakeoverStatus {
            tool: "claude_code".to_string(),
            enabled: claude_enabled,
            takeover_active: claude_manager.is_takeover_active(),
            conflict_paused: claude_conflict_paused,
            config_path: Some(claude_manager.settings_path().display().to_string()),
            auth_path: None,
            auth_mode: None,
            official_provider: claude_manager.uses_official_provider(),
            active_source_id: None,
            managed_provider_ids: None,
            conflict_external_base_url: claude_conflict_external_base_url,
            scope_warning_key: None,
            last_error: None,
        },
        ToolTakeoverStatus {
            tool: "codex".to_string(),
            enabled: codex_enabled,
            takeover_active: codex_active,
            conflict_paused: codex_conflict_paused,
            config_path: Some(codex_manager.config_path().display().to_string()),
            auth_path: Some(codex_manager.auth_path().display().to_string()),
            auth_mode: codex_auth_mode,
            official_provider: codex_official_provider,
            active_source_id: codex_source,
            managed_provider_ids: None,
            conflict_external_base_url: codex_conflict_external_base_url,
            scope_warning_key: None,
            last_error: codex_error,
        },
        ToolTakeoverStatus {
            tool: "opencode".to_string(),
            enabled: opencode_enabled,
            takeover_active: opencode_active,
            conflict_paused: opencode_conflict_paused,
            config_path: Some(opencode_manager.config_path().display().to_string()),
            auth_path: None,
            auth_mode: Some("api_key".to_string()),
            official_provider: opencode_official_provider,
            active_source_id: opencode_source,
            managed_provider_ids: Some(opencode_managed_provider_ids),
            conflict_external_base_url: opencode_conflict_external_base_url,
            scope_warning_key: Some("settings.opencodeConfigScopeWarning".to_string()),
            last_error: opencode_error,
        },
        ToolTakeoverStatus {
            tool: "reasonix".to_string(),
            enabled: reasonix_enabled,
            takeover_active: reasonix_active,
            conflict_paused: reasonix_conflict_paused,
            config_path: Some(reasonix_manager.config_path().display().to_string()),
            auth_path: None,
            auth_mode: Some("api_key".to_string()),
            official_provider: false,
            active_source_id: reasonix_source,
            managed_provider_ids: Some(reasonix_managed_provider_ids),
            conflict_external_base_url: reasonix_conflict_external_base_url,
            scope_warning_key: Some("settings.reasonixConfigScopeWarning".to_string()),
            last_error: reasonix_error,
        },
    ])
}

#[tauri::command]
pub async fn resolve_takeover_conflict(
    tool: String,
    action: String,
    state: State<'_, ProxyState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let tool = if tool == "claude" {
        "claude_code".to_string()
    } else {
        tool
    };

    match action.as_str() {
        "disable_takeover" => set_takeover_for_app(tool, false, state, app_handle).await,
        "pause" => {
            let server_guard = state.server.read().await;
            let Some(server) = server_guard.as_ref() else {
                return Ok(());
            };
            server.pause_takeover_conflict(&tool).await;
            Ok(())
        }
        "force_reclaim" => {
            {
                let server_guard = state.server.read().await;
                let Some(server) = server_guard.as_ref() else {
                    return Err("Proxy server is not running".to_string());
                };
                server.force_reclaim_takeover(&tool).await?;
            }
            mark_client_tool_enabled(&tool, true)?;
            Ok(())
        }
        other => Err(format!("Unsupported takeover conflict action: {}", other)),
    }
}

/// 获取代理使用量统计
#[tauri::command]
pub async fn get_proxy_usage(state: State<'_, ProxyState>) -> Result<ProxyUsageSnapshot, String> {
    let server_guard = state.server.read().await;

    if let Some(server) = server_guard.as_ref() {
        let collector = server.get_collector();
        // 对于代理使用量快照，始终包含所有请求（用于状态显示）
        let window_stats = collector.get_all_window_stats(true).await;

        let windows: Vec<WindowUsageData> = window_stats
            .into_iter()
            .map(|(window, stats)| WindowUsageData {
                window,
                token_used: stats.token_used,
                input_tokens: stats.input_tokens,
                output_tokens: stats.output_tokens,
                cache_create_tokens: stats.cache_create_tokens,
                cache_read_tokens: stats.cache_read_tokens,
                request_used: stats.request_used,
            })
            .collect();

        Ok(ProxyUsageSnapshot {
            generated_at_epoch: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            windows,
            source: "proxy".to_string(),
        })
    } else {
        Ok(ProxyUsageSnapshot {
            generated_at_epoch: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            windows: Vec::new(),
            source: "proxy".to_string(),
        })
    }
}

/// 代理窗口使用量数据
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowUsageData {
    pub window: String,
    pub token_used: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub request_used: u64,
}

/// 代理使用量快照
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyUsageSnapshot {
    pub generated_at_epoch: u64,
    pub windows: Vec<WindowUsageData>,
    pub source: String,
}

/// 准备退出：停止代理并恢复配置
/// 在应用退出前调用，确保 Claude 配置被恢复
#[tauri::command]
pub async fn prepare_exit(state: State<'_, ProxyState>) -> Result<(), String> {
    stop_proxy_runtime_only_inner(&state).await
}

/// 确认退出：前端清理完成后调用
#[tauri::command]
pub async fn confirm_exit(app: tauri::AppHandle) {
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_config_uses_settings_timeouts() {
        let mut settings = crate::models::AppSettings::default();
        settings.proxy.request_timeout_seconds = 240;
        settings.proxy.streaming_idle_timeout_seconds = 15;

        let config = proxy_config_from_settings(18765, &settings);
        assert_eq!(config.request_timeout, 240);
        assert_eq!(config.streaming_idle_timeout, 15);
    }
}
