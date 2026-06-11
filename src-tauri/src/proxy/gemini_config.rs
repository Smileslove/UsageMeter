//! Gemini CLI 配置接管支持。
//!
//! Gemini CLI（API Key 模式）在 `~/.gemini/.env` 读取环境变量，其中
//! `GOOGLE_GEMINI_BASE_URL` 可覆盖 Google Generative Language API 的 endpoint。
//! UsageMeter 仅改写这一个键，指向本地代理
//! `http://127.0.0.1:<port>/usagemeter/gemini/source/<id>`，并把原始 base URL
//! （若有）存入 source handle，停止/恢复时还原；原本没有该键时恢复即删除该行。
//!
//! 不改动 `GEMINI_API_KEY`/`GOOGLE_API_KEY`——Gemini CLI 仍从原处注入密钥，
//! 代理透传认证头。
//!
//! 真实上游默认是 `https://generativelanguage.googleapis.com`。

use super::url_identity;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

const DEFAULT_GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com";
const ENV_KEY: &str = "GOOGLE_GEMINI_BASE_URL";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiRouteState {
    /// 接管前 .env 是否已有 GOOGLE_GEMINI_BASE_URL。
    pub had_base_url: bool,
    /// 接管前的真实 base URL（had_base_url 为 true 时有效）。
    pub real_base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiSourceHandle {
    pub id: String,
    pub real_base_url: String,
    pub route_state: GeminiRouteState,
    pub created_at_ms: i64,
    pub last_seen_at_ms: i64,
    pub last_used_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct GeminiSourceRegistryData {
    #[serde(default)]
    handles: Vec<GeminiSourceHandle>,
}

pub struct GeminiSourceRegistry {
    path: PathBuf,
}

impl GeminiSourceRegistry {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            path: home
                .join(".usagemeter")
                .join("gemini_proxy_source_handles.json"),
        }
    }

    pub fn get(&self, id: &str) -> Option<GeminiSourceHandle> {
        self.read_data()
            .ok()?
            .handles
            .into_iter()
            .find(|handle| handle.id == id)
    }

    pub fn list_handles(&self) -> Vec<GeminiSourceHandle> {
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

    pub fn upsert_from_snapshot(
        &self,
        snapshot: GeminiRouteState,
    ) -> Result<GeminiSourceHandle, String> {
        let real_base_url = if snapshot.had_base_url {
            snapshot.real_base_url.clone()
        } else {
            DEFAULT_GEMINI_BASE_URL.to_string()
        };
        if GeminiConfigManager::is_usagemeter_proxy_url(&real_base_url) {
            return Err(
                "Refusing to register UsageMeter proxy URL as a Gemini upstream".to_string(),
            );
        }

        let id = compute_handle_id(&real_base_url)?;
        let now = now_ms();
        let mut data = self.read_data()?;

        if let Some(existing) = data.handles.iter_mut().find(|handle| handle.id == id) {
            existing.real_base_url = real_base_url;
            existing.route_state = snapshot;
            existing.last_seen_at_ms = now;
            existing.last_used_at_ms = now;
            let handle = existing.clone();
            self.write_data(&data)?;
            return Ok(handle);
        }

        let handle = GeminiSourceHandle {
            id,
            real_base_url,
            route_state: snapshot,
            created_at_ms: now,
            last_seen_at_ms: now,
            last_used_at_ms: now,
        };
        data.handles.push(handle.clone());
        self.write_data(&data)?;
        Ok(handle)
    }

    fn read_data(&self) -> Result<GeminiSourceRegistryData, String> {
        if !self.path.exists() {
            return Ok(GeminiSourceRegistryData::default());
        }
        let content = fs::read_to_string(&self.path)
            .map_err(|e| format!("Failed to read Gemini source registry: {}", e))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse Gemini source registry: {}", e))
    }

    fn write_data(&self, data: &GeminiSourceRegistryData) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create Gemini source registry dir: {}", e))?;
        }
        let content = serde_json::to_string_pretty(data)
            .map_err(|e| format!("Failed to serialize Gemini source registry: {}", e))?;
        fs::write(&self.path, content)
            .map_err(|e| format!("Failed to save Gemini source registry: {}", e))
    }
}

impl Default for GeminiSourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct GeminiConfigManager {
    env_path: PathBuf,
}

