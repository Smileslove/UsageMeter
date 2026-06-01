//! Codex CLI configuration takeover support.
//!
//! Codex keeps live configuration in `~/.codex/config.toml` and auth material in
//! `~/.codex/auth.json`. UsageMeter rewrites only routing-related base URL fields
//! to point at the local proxy, and persists only the minimal route state needed
//! to restore those fields later.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use toml_edit::{DocumentMut, Item, Value};

const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const ROOT_PROVIDER_ID: &str = "__root__";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexRouteState {
    pub provider_id: String,
    pub real_base_url: String,
    #[serde(default)]
    pub auth_mode: CodexAuthMode,
    #[serde(default)]
    pub had_chatgpt_base_url: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodexAuthMode {
    #[default]
    ApiKey,
    ChatGpt,
}

pub fn codex_snapshot_uses_official_provider(snapshot: &CodexRouteState) -> bool {
    snapshot.auth_mode == CodexAuthMode::ChatGpt
        || is_official_openai_base_url(&snapshot.real_base_url)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSourceHandle {
    pub id: String,
    pub real_base_url: String,
    pub provider_id: String,
    pub route_state: CodexRouteState,
    pub created_at_ms: i64,
    pub last_seen_at_ms: i64,
    pub last_used_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct CodexSourceRegistryData {
    #[serde(default)]
    handles: Vec<CodexSourceHandle>,
}

pub struct CodexSourceRegistry {
    path: PathBuf,
}

impl CodexSourceRegistry {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            path: home
                .join(".usagemeter")
                .join("codex_proxy_source_handles.json"),
        }
    }

    pub fn get(&self, id: &str) -> Option<CodexSourceHandle> {
        self.read_data()
            .ok()?
            .handles
            .into_iter()
            .find(|handle| handle.id == id)
    }

    pub fn list_handles(&self) -> Vec<CodexSourceHandle> {
        self.read_data()
            .map(|data| data.handles)
            .unwrap_or_default()
    }

    pub fn latest_for_provider(&self, provider_id: &str) -> Option<CodexSourceHandle> {
        self.list_handles()
            .into_iter()
            .filter(|handle| handle.provider_id == provider_id)
            .max_by_key(|handle| handle.last_used_at_ms.max(handle.last_seen_at_ms))
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

    pub fn upsert_from_snapshot(
        &self,
        snapshot: CodexRouteState,
    ) -> Result<CodexSourceHandle, String> {
        if CodexConfigManager::is_usagemeter_proxy_url(&snapshot.real_base_url) {
            return Err(
                "Refusing to register UsageMeter proxy URL as a Codex upstream".to_string(),
            );
        }

        let id = compute_handle_id(&snapshot)?;
        let now = now_ms();
        let mut data = self.read_data()?;

        if let Some(existing) = data.handles.iter_mut().find(|handle| handle.id == id) {
            existing.real_base_url = snapshot.real_base_url.clone();
            existing.provider_id = snapshot.provider_id.clone();
            existing.route_state = snapshot;
            existing.last_seen_at_ms = now;
            existing.last_used_at_ms = now;
            let handle = existing.clone();
            self.write_data(&data)?;
            return Ok(handle);
        }

        let handle = CodexSourceHandle {
            id,
            real_base_url: snapshot.real_base_url.clone(),
            provider_id: snapshot.provider_id.clone(),
            route_state: snapshot,
            created_at_ms: now,
            last_seen_at_ms: now,
            last_used_at_ms: now,
        };
        data.handles.push(handle.clone());
        self.write_data(&data)?;
        Ok(handle)
    }

    fn read_data(&self) -> Result<CodexSourceRegistryData, String> {
        if !self.path.exists() {
            return Ok(CodexSourceRegistryData::default());
        }
        let content = fs::read_to_string(&self.path)
            .map_err(|e| format!("Failed to read Codex source registry: {}", e))?;
        serde_json::from_str(&content)
            .or_else(|_| migrate_legacy_registry_data(&content))
            .map_err(|e| format!("Failed to parse Codex source registry: {}", e))
    }

    fn write_data(&self, data: &CodexSourceRegistryData) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create Codex source registry directory: {}", e))?;
        }
        let content = serde_json::to_string_pretty(data)
            .map_err(|e| format!("Failed to serialize Codex source registry: {}", e))?;
        fs::write(&self.path, content)
            .map_err(|e| format!("Failed to save Codex source registry: {}", e))?;
        Ok(())
    }
}

