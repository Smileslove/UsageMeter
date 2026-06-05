//! Reasonix CLI/桌面端配置接管支持。
//!
//! Reasonix（Go）将 provider 配置写在 `<config_dir>/reasonix/config.toml`，
//! 其中 `config_dir` 与 Rust `dirs::config_dir()` 逐平台一致：
//! - macOS: `~/Library/Application Support/reasonix/`
//! - Linux: `~/.config/reasonix/`
//! - Windows: `%AppData%\reasonix\`
//!
//! provider 以 TOML 数组表 `[[providers]]` 声明，每项含 `name`/`kind`/`base_url`/
//! `api_key_env`。UsageMeter 仅改写指向真实上游的 provider 的 `base_url`，将其
//! 指向本地代理 `http://127.0.0.1:<port>/reasonix/source/<id>`，并把原始
//! `name`/`kind`/`base_url` 存入 source handle，停止/恢复时还原。
//!
//! 密钥（`api_key_env`）不改动——Reasonix 仍从环境变量注入，代理透传认证头。
//!
//! 限制：Reasonix 解析顺序为 `flag > ./reasonix.toml > <config_dir>/reasonix/config.toml`。
//! 本接管只改写全局 config.toml；若用户在项目级 `./reasonix.toml` 重定义了同名
//! provider，则项目级覆盖全局，代理无法采集该项目数据。前端需在开启代理时提示。

use super::url_identity;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use toml_edit::{DocumentMut, Item, Value};

/// 接管前保存的单个 Reasonix provider 路由状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasonixProviderRouteState {
    pub provider_name: String,
    pub kind: String,
    pub original_base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReasonixRouteState {
    #[serde(default)]
    pub providers: Vec<ReasonixProviderRouteState>,
}

/// 注册到 UsageMeter 的 Reasonix 来源句柄（provider 级）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasonixSourceHandle {
    pub id: String,
    pub provider_name: String,
    pub kind: String,
    pub real_base_url: String,
    pub route_state: ReasonixProviderRouteState,
    pub created_at_ms: i64,
    pub last_seen_at_ms: i64,
    pub last_used_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ReasonixSourceRegistryData {
    #[serde(default)]
    handles: Vec<ReasonixSourceHandle>,
}

// === Source Registry ===

pub struct ReasonixSourceRegistry {
    path: PathBuf,
}

impl ReasonixSourceRegistry {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            path: home
                .join(".usagemeter")
                .join("reasonix_proxy_source_handles.json"),
        }
    }

    pub fn get(&self, id: &str) -> Option<ReasonixSourceHandle> {
        self.read_data()
            .ok()?
            .handles
            .into_iter()
            .find(|handle| handle.id == id)
    }

    #[allow(dead_code)]
    pub fn list_handles(&self) -> Vec<ReasonixSourceHandle> {
        self.read_data()
            .map(|data| data.handles)
            .unwrap_or_default()
    }

    pub fn touch_used(&self, id: &str) -> Result<(), String> {
        let mut data = self.read_data()?;
        let now = now_ms();
        if let Some(handle) = data.handles.iter_mut().find(|handle| handle.id == id) {
            handle.last_used_at_ms = now;
            self.write_data(&data)?;
        }
        Ok(())
    }

    pub fn upsert_provider_state(
        &self,
        provider_state: ReasonixProviderRouteState,
    ) -> Result<ReasonixSourceHandle, String> {
        if ReasonixConfigManager::is_usagemeter_proxy_url(&provider_state.original_base_url) {
            return Err(
                "Refusing to register UsageMeter proxy URL as a Reasonix upstream".to_string(),
            );
        }

        let id = compute_handle_id(
            &provider_state.provider_name,
            &provider_state.original_base_url,
        )?;
        let now = now_ms();
        let mut data = self.read_data()?;

        if let Some(existing) = data.handles.iter_mut().find(|handle| handle.id == id) {
            existing.provider_name = provider_state.provider_name.clone();
            existing.kind = provider_state.kind.clone();
            existing.real_base_url = provider_state.original_base_url.clone();
            existing.route_state = provider_state;
            existing.last_seen_at_ms = now;
            existing.last_used_at_ms = now;
            let handle = existing.clone();
            self.write_data(&data)?;
            return Ok(handle);
        }

        let handle = ReasonixSourceHandle {
            id,
            provider_name: provider_state.provider_name.clone(),
            kind: provider_state.kind.clone(),
            real_base_url: provider_state.original_base_url.clone(),
            route_state: provider_state,
            created_at_ms: now,
            last_seen_at_ms: now,
            last_used_at_ms: now,
        };
        data.handles.push(handle.clone());
        self.write_data(&data)?;
        Ok(handle)
    }

    pub fn upsert_from_state(
        &self,
        route_state: &ReasonixRouteState,
    ) -> Result<Vec<ReasonixSourceHandle>, String> {
        let mut handles = Vec::new();
        for provider in &route_state.providers {
            handles.push(self.upsert_provider_state(provider.clone())?);
        }
        Ok(handles)
    }

    fn read_data(&self) -> Result<ReasonixSourceRegistryData, String> {
        if !self.path.exists() {
            return Ok(ReasonixSourceRegistryData::default());
        }
        let content = fs::read_to_string(&self.path)
            .map_err(|e| format!("Failed to read Reasonix source registry: {}", e))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse Reasonix source registry: {}", e))
    }

    fn write_data(&self, data: &ReasonixSourceRegistryData) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create Reasonix source registry dir: {}", e))?;
        }
        let content = serde_json::to_string_pretty(data)
            .map_err(|e| format!("Failed to serialize Reasonix source registry: {}", e))?;
        fs::write(&self.path, content)
            .map_err(|e| format!("Failed to save Reasonix source registry: {}", e))?;
        Ok(())
    }
}

