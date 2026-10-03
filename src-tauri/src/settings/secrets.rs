//! Secret persistence for settings. Config documents contain only references and metadata.

use crate::models::AppSettings;
#[cfg(any(not(target_os = "macos"), test))]
use std::collections::HashMap;
#[cfg(any(not(target_os = "macos"), test))]
use std::fs;
#[cfg(any(not(target_os = "macos"), test))]
use std::io::Write;
#[cfg(any(not(target_os = "macos"), test))]
use std::path::PathBuf;

#[cfg(any(not(target_os = "macos"), test))]
const FILE_NAME: &str = "settings_secrets.json";

#[cfg(all(target_os = "macos", not(test)))]
const KEYCHAIN_SERVICE: &str = "com.usagemeter.settings";

#[cfg(any(not(target_os = "macos"), test))]
fn path() -> Result<PathBuf, String> {
    Ok(crate::utils::usagemeter_dir()?.join(FILE_NAME))
}

#[cfg(any(not(target_os = "macos"), test))]
fn read() -> Result<HashMap<String, String>, String> {
    let path = path()?;
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let raw = fs::read_to_string(path).map_err(|e| format!("ERR_READ_SETTINGS_SECRETS:{e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("ERR_PARSE_SETTINGS_SECRETS:{e}"))
}

#[cfg(any(not(target_os = "macos"), test))]
fn write(values: &HashMap<String, String>) -> Result<(), String> {
    let path = path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("ERR_CREATE_SETTINGS_SECRET_DIR:{e}"))?;
    }
    let raw =
        serde_json::to_string(values).map_err(|e| format!("ERR_SERIALIZE_SETTINGS_SECRETS:{e}"))?;
    let temp = path.with_extension("json.tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .map_err(|e| format!("ERR_WRITE_SETTINGS_SECRETS:{e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("ERR_SET_SETTINGS_SECRET_PERMISSIONS:{e}"))?;
    }
    file.write_all(raw.as_bytes())
        .map_err(|e| format!("ERR_WRITE_SETTINGS_SECRETS:{e}"))?;
    drop(file);
    fs::rename(&temp, &path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        format!("ERR_RENAME_SETTINGS_SECRETS:{e}")
    })?;
    Ok(())
}

#[cfg(all(target_os = "macos", not(test)))]
fn store_secret(key: &str, value: &str) -> Result<(), String> {
    let status = std::process::Command::new("security")
        .args([
            "add-generic-password",
            "-U",
            "-a",
            key,
            "-s",
            KEYCHAIN_SERVICE,
            "-w",
            value,
        ])
        .status()
        .map_err(|e| format!("ERR_SETTINGS_KEYCHAIN_STORE:{e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "ERR_SETTINGS_KEYCHAIN_STORE_FAILED".to_string())
}

#[cfg(all(target_os = "macos", not(test)))]
fn load_secret(key: &str) -> Result<Option<String>, String> {
    let output = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-a",
            key,
            "-s",
            KEYCHAIN_SERVICE,
            "-w",
        ])
        .output()
        .map_err(|e| format!("ERR_SETTINGS_KEYCHAIN_LOAD:{e}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8(output.stdout)
            .map_err(|e| format!("ERR_SETTINGS_KEYCHAIN_UTF8:{e}"))?
            .trim_end()
            .to_string(),
    ))
}

#[cfg(all(target_os = "macos", not(test)))]
fn delete_secret(key: &str) -> Result<(), String> {
    let output = std::process::Command::new("security")
        .args(["delete-generic-password", "-a", key, "-s", KEYCHAIN_SERVICE])
        .output()
        .map_err(|e| format!("ERR_SETTINGS_KEYCHAIN_DELETE:{e}"))?;
    if output.status.success()
        || String::from_utf8_lossy(&output.stderr)
            .to_ascii_lowercase()
            .contains("could not be found")
    {
        Ok(())
    } else {
        Err("ERR_SETTINGS_KEYCHAIN_DELETE_FAILED".to_string())
    }
}

#[cfg(any(not(target_os = "macos"), test))]
fn set(values: &mut HashMap<String, String>, key: String, value: &str) {
    if value.is_empty() {
        values.remove(&key);
    } else {
        values.insert(key, value.to_string());
    }
}

fn gateway_key(profile: &str, key: &str) -> String {
    format!("gateway:{profile}:{key}")
}
fn gateway_local_key(profile: &str, key: &str) -> String {
    format!("gateway-local:{profile}:{key}")
}

