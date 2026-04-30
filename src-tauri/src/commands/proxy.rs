//! 代理相关的 Tauri 命令

use crate::proxy::{
    ClaudeConfigManager, CodexConfigManager, CodexSourceRegistry, ProxyConfig, ProxyServer,
    ProxyStatus,
};
use tauri::State;

use super::usage::ProxyState;
use super::{load_settings, save_settings};

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
    let config = proxy_config(port);

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
    let settings = load_settings().unwrap_or_default();
    let port = settings.proxy.port;
    let mut server_guard = state.server.write().await;

    if let Some(server) = server_guard.take() {
        server.stop().await?;
    }

    restore_codex_takeover_if_active(port)?;
    mark_all_client_tools_enabled(false)?;

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

fn restore_codex_takeover_if_active(port: u16) -> Result<(), String> {
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

fn proxy_config(port: u16) -> ProxyConfig {
    ProxyConfig {
        enabled: true,
        port,
        target_base_url: "https://api.anthropic.com".to_string(),
        request_timeout: 120,
        streaming_idle_timeout: 30,
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
    save_settings(settings)
}

fn mark_all_client_tools_enabled(enabled: bool) -> Result<(), String> {
    let mut settings = load_settings().unwrap_or_default();
    let now = chrono::Utc::now().timestamp_millis();
    for profile in &mut settings.client_tools.profiles {
        profile.enabled = enabled;
        profile.last_seen_ms = now;
    }
    settings.proxy.enabled = enabled && settings.client_tools.profiles.iter().any(|p| p.enabled);
    save_settings(settings)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolTakeoverStatus {
    pub tool: String,
    pub enabled: bool,
    pub takeover_active: bool,
    pub config_path: Option<String>,
    pub auth_path: Option<String>,
    pub auth_mode: Option<String>,
    pub active_source_id: Option<String>,
    pub last_error: Option<String>,
}

#[tauri::command]
pub async fn get_takeover_statuses() -> Result<Vec<ToolTakeoverStatus>, String> {
    let settings = load_settings().unwrap_or_default();
    let port = settings.proxy.port;
    let codex_manager = CodexConfigManager::new();
    let codex_auth_mode =
        codex_manager
            .read_live_snapshot()
            .ok()
            .map(|snapshot| match snapshot.auth_mode {
                crate::proxy::CodexAuthMode::ChatGpt => "chat_gpt".to_string(),
                crate::proxy::CodexAuthMode::ApiKey => "api_key".to_string(),
            });
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

    Ok(vec![
        ToolTakeoverStatus {
            tool: "claude_code".to_string(),
            enabled: claude_enabled,
            takeover_active: claude_manager.is_takeover_active(),
            config_path: Some(claude_manager.settings_path().display().to_string()),
            auth_path: None,
            auth_mode: None,
            active_source_id: None,
            last_error: None,
        },
        ToolTakeoverStatus {
            tool: "codex".to_string(),
            enabled: codex_enabled,
            takeover_active: codex_active,
            config_path: Some(codex_manager.config_path().display().to_string()),
            auth_path: Some(codex_manager.auth_path().display().to_string()),
            auth_mode: codex_auth_mode,
            active_source_id: codex_source,
            last_error: codex_error,
        },
    ])
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
    let mut server_guard = state.server.write().await;

    if let Some(server) = server_guard.take() {
        // 停止服务器（内部会调用 ClaudeConfigManager::restore()）
        server.stop().await?;
    }

    Ok(())
}

/// 确认退出：前端清理完成后调用
#[tauri::command]
pub async fn confirm_exit(app: tauri::AppHandle) {
    app.exit(0);
}