impl Default for ReasonixSourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// === Config Manager ===

pub struct ReasonixConfigManager {
    config_path: PathBuf,
    config_exists: bool,
}

impl ReasonixConfigManager {
    pub fn new() -> Self {
        let (config_path, config_exists) = resolve_reasonix_config_target();
        Self {
            config_path,
            config_exists,
        }
    }

    #[cfg(test)]
    fn new_for_path(config_path: PathBuf, config_exists: bool) -> Self {
        Self {
            config_path,
            config_exists,
        }
    }

    pub fn config_path(&self) -> &PathBuf {
        &self.config_path
    }

    pub fn ensure_config_exists(&self) -> Result<(), String> {
        if self.config_exists && self.config_path.exists() {
            return Ok(());
        }
        Err(format!(
            "Reasonix config file was not found. UsageMeter will not create it automatically. \
             Run `reasonix setup` first, then enable takeover. Expected path: {}",
            self.config_path.display()
        ))
    }

    /// 读取当前 config.toml 中所有指向真实上游的 provider 路由状态。
    pub fn read_live_snapshot(&self) -> Result<ReasonixRouteState, String> {
        let content = fs::read_to_string(&self.config_path)
            .map_err(|e| format!("Failed to read Reasonix config.toml: {}", e))?;
        let doc = content
            .parse::<DocumentMut>()
            .map_err(|e| format!("Failed to parse Reasonix config.toml: {}", e))?;

        let mut providers = Vec::new();
        if let Some(array) = doc.get("providers").and_then(Item::as_array_of_tables) {
            for table in array.iter() {
                let Some(name) = table.get("name").and_then(Item::as_str) else {
                    continue;
                };
                let Some(base_url) = table.get("base_url").and_then(Item::as_str) else {
                    continue;
                };
                let kind = table
                    .get("kind")
                    .and_then(Item::as_str)
                    .unwrap_or("openai")
                    .to_string();
                providers.push(ReasonixProviderRouteState {
                    provider_name: name.to_string(),
                    kind,
                    original_base_url: base_url.to_string(),
                });
            }
        }
        Ok(ReasonixRouteState { providers })
    }

    /// 将 provider 的 base_url 替换为代理地址。
    pub fn takeover_with_handles(
        &self,
        proxy_port: u16,
        handles: &[ReasonixSourceHandle],
    ) -> Result<(), String> {
        self.ensure_config_exists()?;
        if handles.is_empty() {
            return Err("No Reasonix providers with a base_url were found".to_string());
        }

        let content = fs::read_to_string(&self.config_path)
            .map_err(|e| format!("Failed to read Reasonix config.toml: {}", e))?;
        let mut doc = content
            .parse::<DocumentMut>()
            .map_err(|e| format!("Failed to parse Reasonix config.toml: {}", e))?;

        for handle in handles {
            let proxy_url =
                url_identity::prefixed_proxy_url(proxy_port, "reasonix", &handle.id, "");
            set_provider_base_url(&mut doc, &handle.provider_name, &proxy_url);
        }

        self.write_config_doc(&doc)
    }

