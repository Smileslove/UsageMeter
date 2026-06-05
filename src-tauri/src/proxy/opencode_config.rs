//! OpenCode 配置接管支持
//!
//! OpenCode 会按优先级合并多个配置来源，UsageMeter 在读取时尽量遵循：
//! - 全局 legacy `config.json`
//! - 全局 legacy `opencode.jsonc`
//! - 全局官方 `opencode.json`
//! - 自定义配置 `OPENCODE_CONFIG`
//! - 内联覆盖 `OPENCODE_CONFIG_CONTENT`
//!
//! UsageMeter 只接管用户已经显式配置过 `options.baseURL` 的 provider，
//! 不猜测默认 provider，也不会为未配置的 provider 自动补配置。
//!
//! 文件格式采用 JSONC（支持注释的 JSON），读取时先剥离注释再解析。
//! 写回时仅对目标 provider 的 `options.baseURL` 做原位文本 patch，
//! 尽量保留注释、缩进和其他未修改字段。

use super::url_identity;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

// === 数据类型 ===

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeProviderRouteState {
    pub provider_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_npm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub original_base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<OpenCodeConfigSourceKind>,
}

/// 接管前保存的 OpenCode provider 路由状态。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeRouteState {
    #[serde(default)]
    pub providers: Vec<OpenCodeProviderRouteState>,
}

/// 注册到 UsageMeter 的 OpenCode 来源句柄（provider 级）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeSourceHandle {
    pub id: String,
    pub provider_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_npm: Option<String>,
    pub real_base_url: String,
    pub route_state: OpenCodeProviderRouteState,
    pub created_at_ms: i64,
    pub last_seen_at_ms: i64,
    pub last_used_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct OpenCodeSourceRegistryData {
    #[serde(default)]
    handles: Vec<OpenCodeSourceHandle>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeActiveRoute {
    pub provider_id: String,
    pub source_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenCodeConfigSourceKind {
    GlobalLegacyConfigJson,
    GlobalJsonc,
    GlobalJson,
    CustomFile,
    InlineContent,
}

#[derive(Debug, Clone)]
struct OpenCodeProviderProvenance {
    source_kind: OpenCodeConfigSourceKind,
    source_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct OpenCodeEffectiveProviderRoute {
    route_state: OpenCodeProviderRouteState,
}

#[derive(Debug, Clone)]
struct OpenCodeEffectiveConfig {
    providers: Vec<OpenCodeEffectiveProviderRoute>,
}

#[derive(Debug, Clone)]
struct OpenCodeLoadedLayer {
    kind: OpenCodeConfigSourceKind,
    path: Option<PathBuf>,
    json: serde_json::Value,
}

// === Source Registry ===

pub struct OpenCodeSourceRegistry {
    path: PathBuf,
}

impl OpenCodeSourceRegistry {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            path: home
                .join(".usagemeter")
                .join("opencode_proxy_source_handles.json"),
        }
    }

    pub fn get(&self, id: &str) -> Option<OpenCodeSourceHandle> {
        self.read_data()
            .ok()?
            .handles
            .into_iter()
            .find(|handle| handle.id == id)
    }

    pub fn list_handles(&self) -> Vec<OpenCodeSourceHandle> {
        self.read_data()
            .map(|data| data.handles)
            .unwrap_or_default()
    }

    pub fn latest_for_provider(&self, provider_id: &str) -> Option<OpenCodeSourceHandle> {
        self.list_handles()
            .into_iter()
            .filter(|handle| handle.provider_id == provider_id)
            .max_by_key(|handle| handle.last_used_at_ms.max(handle.last_seen_at_ms))
    }

    pub fn touch_used(&self, id: &str) -> Result<(), String> {
        let mut data = self.read_data()?;
        let now = now_ms();
        if let Some(handle) = data.handles.iter_mut().find(|h| h.id == id) {
            handle.last_used_at_ms = now;
            self.write_data(&data)?;
        }
        Ok(())
    }

    pub fn upsert_provider_state(
        &self,
        provider_state: OpenCodeProviderRouteState,
    ) -> Result<OpenCodeSourceHandle, String> {
        if OpenCodeConfigManager::is_usagemeter_proxy_url(&provider_state.original_base_url) {
            return Err(
                "Refusing to register UsageMeter proxy URL as an OpenCode upstream".to_string(),
            );
        }

        let id = compute_handle_id(
            &provider_state.provider_id,
            &provider_state.original_base_url,
        )?;
        let now = now_ms();
        let mut data = self.read_data()?;

        if let Some(existing) = data.handles.iter_mut().find(|h| h.id == id) {
            existing.provider_id = provider_state.provider_id.clone();
            existing.provider_npm = provider_state.provider_npm.clone();
            existing.real_base_url = provider_state.original_base_url.clone();
            existing.route_state = provider_state;
            existing.last_seen_at_ms = now;
            existing.last_used_at_ms = now;
            let handle = existing.clone();
            self.write_data(&data)?;
            return Ok(handle);
        }

        let handle = OpenCodeSourceHandle {
            id,
            provider_id: provider_state.provider_id.clone(),
            provider_npm: provider_state.provider_npm.clone(),
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
        route_state: &OpenCodeRouteState,
    ) -> Result<Vec<OpenCodeSourceHandle>, String> {
        let mut handles = Vec::new();
        for provider in &route_state.providers {
            handles.push(self.upsert_provider_state(provider.clone())?);
        }
        Ok(handles)
    }

    fn read_data(&self) -> Result<OpenCodeSourceRegistryData, String> {
        if !self.path.exists() {
            return Ok(OpenCodeSourceRegistryData::default());
        }
        let content = fs::read_to_string(&self.path)
            .map_err(|e| format!("Failed to read OpenCode source registry: {}", e))?;
        parse_opencode_source_registry_data(&content)
    }

    fn write_data(&self, data: &OpenCodeSourceRegistryData) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create OpenCode source registry dir: {}", e))?;
        }
        let content = serde_json::to_string_pretty(data)
            .map_err(|e| format!("Failed to serialize OpenCode source registry: {}", e))?;
        fs::write(&self.path, content)
            .map_err(|e| format!("Failed to save OpenCode source registry: {}", e))?;
        Ok(())
    }
}

fn parse_opencode_source_registry_data(
    content: &str,
) -> Result<OpenCodeSourceRegistryData, String> {
    let json: Value = serde_json::from_str(content)
        .map_err(|e| format!("Failed to parse OpenCode source registry: {}", e))?;
    let handles = json
        .get("handles")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(parse_opencode_source_handle)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(OpenCodeSourceRegistryData { handles })
}

