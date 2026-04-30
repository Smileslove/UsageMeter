//! Codex CLI configuration takeover support.
//!
//! Codex keeps live configuration in `~/.codex/config.toml` and auth material in
//! `~/.codex/auth.json`. UsageMeter stores a source handle with the original
//! snapshots, then rewrites the selected provider base URL to the local proxy.
//! For ChatGPT/OAuth auth, Codex uses `chatgpt_base_url` and keeps refreshing
//! tokens itself, so UsageMeter leaves `auth.json` intact and only proxies the
//! backend base URL.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use toml_edit::{DocumentMut, Item, Value};

const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const ROOT_PROVIDER_ID: &str = "__root__";
const USAGEMETER_CHATGPT_PROVIDER_ID: &str = "usagemeter_chatgpt";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexConfigSnapshot {
    pub config_toml: String,
    pub auth_json: Option<serde_json::Value>,
    pub provider_id: String,
    pub real_base_url: String,
    pub api_key: Option<String>,
    #[serde(default)]
    pub auth_mode: CodexAuthMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodexAuthMode {
    #[default]
    ApiKey,
    ChatGpt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSourceHandle {
    pub id: String,
    pub real_base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_prefix: Option<String>,
    pub provider_id: String,
    pub original_snapshot: CodexConfigSnapshot,
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
        snapshot: CodexConfigSnapshot,
    ) -> Result<CodexSourceHandle, String> {
        if CodexConfigManager::is_usagemeter_proxy_url(&snapshot.real_base_url) {
            return Err(
                "Refusing to register UsageMeter proxy URL as a Codex upstream".to_string(),
            );
        }

        let storage_snapshot = sanitize_snapshot_for_storage(snapshot);
        let id = compute_handle_id(&storage_snapshot)?;
        let key_prefix = storage_snapshot
            .api_key
            .as_deref()
            .map(|key| extract_key_prefix(key, 12));
        let now = now_ms();
        let mut data = self.read_data()?;

        if let Some(existing) = data.handles.iter_mut().find(|handle| handle.id == id) {
            existing.real_base_url = storage_snapshot.real_base_url.clone();
            existing.api_key = storage_snapshot.api_key.clone();
            existing.api_key_prefix = key_prefix;
            existing.provider_id = storage_snapshot.provider_id.clone();
            existing.original_snapshot = storage_snapshot;
            existing.last_seen_at_ms = now;
            existing.last_used_at_ms = now;
            let handle = existing.clone();
            self.write_data(&data)?;
            return Ok(handle);
        }

        let handle = CodexSourceHandle {
            id,
            real_base_url: storage_snapshot.real_base_url.clone(),
            api_key: storage_snapshot.api_key.clone(),
            api_key_prefix: key_prefix,
            provider_id: storage_snapshot.provider_id.clone(),
            original_snapshot: storage_snapshot,
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
            .map_err(|e| format!("Failed to parse Codex source registry: {}", e))
    }

    fn write_data(&self, data: &CodexSourceRegistryData) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create Codex source registry directory: {}", e))?;
        }
        let content = serde_json::to_string_pretty(data)
            .map_err(|e| format!("Failed to serialize Codex source registry: {}", e))?;
        let temp_path = self.path.with_extension("json.tmp");
        fs::write(&temp_path, content)
            .map_err(|e| format!("Failed to write Codex source registry temp file: {}", e))?;
        fs::rename(&temp_path, &self.path)
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

    pub fn read_live_snapshot(&self) -> Result<CodexConfigSnapshot, String> {
        let config_toml = fs::read_to_string(&self.config_path)
            .map_err(|e| format!("Failed to read Codex config.toml: {}", e))?;
        let doc = config_toml
            .parse::<DocumentMut>()
            .map_err(|e| format!("Failed to parse Codex config.toml: {}", e))?;
        let auth_json = self.read_auth_json()?;
        let auth_mode = detect_auth_mode(auth_json.as_ref());
        let provider_id = detect_provider_id(&doc)?;
        let real_base_url = match auth_mode {
            CodexAuthMode::ChatGpt => {
                chatgpt_base_url(&doc).unwrap_or_else(|| DEFAULT_CHATGPT_CODEX_BASE_URL.to_string())
            }
            CodexAuthMode::ApiKey => provider_base_url(&doc, &provider_id)
                .unwrap_or_else(|| DEFAULT_OPENAI_BASE_URL.to_string()),
        };
        let api_key = extract_api_key(auth_json.as_ref(), &provider_id);

        Ok(CodexConfigSnapshot {
            config_toml,
            auth_json,
            provider_id,
            real_base_url,
            api_key,
            auth_mode,
        })
    }

    pub fn takeover_with_source(&self, proxy_port: u16, source_id: &str) -> Result<(), String> {
        let snapshot = self.read_live_snapshot()?;
        let mut doc = snapshot
            .config_toml
            .parse::<DocumentMut>()
            .map_err(|e| format!("Failed to parse Codex config.toml: {}", e))?;

        // Keep the active source handle in the proxy URL. Once the base URL points
        // at UsageMeter, requests do not otherwise carry enough stable information
        // to recover the original upstream for the current configuration.
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
            CodexAuthMode::ChatGpt => {
                set_chatgpt_base_url(&mut doc, &proxy_url);
                configure_chatgpt_model_provider(&mut doc, &proxy_url);
            }
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
        self.write_config_raw(&handle.original_snapshot.config_toml)?;
        let should_restore_auth = handle.original_snapshot.auth_mode != CodexAuthMode::ChatGpt;
        if should_restore_auth {
            if let Some(auth) = handle.original_snapshot.auth_json {
                self.write_auth_json(&auth)?;
            }
        }
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
            if chatgpt_base_url(&doc)
                .map(|base_url| Self::is_usagemeter_proxy_url_for_port(&base_url, proxy_port))
                .unwrap_or(false)
            {
                return Ok(true);
            }

            return Ok(chatgpt_http_provider_base_url(&doc)
                .map(|base_url| Self::is_usagemeter_proxy_url_for_port(&base_url, proxy_port))
                .unwrap_or(false));
        }

        // 检查 base_url 字段（在 model_providers 中）
        let provider_id = detect_provider_id(&doc)?;
        if let Some(base_url) = provider_base_url(&doc, &provider_id) {
            if Self::is_usagemeter_proxy_url_for_port(&base_url, proxy_port) {
                return Ok(true);
            }
        }

        // 也检查顶层 base_url
        if let Some(base_url) = doc.get("base_url").and_then(|item| item.as_str()) {
            if Self::is_usagemeter_proxy_url_for_port(base_url, proxy_port) {
                return Ok(true);
            }
        }

        Ok(false)
    }

    pub fn is_chatgpt_http_provider_active(&self, proxy_port: u16) -> Result<bool, String> {
        let config_toml = fs::read_to_string(&self.config_path)
            .map_err(|e| format!("Failed to read Codex config.toml: {}", e))?;
        let doc = config_toml
            .parse::<DocumentMut>()
            .map_err(|e| format!("Failed to parse Codex config.toml: {}", e))?;
        let auth_mode = detect_auth_mode(self.read_auth_json()?.as_ref());
        if auth_mode != CodexAuthMode::ChatGpt {
            return Ok(true);
        }

        if doc.get("model_provider").and_then(Item::as_str) != Some(USAGEMETER_CHATGPT_PROVIDER_ID)
        {
            return Ok(false);
        }

        let Some(provider) = doc
            .get("model_providers")
            .and_then(Item::as_table)
            .and_then(|providers| providers.get(USAGEMETER_CHATGPT_PROVIDER_ID))
            .and_then(Item::as_table)
        else {
            return Ok(false);
        };

        let base_url_ok = provider
            .get("base_url")
            .and_then(Item::as_str)
            .map(|base_url| Self::is_usagemeter_proxy_url_for_port(base_url, proxy_port))
            .unwrap_or(false);
        let wire_api_ok = provider.get("wire_api").and_then(Item::as_str) == Some("responses");
        let auth_ok = provider
            .get("requires_openai_auth")
            .and_then(Item::as_bool)
            .unwrap_or(false);
        let websockets_disabled =
            provider.get("supports_websockets").and_then(Item::as_bool) == Some(false);

        Ok(base_url_ok && wire_api_ok && auth_ok && websockets_disabled)
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
        let temp_path = self.config_path.with_extension("toml.tmp");
        fs::write(&temp_path, content)
            .map_err(|e| format!("Failed to write Codex config temp file: {}", e))?;
        fs::rename(&temp_path, &self.config_path)
            .map_err(|e| format!("Failed to save Codex config.toml: {}", e))?;
        Ok(())
    }

    fn write_auth_json(&self, auth: &serde_json::Value) -> Result<(), String> {
        if let Some(parent) = self.auth_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create Codex auth directory: {}", e))?;
        }
        let content = serde_json::to_string_pretty(auth)
            .map_err(|e| format!("Failed to serialize Codex auth.json: {}", e))?;
        let temp_path = self.auth_path.with_extension("json.tmp");
        fs::write(&temp_path, content)
            .map_err(|e| format!("Failed to write Codex auth temp file: {}", e))?;
        fs::rename(&temp_path, &self.auth_path)
            .map_err(|e| format!("Failed to save Codex auth.json: {}", e))?;
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

fn configure_chatgpt_model_provider(doc: &mut DocumentMut, base_url: &str) {
    doc["model_provider"] = Item::Value(Value::from(USAGEMETER_CHATGPT_PROVIDER_ID));

    let providers = doc["model_providers"].or_insert(toml_edit::table());
    let provider = providers[USAGEMETER_CHATGPT_PROVIDER_ID].or_insert(toml_edit::table());
    provider["name"] = Item::Value(Value::from("UsageMeter ChatGPT"));
    provider["base_url"] = Item::Value(Value::from(base_url));
    provider["wire_api"] = Item::Value(Value::from("responses"));
    provider["requires_openai_auth"] = Item::Value(Value::from(true));
    provider["supports_websockets"] = Item::Value(Value::from(false));
}

fn chatgpt_http_provider_base_url(doc: &DocumentMut) -> Option<String> {
    if doc.get("model_provider").and_then(Item::as_str) != Some(USAGEMETER_CHATGPT_PROVIDER_ID) {
        return None;
    }

    doc.get("model_providers")?
        .as_table()?
        .get(USAGEMETER_CHATGPT_PROVIDER_ID)?
        .as_table()?
        .get("base_url")?
        .as_str()
        .map(str::to_string)
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

fn extract_api_key(auth: Option<&serde_json::Value>, provider_id: &str) -> Option<String> {
    let auth = auth?;
    for key in [
        "OPENAI_API_KEY",
        "api_key",
        "apiKey",
        "access_token",
        "token",
    ] {
        if let Some(value) = auth.get(key).and_then(|v| v.as_str()) {
            return Some(value.to_string());
        }
    }
    auth.get("providers")
        .and_then(|v| v.get(provider_id))
        .and_then(|provider| {
            [
                "OPENAI_API_KEY",
                "api_key",
                "apiKey",
                "access_token",
                "token",
            ]
            .iter()
            .find_map(|key| provider.get(key).and_then(|v| v.as_str()))
        })
        .or_else(|| {
            auth.get("tokens")
                .and_then(|tokens| tokens.get("access_token"))
                .and_then(|v| v.as_str())
        })
        .map(str::to_string)
}

fn extract_key_prefix(key: &str, len: usize) -> String {
    key.chars().take(len).collect()
}

fn sanitize_snapshot_for_storage(mut snapshot: CodexConfigSnapshot) -> CodexConfigSnapshot {
    if snapshot.auth_mode != CodexAuthMode::ChatGpt {
        return snapshot;
    }

    let account_id = extract_chatgpt_account_id_from_auth(snapshot.auth_json.as_ref());
    snapshot.api_key = None;
    snapshot.auth_json = Some(match account_id {
        Some(account_id) => serde_json::json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "account_id": account_id
            }
        }),
        None => serde_json::json!({
            "auth_mode": "chatgpt"
        }),
    });
    snapshot
}

