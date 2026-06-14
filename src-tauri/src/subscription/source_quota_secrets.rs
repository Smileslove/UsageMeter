#[cfg(any(not(target_os = "macos"), test))]
use std::collections::HashMap;
use std::collections::HashSet;

use crate::models::{AppSettings, SourceQuotaBindingConfig};

#[cfg(all(target_os = "macos", not(test)))]
const KEYCHAIN_SERVICE: &str = "UsageMeter.SourceQuota";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SecretKind {
    ManualApiKey,
    ManualAccessToken,
}

impl SecretKind {
    fn suffix(self) -> &'static str {
        match self {
            SecretKind::ManualApiKey => "manual_api_key",
            SecretKind::ManualAccessToken => "manual_access_token",
        }
    }
}

fn secret_account(source_id: &str, kind: SecretKind) -> String {
    format!("source_quota::{source_id}::{}", kind.suffix())
}

#[cfg(any(not(target_os = "macos"), test))]
fn file_store_path() -> Result<std::path::PathBuf, String> {
    Ok(crate::utils::usagemeter_dir()?.join("source_quota_secrets.json"))
}

#[cfg(all(target_os = "macos", not(test)))]
fn store_secret(source_id: &str, kind: SecretKind, value: &str) -> Result<(), String> {
    let status = std::process::Command::new("security")
        .args([
            "add-generic-password",
            "-U",
            "-a",
            &secret_account(source_id, kind),
            "-s",
            KEYCHAIN_SERVICE,
            "-w",
            value,
        ])
        .status()
        .map_err(|e| format!("Failed to invoke macOS Keychain: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Failed to store source quota secret in macOS Keychain: {status}"
        ))
    }
}

#[cfg(all(target_os = "macos", not(test)))]
fn load_secret(source_id: &str, kind: SecretKind) -> Result<Option<String>, String> {
    let output = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-a",
            &secret_account(source_id, kind),
            "-s",
            KEYCHAIN_SERVICE,
            "-w",
        ])
        .output()
        .map_err(|e| format!("Failed to invoke macOS Keychain: {e}"))?;
    if output.status.success() {
        let value = String::from_utf8(output.stdout)
            .map_err(|e| format!("Invalid UTF-8 from macOS Keychain: {e}"))?;
        Ok(Some(value.trim_end_matches('\n').to_string()))
    } else {
        Ok(None)
    }
}

#[cfg(all(target_os = "macos", not(test)))]
fn delete_secret(source_id: &str, kind: SecretKind) -> Result<(), String> {
    let output = std::process::Command::new("security")
        .args([
            "delete-generic-password",
            "-a",
            &secret_account(source_id, kind),
            "-s",
            KEYCHAIN_SERVICE,
        ])
        .output()
        .map_err(|e| format!("Failed to invoke macOS Keychain: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let normalized = stderr.to_ascii_lowercase();
        let not_found = normalized.contains("could not be found")
            || normalized.contains("the specified item could not be found");

        if not_found {
            Ok(())
        } else {
            let account = secret_account(source_id, kind);
            let message = format!(
                "Failed to delete source quota secret from macOS Keychain for {account}: {}",
                stderr.trim()
            );
            eprintln!("[UsageMeter] {message}");
            Err(message)
        }
    }
}

#[cfg(any(not(target_os = "macos"), test))]
fn read_file_store(path: &std::path::Path) -> Result<HashMap<String, String>, String> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read source quota secret store: {e}"))?;
    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse source quota secret store: {e}"))
}