fn parse_opencode_source_handle(value: &Value) -> Option<OpenCodeSourceHandle> {
    let object = value.as_object()?;
    let id = object.get("id")?.as_str()?.trim().to_string();
    if id.is_empty() {
        return None;
    }

    let real_base_url = object
        .get("realBaseUrl")
        .or_else(|| object.get("real_base_url"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();

    let route_state_value = object
        .get("routeState")
        .or_else(|| object.get("route_state"));
    let route_provider_id = route_state_value
        .and_then(|route| route.get("providerId").or_else(|| route.get("provider_id")))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let provider_id = object
        .get("providerId")
        .or_else(|| object.get("provider_id"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or(route_provider_id)
        .or_else(|| infer_provider_id_from_base_url(&real_base_url))?;

    let provider_npm = object
        .get("providerNpm")
        .or_else(|| object.get("provider_npm"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            route_state_value
                .and_then(|route| {
                    route
                        .get("providerNpm")
                        .or_else(|| route.get("provider_npm"))
                })
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        });
    let display_name = route_state_value
        .and_then(|route| {
            route
                .get("displayName")
                .or_else(|| route.get("display_name"))
        })
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let original_base_url = route_state_value
        .and_then(|route| {
            route
                .get("originalBaseUrl")
                .or_else(|| route.get("original_base_url"))
        })
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| real_base_url.clone());

    Some(OpenCodeSourceHandle {
        id,
        provider_id: provider_id.clone(),
        provider_npm: provider_npm.clone(),
        real_base_url: real_base_url.clone(),
        route_state: OpenCodeProviderRouteState {
            provider_id,
            provider_npm,
            display_name,
            original_base_url,
            source_path: route_state_value
                .and_then(|route| route.get("sourcePath").or_else(|| route.get("source_path")))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            source_kind: route_state_value
                .and_then(|route| route.get("sourceKind").or_else(|| route.get("source_kind")))
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok()),
        },
        created_at_ms: object
            .get("createdAtMs")
            .or_else(|| object.get("created_at_ms"))
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        last_seen_at_ms: object
            .get("lastSeenAtMs")
            .or_else(|| object.get("last_seen_at_ms"))
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        last_used_at_ms: object
            .get("lastUsedAtMs")
            .or_else(|| object.get("last_used_at_ms"))
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
    })
}

fn infer_provider_id_from_base_url(base_url: &str) -> Option<String> {
    let lower = base_url.trim().to_ascii_lowercase();
    if lower.contains("anthropic.com") {
        return Some("anthropic".to_string());
    }
    if lower.contains("api.openai.com") {
        return Some("openai".to_string());
    }
    None
}

impl Default for OpenCodeSourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// === Config Manager ===

pub struct OpenCodeConfigManager {
    config_path: PathBuf,
    config_exists: bool,
}

impl OpenCodeConfigManager {
    pub fn new() -> Self {
        let (config_path, config_exists) = resolve_opencode_write_target();
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
            "OpenCode config file was not found. UsageMeter will not create it automatically. \
             Create an OpenCode config first, then enable takeover. Expected path: {}",
            self.config_path.display()
        ))
    }

    /// 读取当前 OpenCode 已显式配置 baseURL 的 provider 路由状态。
    pub fn read_live_snapshot(&self) -> Result<OpenCodeRouteState, String> {
        let mut providers = self
            .read_effective_config()?
            .providers
            .into_iter()
            .map(|provider| provider.route_state)
            .collect::<Vec<_>>();
        providers.sort_by(|a, b| a.provider_id.cmp(&b.provider_id));

        Ok(OpenCodeRouteState { providers })
    }

    fn read_effective_config(&self) -> Result<OpenCodeEffectiveConfig, String> {
        let layers = load_opencode_config_layers()?;
        Self::build_effective_config_from_layers(&layers)
    }

    #[cfg(test)]
    fn read_effective_config_for_test_path(&self) -> Result<OpenCodeEffectiveConfig, String> {
        let content = fs::read_to_string(&self.config_path).map_err(|e| {
            format!(
                "Failed to read OpenCode config {}: {}",
                self.config_path.display(),
                e
            )
        })?;
        let json = parse_jsonc(&content)?;
        let kind = match self.config_path.extension().and_then(|ext| ext.to_str()) {
            Some("jsonc") => OpenCodeConfigSourceKind::GlobalJsonc,
            _ => OpenCodeConfigSourceKind::GlobalJson,
        };
        Self::build_effective_config_from_layers(&[OpenCodeLoadedLayer {
            kind,
            path: Some(self.config_path.clone()),
            safe_for_takeover: true,
            json,
        }])
    }

    fn build_effective_config_from_layers(
        layers: &[OpenCodeLoadedLayer],
    ) -> Result<OpenCodeEffectiveConfig, String> {
        let mut merged = serde_json::json!({});
        let mut provider_origins =
            std::collections::HashMap::<String, OpenCodeProviderProvenance>::new();
        let mut loaded_any = false;

        for layer in layers {
            merge_json_values(&mut merged, layer.json.clone());
            loaded_any = true;

            if let Some(provider_map) = layer
                .json
                .pointer("/provider")
                .and_then(|value| value.as_object())
            {
                for (provider_id, provider) in provider_map {
                    let has_base_url = provider
                        .get("options")
                        .and_then(|options| options.as_object())
                        .and_then(|options| options.get("baseURL"))
                        .and_then(|base_url| base_url.as_str())
                        .is_some();
                    if has_base_url {
                        provider_origins.insert(
                            provider_id.to_string(),
                            OpenCodeProviderProvenance {
                                source_kind: layer.kind,
                                source_path: layer.path.clone(),
                            },
                        );
                    }
                }
            }
        }

        if !loaded_any {
            return Ok(OpenCodeEffectiveConfig {
                providers: Vec::new(),
            });
        }

        let disabled_providers = read_disabled_provider_ids(&merged);
        let mut providers = merged
            .pointer("/provider")
            .and_then(|value| value.as_object())
            .map(|provider_map| {
                provider_map
                    .iter()
                    .filter_map(|(provider_id, provider)| {
                        if disabled_providers.contains(provider_id) {
                            return None;
                        }
                        let provider = provider.as_object()?;
                        let base_url = provider
                            .get("options")
                            .and_then(|options| options.as_object())
                            .and_then(|options| options.get("baseURL"))
                            .and_then(|base_url| base_url.as_str())?;
                        let provider_npm = provider
                            .get("npm")
                            .and_then(|value| value.as_str())
                            .map(str::to_string);
                        let display_name = provider
                            .get("name")
                            .or_else(|| provider.get("displayName"))
                            .and_then(|value| value.as_str())
                            .map(str::to_string);
                        let provenance = provider_origins.get(provider_id)?.clone();
                        // Keep unsupported effective routes visible to callers. When the
                        // winning layer is not a safe writable file source, `source_path`
                        // remains `None` and later takeover attempts are rejected explicitly.
                        Some(OpenCodeEffectiveProviderRoute {
                            route_state: OpenCodeProviderRouteState {
                                provider_id: provider_id.to_string(),
                                provider_npm,
                                display_name,
                                original_base_url: base_url.to_string(),
                                source_path: provenance
                                    .source_path
                                    .as_ref()
                                    .map(|path| path.display().to_string()),
                                source_kind: Some(provenance.source_kind),
                            },
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        providers.sort_by(|a, b| a.route_state.provider_id.cmp(&b.route_state.provider_id));

        Ok(OpenCodeEffectiveConfig { providers })
    }

    /// 将已显式配置 baseURL 的 provider 替换为代理地址。
    pub fn takeover_with_handles(
        &self,
        proxy_port: u16,
        handles: &[OpenCodeSourceHandle],
    ) -> Result<(), String> {
        if handles.is_empty() {
            return Err("No OpenCode providers with explicit baseURL were found".to_string());
        }

        let mut grouped = std::collections::BTreeMap::<PathBuf, Vec<&OpenCodeSourceHandle>>::new();
        for handle in handles {
            let Some(path) = handle.route_state.source_path.as_ref().map(PathBuf::from) else {
                return Err(format!(
                    "OpenCode provider {} has no writable source path; takeover is not supported for this source",
                    handle.provider_id
                ));
            };
            grouped.entry(path).or_default().push(handle);
        }

        for (path, file_handles) in grouped {
            if !path.exists() {
                return Err(format!(
                    "OpenCode config file was not found. UsageMeter will not create it automatically. \
                     Expected path: {}",
                    path.display()
                ));
            }
            let mut content = fs::read_to_string(&path)
                .map_err(|e| format!("Failed to read OpenCode config {}: {}", path.display(), e))?;
            for handle in file_handles {
                let proxy_url =
                    url_identity::prefixed_proxy_url(proxy_port, "opencode", &handle.id, "");
                content = patch_provider_base_url(
                    &content,
                    &handle.provider_id,
                    &handle.route_state.original_base_url,
                    &proxy_url,
                )?;
            }
            fs::write(&path, content)
                .map_err(|e| format!("Failed to save OpenCode config {}: {}", path.display(), e))?;
        }

        Ok(())
    }

    /// 从已保存的 source handles 恢复原始 baseURL。
    pub fn restore_from_sources(&self, source_ids: &[String]) -> Result<usize, String> {
        if source_ids.is_empty() {
            return Ok(0);
        }

        let registry = OpenCodeSourceRegistry::new();
        let handles: Vec<OpenCodeSourceHandle> = source_ids
            .iter()
            .filter_map(|source_id| registry.get(source_id))
            .collect();
        if handles.is_empty() {
            return Err(
                "OpenCode config contains proxy URLs, but no matching source handles \
                 were found in the registry. The registry file may be missing or corrupted. \
                 Restore the OpenCode config manually or re-enable takeover."
                    .to_string(),
            );
        }
        let mut grouped = std::collections::BTreeMap::<PathBuf, Vec<&OpenCodeSourceHandle>>::new();
        for handle in &handles {
            let Some(path) = handle.route_state.source_path.as_ref().map(PathBuf::from) else {
                continue;
            };
            if path.exists() {
                grouped.entry(path).or_default().push(handle);
            }
        }

        let mut restored = 0usize;
        for (path, file_handles) in grouped {
            let mut content = fs::read_to_string(&path)
                .map_err(|e| format!("Failed to read OpenCode config {}: {}", path.display(), e))?;
            for handle in file_handles {
                if let Some(current_base_url) =
                    provider_base_url_value(&content, &handle.provider_id)
                {
                    let should_restore = provider_base_url_is_proxy_for_source(
                        &content,
                        &handle.provider_id,
                        &handle.id,
                    ) || provider_base_url_matches(
                        &content,
                        &handle.provider_id,
                        &current_base_url,
                    )
                        && Self::extract_provider_and_source_from_proxy_url(&current_base_url)
                            .map(|(_, source_id)| source_id == handle.id)
                            .unwrap_or(false);
                    if should_restore {
                        content = patch_provider_base_url(
                            &content,
                            &handle.provider_id,
                            &current_base_url,
                            &handle.route_state.original_base_url,
                        )?;
                        restored += 1;
                    }
                }
            }
            fs::write(&path, content)
                .map_err(|e| format!("Failed to save OpenCode config {}: {}", path.display(), e))?;
        }
        Ok(restored)
    }

    /// 检查当前 OpenCode 是否有任意 provider 指向本地代理。
    pub fn is_takeover_active(&self, proxy_port: u16) -> Result<bool, String> {
        let snapshot = self.read_live_snapshot()?;
        Ok(snapshot.providers.iter().any(|provider| {
            Self::is_usagemeter_proxy_url_for_port(&provider.original_base_url, proxy_port)
        }))
    }

    /// 当前配置中所有指向代理的 provider/source 对。
    pub fn active_routes(&self) -> Vec<OpenCodeActiveRoute> {
        let snapshot = self.read_live_snapshot().unwrap_or_default();
        snapshot
            .providers
            .into_iter()
            .filter_map(|provider| {
                Self::extract_provider_and_source_from_proxy_url(&provider.original_base_url).map(
                    |(provider_id, source_id)| OpenCodeActiveRoute {
                        provider_id: provider_id.unwrap_or_else(|| provider.provider_id.clone()),
                        source_id,
                    },
                )
            })
            .collect()
    }

    /// 向后兼容：返回第一个活动 source id。
    pub fn active_source_id(&self) -> Option<String> {
        self.active_routes()
            .into_iter()
            .map(|route| route.source_id)
            .next()
    }

    pub fn is_usagemeter_proxy_url(base_url: &str) -> bool {
        url_identity::is_usagemeter_proxy_url(base_url, &["opencode"])
    }

    pub fn is_usagemeter_proxy_url_for_port(base_url: &str, proxy_port: u16) -> bool {
        url_identity::is_usagemeter_proxy_url_for_port(base_url, proxy_port, &["opencode"])
    }

    #[allow(dead_code)]
    pub fn extract_source_id_from_proxy_url(base_url: &str) -> Option<String> {
        Self::extract_provider_and_source_from_proxy_url(base_url).map(|(_, source_id)| source_id)
    }

    pub fn extract_provider_and_source_from_proxy_url(
        base_url: &str,
    ) -> Option<(Option<String>, String)> {
        if !Self::is_usagemeter_proxy_url(base_url) {
            return None;
        }

        let url = reqwest::Url::parse(base_url).ok()?;
        let path = url.path().trim_end_matches('/');
        let segments: Vec<&str> = path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .collect();
        let offset = if segments.first().copied() == Some("usagemeter") {
            1
        } else {
            0
        };
        if segments.get(offset).copied() != Some("opencode") {
            return None;
        }

        if segments.len() >= offset + 5
            && segments[offset + 1] == "provider"
            && segments[offset + 3] == "source"
        {
            let provider_id = segments[offset + 2].trim();
            let source_id = segments[offset + 4].trim();
            if provider_id.is_empty() || source_id.is_empty() {
                return None;
            }
            return Some((Some(provider_id.to_string()), source_id.to_string()));
        }

        if segments.len() >= offset + 3 && segments[offset + 1] == "source" {
            let source_id = segments[offset + 2].trim();
            if source_id.is_empty() {
                return None;
            }
            return Some((None, source_id.to_string()));
        }

        None
    }

    fn is_local_opencode_proxy_url(url: &reqwest::Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        if host != "127.0.0.1" && host != "localhost" {
            return false;
        }
        let path = url.path().trim_end_matches('/');
        path == "/opencode"
            || path.starts_with("/opencode/source/")
            || path.starts_with("/opencode/provider/")
    }
}

impl Default for OpenCodeConfigManager {
    fn default() -> Self {
        Self::new()
    }
}

// === 辅助函数 ===

/// 按照 OpenCode 自己的搜索顺序定位配置文件。
fn resolve_opencode_write_target() -> (PathBuf, bool) {
    if let Ok(explicit) = std::env::var("OPENCODE_CONFIG") {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed);
            return (path.clone(), path.exists());
        }
    }

    let config_dir = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config")
        })
        .join("opencode");

    for name in &["opencode.json", "opencode.jsonc", "config.json"] {
        let p = config_dir.join(name);
        if p.exists() {
            return (p, true);
        }
    }

    (config_dir.join("opencode.json"), false)
}

fn load_opencode_config_layers() -> Result<Vec<OpenCodeLoadedLayer>, String> {
    let mut layers = Vec::new();
    for source in collect_opencode_config_sources() {
        let Some(content) = source.read_content()? else {
            continue;
        };
        let json = parse_jsonc(&content)?;
        layers.push(OpenCodeLoadedLayer {
            kind: source.kind,
            path: source.path.clone(),
            json,
        });
    }
    Ok(layers)
}

fn collect_opencode_config_sources() -> Vec<OpenCodeConfigSource> {
    let config_dir = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config")
        })
        .join("opencode");

    let mut sources = vec![
        OpenCodeConfigSource::file(
            config_dir.join("config.json"),
            OpenCodeConfigSourceKind::GlobalLegacyConfigJson,
        ),
        OpenCodeConfigSource::file(
            config_dir.join("opencode.jsonc"),
            OpenCodeConfigSourceKind::GlobalJsonc,
        ),
        OpenCodeConfigSource::file(
            config_dir.join("opencode.json"),
            OpenCodeConfigSourceKind::GlobalJson,
        ),
    ];

    if let Ok(explicit) = std::env::var("OPENCODE_CONFIG") {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed);
            if !sources
                .iter()
                .any(|source| source.path.as_ref() == Some(&path))
            {
                sources.push(OpenCodeConfigSource::file(
                    path,
                    OpenCodeConfigSourceKind::CustomFile,
                ));
            }
        }
    }

    if let Ok(content) = std::env::var("OPENCODE_CONFIG_CONTENT") {
        if !content.trim().is_empty() {
            sources.push(OpenCodeConfigSource::inline(
                content,
                OpenCodeConfigSourceKind::InlineContent,
            ));
        }
    }

    sources
}

