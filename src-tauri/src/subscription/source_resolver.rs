//! 多工具「已配置来源」凭据解析。
//!
//! 逐个工具解析其**已配置真实上游 base_url + 完整 api_key**，供第三方中转
//! 额度查询使用。核心约束（见 `doc/多工具来源额度查询设计.md`）：
//!
//! - 只读、即时：key 从工具自有配置/凭据文件即时读取，调用后随 `ResolvedRelaySource`
//!   生命周期结束而丢弃，**绝不持久化**。
//! - 静默降级：任一步缺失/异常 → 该工具不产生来源，不抛错、不影响其它工具。
//! - 接管只改 base_url：真实上游 key 始终留在工具自身配置里，可即时读回。
//!
//! 覆盖范围：Claude Code、Codex(ApiKey)、OpenCode(尽力)。Reasonix 因 key 仅存在于
//! 环境变量、磁盘无完整密钥，**不在此解析**。

use std::path::PathBuf;

use serde_json::Value;

/// 工具种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    ClaudeCode,
    Codex,
    OpenCode,
}

impl ToolKind {
    /// 与前端 i18n / 展示约定一致的稳定标识。
    pub fn id(self) -> &'static str {
        match self {
            ToolKind::ClaudeCode => "claude-code",
            ToolKind::Codex => "codex",
            ToolKind::OpenCode => "opencode",
        }
    }
}

/// 解析出的「已配置来源」真实上游凭据。`api_key` 为完整密钥，用完即弃。
pub struct ResolvedRelaySource {
    pub tool: ToolKind,
    pub base_url: String,
    pub api_key: String,
}

/// 汇总所有可解析工具的已配置来源（Claude Code 0..1 + Codex 0..1 + OpenCode 0..N）。
pub fn resolve_all_relay_sources() -> Vec<ResolvedRelaySource> {
    let mut out = Vec::new();
    if let Some(src) = resolve_claude_code() {
        out.push(src);
    }
    if let Some(src) = resolve_codex() {
        out.push(src);
    }
    out.extend(resolve_opencode());
    out
}

/// Claude Code：`settings.json` 的 base_url + ANTHROPIC_API_KEY；被接管则经 source
/// registry 还原真实上游。
///
/// `ANTHROPIC_BASE_URL` 缺失时，不猜测历史 registry 来源。
/// 这代表“当前配置无法证明是第三方中继”，应按官方/不可判定处理，避免把用户已切回
/// 官方 Claude 的场景误判为旧中继来源。
fn resolve_claude_code() -> Option<ResolvedRelaySource> {
    use crate::proxy::source_registry::ProxySourceRegistry;
    use crate::proxy::ClaudeConfigManager;

    let manager = ClaudeConfigManager::new();
    let settings = manager.read_settings().ok()?;
    let api_key = settings.get_api_key().filter(|k| !k.trim().is_empty())?;
    let base_url = resolve_claude_base_url(settings.get_base_url(), |source_id| {
        ProxySourceRegistry::new()
            .get(source_id)
            .map(|h| h.real_base_url)
    })?;

    Some(ResolvedRelaySource {
        tool: ToolKind::ClaudeCode,
        base_url,
        api_key,
    })
}

fn resolve_claude_base_url<F>(configured: Option<String>, lookup_source: F) -> Option<String>
where
    F: FnOnce(&str) -> Option<String>,
{
    let configured = configured?;
    match crate::proxy::ClaudeConfigManager::extract_source_id_from_proxy_url(&configured) {
        Some(source_id) => lookup_source(&source_id),
        None => Some(configured),
    }
}

/// Codex(ApiKey)：`config.toml` provider base_url + `auth.json` OPENAI_API_KEY；被接管
/// 则经 codex source registry 还原。ChatGPT(OAuth) 模式走官方链，此处返回 None。
fn resolve_codex() -> Option<ResolvedRelaySource> {
    use crate::proxy::{CodexAuthMode, CodexConfigManager, CodexSourceRegistry};

    let manager = CodexConfigManager::new();
    let snapshot = manager.read_live_snapshot().ok()?;
    if snapshot.auth_mode == CodexAuthMode::ChatGpt {
        return None; // 官方 OAuth，由 GPT 订阅链覆盖
    }

    let base_url = match manager.active_source_id() {
        Some(source_id) => CodexSourceRegistry::new().get(&source_id)?.real_base_url,
        None => snapshot.real_base_url,
    };
    let api_key = manager.read_api_key()?;

    Some(ResolvedRelaySource {
        tool: ToolKind::Codex,
        base_url,
        api_key,
    })
}

