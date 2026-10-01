//! Pi Agent `models.json` takeover and provider source registry.
//!
//! Pi keeps provider credentials and routing in `~/.pi/agent/models.json`.
//! UsageMeter only rewrites `baseUrl`; API keys and all other fields remain in
//! Pi's file and are never copied into the source registry.

use super::url_identity;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(test))]
use std::sync::{LazyLock, Mutex};

const RUNTIME_DOCUMENT_KEY: &str = "pi_proxy_source_handles";
const TOUCH_WRITE_INTERVAL_MS: i64 = 30_000;
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static SOURCE_REGISTRY_CACHE: LazyLock<Mutex<Option<PiSourceRegistryData>>> =
    LazyLock::new(|| Mutex::new(None));
#[cfg(not(test))]
static SOURCE_REGISTRY_UPDATE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PiProviderApi {
    OpenaiCompletions,
    OpenaiResponses,
    AnthropicMessages,
    GoogleGenerativeAi,
    GoogleVertex,
}

impl PiProviderApi {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "openai-completions" => Some(Self::OpenaiCompletions),
            "openai-responses" => Some(Self::OpenaiResponses),
            "anthropic-messages" => Some(Self::AnthropicMessages),
            "google-generative-ai" => Some(Self::GoogleGenerativeAi),
            "google-vertex" => Some(Self::GoogleVertex),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiCompletions => "openai-completions",
            Self::OpenaiResponses => "openai-responses",
            Self::AnthropicMessages => "anthropic-messages",
            Self::GoogleGenerativeAi => "google-generative-ai",
            Self::GoogleVertex => "google-vertex",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiProviderRouteState {
    pub provider_id: String,
    pub api: PiProviderApi,
    pub original_base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PiRouteState {
    #[serde(default)]
    pub providers: Vec<PiProviderRouteState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiSourceHandle {
    pub id: String,
    pub provider_id: String,
    pub api: PiProviderApi,
    pub real_base_url: String,
    pub route_state: PiProviderRouteState,
    pub created_at_ms: i64,
    pub last_seen_at_ms: i64,
    pub last_used_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PiSourceRegistryData {
    #[serde(default)]
    handles: Vec<PiSourceHandle>,
}

pub struct PiSourceRegistry {
    path: PathBuf,
}

impl PiSourceRegistry {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            path: home
                .join(".usagemeter")
                .join("pi_proxy_source_handles.json"),
        }
    }

    #[cfg(test)]
    fn new_with_path(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn get(&self, id: &str) -> Option<PiSourceHandle> {
        self.read_data()
            .ok()?
            .handles
            .into_iter()
            .find(|handle| handle.id == id)
    }

    pub fn touch_used(&self, id: &str) -> Result<(), String> {
        #[cfg(not(test))]
        let _update_guard = SOURCE_REGISTRY_UPDATE_LOCK
            .lock()
            .map_err(|_| "Pi source registry update lock poisoned".to_string())?;
        let mut data = self.read_data()?;
        if let Some(handle) = data.handles.iter_mut().find(|handle| handle.id == id) {
            let now = now_ms();
            if now.saturating_sub(handle.last_used_at_ms) >= TOUCH_WRITE_INTERVAL_MS {
                handle.last_used_at_ms = now;
                self.write_data(&data)?;
            }
        }
        Ok(())
    }

    pub fn upsert_provider_state(
        &self,
        provider_state: PiProviderRouteState,
    ) -> Result<PiSourceHandle, String> {
        #[cfg(not(test))]
        let _update_guard = SOURCE_REGISTRY_UPDATE_LOCK
            .lock()
            .map_err(|_| "Pi source registry update lock poisoned".to_string())?;
        if PiConfigManager::is_usagemeter_proxy_url(&provider_state.original_base_url) {
            return Err("Refusing to register UsageMeter proxy URL as a Pi upstream".to_string());
        }
        let id = compute_handle_id(&provider_state)?;
        let now = now_ms();
        let mut data = self.read_data()?;
        if let Some(existing) = data.handles.iter_mut().find(|handle| handle.id == id) {
            existing.provider_id = provider_state.provider_id.clone();
            existing.api = provider_state.api;
            existing.real_base_url = provider_state.original_base_url.clone();
            existing.route_state = provider_state;
            existing.last_seen_at_ms = now;
            existing.last_used_at_ms = now;
            let handle = existing.clone();
            self.write_data(&data)?;
            return Ok(handle);
        }

        let handle = PiSourceHandle {
            id,
            provider_id: provider_state.provider_id.clone(),
            api: provider_state.api,
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

    pub fn upsert_from_state(&self, state: &PiRouteState) -> Result<Vec<PiSourceHandle>, String> {
        state
            .providers
            .iter()
            .cloned()
            .map(|provider| self.upsert_provider_state(provider))
            .collect()
    }

    fn read_data(&self) -> Result<PiSourceRegistryData, String> {
        #[cfg(not(test))]
        {
            if let Some(data) = SOURCE_REGISTRY_CACHE
                .lock()
                .map_err(|_| "Pi source registry cache lock poisoned".to_string())?
                .clone()
            {
                return Ok(data);
            }
            if let Some(value) = crate::app_config::load_runtime_document(RUNTIME_DOCUMENT_KEY)? {
                let data: PiSourceRegistryData = serde_json::from_value(value)
                    .map_err(|e| format!("Failed to parse Pi source registry: {e}"))?;
                *SOURCE_REGISTRY_CACHE
                    .lock()
                    .map_err(|_| "Pi source registry cache lock poisoned".to_string())? =
                    Some(data.clone());
                return Ok(data);
            }
        }
        if !self.path.exists() {
            return Ok(PiSourceRegistryData::default());
        }
        let content = fs::read_to_string(&self.path)
            .map_err(|e| format!("Failed to read Pi source registry: {e}"))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse Pi source registry: {e}"))
    }

    fn write_data(&self, data: &PiSourceRegistryData) -> Result<(), String> {
        #[cfg(not(test))]
        {
            let value = serde_json::to_value(data)
                .map_err(|e| format!("Failed to serialize Pi source registry: {e}"))?;
            crate::app_config::save_runtime_document(RUNTIME_DOCUMENT_KEY, &value)?;
            *SOURCE_REGISTRY_CACHE
                .lock()
                .map_err(|_| "Pi source registry cache lock poisoned".to_string())? =
                Some(data.clone());
            Ok(())
        }
        #[cfg(test)]
        {
            write_json_atomically(&self.path, data)
        }
    }
}

impl Default for PiSourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct PiConfigManager {
    config_path: PathBuf,
}

impl PiConfigManager {
    pub fn new() -> Self {
        let path = dirs::home_dir()
            .map(|home| home.join(".pi").join("agent").join("models.json"))
            .unwrap_or_else(|| PathBuf::from(".pi/agent/models.json"));
        Self { config_path: path }
    }

    #[cfg(test)]
    fn new_for_path(path: PathBuf) -> Self {
        Self { config_path: path }
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn ensure_config_exists(&self) -> Result<(), String> {
        if self.config_path.exists() {
            Ok(())
        } else {
            Err(format!(
                "Pi Agent models.json was not found. Configure Pi first. Expected path: {}",
                self.config_path.display()
            ))
        }
    }

    pub fn read_live_snapshot(&self) -> Result<PiRouteState, String> {
        self.ensure_config_exists()?;
        let root = read_json(&self.config_path)?;
        Ok(parse_route_state(&root))
    }

    pub fn takeover_with_handles(
        &self,
        proxy_port: u16,
        handles: &[PiSourceHandle],
    ) -> Result<(), String> {
        self.ensure_config_exists()?;
        if handles.is_empty() {
            return Err("No supported Pi providers with a baseUrl were found".to_string());
        }
        let (mut root, expected_fingerprint) = read_json_with_fingerprint(&self.config_path)?;
        let providers = root
            .get_mut("providers")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| "Pi models.json providers must be an object".to_string())?;
        let mut matched = 0;
        let mut modified = 0;
        for handle in handles {
            if let Some(provider) = providers.get_mut(&handle.provider_id) {
                matched += 1;
                let object = provider.as_object_mut().ok_or_else(|| {
                    format!("Pi provider '{}' must be an object", handle.provider_id)
                })?;
                let proxy_url = url_identity::prefixed_proxy_url(proxy_port, "pi", &handle.id, "");
                if object.get("baseUrl").and_then(Value::as_str) != Some(proxy_url.as_str()) {
                    object.insert("baseUrl".to_string(), Value::String(proxy_url));
                    modified += 1;
                }
            }
        }
        if matched == 0 {
            return Err(
                "None of the Pi source handles matched a provider in models.json".to_string(),
            );
        }
        if modified > 0 {
            write_json_atomically_if_unchanged(&self.config_path, &root, &expected_fingerprint)?;
        }
        Ok(())
    }

    pub fn restore_from_sources(&self, source_ids: &[String]) -> Result<usize, String> {
        if source_ids.is_empty() || !self.config_path.exists() {
            return Ok(0);
        }
        let registry = PiSourceRegistry::new();
        self.restore_from_sources_with_registry(&registry, source_ids)
    }

    #[cfg(test)]
    fn restore_from_sources_with_registry(
        &self,
        registry: &PiSourceRegistry,
        source_ids: &[String],
    ) -> Result<usize, String> {
        let (mut root, expected_fingerprint) = read_json_with_fingerprint(&self.config_path)?;
        let providers = root
            .get_mut("providers")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| "Pi models.json providers must be an object".to_string())?;
        let mut restored = 0;
        for source_id in source_ids {
            let Some(handle) = registry.get(source_id) else {
                continue;
            };
            let Some(provider) = providers.get_mut(&handle.provider_id) else {
                continue;
            };
            let Some(current_url) = provider.get("baseUrl").and_then(Value::as_str) else {
                continue;
            };
            if Self::extract_source_id_from_proxy_url(current_url).as_deref() != Some(&handle.id) {
                continue;
            }
            if let Some(object) = provider.as_object_mut() {
                object.insert(
                    "baseUrl".to_string(),
                    Value::String(handle.route_state.original_base_url.clone()),
                );
                restored += 1;
            }
        }
        if restored > 0 {
            write_json_atomically_if_unchanged(&self.config_path, &root, &expected_fingerprint)?;
        }
        Ok(restored)
    }

    #[cfg(not(test))]
    fn restore_from_sources_with_registry(
        &self,
        registry: &PiSourceRegistry,
        source_ids: &[String],
    ) -> Result<usize, String> {
        let (mut root, expected_fingerprint) = read_json_with_fingerprint(&self.config_path)?;
        let providers = root
            .get_mut("providers")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| "Pi models.json providers must be an object".to_string())?;
        let mut restored = 0;
        for source_id in source_ids {
            let Some(handle) = registry.get(source_id) else {
                continue;
            };
            let Some(provider) = providers.get_mut(&handle.provider_id) else {
                continue;
            };
            let Some(current_url) = provider.get("baseUrl").and_then(Value::as_str) else {
                continue;
            };
            if Self::extract_source_id_from_proxy_url(current_url).as_deref() != Some(&handle.id) {
                continue;
            }
            if let Some(object) = provider.as_object_mut() {
                object.insert(
                    "baseUrl".to_string(),
                    Value::String(handle.route_state.original_base_url.clone()),
                );
                restored += 1;
            }
        }
        if restored > 0 {
            write_json_atomically_if_unchanged(&self.config_path, &root, &expected_fingerprint)?;
        }
        Ok(restored)
    }

    pub fn active_source_ids(&self) -> Vec<String> {
        self.read_live_snapshot()
            .unwrap_or_default()
            .providers
            .into_iter()
            .filter_map(|provider| {
                Self::extract_source_id_from_proxy_url(&provider.original_base_url)
            })
            .collect()
    }

    pub fn active_source_ids_for_port(&self, proxy_port: u16) -> Vec<String> {
        self.read_live_snapshot()
            .unwrap_or_default()
            .providers
            .into_iter()
            .filter(|provider| {
                Self::is_usagemeter_proxy_url_for_port(&provider.original_base_url, proxy_port)
            })
            .filter_map(|provider| {
                Self::extract_source_id_from_proxy_url(&provider.original_base_url)
            })
            .collect()
    }

    pub fn active_source_id(&self) -> Option<String> {
        self.active_source_ids().into_iter().next()
    }

    pub fn is_takeover_active(&self, proxy_port: u16) -> Result<bool, String> {
        let snapshot = self.read_live_snapshot()?;
        Ok(snapshot.providers.iter().any(|provider| {
            Self::is_usagemeter_proxy_url_for_port(&provider.original_base_url, proxy_port)
        }))
    }

    pub fn is_usagemeter_proxy_url(base_url: &str) -> bool {
        url_identity::is_usagemeter_proxy_url(base_url, &["pi"])
    }

    pub fn is_usagemeter_proxy_url_for_port(base_url: &str, proxy_port: u16) -> bool {
        url_identity::is_usagemeter_proxy_url_for_port(base_url, proxy_port, &["pi"])
    }

    pub fn extract_source_id_from_proxy_url(base_url: &str) -> Option<String> {
        url_identity::extract_source_id_from_proxy_url(base_url, &["pi"])
    }
}

impl Default for PiConfigManager {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_route_state(root: &Value) -> PiRouteState {
    let providers = root
        .get("providers")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|providers| providers.iter())
        .filter_map(|(provider_id, value)| {
            let object = value.as_object()?;
            let api = PiProviderApi::parse(object.get("api")?.as_str()?)?;
            let base_url = object.get("baseUrl")?.as_str()?.trim();
            if base_url.is_empty() {
                return None;
            }
            Some(PiProviderRouteState {
                provider_id: provider_id.clone(),
                api,
                original_base_url: base_url.to_string(),
            })
        })
        .collect();
    PiRouteState { providers }
}

fn compute_handle_id(state: &PiProviderRouteState) -> Result<String, String> {
    let mut hasher = Sha256::new();
    hasher.update(state.provider_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(state.api.as_str().as_bytes());
    hasher.update(b"\n");
    hasher.update(state.original_base_url.as_bytes());
    let hash = hasher.finalize();
    Ok(format!(
        "pi_{:08x}",
        u32::from_be_bytes(hash[..4].try_into().unwrap())
    ))
}

fn read_json(path: &Path) -> Result<Value, String> {
    read_json_with_fingerprint(path).map(|(value, _)| value)
}

fn read_json_with_fingerprint(path: &Path) -> Result<(Value, [u8; 32]), String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("Failed to read Pi models.json: {e}"))?;
    let fingerprint: [u8; 32] = Sha256::digest(content.as_bytes()).into();
    let value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse Pi models.json: {e}"))?;
    Ok((value, fingerprint))
}

#[cfg(test)]
fn write_json_atomically<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let content = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize Pi models.json: {e}"))?;
    let parent = path
        .parent()
        .ok_or_else(|| "Pi models.json has no parent directory".to_string())?;
    fs::create_dir_all(parent).map_err(|e| format!("Failed to create Pi config directory: {e}"))?;
    let temp = unique_temp_path(path);
    fs::write(&temp, content).map_err(|e| format!("Failed to write Pi temporary config: {e}"))?;
    fs::rename(&temp, path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        format!("Failed to replace Pi models.json atomically: {e}")
    })
}