impl Default for CodexSourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct CodexConfigManager {
    config_path: PathBuf,
    auth_path: PathBuf,
}

impl CodexConfigManager {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let codex_dir = home.join(".codex");
        Self {
            config_path: codex_dir.join("config.toml"),
            auth_path: codex_dir.join("auth.json"),
        }
    }

    pub fn config_path(&self) -> &PathBuf {
        &self.config_path
    }

    pub fn auth_path(&self) -> &PathBuf {
        &self.auth_path
    }

    pub fn read_live_snapshot(&self) -> Result<CodexRouteState, String> {
        let config_toml = fs::read_to_string(&self.config_path)
            .map_err(|e| format!("Failed to read Codex config.toml: {}", e))?;
        let doc = config_toml
            .parse::<DocumentMut>()
            .map_err(|e| format!("Failed to parse Codex config.toml: {}", e))?;
        let auth_json = self.read_auth_json()?;
        let auth_mode = detect_auth_mode(auth_json.as_ref());
        let provider_id = detect_provider_id(&doc)?;
        let had_chatgpt_base_url = chatgpt_base_url(&doc).is_some();
        let real_base_url = match auth_mode {
            CodexAuthMode::ChatGpt => {
                chatgpt_base_url(&doc).unwrap_or_else(|| DEFAULT_CHATGPT_CODEX_BASE_URL.to_string())
            }
            CodexAuthMode::ApiKey => provider_base_url(&doc, &provider_id)
                .unwrap_or_else(|| DEFAULT_OPENAI_BASE_URL.to_string()),
        };

        Ok(CodexRouteState {
            provider_id,
            real_base_url,
            auth_mode,
            had_chatgpt_base_url,
        })
    }

    pub fn takeover_with_source(&self, proxy_port: u16, source_id: &str) -> Result<(), String> {
        let snapshot = self.read_live_snapshot()?;
        let config_toml = fs::read_to_string(&self.config_path)
            .map_err(|e| format!("Failed to read Codex config.toml: {}", e))?;
        let mut doc = config_toml
            .parse::<DocumentMut>()
            .map_err(|e| format!("Failed to parse Codex config.toml: {}", e))?;

        let proxy_url = match snapshot.auth_mode {
            CodexAuthMode::ChatGpt => {
                format!("http://127.0.0.1:{}/codex/source/{}", proxy_port, source_id)
            }
            CodexAuthMode::ApiKey => {
                format!(
                    "http://127.0.0.1:{}/codex/source/{}/v1",
                    proxy_port, source_id
                )
            }
        };
        match snapshot.auth_mode {
            CodexAuthMode::ChatGpt => set_chatgpt_base_url(&mut doc, &proxy_url),
            CodexAuthMode::ApiKey => {
                set_provider_base_url(&mut doc, &snapshot.provider_id, &proxy_url)?
            }
        }

        self.write_config_doc(&doc)?;

        Ok(())
    }

    pub fn restore_from_source(&self, source_id: &str) -> Result<bool, String> {
        let Some(handle) = CodexSourceRegistry::new().get(source_id) else {
            return Ok(false);
        };
        let config_toml = fs::read_to_string(&self.config_path)
            .map_err(|e| format!("Failed to read Codex config.toml: {}", e))?;
        let mut doc = config_toml
            .parse::<DocumentMut>()
            .map_err(|e| format!("Failed to parse Codex config.toml: {}", e))?;
        match handle.route_state.auth_mode {
            CodexAuthMode::ChatGpt => {
                if handle.route_state.had_chatgpt_base_url {
                    set_chatgpt_base_url(&mut doc, &handle.route_state.real_base_url);
                } else {
                    doc.remove("chatgpt_base_url");
                }
            }
            CodexAuthMode::ApiKey => {
                set_provider_base_url(
                    &mut doc,
                    &handle.route_state.provider_id,
                    &handle.route_state.real_base_url,
                )?;
            }
        }
        self.write_config_doc(&doc)?;
        Ok(true)
    }

    pub fn is_takeover_active(&self, proxy_port: u16) -> Result<bool, String> {
        let config_toml = fs::read_to_string(&self.config_path)
            .map_err(|e| format!("Failed to read Codex config.toml: {}", e))?;
        let doc = config_toml
            .parse::<DocumentMut>()
            .map_err(|e| format!("Failed to parse Codex config.toml: {}", e))?;
        let auth_mode = detect_auth_mode(self.read_auth_json()?.as_ref());

        if auth_mode == CodexAuthMode::ChatGpt {
            return Ok(chatgpt_base_url(&doc)
                .map(|base_url| Self::is_usagemeter_proxy_url_for_port(&base_url, proxy_port))
                .unwrap_or(false));
        }

        let provider_id = detect_provider_id(&doc)?;
        if let Some(base_url) = provider_base_url(&doc, &provider_id) {
            if Self::is_usagemeter_proxy_url_for_port(&base_url, proxy_port) {
                return Ok(true);
            }
        }

        if let Some(base_url) = doc.get("base_url").and_then(|item| item.as_str()) {
            if Self::is_usagemeter_proxy_url_for_port(base_url, proxy_port) {
                return Ok(true);
            }
        }

        Ok(false)
    }

    pub fn active_source_id(&self) -> Option<String> {
        let config_toml = fs::read_to_string(&self.config_path).ok()?;
        let doc = config_toml.parse::<DocumentMut>().ok()?;
        chatgpt_base_url(&doc)
            .and_then(|url| Self::extract_source_id_from_proxy_url(&url))
            .or_else(|| {
                let provider_id = detect_provider_id(&doc).ok()?;
                let base_url = provider_base_url(&doc, &provider_id)?;
                Self::extract_source_id_from_proxy_url(&base_url)
            })
    }

    pub fn is_usagemeter_proxy_url(base_url: &str) -> bool {
        let Ok(url) = reqwest::Url::parse(base_url) else {
            return false;
        };
        Self::is_local_codex_proxy_url(&url)
    }

    pub fn is_usagemeter_proxy_url_for_port(base_url: &str, proxy_port: u16) -> bool {
        let Ok(url) = reqwest::Url::parse(base_url) else {
            return false;
        };
        if !Self::is_local_codex_proxy_url(&url) && !Self::is_local_openai_proxy_url(&url) {
            return false;
        }

        url.port() == Some(proxy_port)
    }

    fn is_local_codex_proxy_url(url: &reqwest::Url) -> bool {
        if !Self::is_local_proxy_host(url) {
            return false;
        }

        let path = url.path().trim_end_matches('/');
        path == "/codex" || path.starts_with("/codex/source/")
    }

    fn is_local_openai_proxy_url(url: &reqwest::Url) -> bool {
        if !Self::is_local_proxy_host(url) {
            return false;
        }

        let path = url.path().trim_end_matches('/');
        path == "/v1" || path.starts_with("/v1/")
    }

    fn is_local_proxy_host(url: &reqwest::Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        host == "127.0.0.1" || host == "localhost"
    }

    pub fn extract_source_id_from_proxy_url(base_url: &str) -> Option<String> {
        if !Self::is_usagemeter_proxy_url(base_url) {
            return None;
        }
        let marker = "/source/";
        let marker_index = base_url.find(marker)?;
        let rest = &base_url[(marker_index + marker.len())..];
        let source_id = rest
            .split('/')
            .next()
            .unwrap_or_default()
            .split('?')
            .next()
            .unwrap_or_default()
            .trim();
        (!source_id.is_empty()).then(|| source_id.to_string())
    }

    fn read_auth_json(&self) -> Result<Option<serde_json::Value>, String> {
        if !self.auth_path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&self.auth_path)
            .map_err(|e| format!("Failed to read Codex auth.json: {}", e))?;
        serde_json::from_str(&content)
            .map(Some)
            .map_err(|e| format!("Failed to parse Codex auth.json: {}", e))
    }

    fn write_config_doc(&self, doc: &DocumentMut) -> Result<(), String> {
        self.write_config_raw(&doc.to_string())
    }

    fn write_config_raw(&self, content: &str) -> Result<(), String> {
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create Codex config directory: {}", e))?;
        }
        fs::write(&self.config_path, content)
            .map_err(|e| format!("Failed to save Codex config.toml: {}", e))?;
        Ok(())
    }
}