/// OpenCode：遍历已显式配置 baseURL 的 provider，逐个还原真实上游并从 OpenCode 自有
/// auth 库读取该 provider 的 api key；若 auth 库无条目，则回退读取合并后配置中的
/// `provider.<id>.options.apiKey`。任一 provider 缺 key 即跳过（不影响其它）。
fn resolve_opencode() -> Vec<ResolvedRelaySource> {
    use crate::proxy::{OpenCodeConfigManager, OpenCodeSourceRegistry};

    let manager = OpenCodeConfigManager::new();
    let Ok(snapshot) = manager.read_live_snapshot() else {
        return Vec::new();
    };
    let auth = read_opencode_auth_store();

    let mut out = Vec::new();
    for provider in snapshot.providers {
        let base_url = match OpenCodeConfigManager::extract_provider_and_source_from_proxy_url(
            &provider.original_base_url,
        ) {
            Some((_, source_id)) => match OpenCodeSourceRegistry::new().get(&source_id) {
                Some(handle) => handle.real_base_url,
                None => continue,
            },
            None => provider.original_base_url.clone(),
        };

        let api_key = auth
            .as_ref()
            .and_then(|auth_json| parse_opencode_auth_api_key(auth_json, &provider.provider_id))
            .or_else(|| {
                manager
                    .read_provider_api_key(&provider.provider_id)
                    .ok()
                    .flatten()
            });
        let Some(api_key) = api_key else {
            continue;
        };

        out.push(ResolvedRelaySource {
            tool: ToolKind::OpenCode,
            base_url,
            api_key,
        });
    }
    out
}

/// 读取 OpenCode 凭据库 `auth.json`（`$XDG_DATA_HOME/opencode` 或 `~/.local/share/opencode`）。
fn read_opencode_auth_store() -> Option<Value> {
    let dir = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".local")
                .join("share")
        })
        .join("opencode")
        .join("auth.json");
    let content = std::fs::read_to_string(dir).ok()?;
    serde_json::from_str(&content).ok()
}

/// 从 OpenCode auth 库按 provider id 取完整 api key（仅 `type == "api"`）。纯函数，便于单测。
fn parse_opencode_auth_api_key(auth: &Value, provider_id: &str) -> Option<String> {
    let entry = auth.get(provider_id)?;
    // 仅接受 api 类型凭据；oauth 类型无静态 key，交给各自官方链处理。
    if entry.get("type").and_then(Value::as_str) != Some("api") {
        return None;
    }
    entry
        .get("key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_kind_ids_are_stable() {
        assert_eq!(ToolKind::ClaudeCode.id(), "claude-code");
        assert_eq!(ToolKind::Codex.id(), "codex");
        assert_eq!(ToolKind::OpenCode.id(), "opencode");
    }

    #[test]
    fn parses_opencode_auth_api_key_for_api_type() {
        let auth = serde_json::json!({
            "deepseek": { "type": "api", "key": "sk-abc123" },
        });
        assert_eq!(
            parse_opencode_auth_api_key(&auth, "deepseek").as_deref(),
            Some("sk-abc123")
        );
    }

    #[test]
    fn skips_oauth_and_unknown_providers() {
        let auth = serde_json::json!({
            "anthropic": { "type": "oauth", "access": "tok", "refresh": "r" },
            "deepseek": { "type": "api", "key": "  " },
        });
        // oauth 无静态 key
        assert_eq!(parse_opencode_auth_api_key(&auth, "anthropic"), None);
        // 空白 key 视为缺失
        assert_eq!(parse_opencode_auth_api_key(&auth, "deepseek"), None);
        // 不存在的 provider
        assert_eq!(parse_opencode_auth_api_key(&auth, "missing"), None);
    }

    #[test]
    fn requires_explicit_api_type() {
        // 缺 type 字段不应误判为 api
        let auth = serde_json::json!({ "x": { "key": "sk-x" } });
        assert_eq!(parse_opencode_auth_api_key(&auth, "x"), None);
    }

    #[test]
    fn claude_missing_base_url_does_not_fallback_to_registry() {
        let base_url = resolve_claude_base_url(None, |_source_id| {
            Some("https://relay.example.com/v1".to_string())
        });
        assert_eq!(base_url, None);
    }

    #[test]
    fn claude_direct_base_url_is_used_as_is() {
        let base_url = resolve_claude_base_url(
            Some("https://relay.example.com/v1".to_string()),
            |_source_id| None,
        );
        assert_eq!(base_url.as_deref(), Some("https://relay.example.com/v1"));
    }
}