#[derive(Debug, Clone)]
struct OpenCodeConfigSource {
    kind: OpenCodeConfigSourceKind,
    path: Option<PathBuf>,
    inline_content: Option<String>,
}

impl OpenCodeConfigSource {
    fn file(path: PathBuf, kind: OpenCodeConfigSourceKind) -> Self {
        Self {
            kind,
            path: Some(path),
            inline_content: None,
        }
    }

    fn inline(content: String, kind: OpenCodeConfigSourceKind) -> Self {
        Self {
            kind,
            path: None,
            inline_content: Some(content),
        }
    }

    fn read_content(&self) -> Result<Option<String>, String> {
        if let Some(content) = &self.inline_content {
            return Ok(Some(content.clone()));
        }
        let Some(path) = &self.path else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }
        fs::read_to_string(path)
            .map(Some)
            .map_err(|e| format!("Failed to read OpenCode config: {}", e))
    }
}

fn merge_json_values(base: &mut serde_json::Value, overlay: serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(overlay_map)) => {
            for (key, overlay_value) in overlay_map {
                match base_map.get_mut(&key) {
                    Some(base_value) => merge_json_values(base_value, overlay_value),
                    None => {
                        base_map.insert(key, overlay_value);
                    }
                }
            }
        }
        (base_value, overlay_value) => {
            *base_value = overlay_value;
        }
    }
}