    /// 从已保存的 source handles 恢复 provider 原始 base_url。
    pub fn restore_from_sources(&self, source_ids: &[String]) -> Result<usize, String> {
        if source_ids.is_empty() || !self.config_path.exists() {
            return Ok(0);
        }

        let registry = ReasonixSourceRegistry::new();
        let handles: Vec<ReasonixSourceHandle> = source_ids
            .iter()
            .filter_map(|source_id| registry.get(source_id))
            .collect();
        if handles.is_empty() {
            return Err(
                "Reasonix config.toml contains proxy URLs, but no matching source handles \
                 were found in the registry. The registry file may be missing or corrupted. \
                 Restore the Reasonix config.toml manually or re-enable takeover."
                    .to_string(),
            );
        }

        let content = fs::read_to_string(&self.config_path)
            .map_err(|e| format!("Failed to read Reasonix config.toml: {}", e))?;
        let mut doc = content
            .parse::<DocumentMut>()
            .map_err(|e| format!("Failed to parse Reasonix config.toml: {}", e))?;

        let current_snapshot = self.read_live_snapshot()?;
        let current_provider_urls = current_snapshot
            .providers
            .into_iter()
            .map(|provider| (provider.provider_name, provider.original_base_url))
            .collect::<std::collections::HashMap<_, _>>();

        let mut restored = 0usize;
        for handle in &handles {
            let Some(current_base_url) = current_provider_urls.get(&handle.provider_name) else {
                continue;
            };
            let should_restore = Self::is_usagemeter_proxy_url(current_base_url)
                && Self::extract_source_id_from_proxy_url(current_base_url)
                    .map(|source_id| source_id == handle.id)
                    .unwrap_or(false);
            if !should_restore {
                continue;
            }
            set_provider_base_url(
                &mut doc,
                &handle.provider_name,
                &handle.route_state.original_base_url,
            );
            restored += 1;
        }

        self.write_config_doc(&doc)?;
        Ok(restored)
    }

    /// 当前配置是否有任意 provider 指向本地代理。
    pub fn is_takeover_active(&self, proxy_port: u16) -> Result<bool, String> {
        let snapshot = self.read_live_snapshot()?;
        Ok(snapshot.providers.iter().any(|provider| {
            Self::is_usagemeter_proxy_url_for_port(&provider.original_base_url, proxy_port)
        }))
    }

    /// 当前配置中所有指向代理的 source id。
    pub fn active_source_ids(&self) -> Vec<String> {
        let snapshot = self.read_live_snapshot().unwrap_or_default();
        snapshot
            .providers
            .into_iter()
            .filter_map(|provider| {
                Self::extract_source_id_from_proxy_url(&provider.original_base_url)
            })
            .collect()
    }

    pub fn active_source_id(&self) -> Option<String> {
        self.active_source_ids().into_iter().next()
    }

    pub fn is_usagemeter_proxy_url(base_url: &str) -> bool {
        url_identity::is_usagemeter_proxy_url(base_url, &["reasonix"])
    }

    pub fn is_usagemeter_proxy_url_for_port(base_url: &str, proxy_port: u16) -> bool {
        url_identity::is_usagemeter_proxy_url_for_port(base_url, proxy_port, &["reasonix"])
    }

    pub fn extract_source_id_from_proxy_url(base_url: &str) -> Option<String> {
        url_identity::extract_source_id_from_proxy_url(base_url, &["reasonix"])
    }

    fn write_config_doc(&self, doc: &DocumentMut) -> Result<(), String> {
        if !self.config_path.exists() {
            return Err(format!(
                "Reasonix config file was not found. UsageMeter will not create it automatically. \
                 Expected path: {}",
                self.config_path.display()
            ));
        }
        fs::write(&self.config_path, doc.to_string())
            .map_err(|e| format!("Failed to save Reasonix config.toml: {}", e))
    }
}

impl Default for ReasonixConfigManager {
    fn default() -> Self {
        Self::new()
    }
}

// === 辅助函数 ===