fn write_json_atomically_if_unchanged<T: Serialize>(
    path: &Path,
    value: &T,
    expected_fingerprint: &[u8; 32],
) -> Result<(), String> {
    let current = fs::read(path)
        .map_err(|e| format!("Failed to re-read Pi models.json before write: {e}"))?;
    let current_fingerprint: [u8; 32] = Sha256::digest(&current).into();
    if &current_fingerprint != expected_fingerprint {
        return Err(
            "Pi models.json changed while UsageMeter was preparing a takeover; retry the operation"
                .to_string(),
        );
    }
    let content = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize Pi models.json: {e}"))?;
    let parent = path
        .parent()
        .ok_or_else(|| "Pi models.json has no parent directory".to_string())?;
    fs::create_dir_all(parent).map_err(|e| format!("Failed to create Pi config directory: {e}"))?;
    let temp = unique_temp_path(path);
    fs::write(&temp, content).map_err(|e| format!("Failed to write Pi temporary config: {e}"))?;
    let current = fs::read(path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        format!("Failed to re-read Pi models.json before replace: {e}")
    })?;
    let current_fingerprint: [u8; 32] = Sha256::digest(&current).into();
    if &current_fingerprint != expected_fingerprint {
        let _ = fs::remove_file(&temp);
        return Err(
            "Pi models.json changed while UsageMeter was preparing a takeover; retry the operation"
                .to_string(),
        );
    }
    fs::rename(&temp, path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        format!("Failed to replace Pi models.json atomically: {e}")
    })
}

