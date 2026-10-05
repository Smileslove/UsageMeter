//! Read-only adapters for explicitly configured Harness API-key routes.
//! Never execute Cordis plugins or expose credential/parser error contents.
use super::deepseek_harness_reader::{root_tag, session_root};
use super::meta::LocalProviderEvidence;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_ENTRIES: usize = 256;

pub(crate) struct ApiKeyRoute {
    pub evidence: LocalProviderEvidence,
    pub base_url: String,
    pub api_key: String,
}

pub(crate) fn valid_provider_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn read_yaml(path: &Path) -> Result<Value, String> {
    let metadata = std::fs::metadata(path).map_err(|_| "ERR_DEEPSEEK_CONFIG_UNREADABLE")?;
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err("ERR_DEEPSEEK_CONFIG_TOO_LARGE".into());
    }
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|_| "ERR_DEEPSEEK_CONFIG_UNREADABLE")?;
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "ERR_DEEPSEEK_CONFIG_UNREADABLE")?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err("ERR_DEEPSEEK_CONFIG_TOO_LARGE".into());
    }
    let yaml: serde_yaml_ng::Value =
        serde_yaml_ng::from_slice(&bytes).map_err(|_| "ERR_DEEPSEEK_CONFIG_INVALID")?;
    serde_json::to_value(yaml).map_err(|_| "ERR_DEEPSEEK_CONFIG_INVALID".into())
}

fn safe_base_url(value: &str) -> Option<String> {
    if value.len() > 2048 || value.contains("${") {
        return None;
    }
    let url = reqwest::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || matches!(url.host_str()?, "localhost" | "127.0.0.1" | "[::1]")
    {
        return None;
    }
    if url
        .host_str()?
        .trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
        .ok()
        .is_some_and(|ip| ip.is_loopback() || ip.is_unspecified())
    {
        return None;
    }
    Some(url.as_str().trim_end_matches('/').to_string())
}

// Ordinary root entries and inserted rows establish identity. Named patches may only
// replace fields of an established row; config is replaced wholesale by Cordis.
fn validate_entry(object: &serde_json::Map<String, Value>) -> Result<(), String> {
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "id" | "name" | "config" | "disabled"))
        || object
            .get("disabled")
            .is_some_and(|value| !value.is_boolean())
    {
        return Err("ERR_DEEPSEEK_CONFIG_UNSUPPORTED".into());
    }
    Ok(())
}

fn insert_rows(value: &Value, rows: &mut BTreeMap<String, Value>) -> Result<(), String> {
    let entries = value.as_array().ok_or("ERR_DEEPSEEK_CONFIG_UNSUPPORTED")?;
    if entries.len() > MAX_ENTRIES {
        return Err("ERR_DEEPSEEK_CONFIG_TOO_LARGE".into());
    }
    for entry in entries {
        let object = entry.as_object().ok_or("ERR_DEEPSEEK_CONFIG_UNSUPPORTED")?;
        validate_entry(object)?;
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or("ERR_DEEPSEEK_CONFIG_UNSUPPORTED")?;
        if rows.contains_key(id) {
            return Err("ERR_DEEPSEEK_CONFIG_UNSUPPORTED".into());
        }
        rows.insert(id.to_string(), entry.clone());
        if rows.len() > MAX_ENTRIES {
            return Err("ERR_DEEPSEEK_CONFIG_TOO_LARGE".into());
        }
    }
    Ok(())
}