fn read_disabled_provider_ids(json: &serde_json::Value) -> std::collections::HashSet<String> {
    json.pointer("/disabled_providers")
        .or_else(|| json.pointer("/disabledProviders"))
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn strip_jsonc_comments(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let chars: Vec<char> = content.chars().collect();
    let mut i = 0;
    let mut in_string = false;
    let mut escape = false;

    while i < chars.len() {
        let ch = chars[i];

        if escape {
            result.push(ch);
            escape = false;
            i += 1;
            continue;
        }

        if in_string {
            if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            result.push(ch);
            i += 1;
            continue;
        }

        if ch == '"' {
            in_string = true;
            result.push(ch);
            i += 1;
            continue;
        }

        if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() {
                if chars[i] == '*' && chars[i + 1] == '/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            continue;
        }

        result.push(ch);
        i += 1;
    }

    result
}

fn parse_jsonc(content: &str) -> Result<serde_json::Value, String> {
    let stripped = strip_jsonc_comments(content);
    serde_json::from_str(&stripped)
        .or_else(|_| {
            let cleaned = remove_trailing_commas(&stripped);
            serde_json::from_str(&cleaned)
        })
        .map_err(|e| format!("Failed to parse OpenCode config (JSONC): {}", e))
}

fn remove_trailing_commas(content: &str) -> String {
    let mut result = content.to_string();
    loop {
        let mut changed = false;
        let mut new_result = String::with_capacity(result.len());
        let chars: Vec<char> = result.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == ',' {
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                    changed = true;
                    i += 1;
                    continue;
                }
            }
            new_result.push(chars[i]);
            i += 1;
        }
        result = new_result;
        if !changed {
            break;
        }
    }
    result
}

