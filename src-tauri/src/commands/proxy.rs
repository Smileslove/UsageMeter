//! 代理相关的 Tauri 命令

use crate::proxy::{ProxyConfig, ProxyServer, ProxyStatus};
use tauri::State;

use super::usage::ProxyState;

/// 启动代理服务器
#[tauri::command]
pub async fn start_proxy(
    port: u16,
    state: State<'_, ProxyState>,
) -> Result<(), String> {
    let mut server_guard = state.server.write().await;

    // 检查是否已在运行
    if server_guard.is_some() {
        return Err("Proxy is already running".to_string());
    }

    // 创建配置
    let config = ProxyConfig {
        enabled: true,
        port,
        target_base_url: "https://api.anthropic.com".to_string(),
        request_timeout: 120,
        streaming_idle_timeout: 30,
    };

    // 创建并启动服务器
    let server = ProxyServer::new(config);
    server.start().await?;

    *server_guard = Some(server);

    Ok(())
}

/// 停止代理服务器
#[tauri::command]
pub async fn stop_proxy(state: State<'_, ProxyState>) -> Result<(), String> {
    let mut server_guard = state.server.write().await;

    if let Some(server) = server_guard.take() {
        server.stop().await?;
    }

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