#[cfg(any(not(target_os = "macos"), test))]
fn write_file_store(
    path: &std::path::Path,
    secrets: &HashMap<String, String>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create source quota secret dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(secrets)
        .map_err(|e| format!("Failed to serialize source quota secret store: {e}"))?;
    std::fs::write(path, content)
        .map_err(|e| format!("Failed to write source quota secret store: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(path, permissions);
    }
    Ok(())
}

#[cfg(any(not(target_os = "macos"), test))]
fn store_secret(source_id: &str, kind: SecretKind, value: &str) -> Result<(), String> {
    let path = file_store_path()?;
    let mut secrets = read_file_store(&path)?;
    secrets.insert(secret_account(source_id, kind), value.to_string());
    write_file_store(&path, &secrets)
}

#[cfg(any(not(target_os = "macos"), test))]
fn load_secret(source_id: &str, kind: SecretKind) -> Result<Option<String>, String> {
    let path = file_store_path()?;
    let secrets = read_file_store(&path)?;
    Ok(secrets.get(&secret_account(source_id, kind)).cloned())
}

#[cfg(any(not(target_os = "macos"), test))]
fn delete_secret(source_id: &str, kind: SecretKind) -> Result<(), String> {
    let path = file_store_path()?;
    let mut secrets = read_file_store(&path)?;
    secrets.remove(&secret_account(source_id, kind));
    write_file_store(&path, &secrets)
}

fn hydrate_binding_secrets(
    source_id: &str,
    binding: &mut SourceQuotaBindingConfig,
) -> Result<(), String> {
    if binding.manual_api_key.is_none() {
        binding.manual_api_key = load_secret(source_id, SecretKind::ManualApiKey)?;
    }
    if binding.manual_access_token.is_none() {
        binding.manual_access_token = load_secret(source_id, SecretKind::ManualAccessToken)?;
    }
    Ok(())
}

fn persist_binding_secrets(
    source_id: &str,
    binding: &mut SourceQuotaBindingConfig,
) -> Result<(), String> {
    if let Some(secret) = binding.manual_api_key.clone() {
        store_secret(source_id, SecretKind::ManualApiKey, &secret)?;
        binding.manual_api_key = None;
    } else {
        delete_secret(source_id, SecretKind::ManualApiKey)?;
    }

    if let Some(secret) = binding.manual_access_token.clone() {
        store_secret(source_id, SecretKind::ManualAccessToken, &secret)?;
        binding.manual_access_token = None;
    } else {
        delete_secret(source_id, SecretKind::ManualAccessToken)?;
    }

    Ok(())
}

pub fn hydrate_settings(settings: &mut AppSettings) -> Result<(), String> {
    for source in &mut settings.source_aware.sources {
        if let Some(binding) = &mut source.quota_query {
            hydrate_binding_secrets(&source.id, binding)?;
        }
    }
    Ok(())
}

pub fn persist_settings(
    settings: &mut AppSettings,
    previous_settings: &AppSettings,
) -> Result<(), String> {
    let current_source_ids: HashSet<&str> = settings
        .source_aware
        .sources
        .iter()
        .map(|source| source.id.as_str())
        .collect();

    for previous_source in &previous_settings.source_aware.sources {
        if !current_source_ids.contains(previous_source.id.as_str()) {
            delete_secret(&previous_source.id, SecretKind::ManualApiKey)?;
            delete_secret(&previous_source.id, SecretKind::ManualAccessToken)?;
        }
    }

    for source in &mut settings.source_aware.sources {
        if let Some(binding) = &mut source.quota_query {
            persist_binding_secrets(&source.id, binding)?;
            source.api_key_notes.remove("__quota_api_key");
        } else {
            delete_secret(&source.id, SecretKind::ManualApiKey)?;
            delete_secret(&source.id, SecretKind::ManualAccessToken)?;
            source.api_key_notes.remove("__quota_api_key");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        ApiSource, AppSettings, SourceAwareSettings, SourceCredentialStrategy, SourceQueryProfileId,
    };
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    fn home_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn sample_source(binding: Option<SourceQuotaBindingConfig>) -> ApiSource {
        ApiSource {
            id: "src_demo".to_string(),
            display_name: Some("Demo".to_string()),
            base_url: Some("https://relay.example.com".to_string()),
            api_key_prefixes: vec![],
            api_key_notes: HashMap::new(),
            color: "#000".to_string(),
            icon: None,
            auto_detected: false,
            quota_query: binding,
            first_seen_ms: 0,
            last_seen_ms: 0,
        }
    }

    fn temp_settings(binding: Option<SourceQuotaBindingConfig>) -> AppSettings {
        let mut settings = AppSettings::default();
        settings.source_aware = SourceAwareSettings {
            sources: vec![sample_source(binding)],
            active_source_filter: None,
        };
        settings
    }

    #[test]
    fn persist_settings_scrubs_plain_secrets_from_settings() {
        let _guard = home_env_lock().lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let mut settings = temp_settings(Some(SourceQuotaBindingConfig {
            enabled: true,
            query_profile_id: SourceQueryProfileId::GenericBalanceV1Usage,
            credential_strategy: SourceCredentialStrategy::ManualApiKey,
            manual_api_key: Some("sk-secret".to_string()),
            manual_access_token: Some("tok-secret".to_string()),
            manual_user_id: Some("42".to_string()),
        }));
        settings.source_aware.sources[0]
            .api_key_notes
            .insert("__quota_api_key".to_string(), "sk-secret".to_string());
        let previous = AppSettings::default();
        persist_settings(&mut settings, &previous).unwrap();
        let binding = settings.source_aware.sources[0]
            .quota_query
            .as_ref()
            .unwrap();
        assert!(binding.manual_api_key.is_none());
        assert!(binding.manual_access_token.is_none());
        assert_eq!(binding.manual_user_id.as_deref(), Some("42"));
        assert!(!settings.source_aware.sources[0]
            .api_key_notes
            .contains_key("__quota_api_key"));

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
    }
}