#[cfg(test)]
fn set_provider_base_url(json: &mut serde_json::Value, provider_id: &str, base_url: &str) {
    let Some(root) = json.as_object_mut() else {
        *json = serde_json::Value::Object(serde_json::Map::new());
        return set_provider_base_url(json, provider_id, base_url);
    };

    if !root.contains_key("provider") {
        root.insert(
            "provider".to_string(),
            serde_json::Value::Object(serde_json::Map::new()),
        );
    }
    let provider_root = root
        .get_mut("provider")
        .and_then(|value| value.as_object_mut())
        .expect("provider object");

    if !provider_root.contains_key(provider_id) {
        provider_root.insert(
            provider_id.to_string(),
            serde_json::Value::Object(serde_json::Map::new()),
        );
    }
    let provider = provider_root
        .get_mut(provider_id)
        .and_then(|value| value.as_object_mut())
        .expect("provider entry object");

    if !provider.contains_key("options") {
        provider.insert(
            "options".to_string(),
            serde_json::Value::Object(serde_json::Map::new()),
        );
    }
    let options = provider
        .get_mut("options")
        .and_then(|value| value.as_object_mut())
        .expect("provider options object");

    options.insert(
        "baseURL".to_string(),
        serde_json::Value::String(base_url.to_string()),
    );
}

fn provider_base_url_value(content: &str, provider_id: &str) -> Option<String> {
    let range = find_provider_base_url_range(content, provider_id).ok()?;
    let raw = &content[range.start..range.end];
    serde_json::from_str::<String>(raw).ok()
}

fn provider_base_url_matches(content: &str, provider_id: &str, expected: &str) -> bool {
    provider_base_url_value(content, provider_id)
        .map(|value| value == expected)
        .unwrap_or(false)
}

fn provider_base_url_is_proxy_for_source(
    content: &str,
    provider_id: &str,
    source_id: &str,
) -> bool {
    provider_base_url_value(content, provider_id)
        .and_then(|value| {
            OpenCodeConfigManager::extract_provider_and_source_from_proxy_url(&value)
                .map(|(_, candidate_source_id)| candidate_source_id == source_id)
        })
        .unwrap_or(false)
}

fn patch_provider_base_url(
    content: &str,
    provider_id: &str,
    expected_old_value: &str,
    new_value: &str,
) -> Result<String, String> {
    let range = find_provider_base_url_range(content, provider_id)?;
    let current_value =
        serde_json::from_str::<String>(&content[range.start..range.end]).map_err(|e| {
            format!(
                "Failed to parse OpenCode provider {} baseURL: {}",
                provider_id, e
            )
        })?;
    if current_value != expected_old_value {
        return Err(format!(
            "OpenCode provider {} baseURL changed unexpectedly; expected {}, found {}",
            provider_id, expected_old_value, current_value
        ));
    }

    let mut patched = String::with_capacity(
        content.len() + new_value.len().saturating_sub(expected_old_value.len()),
    );
    patched.push_str(&content[..range.start]);
    patched.push_str(
        &serde_json::to_string(new_value)
            .map_err(|e| format!("Failed to encode baseURL: {}", e))?,
    );
    patched.push_str(&content[range.end..]);
    Ok(patched)
}

#[derive(Debug, Clone, Copy)]
struct TextRange {
    start: usize,
    end: usize,
}

fn find_provider_base_url_range(content: &str, provider_id: &str) -> Result<TextRange, String> {
    let root_start = skip_ws_and_comments(content, 0);
    let provider_object = find_object_member_value_range(content, root_start, "provider")
        .ok_or_else(|| "OpenCode config does not contain provider object".to_string())?;
    let provider_entry =
        find_object_member_value_range(content, provider_object.start, provider_id)
            .ok_or_else(|| format!("OpenCode config does not contain provider {}", provider_id))?;
    let options_object = find_object_member_value_range(content, provider_entry.start, "options")
        .ok_or_else(|| {
        format!(
            "OpenCode provider {} does not contain options object",
            provider_id
        )
    })?;
    find_object_member_value_range(content, options_object.start, "baseURL").ok_or_else(|| {
        format!(
            "OpenCode provider {} does not contain options.baseURL",
            provider_id
        )
    })
}

fn find_object_member_value_range(
    content: &str,
    object_start: usize,
    target_key: &str,
) -> Option<TextRange> {
    let mut idx = skip_ws_and_comments(content, object_start);
    if content.as_bytes().get(idx).copied()? != b'{' {
        return None;
    }
    idx += 1;

    loop {
        idx = skip_ws_and_comments(content, idx);
        match content.as_bytes().get(idx).copied()? {
            b'}' => return None,
            b'"' => {}
            _ => return None,
        }

        let key_end = skip_json_string(content, idx).ok()?;
        let key = serde_json::from_str::<String>(&content[idx..key_end]).ok()?;
        idx = skip_ws_and_comments(content, key_end);
        if content.as_bytes().get(idx).copied()? != b':' {
            return None;
        }
        idx += 1;
        idx = skip_ws_and_comments(content, idx);
        let value_start = idx;
        let value_end = skip_json_value(content, value_start).ok()?;
        if key == target_key {
            return Some(TextRange {
                start: value_start,
                end: value_end,
            });
        }
        idx = skip_ws_and_comments(content, value_end);
        match content.as_bytes().get(idx).copied()? {
            b',' => idx += 1,
            b'}' => return None,
            _ => return None,
        }
    }
}