impl Default for CodexConfigManager {
    fn default() -> Self {
        Self::new()
    }
}

fn detect_provider_id(doc: &DocumentMut) -> Result<String, String> {
    if let Some(provider) = doc
        .get("model_provider")
        .and_then(Item::as_str)
        .map(str::to_string)
    {
        return Ok(provider);
    }

    if doc.get("base_url").and_then(Item::as_str).is_some() {
        return Ok(ROOT_PROVIDER_ID.to_string());
    }

    if let Some(providers) = doc.get("model_providers").and_then(Item::as_table) {
        for (key, item) in providers.iter() {
            if item
                .as_table()
                .and_then(|table| table.get("base_url"))
                .and_then(Item::as_str)
                .is_some()
            {
                return Ok(key.to_string());
            }
        }
    }

    Ok(ROOT_PROVIDER_ID.to_string())
}

fn provider_base_url(doc: &DocumentMut, provider_id: &str) -> Option<String> {
    if provider_id == ROOT_PROVIDER_ID {
        return doc
            .get("base_url")
            .and_then(Item::as_str)
            .map(str::to_string);
    }

    doc.get("model_providers")?
        .as_table()?
        .get(provider_id)?
        .as_table()?
        .get("base_url")?
        .as_str()
        .map(str::to_string)
}