/// 定位全局 config.toml：`<config_dir>/reasonix/config.toml`，
/// 并兼容 Linux 风格 `~/.config/reasonix/config.toml`。
fn resolve_reasonix_config_target() -> (PathBuf, bool) {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(config_dir) = dirs::config_dir() {
        candidates.push(config_dir.join("reasonix").join("config.toml"));
    }
    if let Some(home) = dirs::home_dir() {
        let xdg = home.join(".config").join("reasonix").join("config.toml");
        if !candidates.contains(&xdg) {
            candidates.push(xdg);
        }
    }

    for candidate in &candidates {
        if candidate.exists() {
            return (candidate.clone(), true);
        }
    }
    let fallback = candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from("reasonix/config.toml"));
    (fallback, false)
}

/// 在 `[[providers]]` 数组表中按 name 定位 provider 并设置 base_url。
fn set_provider_base_url(doc: &mut DocumentMut, provider_name: &str, base_url: &str) {
    let Some(array) = doc
        .get_mut("providers")
        .and_then(Item::as_array_of_tables_mut)
    else {
        return;
    };
    for table in array.iter_mut() {
        if table.get("name").and_then(Item::as_str) == Some(provider_name) {
            table["base_url"] = Item::Value(Value::from(base_url));
        }
    }
}

