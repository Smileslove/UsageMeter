//! 网络代理相关 Tauri 命令
//!
//! 由于网络代理配置本身已通过 `load_settings` / `save_settings` 完成读写
//! （前者初始化 client 工厂，后者触发热更新），本模块只暴露"测试连接"命令。

use std::time::Instant;

use serde::Serialize;

use crate::models::NetworkProxyConfig;
use crate::net::HttpClientFactory;

const TEST_TIMEOUT_SECS: u64 = 8;

fn endpoint_for_target(target: &str) -> &'static str {
    match target {
        "anthropic" => "https://api.anthropic.com",
        "openai" => "https://api.openai.com",
        _ => "https://api.github.com/zen",
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkProxyTestResult {
    pub ok: bool,
    pub reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// 标识化错误：networkProxy.testTimeout / testConnectFailed / testAuthFailed / testHttpError / testUnknownError
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
}

/// 测试给定网络代理配置能否连通指定目标（不持久化、不影响全局 client）
/// target: "github" | "anthropic" | "openai"
#[tauri::command]
pub async fn test_network_proxy(
    config: NetworkProxyConfig,
    target: Option<String>,
) -> Result<NetworkProxyTestResult, String> {
    let endpoint = endpoint_for_target(target.as_deref().unwrap_or("github"));
    let client = HttpClientFactory::build_ephemeral(&config, TEST_TIMEOUT_SECS)?;

    let start = Instant::now();
    let response = client.get(endpoint).send().await;
    let elapsed_ms = start.elapsed().as_millis() as u64;

    match response {
        Ok(resp) => {
            let status = resp.status();
            if status.as_u16() == 407 {
                Ok(NetworkProxyTestResult {
                    ok: false,
                    reachable: false,
                    latency_ms: Some(elapsed_ms),
                    status: Some(status.as_u16()),
                    error_kind: Some("testAuthFailed".to_string()),
                    error_detail: None,
                })
            } else if status.is_server_error() {
                Ok(NetworkProxyTestResult {
                    ok: false,
                    reachable: false,
                    latency_ms: Some(elapsed_ms),
                    status: Some(status.as_u16()),
                    error_kind: Some("testHttpError".to_string()),
                    error_detail: Some(status.to_string()),
                })
            } else {
                Ok(NetworkProxyTestResult {
                    ok: status.is_success(),
                    reachable: true,
                    latency_ms: Some(elapsed_ms),
                    status: Some(status.as_u16()),
                    error_kind: if status.is_success() {
                        None
                    } else {
                        Some("testHttpError".to_string())
                    },
                    error_detail: Some(status.to_string()),
                })
            }
        }
        Err(err) => {
            let detail = err.to_string();
            let detail_lower = detail.to_ascii_lowercase();
            // socks5 鉴权失败通常以 connect error 形式呈现（错误链中包含
            // "authentication"、"auth" 等关键词），需在分类前先做字符串识别。
            let looks_like_auth = detail_lower.contains("authentication")
                || detail_lower.contains("auth failed")
                || detail_lower.contains("auth required")
                || detail_lower.contains("407");
            let kind = if looks_like_auth {
                "testAuthFailed"
            } else if err.is_timeout() {
                "testTimeout"
            } else if err.is_connect() {
                "testConnectFailed"
            } else {
                "testUnknownError"
            };
            Ok(NetworkProxyTestResult {
                ok: false,
                reachable: false,
                latency_ms: None,
                status: None,
                error_kind: Some(kind.to_string()),
                error_detail: Some(detail),
            })
        }
    }
}
