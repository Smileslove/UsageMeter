use super::LocalUsageDatabase;
use crate::models::{
    AppSettings, OFFICIAL_ANTHROPIC_CLAUDE_OAUTH_SOURCE_ID, OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID,
    OFFICIAL_OPENAI_OAUTH_SOURCE_ID,
};
use crate::proxy::{
    ClaudeConfigManager, CodexAuthMode, CodexConfigManager, GeminiConfigManager,
    OpenCodeConfigManager, OpenCodeSourceRegistry,
};
use crate::subscription::{ClaudeSubscriptionProvider, GeminiSubscriptionProvider};
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassiveAttributionInterval {
    pub tool: String,
    pub provider_id: String,
    pub base_url: String,
    pub auth_mode: String,
    /// SHA-256 identifier of the configured credential; the secret itself is never persisted.
    pub credential_id: String,
    /// Resolved only when the current configuration maps to exactly one configured source.
    pub source_id: Option<String>,
    /// First-party OAuth plan confirmed by its quota endpoint, never a credential.
    pub plan_type: Option<String>,
    pub valid_from_ms: i64,
    pub confirmed_until_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualAttributionOverride {
    pub request_key: String,
    pub source_id: Option<String>,
}

pub(crate) struct PassiveAttributionSnapshot<'a> {
    pub(crate) tool: &'a str,
    pub(crate) provider_id: &'a str,
    pub(crate) base_url: &'a str,
    pub(crate) auth_mode: &'a str,
    pub(crate) credential_id: &'a str,
    pub(crate) source_id: Option<&'a str>,
    pub(crate) plan_type: Option<&'a str>,
    /// True only for a plan value returned by an official quota endpoint. Config observations
    /// leave this false so they can retain the last confirmed plan for the same OAuth route.
    pub(crate) plan_is_confirmed: bool,
    pub(crate) observed_at_ms: i64,
}

fn normalize_openai_oauth_plan_type(plan_type: Option<&str>) -> Option<String> {
    let plan_type = plan_type?.trim().to_ascii_lowercase();
    matches!(
        plan_type.as_str(),
        "free" | "go" | "plus" | "pro" | "business" | "enterprise" | "edu"
    )
    .then_some(plan_type)
}

fn normalize_gemini_oauth_plan_type(plan_type: Option<&str>) -> Option<String> {
    let plan_type = plan_type?.trim().to_ascii_lowercase();
    matches!(
        plan_type.as_str(),
        "free" | "legacy" | "standard" | "pro" | "ultra" | "enterprise"
    )
    .then_some(plan_type)
}

fn normalize_oauth_plan_type(auth_mode: &str, plan_type: Option<&str>) -> Option<String> {
    match auth_mode {
        "chatgpt_oauth" => normalize_openai_oauth_plan_type(plan_type),
        "gemini_oauth" => normalize_gemini_oauth_plan_type(plan_type),
        _ => None,
    }
}

fn is_first_party_oauth_mode(auth_mode: &str) -> bool {
    matches!(auth_mode, "chatgpt_oauth" | "gemini_oauth")
}

const GEMINI_OAUTH_BASE_URL: &str = "https://cloudcode-pa.googleapis.com";

/// OAuth wins only when readable configuration contains no API-key or custom-route signal.
/// Mixed or environment-overridden setups intentionally remain unattributed.
fn gemini_oauth_is_eligible() -> bool {
    let manager = GeminiConfigManager::new();
    let Ok(route) = manager.read_live_snapshot() else {
        return false;
    };
    !route.had_base_url
        && !manager.has_api_key_configuration()
        && !GeminiConfigManager::is_usagemeter_proxy_url(&route.real_base_url)
        && GeminiSubscriptionProvider::new().has_gemini_oauth()
}