fn compute_handle_id(provider_name: &str, real_base_url: &str) -> Result<String, String> {
    let mut hasher = Sha256::new();
    hasher.update(provider_name.as_bytes());
    hasher.update(b"\n");
    hasher.update(real_base_url.as_bytes());
    let hash = hasher.finalize();
    Ok(format!(
        "rx_{}",
        u64::from_be_bytes(hash[..8].try_into().unwrap())
    ))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CONFIG: &str = r#"
default_model = "deepseek-pro/deepseek-v4-pro"

[[providers]]
name        = "deepseek-pro"
kind        = "openai"
base_url    = "https://api.deepseek.com"
api_key_env = "DEEPSEEK_API_KEY"

[[providers]]
name        = "mimo-pro"
kind        = "openai"
base_url    = "https://token-plan-cn.xiaomimimo.com/v1"
api_key_env = "MIMO_API_KEY"
no_proxy    = true

[[providers]]
name        = "claude"
kind        = "anthropic"
base_url    = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
"#;

    #[test]
    fn read_live_snapshot_extracts_all_providers() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, SAMPLE_CONFIG).unwrap();

        let manager = ReasonixConfigManager::new_for_path(config_path, true);
        let snapshot = manager.read_live_snapshot().unwrap();

        assert_eq!(snapshot.providers.len(), 3);
        assert_eq!(snapshot.providers[0].provider_name, "deepseek-pro");
        assert_eq!(snapshot.providers[0].kind, "openai");
        assert_eq!(snapshot.providers[2].kind, "anthropic");
    }

    #[test]
    fn takeover_then_restore_round_trips_base_urls() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, SAMPLE_CONFIG).unwrap();

        let manager = ReasonixConfigManager::new_for_path(config_path.clone(), true);
        let snapshot = manager.read_live_snapshot().unwrap();

        // 构造 handles（不写真实 registry，仅用于改写）。
        let handles: Vec<ReasonixSourceHandle> = snapshot
            .providers
            .iter()
            .map(|provider| ReasonixSourceHandle {
                id: compute_handle_id(&provider.provider_name, &provider.original_base_url)
                    .unwrap(),
                provider_name: provider.provider_name.clone(),
                kind: provider.kind.clone(),
                real_base_url: provider.original_base_url.clone(),
                route_state: provider.clone(),
                created_at_ms: 0,
                last_seen_at_ms: 0,
                last_used_at_ms: 0,
            })
            .collect();

        manager.takeover_with_handles(18765, &handles).unwrap();

        let after = manager.read_live_snapshot().unwrap();
        for provider in &after.providers {
            assert!(
                ReasonixConfigManager::is_usagemeter_proxy_url_for_port(
                    &provider.original_base_url,
                    18765
                ),
                "provider {} should point at proxy: {}",
                provider.provider_name,
                provider.original_base_url
            );
        }

        // 直接用 handles 还原（绕过 registry 文件）。
        let content = fs::read_to_string(&config_path).unwrap();
        let mut doc = content.parse::<DocumentMut>().unwrap();
        for handle in &handles {
            set_provider_base_url(
                &mut doc,
                &handle.provider_name,
                &handle.route_state.original_base_url,
            );
        }
        fs::write(&config_path, doc.to_string()).unwrap();

        let restored = manager.read_live_snapshot().unwrap();
        assert_eq!(
            restored.providers[0].original_base_url,
            "https://api.deepseek.com"
        );
        assert_eq!(
            restored.providers[2].original_base_url,
            "https://api.anthropic.com"
        );
    }

    #[test]
    fn extracts_reasonix_source_id() {
        assert_eq!(
            ReasonixConfigManager::extract_source_id_from_proxy_url(
                "http://127.0.0.1:18765/reasonix/source/rx_123"
            )
            .as_deref(),
            Some("rx_123")
        );
        assert_eq!(
            ReasonixConfigManager::extract_source_id_from_proxy_url("https://api.deepseek.com"),
            None
        );
    }

    #[test]
    fn takeover_refuses_missing_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("nope").join("config.toml");
        let manager = ReasonixConfigManager::new_for_path(config_path.clone(), false);
        let handle = ReasonixSourceHandle {
            id: "rx_1".to_string(),
            provider_name: "deepseek-pro".to_string(),
            kind: "openai".to_string(),
            real_base_url: "https://api.deepseek.com".to_string(),
            route_state: ReasonixProviderRouteState {
                provider_name: "deepseek-pro".to_string(),
                kind: "openai".to_string(),
                original_base_url: "https://api.deepseek.com".to_string(),
            },
            created_at_ms: 0,
            last_seen_at_ms: 0,
            last_used_at_ms: 0,
        };
        let err = manager.takeover_with_handles(18765, &[handle]).unwrap_err();
        assert!(err.contains("Reasonix config file was not found"));
        assert!(!config_path.exists());
    }

    #[test]
    fn restore_fails_when_no_handles_found_for_active_takeover() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        // 模拟已接管状态：config.toml 包含代理 URL。
        let taken_over = format!(
            r#"[[providers]]
name        = "deepseek-pro"
kind        = "openai"
base_url    = "http://127.0.0.1:18765/reasonix/source/rx_123"
api_key_env = "DEEPSEEK_API_KEY"
"#
        );
        fs::write(&config_path, taken_over).unwrap();
        let manager = ReasonixConfigManager::new_for_path(config_path, true);

        // Registry 中不存在 rx_123 → restore 应该失败，不能静默返回 Ok(0)。
        let err = manager
            .restore_from_sources(&["rx_123".to_string()])
            .unwrap_err();
        assert!(
            err.contains("no matching source handles"),
            "expected registry-missing error, got: {err}"
        );
    }

    #[test]
    fn restore_from_sources_skips_provider_that_is_no_longer_proxy_url() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, SAMPLE_CONFIG).unwrap();

        let manager = ReasonixConfigManager::new_for_path(config_path.clone(), true);
        let snapshot = manager.read_live_snapshot().unwrap();
        let handles: Vec<ReasonixSourceHandle> = snapshot
            .providers
            .iter()
            .map(|provider| ReasonixSourceHandle {
                id: compute_handle_id(&provider.provider_name, &provider.original_base_url)
                    .unwrap(),
                provider_name: provider.provider_name.clone(),
                kind: provider.kind.clone(),
                real_base_url: provider.original_base_url.clone(),
                route_state: provider.clone(),
                created_at_ms: 0,
                last_seen_at_ms: 0,
                last_used_at_ms: 0,
            })
            .collect();

        manager.takeover_with_handles(18765, &handles).unwrap();

        let mimo_handle = handles
            .iter()
            .find(|handle| handle.provider_name == "mimo-pro")
            .unwrap();
        let mut content = fs::read_to_string(&config_path).unwrap();
        content = content.replace(
            &format!(
                "http://127.0.0.1:18765/usagemeter/reasonix/source/{}",
                mimo_handle.id
            ),
            "https://custom.example/v1",
        );
        fs::write(&config_path, content).unwrap();

        let restored = manager
            .restore_from_sources(
                &handles
                    .iter()
                    .map(|handle| handle.id.clone())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        assert_eq!(restored, 2);

        let after = manager.read_live_snapshot().unwrap();
        let deepseek = after
            .providers
            .iter()
            .find(|provider| provider.provider_name == "deepseek-pro")
            .unwrap();
        let mimo = after
            .providers
            .iter()
            .find(|provider| provider.provider_name == "mimo-pro")
            .unwrap();
        let claude = after
            .providers
            .iter()
            .find(|provider| provider.provider_name == "claude")
            .unwrap();

        assert_eq!(deepseek.original_base_url, "https://api.deepseek.com");
        assert_eq!(claude.original_base_url, "https://api.anthropic.com");
        assert_eq!(mimo.original_base_url, "https://custom.example/v1");
    }
}