fn chatgpt_base_url(doc: &DocumentMut) -> Option<String> {
    doc.get("chatgpt_base_url")
        .and_then(Item::as_str)
        .map(str::to_string)
}

fn set_chatgpt_base_url(doc: &mut DocumentMut, base_url: &str) {
    doc["chatgpt_base_url"] = Item::Value(Value::from(base_url));
}

fn detect_auth_mode(auth: Option<&serde_json::Value>) -> CodexAuthMode {
    if auth
        .and_then(|auth| auth.get("auth_mode"))
        .and_then(|value| value.as_str())
        == Some("chatgpt")
    {
        CodexAuthMode::ChatGpt
    } else {
        CodexAuthMode::ApiKey
    }
}

fn is_official_openai_base_url(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    if host != "api.openai.com" {
        return false;
    }

    let path = url.path().trim_end_matches('/');
    path.is_empty() || path == "/v1"
}

fn set_provider_base_url(
    doc: &mut DocumentMut,
    provider_id: &str,
    base_url: &str,
) -> Result<(), String> {
    if provider_id == ROOT_PROVIDER_ID {
        doc["base_url"] = Item::Value(Value::from(base_url));
        return Ok(());
    }

    let providers = doc["model_providers"].or_insert(toml_edit::table());
    let provider = providers[provider_id].or_insert(toml_edit::table());
    provider["base_url"] = Item::Value(Value::from(base_url));
    if doc.get("model_provider").is_none() {
        doc["model_provider"] = Item::Value(Value::from(provider_id));
    }
    Ok(())
}

