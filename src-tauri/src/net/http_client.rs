//! 全局 HTTP 客户端工厂
//!
//! 设计目标：
//! - 所有出站 HTTP 请求经由本工厂取用 `reqwest::Client`，统一应用代理配置
//! - 配置变更时调用 [`HttpClientFactory::reload`] 即时重建内部 client，无需重启
//! - 业务模块按超时档位选择：`short` / `standard` / `long` / `webdav`
//!
//! 代理行为约定：
//! - `NetworkProxyConfig.enabled == false`：不调用 `.proxy()` 也不调用 `.no_proxy()`，
//!   reqwest 默认会读 `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` 环境变量。
//!   这正好实现了"自动跟随系统代理（如 Clash）"。
//! - `NetworkProxyConfig.enabled == true`：通过 `.proxy(Proxy::all(...))` 强制走指定地址；
//!   若配置非法则直接拒绝构建，避免静默回退导致 UI 与实际行为不一致。

use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use reqwest::Client;

use crate::models::NetworkProxyConfig;

const APP_USER_AGENT: &str = concat!("UsageMeter/", env!("CARGO_PKG_VERSION"));

const TIMEOUT_SHORT_SECS: u64 = 15;
const TIMEOUT_STANDARD_SECS: u64 = 30;
const TIMEOUT_LONG_SECS: u64 = 120;
const TIMEOUT_WEBDAV_TOTAL_SECS: u64 = 60;
const TIMEOUT_WEBDAV_CONNECT_SECS: u64 = 10;

/// 内部持有的一组 client 与生效配置快照
struct ClientBundle {
    short: Client,
    standard: Client,
    long: Client,
    webdav: Client,
    config: NetworkProxyConfig,
}

/// 全局 HTTP 客户端工厂（单例）
pub struct HttpClientFactory {
    inner: RwLock<ClientBundle>,
}

static INSTANCE: OnceLock<HttpClientFactory> = OnceLock::new();

impl HttpClientFactory {
    /// 用初始配置创建并初始化全局实例。重复调用安全（仅第一次生效）。
    ///
    /// 若传入配置非法或构建失败，将退化到默认配置（跟随系统代理），
    /// 并在 stderr 打印错误标识，保证应用可继续启动。
    pub fn init(config: NetworkProxyConfig) {
        let _ = INSTANCE.get_or_init(|| {
            let bundle = match config.validate().and_then(|_| {
                build_bundle(&config).map_err(|e| format!("ERR_HTTP_CLIENT_INIT: {}", e))
            }) {
                Ok(b) => b,
                Err(err) => {
                    eprintln!("[UsageMeter] {err} — falling back to default (system proxy).");
                    build_bundle(&NetworkProxyConfig::default()).expect("ERR_HTTP_CLIENT_FALLBACK")
                }
            };
            HttpClientFactory {
                inner: RwLock::new(bundle),
            }
        });
    }

    /// 获取全局实例。必须先调用 [`HttpClientFactory::init`]，否则 panic。
    /// 这是强契约：避免业务代码无声地使用未配置的 client。
    pub fn global() -> &'static HttpClientFactory {
        INSTANCE
            .get()
            .expect("ERR_HTTP_CLIENT_NOT_INITIALIZED: call HttpClientFactory::init() at startup")
    }

    /// 短超时 client：检查更新、汇率查询等轻量请求（15s）
    pub fn short(&self) -> Client {
        self.inner
            .read()
            .expect("ERR_HTTP_CLIENT_LOCK")
            .short
            .clone()
    }

    /// 标准超时 client：用量查询、模型价格同步等（30s）
    pub fn standard(&self) -> Client {
        self.inner
            .read()
            .expect("ERR_HTTP_CLIENT_LOCK")
            .standard
            .clone()
    }

    /// 长超时 client：API 转发（120s）
    pub fn long(&self) -> Client {
        self.inner
            .read()
            .expect("ERR_HTTP_CLIENT_LOCK")
            .long
            .clone()
    }

    /// WebDAV 专用 client：10s connect + 60s total
    pub fn webdav(&self) -> Client {
        self.inner
            .read()
            .expect("ERR_HTTP_CLIENT_LOCK")
            .webdav
            .clone()
    }

    /// 配置热更新：保存设置后调用。仅当配置实际变化时才重建。
    /// 启用模式下若配置非法（如 host 为空、端口为 0），直接返回 Err，不应用变更，
    /// 避免静默回退到系统代理而 UI 仍显示"已启用"。
    pub fn reload(&self, config: &NetworkProxyConfig) -> Result<(), String> {
        config.validate()?;
        let mut guard = self
            .inner
            .write()
            .map_err(|_| "ERR_HTTP_CLIENT_LOCK".to_string())?;
        if &guard.config == config {
            return Ok(());
        }
        let new_bundle =
            build_bundle(config).map_err(|e| format!("ERR_HTTP_CLIENT_REBUILD: {}", e))?;
        *guard = new_bundle;
        eprintln!(
            "[UsageMeter] Network proxy reloaded: enabled={}, scheme={}, host={}, port={}",
            config.enabled, config.scheme, config.host, config.port
        );
        Ok(())
    }

    /// 用给定配置构造一次性 client，用于"测试连接"功能（不影响全局状态）。
    pub fn build_ephemeral(
        config: &NetworkProxyConfig,
        timeout_secs: u64,
    ) -> Result<Client, String> {
        config.validate()?;
        build_one(config, timeout_secs, None)
            .map_err(|e| format!("ERR_HTTP_CLIENT_EPHEMERAL: {}", e))
    }

    /// 构造一次性流式 client：仅设置连接超时和读空闲超时，不设置总时长超时。
    ///
    /// 适用于 SSE / 长流式响应，避免正常的长输出因为总超时被提前截断。
    pub fn build_streaming(
        &self,
        connect_timeout_secs: u64,
        read_idle_timeout_secs: u64,
    ) -> Result<Client, String> {
        let guard = self
            .inner
            .read()
            .map_err(|_| "ERR_HTTP_CLIENT_LOCK".to_string())?;
        build_streaming_one(&guard.config, connect_timeout_secs, read_idle_timeout_secs)
            .map_err(|e| format!("ERR_HTTP_CLIENT_STREAMING: {}", e))
    }

    /// 将当前生效的代理配置应用到外部传入的 `ClientBuilder`。
    ///
    /// 用于需要自定义 builder 链（如 OpenAI 流式响应所需的 http1_only、no_gzip 等）
    /// 但仍需统一应用代理配置的场景。
    pub fn apply_proxy_to_builder(
        &self,
        builder: reqwest::ClientBuilder,
    ) -> reqwest::ClientBuilder {
        let guard = self.inner.read().expect("ERR_HTTP_CLIENT_LOCK");
        let config = &guard.config;
        if !config.enabled || config.host.trim().is_empty() {
            return builder;
        }
        let url = config.build_url();
        match reqwest::Proxy::all(&url) {
            Err(err) => {
                // 理论上不可达：构造期已校验。防御性兜底：不应用代理，
                // 保留调用者 builder 的全部设置（http1_only、no_gzip 等）。
                eprintln!("[UsageMeter] ERR_PROXY_APPLY_UNEXPECTED: {err}");
                builder
            }
            Ok(mut proxy) => {
                if config.has_auth() {
                    let user = config.username.clone().unwrap_or_default();
                    let pass = config.password.clone().unwrap_or_default();
                    proxy = proxy.basic_auth(&user, &pass);
                }
                builder.proxy(proxy)
            }
        }
    }
}