fn extract_chatgpt_account_id_from_auth(auth: Option<&serde_json::Value>) -> Option<String> {
    let auth = auth?;
    auth.pointer("/tokens/account_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            auth.pointer("/tokens/access_token")
                .and_then(|value| value.as_str())
                .and_then(extract_chatgpt_account_id_from_jwt)
        })
        .or_else(|| {
            auth.pointer("/tokens/id_token")
                .and_then(|value| value.as_str())
                .and_then(extract_chatgpt_account_id_from_jwt)
        })
}

fn extract_chatgpt_account_id_from_jwt(token: &str) -> Option<String> {
    use base64::Engine;

    let payload = token.split('.').nth(1)?;
    let mut value = payload.replace('-', "+").replace('_', "/");
    while !value.len().is_multiple_of(4) {
        value.push('=');
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value.as_bytes())
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    json.pointer("/https://api.openai.com/auth/chatgpt_account_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn compute_handle_id(snapshot: &CodexConfigSnapshot) -> Result<String, String> {
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
    fn chatgpt_takeover_sets_chatgpt_base_url() {
        let mut doc = r#"
base_url = "https://api.example.com/v1"
"#
        .parse::<DocumentMut>()
        .unwrap();
        set_chatgpt_base_url(&mut doc, "http://127.0.0.1:18765/codex/source/h_1");
        configure_chatgpt_model_provider(&mut doc, "http://127.0.0.1:18765/codex/source/h_1");
        assert_eq!(
            chatgpt_base_url(&doc).as_deref(),
            Some("http://127.0.0.1:18765/codex/source/h_1")
        );
        assert_eq!(
            doc.get("base_url").and_then(Item::as_str),
            Some("https://api.example.com/v1")
        );
        assert_eq!(
            doc.get("model_provider").and_then(Item::as_str),
            Some(USAGEMETER_CHATGPT_PROVIDER_ID)
        );
        let provider = doc
            .get("model_providers")
            .and_then(Item::as_table)
            .and_then(|providers| providers.get(USAGEMETER_CHATGPT_PROVIDER_ID))
            .and_then(Item::as_table)
            .unwrap();
        assert_eq!(
            provider.get("base_url").and_then(Item::as_str),
            Some("http://127.0.0.1:18765/codex/source/h_1")
        );
        assert_eq!(
            provider.get("requires_openai_auth").and_then(Item::as_bool),
            Some(true)
        );
        assert_eq!(
            provider.get("supports_websockets").and_then(Item::as_bool),
            Some(false)
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
    fn detects_chatgpt_http_provider_configuration() {
        let mut doc = r#"
chatgpt_base_url = "http://127.0.0.1:18765/codex/source/h_1"
"#
        .parse::<DocumentMut>()
        .unwrap();
        configure_chatgpt_model_provider(&mut doc, "http://127.0.0.1:18765/codex/source/h_1");
        let provider = doc
            .get("model_providers")
            .and_then(Item::as_table)
            .and_then(|providers| providers.get(USAGEMETER_CHATGPT_PROVIDER_ID))
            .and_then(Item::as_table)
            .unwrap();
        assert_eq!(
            provider.get("supports_websockets").and_then(Item::as_bool),
            Some(false)
        );
        assert_eq!(
            provider.get("requires_openai_auth").and_then(Item::as_bool),
            Some(true)
        );
        assert_eq!(
            provider.get("wire_api").and_then(Item::as_str),
            Some("responses")
        );
    }

    #[test]
    fn detects_chatgpt_http_provider_base_url_without_top_level_base() {
        let mut doc = r#"
chatgpt_base_url = "https://chatgpt.com/backend-api/codex"
"#
        .parse::<DocumentMut>()
        .unwrap();
        configure_chatgpt_model_provider(&mut doc, "http://127.0.0.1:18765/codex/source/h_1");
        doc.remove("chatgpt_base_url");

        assert_eq!(
            chatgpt_http_provider_base_url(&doc).as_deref(),
            Some("http://127.0.0.1:18765/codex/source/h_1")
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
    fn extracts_chatgpt_login_access_token() {
        let auth = serde_json::json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": "chatgpt-access-token",
                "refresh_token": "refresh-token"
            }
        });
        assert_eq!(
            extract_api_key(Some(&auth), ROOT_PROVIDER_ID).as_deref(),
            Some("chatgpt-access-token")
        );
        assert_eq!(detect_auth_mode(Some(&auth)), CodexAuthMode::ChatGpt);
    }

    #[test]
    fn sanitizes_chatgpt_snapshot_before_registry_storage() {
        let snapshot = CodexConfigSnapshot {
            config_toml: "chatgpt_base_url = \"https://chatgpt.com/backend-api/codex\"".to_string(),
            auth_json: Some(serde_json::json!({
                "auth_mode": "chatgpt",
                "tokens": {
                    "account_id": "acct_safe",
                    "access_token": "access-secret",
                    "refresh_token": "refresh-secret",
                    "id_token": "id-secret"
                }
            })),
            provider_id: ROOT_PROVIDER_ID.to_string(),
            real_base_url: DEFAULT_CHATGPT_CODEX_BASE_URL.to_string(),
            api_key: Some("access-secret".to_string()),
            auth_mode: CodexAuthMode::ChatGpt,
        };

        let sanitized = sanitize_snapshot_for_storage(snapshot);
        assert_eq!(sanitized.api_key, None);
        assert_eq!(
            sanitized
                .auth_json
                .as_ref()
                .and_then(|auth| auth.pointer("/tokens/account_id"))
                .and_then(|value| value.as_str()),
            Some("acct_safe")
        );
        let serialized = serde_json::to_string(&sanitized).unwrap();
        assert!(!serialized.contains("access-secret"));
        assert!(!serialized.contains("refresh-secret"));
        assert!(!serialized.contains("id-secret"));
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