impl GeminiConfigManager {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            env_path: home.join(".gemini").join(".env"),
        }
    }

    #[cfg(test)]
    fn new_for_path(env_path: PathBuf) -> Self {
        Self { env_path }
    }

    pub fn config_path(&self) -> &PathBuf {
        &self.env_path
    }

    /// 读取当前 .env 中的真实路由状态（GOOGLE_GEMINI_BASE_URL）。
    pub fn read_live_snapshot(&self) -> Result<GeminiRouteState, String> {
        let content = self.read_env()?;
        match env_get(&content, ENV_KEY) {
            Some(value) if !value.trim().is_empty() => Ok(GeminiRouteState {
                had_base_url: true,
                real_base_url: value,
            }),
            _ => Ok(GeminiRouteState {
                had_base_url: false,
                real_base_url: DEFAULT_GEMINI_BASE_URL.to_string(),
            }),
        }
    }

    /// 将 GOOGLE_GEMINI_BASE_URL 指向本地代理。
    pub fn takeover_with_source(&self, proxy_port: u16, source_id: &str) -> Result<(), String> {
        let proxy_url = url_identity::prefixed_proxy_url(proxy_port, "gemini", source_id, "");
        let content = self.read_env()?;
        let updated = env_set(&content, ENV_KEY, &proxy_url);
        self.write_env(&updated)
    }

    /// 从 source handle 恢复 .env 中的原始 base URL；原本没有则删除该键。
    pub fn restore_from_source(&self, source_id: &str) -> Result<bool, String> {
        let Some(handle) = GeminiSourceRegistry::new().get(source_id) else {
            return Ok(false);
        };
        let content = self.read_env()?;
        let updated = if handle.route_state.had_base_url {
            env_set(&content, ENV_KEY, &handle.route_state.real_base_url)
        } else {
            env_remove(&content, ENV_KEY)
        };
        self.write_env(&updated)?;
        Ok(true)
    }

    pub fn is_takeover_active(&self, proxy_port: u16) -> Result<bool, String> {
        let content = self.read_env()?;
        Ok(env_get(&content, ENV_KEY)
            .map(|base_url| Self::is_usagemeter_proxy_url_for_port(&base_url, proxy_port))
            .unwrap_or(false))
    }

    pub fn active_source_id(&self) -> Option<String> {
        let content = self.read_env().ok()?;
        let base_url = env_get(&content, ENV_KEY)?;
        Self::extract_source_id_from_proxy_url(&base_url)
    }

    pub fn is_usagemeter_proxy_url(base_url: &str) -> bool {
        url_identity::is_usagemeter_proxy_url(base_url, &["gemini"])
    }

    pub fn is_usagemeter_proxy_url_for_port(base_url: &str, proxy_port: u16) -> bool {
        url_identity::is_usagemeter_proxy_url_for_port(base_url, proxy_port, &["gemini"])
    }

    pub fn extract_source_id_from_proxy_url(base_url: &str) -> Option<String> {
        url_identity::extract_source_id_from_proxy_url(base_url, &["gemini"])
    }

    fn read_env(&self) -> Result<String, String> {
        if !self.env_path.exists() {
            return Ok(String::new());
        }
        fs::read_to_string(&self.env_path).map_err(|e| format!("Failed to read Gemini .env: {}", e))
    }

    fn write_env(&self, content: &str) -> Result<(), String> {
        if let Some(parent) = self.env_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create Gemini config directory: {}", e))?;
        }
        fs::write(&self.env_path, content).map_err(|e| format!("Failed to save Gemini .env: {}", e))
    }
}

impl Default for GeminiConfigManager {
    fn default() -> Self {
        Self::new()
    }
}

// === .env 行编辑（保留其他键、注释与空行） ===

fn parse_env_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let without_export = trimmed.strip_prefix("export ").unwrap_or(trimmed);
    let (key, value) = without_export.split_once('=')?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    let value = value.trim().trim_matches('"').trim_matches('\'');
    Some((key.to_string(), value.to_string()))
}

fn env_get(content: &str, key: &str) -> Option<String> {
    content
        .lines()
        .filter_map(parse_env_line)
        .find(|(k, _)| k == key)
        .map(|(_, value)| value)
}

fn env_set(content: &str, key: &str, value: &str) -> String {
    let new_line = format!("{key}={value}");
    let mut replaced = false;
    let mut out: Vec<String> = content
        .lines()
        .map(|line| match parse_env_line(line) {
            Some((k, _)) if k == key => {
                replaced = true;
                new_line.clone()
            }
            _ => line.to_string(),
        })
        .collect();
    if !replaced {
        out.push(new_line);
    }
    finalize_env(out)
}

fn env_remove(content: &str, key: &str) -> String {
    let out: Vec<String> = content
        .lines()
        .filter(|line| match parse_env_line(line) {
            Some((k, _)) => k != key,
            None => true,
        })
        .map(str::to_string)
        .collect();
    finalize_env(out)
}

fn finalize_env(lines: Vec<String>) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut result = lines.join("\n");
    result.push('\n');
    result
}