fn compose_rows(value: &Value, rows: &mut BTreeMap<String, Value>) -> Result<(), String> {
    let entries = value.as_array().ok_or("ERR_DEEPSEEK_CONFIG_UNSUPPORTED")?;
    if entries.len() > MAX_ENTRIES {
        return Err("ERR_DEEPSEEK_CONFIG_TOO_LARGE".into());
    }
    for entry in entries {
        let object = entry.as_object().ok_or("ERR_DEEPSEEK_CONFIG_UNSUPPORTED")?;
        if let Some(insert) = object.get("insert") {
            if object.len() != 1 {
                return Err("ERR_DEEPSEEK_CONFIG_UNSUPPORTED".into());
            }
            insert_rows(insert, rows)?;
            continue;
        }
        validate_entry(object)?;
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .ok_or("ERR_DEEPSEEK_CONFIG_UNSUPPORTED")?;
        // Bundled entries that cannot be read from this root are not proof of a live route.
        let Some(row) = rows.get_mut(id) else {
            continue;
        };
        if object
            .get("name")
            .is_some_and(|name| row.get("name") != Some(name))
        {
            continue;
        }
        let target = row
            .as_object_mut()
            .ok_or("ERR_DEEPSEEK_CONFIG_UNSUPPORTED")?;
        for (key, value) in object {
            if !matches!(key.as_str(), "id" | "name") {
                target.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(())
}

fn has_custom_headers(config: &Value) -> bool {
    let has_headers = |value: &Value| {
        value.get("headers").is_some_and(|headers| {
            !headers
                .as_object()
                .is_some_and(|headers| headers.is_empty())
        })
    };
    has_headers(config)
        || config
            .get("models")
            .and_then(Value::as_array)
            .is_some_and(|models| models.iter().any(has_headers))
        || config
            .get("modelOverrides")
            .and_then(Value::as_object)
            .is_some_and(|models| models.values().any(has_headers))
}

fn explicit_routes(rows: &BTreeMap<String, Value>) -> BTreeMap<String, Option<(String, String)>> {
    let mut routes = BTreeMap::new();
    for row in rows.values() {
        if row.get("disabled").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let configs: Vec<(&str, &Value)> = match row.get("name").and_then(Value::as_str) {
            Some("@deepseek-ai/dsh-llm-deepseek-api-key") => {
                vec![("deepseek-official", &row["config"])]
            }
            Some("@deepseek-ai/dsh-llm-pi-ai") => row
                .pointer("/config/providers")
                .and_then(Value::as_object)
                .map(|providers| {
                    providers
                        .iter()
                        .map(|(id, config)| (id.as_str(), config))
                        .collect()
                })
                .unwrap_or_default(),
            _ => continue,
        };
        for (id, config) in configs {
            if !valid_provider_id(id) {
                continue;
            }
            let route = config
                .get("baseURL")
                .and_then(Value::as_str)
                .and_then(safe_base_url)
                .zip(
                    config
                        .get("apiKeyEnv")
                        .and_then(Value::as_str)
                        .filter(|key| {
                            !key.is_empty()
                                && key.len() <= 128
                                && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                        })
                        .map(str::to_string),
                )
                .filter(|_| !has_custom_headers(config));
            match routes.entry(id.to_string()) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(route);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    if entry.get() != &route {
                        entry.insert(None);
                    }
                }
            }
        }
    }
    routes
}

fn credential_refs(home: &Path) -> Result<Value, String> {
    let path = home.join(".credentials.yaml");
    if !path.exists() {
        return Ok(Value::Null);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path)
            .map_err(|_| "ERR_DEEPSEEK_CREDENTIALS_UNREADABLE")?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err("ERR_DEEPSEEK_CREDENTIALS_PERMISSIONS".into());
        }
    }
    let document = read_yaml(&path)?;
    if document.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("ERR_DEEPSEEK_CREDENTIALS_UNSUPPORTED".into());
    }
    Ok(document.get("refs").cloned().unwrap_or(Value::Null))
}

pub(crate) fn config_paths() -> Vec<PathBuf> {
    let Ok(Some(root)) = session_root() else {
        return Vec::new();
    };
    let Some(home) = root.parent() else {
        return Vec::new();
    };
    let mut paths = vec![
        home.join(".credentials.yaml"),
        home.join("cordis.patch.yml"),
    ];
    if let Ok(profiles) = std::fs::read_dir(home.join("profiles")) {
        for profile in profiles.flatten().take(MAX_ENTRIES) {
            if profile.path().is_dir() {
                paths.push(profile.path().join("cordis.yml"));
                paths.push(profile.path().join("cordis.patch.yml"));
            }
        }
    }
    paths
}

pub(crate) fn read_api_key_routes() -> Result<Vec<ApiKeyRoute>, String> {
    let Some(root) = session_root()? else {
        return Ok(Vec::new());
    };
    let Some(home) = root.parent() else {
        return Ok(Vec::new());
    };
    read_routes_at(home, &root, |name| std::env::var(name).ok())
}