/// Move secret fields out of the serializable settings snapshot.
pub fn persist_settings(
    settings: &mut AppSettings,
    previous_settings: &AppSettings,
) -> Result<(), String> {
    #[cfg(all(target_os = "macos", not(test)))]
    {
        persist_secret("sync:webdav_password", &settings.sync.password)?;
        persist_secret("sync:encryption_password", &settings.sync.sync_password)?;
        persist_secret(
            "network_proxy:password",
            settings
                .network_proxy
                .password
                .as_deref()
                .unwrap_or_default(),
        )?;
    }
    #[cfg(any(not(target_os = "macos"), test))]
    let mut values = read()?;
    #[cfg(any(not(target_os = "macos"), test))]
    set(
        &mut values,
        "sync:webdav_password".to_string(),
        &settings.sync.password,
    );
    #[cfg(any(not(target_os = "macos"), test))]
    set(
        &mut values,
        "sync:encryption_password".to_string(),
        &settings.sync.sync_password,
    );
    #[cfg(any(not(target_os = "macos"), test))]
    set(
        &mut values,
        "network_proxy:password".to_string(),
        settings
            .network_proxy
            .password
            .as_deref()
            .unwrap_or_default(),
    );
    settings.sync.password.clear();
    settings.sync.sync_password.clear();
    settings.network_proxy.password = None;

    for profile in &mut settings.gateway.profiles {
        for key in &mut profile.upstream_keys {
            let ref_key = gateway_key(&profile.id, &key.id);
            #[cfg(any(not(target_os = "macos"), test))]
            set(&mut values, ref_key.clone(), &key.secret);
            #[cfg(all(target_os = "macos", not(test)))]
            persist_secret(&ref_key, &key.secret)?;
            key.secret_ref = ref_key;
            key.secret.clear();
        }
        for key in &mut profile.local_keys {
            let ref_key = gateway_local_key(&profile.id, &key.id);
            #[cfg(any(not(target_os = "macos"), test))]
            set(&mut values, ref_key.clone(), &key.secret);
            #[cfg(all(target_os = "macos", not(test)))]
            persist_secret(&ref_key, &key.secret)?;
            key.secret_ref = ref_key;
            key.secret.clear();
        }
    }
    #[cfg(any(not(target_os = "macos"), test))]
    {
        let current_keys: std::collections::HashSet<String> = settings
            .gateway
            .profiles
            .iter()
            .flat_map(|profile| {
                profile
                    .upstream_keys
                    .iter()
                    .map(|key| gateway_key(&profile.id, &key.id))
                    .chain(
                        profile
                            .local_keys
                            .iter()
                            .map(|key| gateway_local_key(&profile.id, &key.id)),
                    )
            })
            .collect();
        for profile in &previous_settings.gateway.profiles {
            for key in &profile.upstream_keys {
                let ref_key = gateway_key(&profile.id, &key.id);
                if !current_keys.contains(&ref_key) {
                    values.remove(&ref_key);
                }
            }
            for key in &profile.local_keys {
                let ref_key = gateway_local_key(&profile.id, &key.id);
                if !current_keys.contains(&ref_key) {
                    values.remove(&ref_key);
                }
            }
        }
        write(&values)
    }
    #[cfg(all(target_os = "macos", not(test)))]
    {
        let current_keys: std::collections::HashSet<String> = settings
            .gateway
            .profiles
            .iter()
            .flat_map(|profile| {
                profile
                    .upstream_keys
                    .iter()
                    .map(|key| gateway_key(&profile.id, &key.id))
                    .chain(
                        profile
                            .local_keys
                            .iter()
                            .map(|key| gateway_local_key(&profile.id, &key.id)),
                    )
            })
            .collect();
        for profile in &previous_settings.gateway.profiles {
            for key in &profile.upstream_keys {
                let ref_key = gateway_key(&profile.id, &key.id);
                if !current_keys.contains(&ref_key) {
                    delete_secret(&ref_key)?;
                }
            }
            for key in &profile.local_keys {
                let ref_key = gateway_local_key(&profile.id, &key.id);
                if !current_keys.contains(&ref_key) {
                    delete_secret(&ref_key)?;
                }
            }
        }
        Ok(())
    }
}

/// Rehydrate secrets for runtime use after loading the non-secret settings document.
pub fn hydrate_settings(settings: &mut AppSettings) -> Result<(), String> {
    #[cfg(all(target_os = "macos", not(test)))]
    {
        settings.sync.password = load_secret("sync:webdav_password")?.unwrap_or_default();
        settings.sync.sync_password = load_secret("sync:encryption_password")?.unwrap_or_default();
        settings.network_proxy.password = load_secret("network_proxy:password")?;
        for profile in &mut settings.gateway.profiles {
            for key in &mut profile.upstream_keys {
                if !key.secret_ref.is_empty() {
                    key.secret = load_secret(&key.secret_ref)?.unwrap_or_default();
                }
            }
            for key in &mut profile.local_keys {
                if !key.secret_ref.is_empty() {
                    key.secret = load_secret(&key.secret_ref)?.unwrap_or_default();
                }
            }
        }
    }
    #[cfg(any(not(target_os = "macos"), test))]
    {
        let values = read()?;
        settings.sync.password = values
            .get("sync:webdav_password")
            .cloned()
            .unwrap_or_default();
        settings.sync.sync_password = values
            .get("sync:encryption_password")
            .cloned()
            .unwrap_or_default();
        if let Some(value) = values.get("network_proxy:password") {
            settings.network_proxy.password = Some(value.clone());
        }
        for profile in &mut settings.gateway.profiles {
            for key in &mut profile.upstream_keys {
                if !key.secret_ref.is_empty() {
                    key.secret = values.get(&key.secret_ref).cloned().unwrap_or_default();
                }
            }
            for key in &mut profile.local_keys {
                if !key.secret_ref.is_empty() {
                    key.secret = values.get(&key.secret_ref).cloned().unwrap_or_default();
                }
            }
        }
    }
    Ok(())
}