fn migrate_legacy_registry_data(
    content: &str,
) -> Result<CodexSourceRegistryData, serde_json::Error> {
    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LegacySnapshot {
        provider_id: String,
        real_base_url: String,
        #[serde(default)]
        auth_mode: CodexAuthMode,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LegacyHandle {
        id: String,
        real_base_url: String,
        provider_id: String,
        original_snapshot: LegacySnapshot,
        created_at_ms: i64,
        last_seen_at_ms: i64,
        last_used_at_ms: i64,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LegacyData {
        #[serde(default)]
        handles: Vec<LegacyHandle>,
    }

    let legacy: LegacyData = serde_json::from_str(content)?;
    Ok(CodexSourceRegistryData {
        handles: legacy
            .handles
            .into_iter()
            .map(|handle| CodexSourceHandle {
                id: handle.id,
                real_base_url: handle.real_base_url.clone(),
                provider_id: handle.provider_id.clone(),
                route_state: CodexRouteState {
                    provider_id: handle.original_snapshot.provider_id,
                    real_base_url: handle.original_snapshot.real_base_url,
                    auth_mode: handle.original_snapshot.auth_mode,
                    had_chatgpt_base_url: handle.original_snapshot.auth_mode
                        == CodexAuthMode::ChatGpt,
                },
                created_at_ms: handle.created_at_ms,
                last_seen_at_ms: handle.last_seen_at_ms,
                last_used_at_ms: handle.last_used_at_ms,
            })
            .collect(),
    })
}

fn compute_handle_id(snapshot: &CodexRouteState) -> Result<String, String> {
    let payload = serde_json::to_vec(snapshot)
        .map_err(|e| format!("Failed to serialize Codex source snapshot: {}", e))?;
    let mut hasher = Sha256::new();
    hasher.update(payload);
    let hash = hasher.finalize();
    Ok(format!(
        "h_{}",
        u64::from_be_bytes(hash[..8].try_into().unwrap())
    ))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_provider_from_root_key() {
        let doc = r#"
model_provider = "custom"

[model_providers.custom]
base_url = "https://api.example.com/v1"
"#
        .parse::<DocumentMut>()
        .unwrap();
        assert_eq!(detect_provider_id(&doc).unwrap(), "custom");
        assert_eq!(
            provider_base_url(&doc, "custom").as_deref(),
            Some("https://api.example.com/v1")
        );
    }

    #[test]
    fn active_model_provider_wins_over_legacy_root_base_url() {
        let doc = r#"
base_url = "https://legacy.example.com/v1"
model_provider = "custom"

[model_providers.custom]
base_url = "https://api.example.com/v1"
"#
        .parse::<DocumentMut>()
        .unwrap();
        assert_eq!(detect_provider_id(&doc).unwrap(), "custom");
        assert_eq!(
            provider_base_url(&doc, "custom").as_deref(),
            Some("https://api.example.com/v1")
        );
    }

    #[test]
    fn supports_legacy_root_base_url() {
        let mut doc = r#"
base_url = "https://api.example.com/v1"
"#
        .parse::<DocumentMut>()
        .unwrap();
        assert_eq!(detect_provider_id(&doc).unwrap(), ROOT_PROVIDER_ID);
        assert_eq!(
            provider_base_url(&doc, ROOT_PROVIDER_ID).as_deref(),
            Some("https://api.example.com/v1")
        );

        set_provider_base_url(
            &mut doc,
            ROOT_PROVIDER_ID,
            "http://127.0.0.1:18765/codex/source/h_1/v1",
        )
        .unwrap();
        assert_eq!(
            doc.get("base_url").and_then(Item::as_str),
            Some("http://127.0.0.1:18765/codex/source/h_1/v1")
        );
    }

    #[test]
    fn chatgpt_takeover_sets_chatgpt_base_url_only() {
        let mut doc = r#"
base_url = "https://api.example.com/v1"
"#
        .parse::<DocumentMut>()
        .unwrap();
        set_chatgpt_base_url(&mut doc, "http://127.0.0.1:18765/codex/source/h_1");
        assert_eq!(
            chatgpt_base_url(&doc).as_deref(),
            Some("http://127.0.0.1:18765/codex/source/h_1")
        );
        assert_eq!(
            doc.get("base_url").and_then(Item::as_str),
            Some("https://api.example.com/v1")
        );
    }

    #[test]
    fn chatgpt_active_source_prefers_chatgpt_base_url() {
        let doc = r#"
base_url = "http://127.0.0.1:18765/codex/source/h_old/v1"
chatgpt_base_url = "http://127.0.0.1:18765/codex/source/h_chatgpt"
"#
        .parse::<DocumentMut>()
        .unwrap();
        assert_eq!(
            chatgpt_base_url(&doc)
                .and_then(|url| CodexConfigManager::extract_source_id_from_proxy_url(&url))
                .as_deref(),
            Some("h_chatgpt")
        );
    }

    #[test]
    fn falls_back_to_root_provider_without_base_url_or_providers() {
        let doc = r#"
[features]
codex_hooks = true
"#
        .parse::<DocumentMut>()
        .unwrap();
        assert_eq!(detect_provider_id(&doc).unwrap(), ROOT_PROVIDER_ID);
        assert_eq!(provider_base_url(&doc, ROOT_PROVIDER_ID), None);
    }

    #[test]
    fn detects_chatgpt_auth_mode() {
        let auth = serde_json::json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": "chatgpt-access-token",
                "refresh_token": "refresh-token"
            }
        });
        assert_eq!(detect_auth_mode(Some(&auth)), CodexAuthMode::ChatGpt);
    }

    #[test]
    fn detects_official_openai_provider_from_default_base_url() {
        let snapshot = CodexRouteState {
            provider_id: ROOT_PROVIDER_ID.to_string(),
            real_base_url: DEFAULT_OPENAI_BASE_URL.to_string(),
            auth_mode: CodexAuthMode::ApiKey,
            had_chatgpt_base_url: false,
        };

        assert!(codex_snapshot_uses_official_provider(&snapshot));
    }

    #[test]
    fn does_not_mark_openai_compatible_provider_as_official() {
        let snapshot = CodexRouteState {
            provider_id: "openai".to_string(),
            real_base_url: "https://api.example.com/v1".to_string(),
            auth_mode: CodexAuthMode::ApiKey,
            had_chatgpt_base_url: false,
        };

        assert!(!codex_snapshot_uses_official_provider(&snapshot));
    }

    #[test]
    fn migrates_legacy_registry_without_secrets() {
        let legacy = serde_json::json!({
            "handles": [{
                "id": "h_1",
                "realBaseUrl": "https://chatgpt.com/backend-api/codex",
                "providerId": "__root__",
                "originalSnapshot": {
                    "providerId": "__root__",
                    "realBaseUrl": "https://chatgpt.com/backend-api/codex",
                    "authMode": "chat_gpt"
                },
                "createdAtMs": 1,
                "lastSeenAtMs": 2,
                "lastUsedAtMs": 3
            }]
        });
        let migrated = migrate_legacy_registry_data(&legacy.to_string()).unwrap();
        assert_eq!(migrated.handles.len(), 1);
        assert_eq!(
            migrated.handles[0].route_state.auth_mode,
            CodexAuthMode::ChatGpt
        );
    }

    #[test]
    fn extracts_codex_source_id() {
        assert_eq!(
            CodexConfigManager::extract_source_id_from_proxy_url(
                "http://127.0.0.1:18765/codex/source/h_123/v1"
            )
            .as_deref(),
            Some("h_123")
        );
    }

    #[test]
    fn does_not_treat_any_local_v1_as_usagemeter_without_port_context() {
        assert!(!CodexConfigManager::is_usagemeter_proxy_url(
            "http://localhost:11434/v1"
        ));
        assert!(!CodexConfigManager::is_usagemeter_proxy_url(
            "http://127.0.0.1:11434/v1/chat/completions"
        ));
        assert!(CodexConfigManager::is_usagemeter_proxy_url(
            "http://127.0.0.1:18765/codex/source/h_123/v1"
        ));
    }

    #[test]
    fn local_openai_proxy_url_requires_matching_usagemeter_port() {
        assert!(CodexConfigManager::is_usagemeter_proxy_url_for_port(
            "http://127.0.0.1:18765/v1",
            18765
        ));
        assert!(CodexConfigManager::is_usagemeter_proxy_url_for_port(
            "http://localhost:18765/v1/responses",
            18765
        ));
        assert!(!CodexConfigManager::is_usagemeter_proxy_url_for_port(
            "http://localhost:11434/v1",
            18765
        ));
    }
}