fn read_routes_at(
    home: &Path,
    root: &Path,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Vec<ApiKeyRoute>, String> {
    let profiles = match std::fs::read_dir(home.join("profiles")) {
        Ok(profiles) => profiles,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("ERR_DEEPSEEK_CONFIG_UNREADABLE".into()),
    };
    let mut candidates = Vec::new();
    for (index, profile) in profiles.enumerate() {
        if index >= MAX_ENTRIES {
            return Err("ERR_DEEPSEEK_CONFIG_TOO_LARGE".into());
        }
        let profile = profile
            .map_err(|_| "ERR_DEEPSEEK_CONFIG_UNREADABLE")?
            .path();
        if !profile.is_dir() {
            continue;
        }
        let mut rows = BTreeMap::new();
        let root_config = profile.join("cordis.yml");
        if root_config.exists() {
            insert_rows(&read_yaml(&root_config)?, &mut rows)?;
        }
        for path in [
            profile.join("cordis.patch.yml"),
            home.join("cordis.patch.yml"),
        ] {
            if path.exists() {
                compose_rows(&read_yaml(&path)?, &mut rows)?;
            }
        }
        candidates.push(explicit_routes(&rows));
    }
    let refs = credential_refs(home)?;
    let mut routes = Vec::new();
    // A transcript does not identify its launch profile. All profiles must confirm a route.
    let Some(first) = candidates.first() else {
        return Ok(routes);
    };
    for (provider, route) in first {
        let Some((base_url, key_ref)) = route else {
            continue;
        };
        if candidates
            .iter()
            .any(|profile| profile.get(provider) != Some(route))
        {
            continue;
        }
        let api_key = env(key_ref).or_else(|| {
            refs.get(key_ref)
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        let Some(api_key) = api_key
            .filter(|key| !key.trim().is_empty() && key.len() <= 8192 && !key.contains("${"))
        else {
            continue;
        };
        routes.push(ApiKeyRoute {
            evidence: LocalProviderEvidence {
                scope: root_tag(root),
                provider_id: provider.clone(),
                request_started_at_ms: None,
            },
            base_url: base_url.clone(),
            api_key,
        });
    }
    Ok(routes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("profiles/desktop")).unwrap();
        home
    }

    fn route_yaml(base_url: &str) -> String {
        format!("- id: llm-pi-ai\n  name: '@deepseek-ai/dsh-llm-pi-ai'\n  config:\n    providers:\n      relay:\n        baseURL: '{base_url}'\n        apiKeyEnv: TEST_KEY\n")
    }

    #[test]
    fn replaces_explicit_config_and_rejects_conflicting_profiles() {
        let home = home();
        let profile = home.path().join("profiles/desktop");
        std::fs::write(
            profile.join("cordis.yml"),
            route_yaml("https://first.example/v1"),
        )
        .unwrap();
        std::fs::write(profile.join("cordis.patch.yml"), "- id: llm-pi-ai\n  config:\n    providers:\n      relay:\n        baseURL: https://second.example/v1\n        apiKeyEnv: TEST_KEY\n").unwrap();
        let routes = read_routes_at(home.path(), &home.path().join("sessions"), |_| {
            Some("test-secret".into())
        })
        .unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].base_url, "https://second.example/v1");
        assert_eq!(routes[0].evidence.provider_id, "relay");
        std::fs::create_dir_all(home.path().join("profiles/cli")).unwrap();
        std::fs::write(
            home.path().join("profiles/cli/cordis.yml"),
            route_yaml("https://other.example/v1"),
        )
        .unwrap();
        assert!(
            read_routes_at(home.path(), &home.path().join("sessions"), |_| Some(
                "test-secret".into()
            ))
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn partial_config_replacement_does_not_reuse_an_old_credential_reference() {
        let home = home();
        let profile = home.path().join("profiles/desktop");
        std::fs::write(
            profile.join("cordis.yml"),
            route_yaml("https://first.example/v1"),
        )
        .unwrap();
        std::fs::write(profile.join("cordis.patch.yml"), "- id: llm-pi-ai\n  config:\n    providers:\n      relay:\n        baseURL: https://second.example/v1\n").unwrap();
        assert!(
            read_routes_at(home.path(), &home.path().join("sessions"), |_| Some(
                "test-secret".into()
            ))
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn patch_identity_and_disabled_state_follow_cordis_semantics() {
        let mut rows = BTreeMap::new();
        let original =
            serde_yaml_ng::from_str::<Value>(&route_yaml("https://first.example/v1")).unwrap();
        insert_rows(&original, &mut rows).unwrap();
        // A named patch cannot create a missing entry or rename an existing entry.
        compose_rows(&serde_json::json!([
            {"id":"missing","name":"@deepseek-ai/dsh-llm-pi-ai","config":{"providers":{"fake":{"baseURL":"https://fake.example/v1","apiKeyEnv":"KEY"}}}},
            {"id":"llm-pi-ai","name":"wrong-plugin","disabled":true}
        ]), &mut rows).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(explicit_routes(&rows)["relay"].is_some());
        compose_rows(
            &serde_json::json!([{"id":"llm-pi-ai","disabled":true}]),
            &mut rows,
        )
        .unwrap();
        assert!(explicit_routes(&rows).is_empty());
        for disabled in [
            serde_json::json!("true"),
            serde_json::json!(1),
            serde_json::json!({"__jsExpr":"false"}),
        ] {
            assert!(compose_rows(
                &serde_json::json!([{"id":"llm-pi-ai","disabled":disabled}]),
                &mut rows
            )
            .is_err());
        }
        let mut inserted = BTreeMap::new();
        compose_rows(&serde_json::json!([{"insert": original}]), &mut inserted).unwrap();
        assert!(explicit_routes(&inserted)["relay"].is_some());
    }

    #[test]
    fn credentials_are_read_only_and_never_appear_in_errors() {
        let home = home();
        std::fs::write(
            home.path().join("profiles/desktop/cordis.yml"),
            route_yaml("https://relay.example/v1"),
        )
        .unwrap();
        let credentials = home.path().join(".credentials.yaml");
        let content = "version: 1\nrefs:\n  TEST_KEY: test-secret\nrecords: {}\n";
        std::fs::write(&credentials, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let routes = read_routes_at(home.path(), &home.path().join("sessions"), |_| None).unwrap();
        assert_eq!(routes[0].api_key, "test-secret");
        assert_eq!(std::fs::read_to_string(&credentials).unwrap(), content);
        std::fs::write(&credentials, "TEST_KEY: [test-secret\n").unwrap();
        assert_eq!(
            read_routes_at(home.path(), &home.path().join("sessions"), |_| None)
                .err()
                .unwrap(),
            "ERR_DEEPSEEK_CONFIG_INVALID"
        );
    }

    #[test]
    fn custom_headers_cannot_be_mistaken_for_api_key_environment_auth() {
        assert!(has_custom_headers(
            &serde_json::json!({"headers":{"Authorization":"test-secret"}})
        ));
        assert!(has_custom_headers(
            &serde_json::json!({"models":[{"id":"model","headers":{"x-api-key":"test-secret"}}]})
        ));
        assert!(has_custom_headers(
            &serde_json::json!({"modelOverrides":{"model":{"headers":{"Authorization":"test-secret"}}}})
        ));
        let mut rows = BTreeMap::new();
        insert_rows(&serde_json::json!([{"id":"llm-pi-ai","name":"@deepseek-ai/dsh-llm-pi-ai","config":{"providers":{"relay":{"baseURL":"https://relay.example/v1","apiKeyEnv":"TEST_KEY","headers":{"Authorization":"test-secret"}}}}}]), &mut rows).unwrap();
        assert!(explicit_routes(&rows)["relay"].is_none());
    }

    #[test]
    fn unknown_composition_and_sensitive_urls_are_ineligible() {
        for url in [
            "https://user:secret@example.com/v1",
            "https://example.com/v1?key=secret",
            "https://example.com/#secret",
            "http://127.0.0.2:8080",
            "http://[::1]:8080",
            "https://${HOST}/v1",
        ] {
            assert!(safe_base_url(url).is_none(), "unsafe URL accepted");
        }
        let value = serde_json::json!([{"id":"llm-pi-ai","remove":true}]);
        assert!(compose_rows(&value, &mut BTreeMap::new()).is_err());
        assert!(!valid_provider_id("secret:token"));
        assert!(!valid_provider_id(&"a".repeat(129)));
    }

    #[test]
    fn scope_prevents_shared_provider_ids_from_crossing_homes() {
        let first = LocalProviderEvidence {
            scope: root_tag(Path::new("/first/sessions")),
            provider_id: "relay".into(),
            request_started_at_ms: None,
        };
        let second = LocalProviderEvidence {
            scope: root_tag(Path::new("/second/sessions")),
            provider_id: "relay".into(),
            request_started_at_ms: None,
        };
        assert_ne!(first.scoped_tool(), second.scoped_tool());
    }
}