#[cfg(all(target_os = "macos", not(test)))]
fn persist_secret(key: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        delete_secret(key)
    } else {
        store_secret(key, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AppSettings, GatewayAuthMode, GatewayDispatchStrategy, GatewayLocalKey, GatewayProfile,
        GatewayProtocol, GatewayUpstreamKey,
    };
    use std::ffi::OsString;
    use tempfile::tempdir;

    fn restore_home(previous: Option<OsString>) {
        match previous {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn persistence_round_trip_keeps_secrets_out_of_settings_snapshot() {
        let _guard = crate::test_support::env_lock();
        let previous_home = std::env::var_os("HOME");
        let dir = tempdir().unwrap();
        std::env::set_var("HOME", dir.path());

        let result = (|| -> Result<(), String> {
            let mut settings = AppSettings::default();
            settings.sync.password = "webdav-secret".to_string();
            settings.sync.sync_password = "encryption-secret".to_string();
            settings.network_proxy.password = Some("proxy-secret".to_string());
            settings.gateway.profiles.push(GatewayProfile {
                id: "profile-1".to_string(),
                name: "Test".to_string(),
                protocol: GatewayProtocol::OpenAiResponses,
                base_url: "https://example.com".to_string(),
                enabled: true,
                client_label: String::new(),
                auth_mode: GatewayAuthMode::ManagedKeys,
                dispatch_strategy: GatewayDispatchStrategy::RoundRobin,
                upstream_keys: vec![GatewayUpstreamKey {
                    id: "upstream-1".to_string(),
                    remark: String::new(),
                    enabled: true,
                    weight: 1,
                    priority: 0,
                    secret: "upstream-secret".to_string(),
                    secret_ref: String::new(),
                }],
                local_keys: vec![GatewayLocalKey {
                    id: "local-1".to_string(),
                    remark: String::new(),
                    enabled: true,
                    secret_hash: "hash".to_string(),
                    secret_salt: "salt".to_string(),
                    secret: "local-secret".to_string(),
                    secret_ref: String::new(),
                    created_at_ms: 1,
                    last_used_at_ms: None,
                    expires_at_ms: None,
                    max_requests: None,
                    request_count: 0,
                }],
                upstream_models: Vec::new(),
            });

            let previous = AppSettings::default();
            persist_settings(&mut settings, &previous)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(path()?)
                    .map_err(|error| error.to_string())?
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o600);
            }
            assert!(settings.sync.password.is_empty());
            assert!(settings.sync.sync_password.is_empty());
            assert!(settings.network_proxy.password.is_none());
            let profile = &settings.gateway.profiles[0];
            assert!(profile.upstream_keys[0].secret.is_empty());
            assert!(profile.local_keys[0].secret.is_empty());

            let serialized = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
            for secret in [
                "webdav-secret",
                "encryption-secret",
                "proxy-secret",
                "upstream-secret",
                "local-secret",
            ] {
                assert!(!serialized.contains(secret));
            }
            let mut database = crate::app_config::AppConfigDatabase::new_with_path(
                &dir.path().join("app_config.db"),
            )?;
            database
                .save_settings(&settings)
                .map_err(|error| error.to_string())?;
            let loaded = database
                .load_settings()
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "missing persisted settings".to_string())?;
            let loaded_serialized = serde_json::to_string(&loaded).map_err(|e| e.to_string())?;
            for secret in [
                "webdav-secret",
                "encryption-secret",
                "proxy-secret",
                "upstream-secret",
                "local-secret",
            ] {
                assert!(!loaded_serialized.contains(secret));
            }

            hydrate_settings(&mut settings)?;
            assert_eq!(settings.sync.password, "webdav-secret");
            assert_eq!(settings.sync.sync_password, "encryption-secret");
            assert_eq!(
                settings.network_proxy.password.as_deref(),
                Some("proxy-secret")
            );
            assert_eq!(
                settings.gateway.profiles[0].upstream_keys[0].secret,
                "upstream-secret"
            );
            assert_eq!(
                settings.gateway.profiles[0].local_keys[0].secret,
                "local-secret"
            );

            let mut cleared = settings.clone();
            cleared.sync.password.clear();
            cleared.sync.sync_password.clear();
            cleared.network_proxy.password = None;
            cleared.gateway.profiles[0].upstream_keys[0].secret.clear();
            cleared.gateway.profiles[0].local_keys[0].secret.clear();
            persist_settings(&mut cleared, &settings)?;
            hydrate_settings(&mut cleared)?;
            assert!(cleared.sync.password.is_empty());
            assert!(cleared.sync.sync_password.is_empty());
            assert!(cleared.network_proxy.password.is_none());
            assert!(cleared.gateway.profiles[0].upstream_keys[0]
                .secret
                .is_empty());
            assert!(cleared.gateway.profiles[0].local_keys[0].secret.is_empty());
            Ok(())
        })();

        restore_home(previous_home);
        result.unwrap();
    }
}
