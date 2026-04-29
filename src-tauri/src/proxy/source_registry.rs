//! Persistent source handles for proxy URLs.
//!
//! A handle lets UsageMeter encode the real upstream identity in the local
//! proxy URL without exposing the upstream URL or API key in Claude settings.

use super::config_manager::ClaudeConfigManager;
use super::source_detector::{compute_source_id, extract_key_prefix, normalize_base_url};
use super::types::ClaudeSettings;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxySourceHandle {
    /// Stable handle encoded into the local proxy URL. This identifies a
    /// restorable Claude settings snapshot, not just an analytics source.
    pub id: String,
    /// Analytics source ID based on API key prefix + base URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analytics_source_id: Option<String>,
    pub real_base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_prefix: Option<String>,
    pub original_settings_snapshot: ClaudeSettings,
    pub created_at_ms: i64,
    pub last_seen_at_ms: i64,
    pub last_used_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ProxySourceRegistryData {
    #[serde(default)]
    handles: Vec<ProxySourceHandle>,
}

pub struct ProxySourceRegistry {
    path: PathBuf,
}

impl ProxySourceRegistry {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            path: home.join(".usagemeter").join("proxy_source_handles.json"),
        }
    }

    pub fn get(&self, id: &str) -> Option<ProxySourceHandle> {
        self.read_data()
            .ok()?
            .handles
            .into_iter()
            .find(|handle| handle.id == id)
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

    pub fn upsert_from_settings(
        &self,
        settings: &ClaudeSettings,
    ) -> Result<Option<ProxySourceHandle>, String> {
        let real_base_url = settings
            .get_base_url()
            .unwrap_or_else(|| DEFAULT_ANTHROPIC_BASE_URL.to_string());
        if ClaudeConfigManager::is_usagemeter_proxy_url(&real_base_url) {
            return Err("Refusing to register UsageMeter proxy URL as an upstream".to_string());
        }

        let api_key = settings.get_api_key();
        let key_prefix = api_key
            .as_deref()
            .map(|key| extract_key_prefix(key, 12))
            .unwrap_or_default();
        let normalized_base_url = normalize_base_url(&real_base_url);
        let analytics_source_id = format!(
            "src_{}",
            compute_source_id(&key_prefix, normalized_base_url.as_deref())
        );
        let id = compute_handle_id(settings)?;

        let mut data = self.read_data()?;
        let now = now_ms();
        if let Some(existing) = data.handles.iter_mut().find(|handle| handle.id == id) {
            existing.analytics_source_id = Some(analytics_source_id);
            existing.real_base_url = real_base_url;
            existing.api_key = api_key;
            existing.api_key_prefix = if key_prefix.is_empty() {
                None
            } else {
                Some(key_prefix)
            };
            existing.original_settings_snapshot = settings.clone();
            existing.last_seen_at_ms = now;
            existing.last_used_at_ms = now;
            let handle = existing.clone();
            self.write_data(&data)?;
            Ok(Some(handle))
        } else {
            let handle = ProxySourceHandle {
                id,
                analytics_source_id: Some(analytics_source_id),
                real_base_url,
                api_key,
                api_key_prefix: if key_prefix.is_empty() {
                    None
                } else {
                    Some(key_prefix)
                },
                original_settings_snapshot: settings.clone(),
                created_at_ms: now,
                last_seen_at_ms: now,
                last_used_at_ms: now,
            };
            data.handles.push(handle.clone());
            self.write_data(&data)?;
            Ok(Some(handle))
        }
    }

    fn read_data(&self) -> Result<ProxySourceRegistryData, String> {
        if !self.path.exists() {
            return Ok(ProxySourceRegistryData::default());
        }
        let content = fs::read_to_string(&self.path)
            .map_err(|e| format!("Failed to read proxy source registry: {}", e))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse proxy source registry: {}", e))
    }

    fn write_data(&self, data: &ProxySourceRegistryData) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create proxy source registry directory: {}", e))?;
        }
        let content = serde_json::to_string_pretty(data)
            .map_err(|e| format!("Failed to serialize proxy source registry: {}", e))?;
        let temp_path = self.path.with_extension("json.tmp");
        fs::write(&temp_path, content)
            .map_err(|e| format!("Failed to write proxy source registry temp file: {}", e))?;
        fs::rename(&temp_path, &self.path)
            .map_err(|e| format!("Failed to save proxy source registry: {}", e))?;
        Ok(())
    }
}

impl Default for ProxySourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn compute_handle_id(settings: &ClaudeSettings) -> Result<String, String> {
    let snapshot = serde_json::to_vec(settings)
        .map_err(|e| format!("Failed to serialize proxy source handle snapshot: {}", e))?;
    let mut hasher = Sha256::new();
    hasher.update(&snapshot);
    let hash = hasher.finalize();
    Ok(format!(
        "h_{}",
        u64::from_be_bytes(hash[..8].try_into().unwrap())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with_hook(command: &str) -> ClaudeSettings {
        let mut settings = ClaudeSettings::default();
        settings.env.insert(
            "ANTHROPIC_BASE_URL".to_string(),
            serde_json::Value::String("https://api.example.com/v1".to_string()),
        );
        settings.env.insert(
            "ANTHROPIC_API_KEY".to_string(),
            serde_json::Value::String("sk-test-key".to_string()),
        );
        settings.other.insert(
            "hooks".to_string(),
            serde_json::json!({ "PostToolUse": [{ "command": command }] }),
        );
        settings
    }

    #[test]
    fn test_handle_id_includes_full_settings_snapshot() {
        let first = settings_with_hook("first.sh");
        let second = settings_with_hook("second.sh");

        assert_ne!(
            compute_handle_id(&first).unwrap(),
            compute_handle_id(&second).unwrap()
        );
    }
}