fn compute_handle_id(real_base_url: &str) -> Result<String, String> {
    let mut hasher = Sha256::new();
    hasher.update(real_base_url.as_bytes());
    let hash = hasher.finalize();
    Ok(format!(
        "gm_{}",
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
    fn env_set_replaces_existing_key_only() {
        let content =
            "GEMINI_API_KEY=secret\nGOOGLE_GEMINI_BASE_URL=https://old.example\n# comment\n";
        let updated = env_set(
            content,
            ENV_KEY,
            "http://127.0.0.1:18765/usagemeter/gemini/source/gm_1",
        );
        assert!(updated.contains("GEMINI_API_KEY=secret"));
        assert!(updated.contains("# comment"));
        assert!(updated.contains(
            "GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:18765/usagemeter/gemini/source/gm_1"
        ));
        assert!(!updated.contains("https://old.example"));
    }

    #[test]
    fn env_set_appends_when_missing() {
        let content = "GEMINI_API_KEY=secret\n";
        let updated = env_set(content, ENV_KEY, "http://proxy");
        assert!(updated.contains("GEMINI_API_KEY=secret"));
        assert!(updated.contains("GOOGLE_GEMINI_BASE_URL=http://proxy"));
    }

    #[test]
    fn env_remove_drops_only_target_key() {
        let content = "GEMINI_API_KEY=secret\nGOOGLE_GEMINI_BASE_URL=http://proxy\n";
        let updated = env_remove(content, ENV_KEY);
        assert!(updated.contains("GEMINI_API_KEY=secret"));
        assert!(!updated.contains("GOOGLE_GEMINI_BASE_URL"));
    }

    #[test]
    fn env_get_reads_quoted_and_export_values() {
        assert_eq!(
            env_get(
                "export GOOGLE_GEMINI_BASE_URL=\"https://x.example\"\n",
                ENV_KEY
            )
            .as_deref(),
            Some("https://x.example")
        );
        assert_eq!(env_get("GEMINI_API_KEY=secret\n", ENV_KEY), None);
    }

    #[test]
    fn takeover_then_restore_round_trips_existing_base_url() {
        let dir = tempfile::tempdir().unwrap();
        let env_path = dir.path().join(".gemini").join(".env");
        fs::create_dir_all(env_path.parent().unwrap()).unwrap();
        fs::write(
            &env_path,
            "GEMINI_API_KEY=secret\nGOOGLE_GEMINI_BASE_URL=https://custom.example\n",
        )
        .unwrap();

        let manager = GeminiConfigManager::new_for_path(env_path.clone());
        let snapshot = manager.read_live_snapshot().unwrap();
        assert!(snapshot.had_base_url);
        assert_eq!(snapshot.real_base_url, "https://custom.example");

        manager.takeover_with_source(18765, "gm_1").unwrap();
        assert!(manager.is_takeover_active(18765).unwrap());
        assert_eq!(manager.active_source_id().as_deref(), Some("gm_1"));

        // 直接构造一次恢复（绕过 registry 文件）。
        let content = fs::read_to_string(&env_path).unwrap();
        let restored = env_set(&content, ENV_KEY, "https://custom.example");
        fs::write(&env_path, restored).unwrap();
        let after = manager.read_live_snapshot().unwrap();
        assert_eq!(after.real_base_url, "https://custom.example");
        // GEMINI_API_KEY 始终保留。
        assert!(fs::read_to_string(&env_path)
            .unwrap()
            .contains("GEMINI_API_KEY=secret"));
    }

    #[test]
    fn takeover_without_existing_base_url_records_default() {
        let dir = tempfile::tempdir().unwrap();
        let env_path = dir.path().join(".gemini").join(".env");
        fs::create_dir_all(env_path.parent().unwrap()).unwrap();
        fs::write(&env_path, "GEMINI_API_KEY=secret\n").unwrap();

        let manager = GeminiConfigManager::new_for_path(env_path.clone());
        let snapshot = manager.read_live_snapshot().unwrap();
        assert!(!snapshot.had_base_url);
        assert_eq!(snapshot.real_base_url, DEFAULT_GEMINI_BASE_URL);

        manager.takeover_with_source(18765, "gm_1").unwrap();
        assert!(manager.is_takeover_active(18765).unwrap());

        // had_base_url=false → 恢复应删除该键。
        let content = fs::read_to_string(&env_path).unwrap();
        let restored = env_remove(&content, ENV_KEY);
        fs::write(&env_path, restored).unwrap();
        assert!(!manager.is_takeover_active(18765).unwrap());
        assert!(fs::read_to_string(&env_path)
            .unwrap()
            .contains("GEMINI_API_KEY=secret"));
    }

    #[test]
    fn extracts_gemini_source_id() {
        assert_eq!(
            GeminiConfigManager::extract_source_id_from_proxy_url(
                "http://127.0.0.1:18765/usagemeter/gemini/source/gm_123"
            )
            .as_deref(),
            Some("gm_123")
        );
        assert_eq!(
            GeminiConfigManager::extract_source_id_from_proxy_url(
                "https://generativelanguage.googleapis.com"
            ),
            None
        );
    }

    #[test]
    fn registry_refuses_proxy_url_as_upstream() {
        let snapshot = GeminiRouteState {
            had_base_url: true,
            real_base_url: "http://127.0.0.1:18765/usagemeter/gemini/source/gm_1".to_string(),
        };
        let result = GeminiSourceRegistry::new().upsert_from_snapshot(snapshot);
        assert!(result.is_err());
    }
}