fn normalize_base_url(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn credential_id(api_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(api_key.trim().as_bytes());
    format!("{:x}", hasher.finalize())
}

fn snapshot_digest(
    provider_id: &str,
    base_url: &str,
    auth_mode: &str,
    credential_id: &str,
    source_id: Option<&str>,
    plan_type: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(provider_id.trim().as_bytes());
    hasher.update(b"\n");
    hasher.update(base_url.as_bytes());
    hasher.update(b"\n");
    hasher.update(auth_mode.as_bytes());
    hasher.update(b"\n");
    hasher.update(credential_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(source_id.unwrap_or_default().as_bytes());
    hasher.update(b"\n");
    hasher.update(plan_type.unwrap_or_default().as_bytes());
    format!("{:x}", hasher.finalize())
}

fn normalized_source_base_url(base_url: Option<&str>) -> Option<String> {
    let base_url = base_url.map(normalize_base_url)?;
    let official_anthropic = base_url == "https://api.anthropic.com"
        || base_url == "https://api.anthropic.com/v1"
        || base_url == "api.anthropic.com";
    (!official_anthropic).then_some(base_url)
}

/// The app cannot inspect an arbitrary Claude Code process's environment.
/// A readable override in UsageMeter's own environment is nevertheless a
/// definite conflicting signal, so leave the request unattributed.
fn has_claude_environment_override() -> bool {
    claude_environment_override_from(|name| std::env::var(name).ok())
}

fn claude_environment_override_from(read_var: impl Fn(&str) -> Option<String>) -> bool {
    ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"]
        .iter()
        .any(|name| read_var(name).is_some_and(|value| !value.trim().is_empty()))
        || read_var("ANTHROPIC_BASE_URL")
            .filter(|value| !value.trim().is_empty())
            .is_some_and(|value| normalized_source_base_url(Some(&value)).is_some())
}

fn source_id_for_config(settings: &AppSettings, base_url: &str, api_key: &str) -> Option<String> {
    let key_prefix: String = api_key.trim().chars().take(12).collect();
    if key_prefix.is_empty() {
        return None;
    }
    let expected_base_url = normalized_source_base_url(Some(base_url));
    let mut matches = settings.source_aware.sources.iter().filter(|source| {
        source
            .api_key_prefixes
            .iter()
            .any(|prefix| prefix == &key_prefix)
            && normalized_source_base_url(source.base_url.as_deref()) == expected_base_url
    });
    let source = matches.next()?;
    matches.next().is_none().then(|| source.id.clone())
}

fn register_direct_source(
    settings: &AppSettings,
    api_key: &str,
    base_url: &str,
) -> (AppSettings, Option<String>) {
    let mut effective_settings =
        crate::settings::load_settings_blocking().unwrap_or_else(|_| settings.clone());
    crate::proxy::register_source_to_settings(&mut effective_settings, api_key, base_url);
    let source_id = source_id_for_config(&effective_settings, base_url, api_key);
    (effective_settings, source_id)
}

impl LocalUsageDatabase {
    pub(super) fn create_passive_attribution_tables(
        conn: &rusqlite::Connection,
    ) -> Result<(), String> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS passive_attribution_intervals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tool TEXT NOT NULL,
                config_digest TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                base_url TEXT NOT NULL,
                auth_mode TEXT NOT NULL,
                credential_id TEXT NOT NULL,
                source_id TEXT,
                plan_type TEXT,
                valid_from_ms INTEGER NOT NULL,
                confirmed_until_ms INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_passive_attribution_intervals_lookup
                ON passive_attribution_intervals(tool, valid_from_ms, confirmed_until_ms);

            CREATE TABLE IF NOT EXISTS passive_attribution_overrides (
                request_key TEXT PRIMARY KEY,
                source_id TEXT,
                updated_at_ms INTEGER NOT NULL
            );
            "#,
        )
        .map_err(|error| format!("Failed to create passive attribution tables: {error}"))
    }

    /// Observes tool-owned configuration without persisting upstream secrets. A configuration is
    /// eligible only when its readable key and route map to exactly one saved source. Official
    /// OAuth is the exception: it can be a stable first-party source without an API key.
    pub fn observe_passive_attribution(
        &self,
        settings: &AppSettings,
        observed_at_ms: i64,
    ) -> Result<bool, String> {
        let codex_changed = self.observe_codex_passive_attribution(settings, observed_at_ms)?;
        let claude_changed = self.observe_claude_passive_attribution(settings, observed_at_ms)?;
        let gemini_changed = self.observe_gemini_passive_attribution(settings, observed_at_ms)?;
        let opencode_changed =
            self.observe_opencode_passive_attribution(settings, observed_at_ms)?;
        Ok(codex_changed || claude_changed || gemini_changed || opencode_changed)
    }

    fn observe_codex_passive_attribution(
        &self,
        settings: &AppSettings,
        observed_at_ms: i64,
    ) -> Result<bool, String> {
        let manager = CodexConfigManager::new();
        let Ok(snapshot) = manager.read_live_snapshot() else {
            return Ok(false);
        };
        if CodexConfigManager::is_usagemeter_proxy_url(&snapshot.real_base_url) {
            return Ok(false);
        }
        if snapshot.auth_mode == CodexAuthMode::ChatGpt {
            let credential_id = credential_id("openai:chatgpt_oauth");
            return self.record_passive_attribution_snapshot(PassiveAttributionSnapshot {
                tool: "codex",
                provider_id: &snapshot.provider_id,
                base_url: &snapshot.real_base_url,
                auth_mode: "chatgpt_oauth",
                credential_id: &credential_id,
                source_id: Some(OFFICIAL_OPENAI_OAUTH_SOURCE_ID),
                plan_type: None,
                plan_is_confirmed: false,
                observed_at_ms,
            });
        }
        if snapshot.auth_mode != CodexAuthMode::ApiKey {
            return Ok(false);
        }
        let Some(api_key) = manager.read_api_key() else {
            return Ok(false);
        };
        // Direct Codex providers do not pass through UsageMeter, so their source cannot be
        // auto-registered from a proxy request. Register the non-secret identity observed in
        // config.toml and use the updated snapshot for this attribution interval.
        let mut effective_settings = settings.clone();
        let registration = crate::proxy::register_source_to_settings(
            &mut effective_settings,
            &api_key,
            &snapshot.real_base_url,
        );
        if registration.is_new {
            if let Err(error) = crate::settings::save_settings_internal(effective_settings.clone())
            {
                eprintln!("[usagemeter] failed to persist Codex source: {error}");
            }
        }
        let source_id =
            source_id_for_config(&effective_settings, &snapshot.real_base_url, &api_key);
        let credential_id = credential_id(&api_key);
        self.record_passive_attribution_snapshot(PassiveAttributionSnapshot {
            tool: "codex",
            provider_id: &snapshot.provider_id,
            base_url: &snapshot.real_base_url,
            auth_mode: "api_key",
            credential_id: &credential_id,
            source_id: source_id.as_deref(),
            plan_type: None,
            plan_is_confirmed: false,
            observed_at_ms,
        })
    }

    fn observe_claude_passive_attribution(
        &self,
        settings: &AppSettings,
        observed_at_ms: i64,
    ) -> Result<bool, String> {
        let manager = ClaudeConfigManager::new();
        let Ok(claude_settings) = manager.read_settings() else {
            return Ok(false);
        };
        let base_url = claude_settings
            .get_base_url()
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());
        if ClaudeConfigManager::is_usagemeter_proxy_url(&base_url) {
            return Ok(false);
        }
        if let Some(api_key) = claude_settings
            .get_api_key()
            .filter(|api_key| !api_key.trim().is_empty())
        {
            let (effective_settings, source_id) =
                register_direct_source(settings, &api_key, &base_url);
            if effective_settings.source_aware.sources.len() != settings.source_aware.sources.len()
            {
                if let Err(error) = crate::settings::save_settings_internal(effective_settings) {
                    eprintln!("[usagemeter] failed to persist Claude source: {error}");
                }
            }
            let credential_id = credential_id(&api_key);
            return self.record_passive_attribution_snapshot(PassiveAttributionSnapshot {
                tool: "claude_code",
                provider_id: "settings",
                base_url: &base_url,
                auth_mode: "api_key",
                credential_id: &credential_id,
                source_id: source_id.as_deref(),
                plan_type: None,
                plan_is_confirmed: false,
                observed_at_ms,
            });
        }
        if has_claude_environment_override() {
            return Ok(false);
        }
        if normalized_source_base_url(Some(&base_url)).is_some()
            || !ClaudeSubscriptionProvider::new().has_claude_oauth()
        {
            return Ok(false);
        }
        let credential_id = credential_id("anthropic:claude_code_oauth");
        self.record_passive_attribution_snapshot(PassiveAttributionSnapshot {
            tool: "claude_code",
            provider_id: "anthropic",
            base_url: &base_url,
            auth_mode: "claude_oauth",
            credential_id: &credential_id,
            source_id: Some(OFFICIAL_ANTHROPIC_CLAUDE_OAUTH_SOURCE_ID),
            plan_type: None,
            plan_is_confirmed: false,
            observed_at_ms,
        })
    }

    fn observe_gemini_passive_attribution(
        &self,
        settings: &AppSettings,
        observed_at_ms: i64,
    ) -> Result<bool, String> {
        let manager = GeminiConfigManager::new();
        if let Ok(route) = manager.read_live_snapshot() {
            if !GeminiConfigManager::is_usagemeter_proxy_url(&route.real_base_url) {
                if let Some(api_key) = manager.read_api_key() {
                    let (effective_settings, source_id) =
                        register_direct_source(settings, &api_key, &route.real_base_url);
                    if effective_settings.source_aware.sources.len()
                        != settings.source_aware.sources.len()
                    {
                        if let Err(error) =
                            crate::settings::save_settings_internal(effective_settings)
                        {
                            eprintln!("[usagemeter] failed to persist Gemini source: {error}");
                        }
                    }
                    let credential_id = credential_id(&api_key);
                    return self.record_passive_attribution_snapshot(PassiveAttributionSnapshot {
                        tool: "gemini",
                        provider_id: "google",
                        base_url: &route.real_base_url,
                        auth_mode: "api_key",
                        credential_id: &credential_id,
                        source_id: source_id.as_deref(),
                        plan_type: None,
                        plan_is_confirmed: false,
                        observed_at_ms,
                    });
                }
            }
        }
        if !gemini_oauth_is_eligible() {
            return Ok(false);
        }
        let credential_id = credential_id("google:gemini_cli_oauth");
        self.record_passive_attribution_snapshot(PassiveAttributionSnapshot {
            tool: "gemini",
            provider_id: "google",
            base_url: GEMINI_OAUTH_BASE_URL,
            auth_mode: "gemini_oauth",
            credential_id: &credential_id,
            source_id: Some(OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID),
            plan_type: None,
            plan_is_confirmed: false,
            observed_at_ms,
        })
    }

    fn observe_opencode_passive_attribution(
        &self,
        settings: &AppSettings,
        observed_at_ms: i64,
    ) -> Result<bool, String> {
        let manager = OpenCodeConfigManager::new();
        let Ok(snapshot) = manager.read_live_snapshot() else {
            return Ok(false);
        };
        let mut effective_settings =
            crate::settings::load_settings_blocking().unwrap_or_else(|_| settings.clone());
        let mut candidates = Vec::new();
        for provider in snapshot.providers {
            let base_url = match OpenCodeConfigManager::extract_provider_and_source_from_proxy_url(
                &provider.original_base_url,
            ) {
                Some((_, source_id)) => match OpenCodeSourceRegistry::new().get(&source_id) {
                    Some(handle) => handle.real_base_url,
                    None => continue,
                },
                None => provider.original_base_url,
            };
            if OpenCodeConfigManager::is_usagemeter_proxy_url(&base_url) {
                continue;
            }
            let Some(api_key) = manager
                .read_provider_api_key(&provider.provider_id)
                .ok()
                .flatten()
            else {
                continue;
            };
            candidates.push((provider.provider_id, base_url, api_key));
        }
        // Local OpenCode facts do not carry provider identity. Only infer a source when the
        // current config has one API-key provider; otherwise attribution would be ambiguous.
        let Some((provider_id, base_url, api_key)) = (candidates.len() == 1)
            .then(|| candidates.into_iter().next())
            .flatten()
        else {
            return Ok(false);
        };
        let registration =
            crate::proxy::register_source_to_settings(&mut effective_settings, &api_key, &base_url);
        if registration.is_new {
            if let Err(error) = crate::settings::save_settings_internal(effective_settings.clone())
            {
                eprintln!("[usagemeter] failed to persist OpenCode source: {error}");
            }
        }
        let source_id = source_id_for_config(&effective_settings, &base_url, &api_key);
        let credential_id = credential_id(&api_key);
        self.record_passive_attribution_snapshot(PassiveAttributionSnapshot {
            tool: "opencode",
            provider_id: &provider_id,
            base_url: &base_url,
            auth_mode: "api_key",
            credential_id: &credential_id,
            source_id: source_id.as_deref(),
            plan_type: None,
            plan_is_confirmed: false,
            observed_at_ms,
        })
    }

    /// Stores a plan only after the official ChatGPT OAuth usage endpoint has returned it.
    /// No token, account identifier, or refresh credential is persisted.
    pub fn observe_openai_oauth_plan(
        &self,
        plan_type: &str,
        observed_at_ms: i64,
    ) -> Result<bool, String> {
        let plan_type = normalize_openai_oauth_plan_type(Some(plan_type));
        let manager = CodexConfigManager::new();
        let Ok(snapshot) = manager.read_live_snapshot() else {
            return Ok(false);
        };
        if snapshot.auth_mode != CodexAuthMode::ChatGpt
            || CodexConfigManager::is_usagemeter_proxy_url(&snapshot.real_base_url)
        {
            return Ok(false);
        }
        let credential_id = credential_id("openai:chatgpt_oauth");
        self.record_passive_attribution_snapshot(PassiveAttributionSnapshot {
            tool: "codex",
            provider_id: &snapshot.provider_id,
            base_url: &snapshot.real_base_url,
            auth_mode: "chatgpt_oauth",
            credential_id: &credential_id,
            source_id: Some(OFFICIAL_OPENAI_OAUTH_SOURCE_ID),
            plan_type: plan_type.as_deref(),
            plan_is_confirmed: true,
            observed_at_ms,
        })
    }

    /// Stores a Gemini plan only after the official Cloud Code Assist quota endpoint confirms it.
    /// No token, email, project identifier, or refresh credential is persisted.
    pub fn observe_gemini_oauth_plan(
        &self,
        plan_type: &str,
        observed_at_ms: i64,
    ) -> Result<bool, String> {
        let plan_type = normalize_gemini_oauth_plan_type(Some(plan_type));
        if !gemini_oauth_is_eligible() {
            return Ok(false);
        }
        let credential_id = credential_id("google:gemini_cli_oauth");
        self.record_passive_attribution_snapshot(PassiveAttributionSnapshot {
            tool: "gemini",
            provider_id: "google",
            base_url: GEMINI_OAUTH_BASE_URL,
            auth_mode: "gemini_oauth",
            credential_id: &credential_id,
            source_id: Some(OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID),
            plan_type: plan_type.as_deref(),
            plan_is_confirmed: true,
            observed_at_ms,
        })
    }

    /// Records a read-only effective configuration observation. A route remains eligible only
    /// through the last time the same configuration was confirmed; gaps between different
    /// observations intentionally remain unattributed.
    pub(crate) fn record_passive_attribution_snapshot(
        &self,
        snapshot: PassiveAttributionSnapshot<'_>,
    ) -> Result<bool, String> {
        let tool = snapshot.tool.trim();
        let provider_id = snapshot.provider_id.trim();
        let base_url = normalize_base_url(snapshot.base_url);
        let auth_mode = snapshot.auth_mode.trim();
        let credential_id = snapshot.credential_id.trim();
        let source_id = snapshot
            .source_id
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if tool.is_empty()
            || provider_id.is_empty()
            || base_url.is_empty()
            || auth_mode.is_empty()
            || credential_id.is_empty()
        {
            return Err("ERR_PASSIVE_ATTRIBUTION_INVALID_SNAPSHOT".to_string());
        }
        let now = snapshot.observed_at_ms.max(0);
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        let tx = conn
            .unchecked_transaction()
            .map_err(|error| format!("Failed to start passive attribution transaction: {error}"))?;
        let previous = tx
            .query_row(
                "SELECT id, config_digest, provider_id, base_url, auth_mode, credential_id,
                        source_id, plan_type
                 FROM passive_attribution_intervals
                 WHERE tool = ?1 ORDER BY id DESC LIMIT 1",
                [tool],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("Failed to load passive attribution snapshot: {error}"))?;

        let plan_type = normalize_oauth_plan_type(auth_mode, snapshot.plan_type).or_else(|| {
            (!snapshot.plan_is_confirmed)
                .then_some(previous.as_ref())
                .flatten()
                .and_then(
                    |(
                        _,
                        _,
                        previous_provider,
                        previous_base_url,
                        previous_auth_mode,
                        previous_credential_id,
                        previous_source_id,
                        previous_plan_type,
                    )| {
                        (previous_provider == provider_id
                            && previous_base_url == &base_url
                            && previous_auth_mode == auth_mode
                            && previous_credential_id == credential_id
                            && previous_source_id.as_deref() == source_id
                            && is_first_party_oauth_mode(auth_mode))
                        .then(|| previous_plan_type.clone())
                        .flatten()
                    },
                )
        });
        let digest = snapshot_digest(
            provider_id,
            &base_url,
            auth_mode,
            credential_id,
            source_id,
            plan_type.as_deref(),
        );
        let changed = match previous {
            Some((id, previous_digest, ..)) if previous_digest == digest => {
                tx.execute(
                    "UPDATE passive_attribution_intervals
                     SET confirmed_until_ms = MAX(confirmed_until_ms, ?1) WHERE id = ?2",
                    params![now, id],
                )
                .map_err(|error| {
                    format!("Failed to extend passive attribution interval: {error}")
                })?;
                false
            }
            Some((
                id,
                _,
                previous_provider,
                previous_base_url,
                previous_auth_mode,
                previous_credential_id,
                previous_source_id,
                _,
            )) if previous_source_id.is_none()
                && source_id.is_some()
                && previous_provider == provider_id
                && previous_base_url == base_url
                && previous_auth_mode == auth_mode
                && previous_credential_id == credential_id =>
            {
                tx.execute(
                    "UPDATE passive_attribution_intervals
                     SET config_digest = ?1, source_id = ?2,
                         confirmed_until_ms = MAX(confirmed_until_ms, ?3)
                     WHERE id = ?4",
                    params![digest, source_id, now, id],
                )
                .map_err(|error| {
                    format!("Failed to enrich passive attribution interval: {error}")
                })?;
                true
            }
            _ => {
                tx.execute(
                    "INSERT INTO passive_attribution_intervals (
                        tool, config_digest, provider_id, base_url, auth_mode,
                        credential_id, source_id, plan_type, valid_from_ms, confirmed_until_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                    params![
                        tool,
                        digest,
                        provider_id,
                        base_url,
                        auth_mode,
                        credential_id,
                        source_id,
                        plan_type,
                        now
                    ],
                )
                .map_err(|error| format!("Failed to save passive attribution interval: {error}"))?;
                true
            }
        };
        tx.commit()
            .map_err(|error| format!("Failed to commit passive attribution snapshot: {error}"))?;
        Ok(changed)
    }

    pub(crate) fn passive_attribution_intervals_for_range(
        &self,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<Vec<PassiveAttributionInterval>, String> {
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        let mut statement = conn
            .prepare(
                "SELECT tool, provider_id, base_url, auth_mode, credential_id, source_id,
                        plan_type, valid_from_ms, confirmed_until_ms
                 FROM passive_attribution_intervals
                 WHERE confirmed_until_ms >= ?1 AND valid_from_ms <= ?2
                 ORDER BY valid_from_ms ASC, id ASC",
            )
            .map_err(|error| {
                format!("Failed to prepare passive attribution interval query: {error}")
            })?;
        let rows = statement
            .query_map(params![start_ms.max(0), end_ms.max(start_ms)], |row| {
                Ok(PassiveAttributionInterval {
                    tool: row.get(0)?,
                    provider_id: row.get(1)?,
                    base_url: row.get(2)?,
                    auth_mode: row.get(3)?,
                    credential_id: row.get(4)?,
                    source_id: row.get(5)?,
                    plan_type: row.get(6)?,
                    valid_from_ms: row.get(7)?,
                    confirmed_until_ms: row.get(8)?,
                })
            })
            .map_err(|error| format!("Failed to query passive attribution intervals: {error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to read passive attribution interval: {error}"))
    }

    pub(crate) fn manual_attribution_overrides(
        &self,
        request_keys: &[String],
    ) -> Result<Vec<ManualAttributionOverride>, String> {
        if request_keys.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        let mut statement = conn
            .prepare(
                "SELECT request_key, source_id FROM passive_attribution_overrides WHERE request_key = ?1",
            )
            .map_err(|error| format!("Failed to prepare manual attribution query: {error}"))?;
        let mut overrides = Vec::new();
        for request_key in request_keys {
            if request_key.trim().is_empty() {
                continue;
            }
            if let Some(override_row) = statement
                .query_row([request_key], |row| {
                    Ok(ManualAttributionOverride {
                        request_key: row.get(0)?,
                        source_id: row.get(1)?,
                    })
                })
                .optional()
                .map_err(|error| format!("Failed to read manual attribution override: {error}"))?
            {
                overrides.push(override_row);
            }
        }
        Ok(overrides)
    }

    pub fn set_manual_attribution_overrides(
        &self,
        request_keys: &[String],
        source_id: Option<&str>,
        updated_at_ms: i64,
    ) -> Result<u64, String> {
        if request_keys.iter().any(|key| key.starts_with("cursor:")) {
            return Err("cursor_fixed_account_source".into());
        }
        let source_id = source_id.map(str::trim).filter(|value| !value.is_empty());
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        let tx = conn
            .unchecked_transaction()
            .map_err(|error| format!("Failed to start manual attribution transaction: {error}"))?;
        let mut statement = tx
            .prepare(
                "INSERT INTO passive_attribution_overrides (request_key, source_id, updated_at_ms)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(request_key) DO UPDATE SET
                   source_id = excluded.source_id,
                   updated_at_ms = excluded.updated_at_ms",
            )
            .map_err(|error| format!("Failed to prepare manual attribution upsert: {error}"))?;
        let mut changed = 0_u64;
        for request_key in request_keys
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            statement
                .execute(params![request_key, source_id, updated_at_ms.max(0)])
                .map_err(|error| format!("Failed to save manual attribution override: {error}"))?;
            changed += 1;
        }
        drop(statement);
        tx.commit()
            .map_err(|error| format!("Failed to commit manual attribution overrides: {error}"))?;
        Ok(changed)
    }

    pub fn clear_manual_attribution_overrides(
        &self,
        request_keys: &[String],
    ) -> Result<u64, String> {
        let conn = self.conn.lock().unwrap_or_else(|error| error.into_inner());
        let tx = conn
            .unchecked_transaction()
            .map_err(|error| format!("Failed to start manual attribution delete: {error}"))?;
        let mut statement = tx
            .prepare("DELETE FROM passive_attribution_overrides WHERE request_key = ?1")
            .map_err(|error| format!("Failed to prepare manual attribution delete: {error}"))?;
        let mut removed = 0_u64;
        for request_key in request_keys
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            removed += statement
                .execute([request_key])
                .map_err(|error| format!("Failed to delete manual attribution override: {error}"))?
                as u64;
        }
        drop(statement);
        tx.commit()
            .map_err(|error| format!("Failed to commit manual attribution delete: {error}"))?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ApiSource;
    use std::collections::HashMap;

    fn temp_db() -> (tempfile::TempDir, LocalUsageDatabase) {
        let directory = tempfile::tempdir().expect("create temp dir");
        let path = directory.path().join("local_usage.db");
        let db = LocalUsageDatabase::new_with_path(&path).expect("open temp db");
        (directory, db)
    }

    #[test]
    fn readable_claude_environment_overrides_disable_oauth_inference() {
        assert!(claude_environment_override_from(|name| {
            (name == "ANTHROPIC_API_KEY").then(|| "sk-ant-api-key".to_string())
        }));
        assert!(claude_environment_override_from(|name| {
            (name == "ANTHROPIC_AUTH_TOKEN").then(|| "bearer-token".to_string())
        }));
        assert!(claude_environment_override_from(|name| {
            (name == "ANTHROPIC_BASE_URL").then(|| "https://proxy.example.com/v1".to_string())
        }));
        assert!(!claude_environment_override_from(|name| {
            (name == "ANTHROPIC_BASE_URL").then(|| "https://api.anthropic.com".to_string())
        }));
    }

    fn snapshot<'a>(
        tool: &'a str,
        provider_id: &'a str,
        base_url: &'a str,
        credential_id: &'a str,
        source_id: Option<&'a str>,
        observed_at_ms: i64,
    ) -> PassiveAttributionSnapshot<'a> {
        PassiveAttributionSnapshot {
            tool,
            provider_id,
            base_url,
            auth_mode: "api_key",
            credential_id,
            source_id,
            plan_type: None,
            plan_is_confirmed: false,
            observed_at_ms,
        }
    }

    #[test]
    fn route_changes_leave_an_unconfirmed_gap() {
        let (_directory, db) = temp_db();
        assert!(db
            .record_passive_attribution_snapshot(snapshot(
                "codex",
                "openai",
                "https://api.one.example/v1",
                "credential-one",
                Some("source-one"),
                100
            ))
            .expect("record first"));
        assert!(!db
            .record_passive_attribution_snapshot(snapshot(
                "codex",
                "openai",
                "https://api.one.example/v1/",
                "credential-one",
                Some("source-one"),
                200
            ))
            .expect("confirm first"));
        assert!(db
            .record_passive_attribution_snapshot(snapshot(
                "codex",
                "openai",
                "https://api.two.example/v1",
                "credential-two",
                Some("source-two"),
                300
            ))
            .expect("record second"));

        let intervals = db
            .passive_attribution_intervals_for_range(0, 400)
            .expect("load intervals");
        assert_eq!(intervals.len(), 2);
        assert_eq!(intervals[0].confirmed_until_ms, 200);
        assert_eq!(intervals[1].valid_from_ms, 300);
    }

    #[test]
    fn credential_or_source_change_starts_a_new_interval() {
        let (_directory, db) = temp_db();
        db.record_passive_attribution_snapshot(snapshot(
            "claude_code",
            "settings",
            "https://api.example.com",
            "credential-one",
            Some("source-one"),
            100,
        ))
        .expect("record first credential");
        assert!(db
            .record_passive_attribution_snapshot(snapshot(
                "claude_code",
                "settings",
                "https://api.example.com",
                "credential-two",
                Some("source-two"),
                200,
            ))
            .expect("record changed credential"));

        let intervals = db
            .passive_attribution_intervals_for_range(0, 300)
            .expect("load intervals");
        assert_eq!(intervals.len(), 2);
        assert_eq!(intervals[1].source_id.as_deref(), Some("source-two"));
    }

    #[test]
    fn source_resolution_enriches_existing_route_without_moving_interval_start() {
        let (_directory, db) = temp_db();
        db.record_passive_attribution_snapshot(snapshot(
            "codex",
            "custom",
            "https://api.example.com/v1",
            "credential-one",
            None,
            100,
        ))
        .expect("record unresolved route");
        assert!(db
            .record_passive_attribution_snapshot(snapshot(
                "codex",
                "custom",
                "https://api.example.com/v1",
                "credential-one",
                Some("source-one"),
                200,
            ))
            .expect("enrich route source"));

        let intervals = db
            .passive_attribution_intervals_for_range(0, 300)
            .expect("load intervals");
        assert_eq!(intervals.len(), 1);
        assert_eq!(intervals[0].valid_from_ms, 100);
        assert_eq!(intervals[0].confirmed_until_ms, 200);
        assert_eq!(intervals[0].source_id.as_deref(), Some("source-one"));
    }

    #[test]
    fn config_source_binding_requires_one_matching_key_and_route() {
        let mut settings = AppSettings::default();
        let source = |id: &str| ApiSource {
            id: id.to_string(),
            display_name: None,
            base_url: Some("https://api.example.com/v1".to_string()),
            api_key_prefixes: vec!["123456789012".to_string()],
            api_key_notes: HashMap::new(),
            color: "#000000".to_string(),
            icon: None,
            auto_detected: false,
            quota_query: None,
            first_seen_ms: 0,
            last_seen_ms: 0,
        };
        settings.source_aware.sources = vec![source("one")];
        assert_eq!(
            source_id_for_config(&settings, "https://api.example.com/v1/", "123456789012abcd"),
            Some("one".to_string())
        );

        settings.source_aware.sources.push(source("two"));
        assert_eq!(
            source_id_for_config(&settings, "https://api.example.com/v1", "123456789012abcd"),
            None
        );
    }

    #[test]
    fn manual_overrides_round_trip_without_a_source() {
        let (_directory, db) = temp_db();
        db.set_manual_attribution_overrides(
            &["codex:one".to_string(), "codex:two".to_string()],
            None,
            100,
        )
        .expect("save overrides");
        let overrides = db
            .manual_attribution_overrides(&["codex:one".to_string(), "missing".to_string()])
            .expect("read overrides");
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].source_id, None);
        assert_eq!(
            db.clear_manual_attribution_overrides(&["codex:one".to_string()])
                .expect("clear override"),
            1
        );
    }

    #[test]
    fn confirmed_oauth_plan_is_preserved_by_later_config_observations() {
        let (_directory, db) = temp_db();
        let oauth_snapshot = |plan_type, observed_at_ms| PassiveAttributionSnapshot {
            tool: "codex",
            provider_id: "openai",
            base_url: "https://chatgpt.com/backend-api/codex",
            auth_mode: "chatgpt_oauth",
            credential_id: "oauth-credential",
            source_id: Some(OFFICIAL_OPENAI_OAUTH_SOURCE_ID),
            plan_type,
            plan_is_confirmed: plan_type.is_some(),
            observed_at_ms,
        };

        db.record_passive_attribution_snapshot(oauth_snapshot(None, 100))
            .expect("record OAuth configuration");
        db.record_passive_attribution_snapshot(oauth_snapshot(Some("plus"), 200))
            .expect("record confirmed plan");
        assert!(!db
            .record_passive_attribution_snapshot(oauth_snapshot(None, 300))
            .expect("confirm existing OAuth plan"));

        let intervals = db
            .passive_attribution_intervals_for_range(0, 400)
            .expect("load intervals");
        assert_eq!(intervals.len(), 2);
        assert_eq!(intervals[0].plan_type, None);
        assert_eq!(intervals[1].plan_type.as_deref(), Some("plus"));
        assert_eq!(intervals[1].confirmed_until_ms, 300);
    }

    #[test]
    fn confirmed_gemini_plan_is_preserved_by_later_config_observations() {
        let (_directory, db) = temp_db();
        let oauth_snapshot = |plan_type, observed_at_ms| PassiveAttributionSnapshot {
            tool: "gemini",
            provider_id: "google",
            base_url: GEMINI_OAUTH_BASE_URL,
            auth_mode: "gemini_oauth",
            credential_id: "oauth-credential",
            source_id: Some(OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID),
            plan_type,
            plan_is_confirmed: plan_type.is_some(),
            observed_at_ms,
        };

        db.record_passive_attribution_snapshot(oauth_snapshot(None, 100))
            .expect("record OAuth configuration");
        db.record_passive_attribution_snapshot(oauth_snapshot(Some("ultra"), 200))
            .expect("record confirmed plan");
        assert!(!db
            .record_passive_attribution_snapshot(oauth_snapshot(None, 300))
            .expect("confirm existing OAuth plan"));

        let intervals = db
            .passive_attribution_intervals_for_range(0, 400)
            .expect("load intervals");
        assert_eq!(intervals.len(), 2);
        assert_eq!(intervals[1].plan_type.as_deref(), Some("ultra"));
        assert_eq!(intervals[1].confirmed_until_ms, 300);
    }

    #[test]
    fn unknown_confirmed_gemini_plan_clears_a_stale_plan_label() {
        let (_directory, db) = temp_db();
        let oauth_snapshot =
            |plan_type, plan_is_confirmed, observed_at_ms| PassiveAttributionSnapshot {
                tool: "gemini",
                provider_id: "google",
                base_url: GEMINI_OAUTH_BASE_URL,
                auth_mode: "gemini_oauth",
                credential_id: "oauth-credential",
                source_id: Some(OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID),
                plan_type,
                plan_is_confirmed,
                observed_at_ms,
            };

        db.record_passive_attribution_snapshot(oauth_snapshot(Some("pro"), true, 100))
            .expect("record known plan");
        db.record_passive_attribution_snapshot(oauth_snapshot(Some("future-tier"), true, 200))
            .expect("record unknown plan");

        let intervals = db
            .passive_attribution_intervals_for_range(0, 300)
            .expect("load intervals");
        assert_eq!(intervals.len(), 2);
        assert_eq!(intervals[1].plan_type, None);
    }
}