fn unique_temp_path(path: &Path) -> PathBuf {
    let sequence = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_extension(format!("json.usagemeter-{}-{sequence}", std::process::id()))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_only_supported_providers_without_reading_keys() {
        let root = serde_json::json!({
            "providers": {
                "anthropic": {"api": "anthropic-messages", "baseUrl": "https://anthropic.example", "apiKey": "secret"},
                "unknown": {"api": "custom", "baseUrl": "https://custom.example"},
                "empty": {"api": "openai-completions"}
            },
            "unknownField": true
        });
        let state = parse_route_state(&root);
        assert_eq!(state.providers.len(), 1);
        assert_eq!(state.providers[0].provider_id, "anthropic");
        assert_eq!(state.providers[0].api, PiProviderApi::AnthropicMessages);
    }

    #[test]
    fn takeover_and_restore_preserve_unknown_fields() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("models.json");
        fs::write(
            &path,
            r#"{"providers":{"openai":{"api":"openai-completions","baseUrl":"https://api.example","apiKey":"secret","extra":1}},"other":true}"#,
        )
        .unwrap();
        let manager = PiConfigManager::new_for_path(path.clone());
        let state = manager.read_live_snapshot().unwrap();
        let registry = PiSourceRegistry::new_with_path(dir.path().join("handles.json"));
        let handles = registry.upsert_from_state(&state).unwrap();
        manager.takeover_with_handles(18765, &handles).unwrap();
        let routed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(routed["other"].as_bool().unwrap());
        assert_eq!(routed["providers"]["openai"]["apiKey"], "secret");
        assert!(PiConfigManager::is_usagemeter_proxy_url(
            routed["providers"]["openai"]["baseUrl"].as_str().unwrap()
        ));
        manager
            .restore_from_sources_with_registry(&registry, &[handles[0].id.clone()])
            .unwrap();
        let restored: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            restored["providers"]["openai"]["baseUrl"],
            "https://api.example"
        );
    }

    #[test]
    fn takeover_rejects_external_change_before_replacing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("models.json");
        fs::write(
            &path,
            r#"{"providers":{"openai":{"api":"openai-completions","baseUrl":"https://api.example"}}}"#,
        )
        .unwrap();
        let (root, expected_fingerprint) = read_json_with_fingerprint(&path).unwrap();
        fs::write(
            &path,
            r#"{"providers":{"openai":{"api":"openai-completions","baseUrl":"https://user.changed"}}}"#,
        )
        .unwrap();
        let error =
            write_json_atomically_if_unchanged(&path, &root, &expected_fingerprint).unwrap_err();
        assert!(error.contains("changed while UsageMeter"));
        let current: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            current["providers"]["openai"]["baseUrl"],
            "https://user.changed"
        );
    }

    #[test]
    fn takeover_rejects_handles_that_no_longer_match_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("models.json");
        fs::write(
            &path,
            r#"{"providers":{"anthropic":{"api":"anthropic-messages","baseUrl":"https://api.example"}}}"#,
        )
        .unwrap();
        let manager = PiConfigManager::new_for_path(path);
        let handle = PiSourceHandle {
            id: "pi_deadbeef".to_string(),
            provider_id: "missing".to_string(),
            api: PiProviderApi::AnthropicMessages,
            real_base_url: "https://api.example".to_string(),
            route_state: PiProviderRouteState {
                provider_id: "missing".to_string(),
                api: PiProviderApi::AnthropicMessages,
                original_base_url: "https://api.example".to_string(),
            },
            created_at_ms: 0,
            last_seen_at_ms: 0,
            last_used_at_ms: 0,
        };
        let error = manager.takeover_with_handles(18765, &[handle]).unwrap_err();
        assert!(error.contains("matched a provider"));
    }

    #[test]
    fn active_source_ids_are_filtered_by_port() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("models.json");
        fs::write(
            &path,
            serde_json::to_string(&serde_json::json!({
                "providers": {
                    "one": {"api": "openai-completions", "baseUrl": url_identity::prefixed_proxy_url(18765, "pi", "pi_one", "")},
                    "two": {"api": "openai-completions", "baseUrl": url_identity::prefixed_proxy_url(18766, "pi", "pi_two", "")}
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let manager = PiConfigManager::new_for_path(path);
        assert_eq!(manager.active_source_ids_for_port(18765), vec!["pi_one"]);
    }

    #[test]
    fn provider_api_is_explicit_and_case_insensitive() {
        assert_eq!(
            PiProviderApi::parse("OPENAI-RESPONSES"),
            Some(PiProviderApi::OpenaiResponses)
        );
        assert_eq!(PiProviderApi::parse("custom"), None);
    }
}