fn skip_json_value(content: &str, start: usize) -> Result<usize, String> {
    let idx = skip_ws_and_comments(content, start);
    let Some(byte) = content.as_bytes().get(idx).copied() else {
        return Err("Unexpected end of JSON while parsing value".to_string());
    };
    match byte {
        b'"' => skip_json_string(content, idx),
        b'{' => skip_json_object(content, idx),
        b'[' => skip_json_array(content, idx),
        _ => Ok(skip_json_primitive(content, idx)),
    }
}

fn skip_json_string(content: &str, start: usize) -> Result<usize, String> {
    let bytes = content.as_bytes();
    if bytes.get(start).copied() != Some(b'"') {
        return Err("Expected JSON string".to_string());
    }
    let mut idx = start + 1;
    let mut escaped = false;
    while let Some(byte) = bytes.get(idx).copied() {
        if escaped {
            escaped = false;
            idx += 1;
            continue;
        }
        match byte {
            b'\\' => {
                escaped = true;
                idx += 1;
            }
            b'"' => return Ok(idx + 1),
            _ => idx += 1,
        }
    }
    Err("Unterminated JSON string".to_string())
}

fn skip_json_object(content: &str, start: usize) -> Result<usize, String> {
    let bytes = content.as_bytes();
    if bytes.get(start).copied() != Some(b'{') {
        return Err("Expected JSON object".to_string());
    }
    let mut idx = start + 1;
    loop {
        idx = skip_ws_and_comments(content, idx);
        match bytes.get(idx).copied() {
            Some(b'}') => return Ok(idx + 1),
            Some(b'"') => {}
            Some(_) => return Err("Expected JSON object key".to_string()),
            None => return Err("Unterminated JSON object".to_string()),
        }
        idx = skip_json_string(content, idx)?;
        idx = skip_ws_and_comments(content, idx);
        if bytes.get(idx).copied() != Some(b':') {
            return Err("Expected ':' after JSON object key".to_string());
        }
        idx += 1;
        idx = skip_json_value(content, idx)?;
        idx = skip_ws_and_comments(content, idx);
        match bytes.get(idx).copied() {
            Some(b',') => idx += 1,
            Some(b'}') => return Ok(idx + 1),
            Some(_) => return Err("Expected ',' or '}' in JSON object".to_string()),
            None => return Err("Unterminated JSON object".to_string()),
        }
    }
}

fn skip_json_array(content: &str, start: usize) -> Result<usize, String> {
    let bytes = content.as_bytes();
    if bytes.get(start).copied() != Some(b'[') {
        return Err("Expected JSON array".to_string());
    }
    let mut idx = start + 1;
    loop {
        idx = skip_ws_and_comments(content, idx);
        match bytes.get(idx).copied() {
            Some(b']') => return Ok(idx + 1),
            Some(_) => {}
            None => return Err("Unterminated JSON array".to_string()),
        }
        idx = skip_json_value(content, idx)?;
        idx = skip_ws_and_comments(content, idx);
        match bytes.get(idx).copied() {
            Some(b',') => idx += 1,
            Some(b']') => return Ok(idx + 1),
            Some(_) => return Err("Expected ',' or ']' in JSON array".to_string()),
            None => return Err("Unterminated JSON array".to_string()),
        }
    }
}

fn skip_json_primitive(content: &str, start: usize) -> usize {
    let bytes = content.as_bytes();
    let mut idx = start;
    while let Some(byte) = bytes.get(idx).copied() {
        match byte {
            b',' | b'}' | b']' => break,
            _ if byte.is_ascii_whitespace() => break,
            _ => idx += 1,
        }
    }
    idx
}

fn skip_ws_and_comments(content: &str, start: usize) -> usize {
    let bytes = content.as_bytes();
    let mut idx = start;
    while let Some(byte) = bytes.get(idx).copied() {
        match byte {
            b' ' | b'\n' | b'\r' | b'\t' => idx += 1,
            b'/' if bytes.get(idx + 1).copied() == Some(b'/') => {
                idx += 2;
                while let Some(inner) = bytes.get(idx).copied() {
                    idx += 1;
                    if inner == b'\n' {
                        break;
                    }
                }
            }
            b'/' if bytes.get(idx + 1).copied() == Some(b'*') => {
                idx += 2;
                while idx + 1 < bytes.len() {
                    if bytes[idx] == b'*' && bytes[idx + 1] == b'/' {
                        idx += 2;
                        break;
                    }
                    idx += 1;
                }
            }
            _ => break,
        }
    }
    idx
}