fn build_bundle(config: &NetworkProxyConfig) -> Result<ClientBundle, String> {
    Ok(ClientBundle {
        short: build_one(config, TIMEOUT_SHORT_SECS, None)?,
        standard: build_one(config, TIMEOUT_STANDARD_SECS, None)?,
        long: build_one(config, TIMEOUT_LONG_SECS, None)?,
        webdav: build_one(
            config,
            TIMEOUT_WEBDAV_TOTAL_SECS,
            Some(TIMEOUT_WEBDAV_CONNECT_SECS),
        )?,
        config: config.clone(),
    })
}

fn build_one(
    config: &NetworkProxyConfig,
    timeout_secs: u64,
    connect_timeout_secs: Option<u64>,
) -> Result<Client, String> {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .user_agent(APP_USER_AGENT);

    if let Some(connect) = connect_timeout_secs {
        builder = builder.connect_timeout(Duration::from_secs(connect));
    }

    builder = apply_proxy(builder, config)?;
    builder
        .build()
        .map_err(|e| format!("ERR_HTTP_CLIENT_BUILD: {}", e))
}

fn build_streaming_one(
    config: &NetworkProxyConfig,
    connect_timeout_secs: u64,
    read_idle_timeout_secs: u64,
) -> Result<Client, String> {
    let mut builder = Client::builder().user_agent(APP_USER_AGENT);

    if connect_timeout_secs > 0 {
        builder = builder.connect_timeout(Duration::from_secs(connect_timeout_secs));
    }
    if read_idle_timeout_secs > 0 {
        builder = builder.read_timeout(Duration::from_secs(read_idle_timeout_secs));
    }

    builder = apply_proxy(builder, config)?;
    builder
        .build()
        .map_err(|e| format!("ERR_HTTP_CLIENT_BUILD: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_streaming_allows_disabling_read_idle_timeout() {
        let config = NetworkProxyConfig::default();
        let client = build_streaming_one(&config, 120, 0);
        assert!(client.is_ok());
    }
}

/// 把代理配置应用到 builder 上。`enabled=false` 时什么都不做，
/// reqwest 默认会读 HTTP_PROXY/HTTPS_PROXY/ALL_PROXY 环境变量。
/// 启用模式下若代理 URL 非法，返回 Err 而非静默回退。
fn apply_proxy(
    builder: reqwest::ClientBuilder,
    config: &NetworkProxyConfig,
) -> Result<reqwest::ClientBuilder, String> {
    if !config.enabled || config.host.trim().is_empty() {
        return Ok(builder);
    }
    let url = config.build_url();
    let mut proxy = reqwest::Proxy::all(&url)
        .map_err(|e| format!("ERR_PROXY_URL_INVALID: '{url}' detail: {e}"))?;
    if config.has_auth() {
        let user = config.username.clone().unwrap_or_default();
        let pass = config.password.clone().unwrap_or_default();
        proxy = proxy.basic_auth(&user, &pass);
    }
    Ok(builder.proxy(proxy))
}