fn compute_handle_id(provider_id: &str, real_base_url: &str) -> Result<String, String> {
    let mut hasher = Sha256::new();
    hasher.update(provider_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(real_base_url.as_bytes());
    let hash = hasher.finalize();
    Ok(format!(
        "oc_{}",
        u64::from_be_bytes(hash[..8].try_into().unwrap())
    ))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn with_env_var<T>(key: &str, value: Option<&Path>, f: impl FnOnce() -> T) -> T {
        let old = std::env::var_os(key);
        match value {
            Some(path) => std::env::set_var(key, path),
            None => std::env::remove_var(key),
        }
        let result = f();
        match old {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
        result
    }

    fn with_clean_opencode_env<T>(xdg_config_home: &Path, f: impl FnOnce() -> T) -> T {
        let old_config = std::env::var_os("OPENCODE_CONFIG");
        let old_inline = std::env::var_os("OPENCODE_CONFIG_CONTENT");
        std::env::remove_var("OPENCODE_CONFIG");
        std::env::remove_var("OPENCODE_CONFIG_CONTENT");
        let result = with_env_var("XDG_CONFIG_HOME", Some(xdg_config_home), f);
        match old_config {
            Some(value) => std::env::set_var("OPENCODE_CONFIG", value),
            None => std::env::remove_var("OPENCODE_CONFIG"),
        }
        match old_inline {
            Some(value) => std::env::set_var("OPENCODE_CONFIG_CONTENT", value),
            None => std::env::remove_var("OPENCODE_CONFIG_CONTENT"),
        }
        result
    }

    #[test]
    fn strips_line_comments() {
        let input = r#"{
  // This is a comment
  "key": "value"
}"#;
        let result = strip_jsonc_comments(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["key"], "value");
    }

    #[test]
    fn strips_block_comments() {
        let input = r#"{
  /* block comment */
  "key": "value"
}"#;
        let result = strip_jsonc_comments(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["key"], "value");
    }

    #[test]
    fn read_live_snapshot_extracts_only_configured_provider_base_urls() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("opencode");
        fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("opencode.json");
        fs::write(
            &config_path,
            r#"{
              "provider": {
                "anthropic": {
                  "npm": "@ai-sdk/anthropic",
                  "options": { "baseURL": "https://api.anthropic.com/v1" }
                },
                "xiaomi": {
                  "npm": "@ai-sdk/openai-compatible",
                  "options": { "baseURL": "https://api.xiaomi.example/v1" }
                },
                "openai": {
                  "npm": "@ai-sdk/openai"
                }
              }
            }"#,
        )
        .unwrap();
        let manager = OpenCodeConfigManager::new_for_path(config_path, true);
        let effective = manager.read_effective_config_for_test_path().unwrap();
        let provider_ids: Vec<&str> = effective
            .providers
            .iter()
            .map(|provider| provider.route_state.provider_id.as_str())
            .collect();

        assert_eq!(provider_ids, vec!["anthropic", "xiaomi"]);
    }

    #[test]
    fn read_live_snapshot_merges_legacy_jsonc_and_official_json() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("opencode");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("opencode.jsonc"),
            r#"{
              "provider": {
                "joybuilder-coding-plan": {
                  "npm": "@ai-sdk/openai-compatible",
                  "options": { "baseURL": "https://modelservice.jdcloud.com/coding/openai/v1" }
                }
              }
            }"#,
        )
        .unwrap();
        fs::write(
            config_dir.join("opencode.json"),
            r#"{
              "provider": {
                "xiaomi-mini": {
                  "npm": "@ai-sdk/openai-compatible",
                  "name": "Xiaomi MiMo",
                  "options": { "baseURL": "https://token-plan-cn.xiaomimimo.com/v1" }
                }
              }
            }"#,
        )
        .unwrap();

        with_clean_opencode_env(dir.path(), || {
            let manager = OpenCodeConfigManager::new();
            let snapshot = manager.read_live_snapshot().unwrap();
            let provider_ids: Vec<&str> = snapshot
                .providers
                .iter()
                .map(|provider| provider.provider_id.as_str())
                .collect();
            assert_eq!(provider_ids, vec!["joybuilder-coding-plan", "xiaomi-mini"]);
            assert_eq!(manager.config_path(), &config_dir.join("opencode.json"));
        });
    }

    #[test]
    fn read_live_snapshot_filters_disabled_providers() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("opencode");
        fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("opencode.json");
        fs::write(
            &config_path,
            r#"{
              "disabled_providers": ["joybuilder-coding-plan"],
              "provider": {
                "anthropic": {
                  "npm": "@ai-sdk/anthropic",
                  "options": { "baseURL": "https://api.anthropic.com/v1" }
                },
                "joybuilder-coding-plan": {
                  "npm": "@ai-sdk/openai-compatible",
                  "options": { "baseURL": "https://modelservice.jdcloud.com/coding/openai/v1" }
                }
              }
            }"#,
        )
        .unwrap();
        let manager = OpenCodeConfigManager::new_for_path(config_path, true);
        let effective = manager.read_effective_config_for_test_path().unwrap();
        let provider_ids: Vec<&str> = effective
            .providers
            .iter()
            .map(|provider| provider.route_state.provider_id.as_str())
            .collect();

        assert_eq!(provider_ids, vec!["anthropic"]);
    }

    #[test]
    fn source_registry_tolerates_legacy_null_fields() {
        let content = r#"{
          "handles": [
            {
              "id": "oc_4387461687677135781",
              "realBaseUrl": "https://api.anthropic.com/v1",
              "routeState": {
                "originalBaseUrl": null
              },
              "createdAtMs": 1780400984168,
              "lastSeenAtMs": 1780400984168,
              "lastUsedAtMs": 1780400984168
            }
          ]
        }"#;

        let data = parse_opencode_source_registry_data(content).unwrap();

        assert_eq!(data.handles.len(), 1);
        assert_eq!(data.handles[0].provider_id, "anthropic");
        assert_eq!(
            data.handles[0].route_state.original_base_url,
            "https://api.anthropic.com/v1"
        );
    }

    #[test]
    fn set_provider_base_url_updates_only_requested_provider() {
        let mut json = serde_json::json!({
            "provider": {
                "anthropic": { "options": { "baseURL": "https://api.anthropic.com/v1" } },
                "xiaomi": { "options": { "baseURL": "https://api.xiaomi.example/v1" } }
            }
        });

        set_provider_base_url(
            &mut json,
            "xiaomi",
            "http://127.0.0.1:18765/opencode/provider/xiaomi/source/abc",
        );

        assert_eq!(
            json.pointer("/provider/xiaomi/options/baseURL")
                .and_then(|v| v.as_str()),
            Some("http://127.0.0.1:18765/opencode/provider/xiaomi/source/abc")
        );
        assert_eq!(
            json.pointer("/provider/anthropic/options/baseURL")
                .and_then(|v| v.as_str()),
            Some("https://api.anthropic.com/v1")
        );
    }

    #[test]
    fn proxy_url_parser_supports_provider_scoped_paths() {
        assert_eq!(
            OpenCodeConfigManager::extract_provider_and_source_from_proxy_url(
                "http://127.0.0.1:18765/opencode/provider/xiaomi/source/oc_123"
            ),
            Some((Some("xiaomi".to_string()), "oc_123".to_string()))
        );
        assert_eq!(
            OpenCodeConfigManager::extract_source_id_from_proxy_url(
                "http://127.0.0.1:18765/opencode/source/oc_legacy"
            ),
            Some("oc_legacy".to_string())
        );
    }

    #[test]
    fn takeover_refuses_to_create_missing_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("missing-opencode-dir");
        let config_path = config_dir.join("opencode.json");
        let manager = OpenCodeConfigManager::new_for_path(config_path.clone(), false);
        let handles = vec![OpenCodeSourceHandle {
            id: "oc_test".to_string(),
            provider_id: "xiaomi".to_string(),
            provider_npm: Some("@ai-sdk/openai-compatible".to_string()),
            real_base_url: "https://api.xiaomi.example/v1".to_string(),
            route_state: OpenCodeProviderRouteState {
                provider_id: "xiaomi".to_string(),
                provider_npm: Some("@ai-sdk/openai-compatible".to_string()),
                display_name: None,
                original_base_url: "https://api.xiaomi.example/v1".to_string(),
                source_path: Some(config_path.display().to_string()),
                source_kind: Some(OpenCodeConfigSourceKind::GlobalJson),
            },
            created_at_ms: 0,
            last_seen_at_ms: 0,
            last_used_at_ms: 0,
        }];

        let err = manager.takeover_with_handles(18765, &handles).unwrap_err();

        assert!(err.contains("OpenCode config file was not found"));
        assert!(!config_path.exists());
        assert!(!config_dir.exists());
    }

    #[test]
    fn takeover_updates_existing_config_file_only() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("opencode.json");
        fs::write(
            &config_path,
            r#"{
              "provider": {
                "anthropic": {
                  "npm": "@ai-sdk/anthropic",
                  "options": { "baseURL": "https://api.anthropic.com/v1" }
                },
                "xiaomi": {
                  "npm": "@ai-sdk/openai-compatible",
                  "options": { "baseURL": "https://api.xiaomi.example/v1" }
                }
              }
            }"#,
        )
        .unwrap();
        let manager = OpenCodeConfigManager::new_for_path(config_path.clone(), true);
        let handles = vec![
            OpenCodeSourceHandle {
                id: "oc_ant".to_string(),
                provider_id: "anthropic".to_string(),
                provider_npm: Some("@ai-sdk/anthropic".to_string()),
                real_base_url: "https://api.anthropic.com/v1".to_string(),
                route_state: OpenCodeProviderRouteState {
                    provider_id: "anthropic".to_string(),
                    provider_npm: Some("@ai-sdk/anthropic".to_string()),
                    display_name: None,
                    original_base_url: "https://api.anthropic.com/v1".to_string(),
                    source_path: Some(config_path.display().to_string()),
                    source_kind: Some(OpenCodeConfigSourceKind::GlobalJson),
                },
                created_at_ms: 0,
                last_seen_at_ms: 0,
                last_used_at_ms: 0,
            },
            OpenCodeSourceHandle {
                id: "oc_xm".to_string(),
                provider_id: "xiaomi".to_string(),
                provider_npm: Some("@ai-sdk/openai-compatible".to_string()),
                real_base_url: "https://api.xiaomi.example/v1".to_string(),
                route_state: OpenCodeProviderRouteState {
                    provider_id: "xiaomi".to_string(),
                    provider_npm: Some("@ai-sdk/openai-compatible".to_string()),
                    display_name: None,
                    original_base_url: "https://api.xiaomi.example/v1".to_string(),
                    source_path: Some(config_path.display().to_string()),
                    source_kind: Some(OpenCodeConfigSourceKind::GlobalJson),
                },
                created_at_ms: 0,
                last_seen_at_ms: 0,
                last_used_at_ms: 0,
            },
        ];

        manager.takeover_with_handles(18765, &handles).unwrap();

        let content = fs::read_to_string(&config_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            json.pointer("/provider/anthropic/options/baseURL")
                .and_then(|value| value.as_str()),
            Some("http://127.0.0.1:18765/opencode/provider/anthropic/source/oc_ant")
        );
        assert_eq!(
            json.pointer("/provider/xiaomi/options/baseURL")
                .and_then(|value| value.as_str()),
            Some("http://127.0.0.1:18765/opencode/provider/xiaomi/source/oc_xm")
        );
    }

    #[test]
    fn restore_fails_when_no_handles_found_for_active_takeover() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("opencode.json");
        // 模拟已接管状态：opencode.json 包含代理 URL。
        fs::write(
            &config_path,
            r#"{
              "provider": {
                "anthropic": {
                  "npm": "@ai-sdk/anthropic",
                  "options": { "baseURL": "http://127.0.0.1:18765/opencode/provider/anthropic/source/oc_ant" }
                }
              }
            }"#,
        )
        .unwrap();
        let manager = OpenCodeConfigManager::new_for_path(config_path, true);

        // Registry 中不存在 oc_ant → restore 应该失败，不能静默返回 Ok(0)。
        let err = manager
            .restore_from_sources(&["oc_ant".to_string()])
            .unwrap_err();
        assert!(
            err.contains("no matching source handles"),
            "expected registry-missing error, got: {err}"
        );
    }

    #[test]
    fn takeover_preserves_jsonc_comments_and_updates_only_effective_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("opencode");
        fs::create_dir_all(&config_dir).unwrap();
        let jsonc_path = config_dir.join("opencode.jsonc");
        let json_path = config_dir.join("opencode.json");
        fs::write(
            &jsonc_path,
            r#"{
  // anthropic comment
  "provider": {
    "anthropic": {
      "options": { "baseURL": "https://api.anthropic.com/v1" }
    }
  }
}"#,
        )
        .unwrap();
        fs::write(
            &json_path,
            r#"{
  "provider": {
    "anthropic": {
      "options": { "baseURL": "https://override.example/v1" }
    }
  }
}"#,
        )
        .unwrap();

        with_clean_opencode_env(dir.path(), || {
            let manager = OpenCodeConfigManager::new();
            let route_state = manager.read_live_snapshot().unwrap();
            assert_eq!(route_state.providers.len(), 1);
            assert_eq!(
                route_state.providers[0].source_path.as_deref(),
                Some(json_path.to_string_lossy().as_ref())
            );

            let handles = vec![OpenCodeSourceHandle {
                id: "oc_ant".to_string(),
                provider_id: "anthropic".to_string(),
                provider_npm: Some("@ai-sdk/anthropic".to_string()),
                real_base_url: "https://override.example/v1".to_string(),
                route_state: route_state.providers[0].clone(),
                created_at_ms: 0,
                last_seen_at_ms: 0,
                last_used_at_ms: 0,
            }];
            manager.takeover_with_handles(18765, &handles).unwrap();

            let jsonc_content = fs::read_to_string(&jsonc_path).unwrap();
            let json_content = fs::read_to_string(&json_path).unwrap();
            assert!(jsonc_content.contains("// anthropic comment"));
            assert!(jsonc_content.contains("https://api.anthropic.com/v1"));
            assert!(json_content
                .contains("http://127.0.0.1:18765/opencode/provider/anthropic/source/oc_ant"));
            assert!(!json_content.contains("// anthropic comment"));
        });
    }

    #[test]
    fn patch_provider_base_url_preserves_jsonc_comment() {
        let content = r#"{
  "provider": {
    "anthropic": {
      // keep me
      "options": { "baseURL": "https://api.anthropic.com/v1" }
    }
  }
}"#;

        let patched = patch_provider_base_url(
            content,
            "anthropic",
            "https://api.anthropic.com/v1",
            "http://127.0.0.1:18765/opencode/provider/anthropic/source/oc_ant",
        )
        .unwrap();

        assert!(patched.contains("// keep me"));
        assert!(
            patched.contains("http://127.0.0.1:18765/opencode/provider/anthropic/source/oc_ant")
        );
    }
}
