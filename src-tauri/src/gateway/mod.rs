//! Local API gateway domain types and validation.
//!
//! Gateway profiles keep only credential metadata. Raw credentials live in the
//! operating system credential store.

use crate::models::{
    GatewayAuthMode, GatewayDispatchStrategy, GatewayLocalKey, GatewayProfile, GatewayProtocol,
    GatewayUpstreamKey,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

static NEXT_PROFILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const KEYRING_SERVICE: &str = "com.usagemeter.gateway";
const CIRCUIT_FAILURE_THRESHOLD: u32 = 5;
const CIRCUIT_BASE_COOLDOWN: Duration = Duration::from_secs(60);
const CIRCUIT_MAX_COOLDOWN: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Default)]
struct KeyHealth {
    consecutive_failures: u32,
    trip_count: u32,
    cooling_until: Option<Instant>,
}

static KEY_HEALTH: LazyLock<Mutex<HashMap<String, KeyHealth>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static ROUND_ROBIN_CURSORS: LazyLock<Mutex<HashMap<String, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayProfileInput {
    pub name: String,
    pub protocol: GatewayProtocol,
    pub base_url: String,
    #[serde(default = "crate::models::default_gateway_profile_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub client_label: String,
    #[serde(default)]
    pub dispatch_strategy: GatewayDispatchStrategy,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayUpstreamKeyInput {
    pub remark: String,
    pub secret: String,
    #[serde(default = "crate::models::default_gateway_profile_enabled")]
    pub enabled: bool,
    #[serde(default = "crate::models::default_gateway_key_weight")]
    pub weight: u16,
    #[serde(default)]
    pub priority: u16,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayUpstreamKeyUpdateInput {
    pub enabled: bool,
    pub weight: u16,
    pub priority: u16,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayLocalKeyInput {
    #[serde(default)]
    pub remark: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedGatewayLocalKey {
    pub id: String,
    pub remark: String,
    pub key: String,
}

/// The Tauri-facing gateway view. It intentionally has the same shape as the
/// editable profile fields, but never serializes secret references or hashes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayProfileView {
    pub id: String,
    pub name: String,
    pub protocol: GatewayProtocol,
    pub base_url: String,
    pub enabled: bool,
    pub client_label: String,
    pub auth_mode: GatewayAuthMode,
    pub dispatch_strategy: GatewayDispatchStrategy,
    pub upstream_keys: Vec<GatewayUpstreamKeyView>,
    pub local_keys: Vec<GatewayLocalKeyView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayUpstreamKeyView {
    pub id: String,
    pub remark: String,
    pub enabled: bool,
    pub weight: u16,
    pub priority: u16,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayLocalKeyView {
    pub id: String,
    pub remark: String,
    pub enabled: bool,
    pub created_at_ms: i64,
    pub last_used_at_ms: Option<i64>,
}

impl From<&GatewayProfile> for GatewayProfileView {
    fn from(profile: &GatewayProfile) -> Self {
        Self {
            id: profile.id.clone(),
            name: profile.name.clone(),
            protocol: profile.protocol.clone(),
            base_url: profile.base_url.clone(),
            enabled: profile.enabled,
            client_label: profile.client_label.clone(),
            auth_mode: profile.auth_mode.clone(),
            dispatch_strategy: profile.dispatch_strategy.clone(),
            upstream_keys: profile
                .upstream_keys
                .iter()
                .map(|key| GatewayUpstreamKeyView {
                    id: key.id.clone(),
                    remark: key.remark.clone(),
                    enabled: key.enabled,
                    weight: key.weight,
                    priority: key.priority,
                })
                .collect(),
            local_keys: profile
                .local_keys
                .iter()
                .map(|key| GatewayLocalKeyView {
                    id: key.id.clone(),
                    remark: key.remark.clone(),
                    enabled: key.enabled,
                    created_at_ms: key.created_at_ms,
                    last_used_at_ms: key.last_used_at_ms,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GatewayStatus {
    pub proxy_running: bool,
    pub routing_active: bool,
    pub enabled_profile_count: usize,
    pub listener_address: Option<String>,
}

pub fn create_profile(input: GatewayProfileInput) -> Result<GatewayProfile, String> {
    let profile = GatewayProfile {
        id: next_profile_id(),
        name: input.name,
        protocol: input.protocol,
        base_url: input.base_url,
        enabled: input.enabled,
        client_label: input.client_label,
        auth_mode: GatewayAuthMode::ManagedKeys,
        dispatch_strategy: input.dispatch_strategy,
        upstream_keys: Vec::new(),
        local_keys: Vec::new(),
    };
    validate_profile(&profile)?;
    Ok(normalize_profile(profile))
}

pub fn update_profile(id: &str, input: GatewayProfileInput) -> Result<GatewayProfile, String> {
    let profile = GatewayProfile {
        id: id.to_string(),
        name: input.name,
        protocol: input.protocol,
        base_url: input.base_url,
        enabled: input.enabled,
        client_label: input.client_label,
        auth_mode: GatewayAuthMode::ManagedKeys,
        dispatch_strategy: input.dispatch_strategy,
        upstream_keys: Vec::new(),
        local_keys: Vec::new(),
    };
    validate_profile(&profile)?;
    Ok(normalize_profile(profile))
}

pub fn validate_profile(profile: &GatewayProfile) -> Result<(), String> {
    if !is_valid_profile_id(&profile.id) {
        return Err("ERR_GATEWAY_PROFILE_ID_INVALID".to_string());
    }
    if profile.name.trim().is_empty() || profile.name.chars().count() > 80 {
        return Err("ERR_GATEWAY_PROFILE_NAME_INVALID".to_string());
    }
    if profile.client_label.chars().count() > 80 {
        return Err("ERR_GATEWAY_CLIENT_LABEL_INVALID".to_string());
    }

    let url = reqwest::Url::parse(profile.base_url.trim())
        .map_err(|_| "ERR_GATEWAY_BASE_URL_INVALID".to_string())?;
    let Some(host) = url.host_str() else {
        return Err("ERR_GATEWAY_BASE_URL_INVALID".to_string());
    };
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || is_disallowed_upstream_host(host)
    {
        return Err("ERR_GATEWAY_BASE_URL_INVALID".to_string());
    }
    Ok(())
}

/// Gateway profiles forward client-supplied credentials. They must never be
/// configured to target the local machine or non-public literal addresses.
/// Hostname resolution is intentionally left to the HTTPS stack: resolving at
/// save time would be stale by connection time and would break legitimate
/// providers using managed DNS.
fn is_disallowed_upstream_host(host: &str) -> bool {
    let normalized = host.trim_end_matches('.');
    if normalized.eq_ignore_ascii_case("localhost") {
        return true;
    }

    normalized
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .map(is_disallowed_upstream_ip)
        .unwrap_or(false)
}

fn is_disallowed_upstream_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_disallowed_ipv4(ip),
        IpAddr::V6(ip) => is_disallowed_ipv6(ip),
    }
}

fn is_disallowed_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        // Carrier-grade NAT is not publicly routable.
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
}

fn is_disallowed_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || ip.is_multicast()
        || ip.to_ipv4_mapped().map(is_disallowed_ipv4).unwrap_or(false)
}

pub fn normalize_profile(mut profile: GatewayProfile) -> GatewayProfile {
    profile.name = profile.name.trim().to_string();
    profile.client_label = profile.client_label.trim().to_string();
    profile.base_url = profile.base_url.trim().trim_end_matches('/').to_string();
    profile
}

pub fn create_upstream_key(
    profile_id: &str,
    input: GatewayUpstreamKeyInput,
) -> Result<(GatewayUpstreamKey, String), String> {
    validate_key_remark(&input.remark)?;
    if input.secret.trim().is_empty() || input.secret.chars().count() > 4096 {
        return Err("ERR_GATEWAY_UPSTREAM_KEY_INVALID".to_string());
    }
    if input.weight == 0 {
        return Err("ERR_GATEWAY_UPSTREAM_KEY_WEIGHT_INVALID".to_string());
    }
    let id = next_key_id("upstream");
    let secret_ref = upstream_secret_ref(profile_id, &id);
    Ok((
        GatewayUpstreamKey {
            id,
            remark: input.remark.trim().to_string(),
            enabled: input.enabled,
            weight: input.weight,
            priority: input.priority,
            secret_ref,
        },
        input.secret.trim().to_string(),
    ))
}

pub fn create_local_key(
    profile_id: &str,
    input: GatewayLocalKeyInput,
) -> Result<(GatewayLocalKey, GeneratedGatewayLocalKey), String> {
    validate_key_remark(&input.remark)?;
    let id = next_key_id("local");
    let secret = random_token(32);
    let salt = random_token(16);
    let key = format!("umg_{id}_{secret}");
    let metadata = GatewayLocalKey {
        id: id.clone(),
        remark: input.remark.trim().to_string(),
        enabled: true,
        secret_hash: hash_local_key(&key, &salt),
        secret_salt: salt,
        secret_ref: local_secret_ref(profile_id, &id),
        created_at_ms: chrono::Utc::now().timestamp_millis(),
        last_used_at_ms: None,
    };
    let generated = GeneratedGatewayLocalKey {
        id,
        remark: metadata.remark.clone(),
        key,
    };
    Ok((metadata, generated))
}

pub fn verify_local_key(key: &str, candidate: &GatewayLocalKey) -> bool {
    if !candidate.enabled || !key.starts_with("umg_") {
        return false;
    }
    let actual = hash_local_key(key, &candidate.secret_salt);
    constant_time_eq(actual.as_bytes(), candidate.secret_hash.as_bytes())
}

pub fn select_upstream_key(profile: &GatewayProfile) -> Option<&GatewayUpstreamKey> {
    select_upstream_key_excluding(profile, &[])
}

pub fn select_upstream_key_excluding<'a>(
    profile: &'a GatewayProfile,
    excluded_key_ids: &[String],
) -> Option<&'a GatewayUpstreamKey> {
    let mut keys: Vec<&GatewayUpstreamKey> = profile
        .upstream_keys
        .iter()
        .filter(|key| {
            key.enabled
                && !excluded_key_ids.iter().any(|id| id == &key.id)
                && upstream_key_is_available(&profile.id, &key.id)
        })
        .collect();
    if keys.is_empty() {
        return None;
    }
    match profile.dispatch_strategy {
        GatewayDispatchStrategy::PriorityFailover => {
            keys.sort_by_key(|key| key.priority);
            keys.into_iter().next()
        }
        GatewayDispatchStrategy::Weighted => {
            let total: u32 = keys.iter().map(|key| key.weight.max(1) as u32).sum();
            let ticket = OsRng.next_u32() % total.max(1);
            let mut cursor = 0u32;
            keys.into_iter().find(|key| {
                cursor += key.weight.max(1) as u32;
                ticket < cursor
            })
        }
        GatewayDispatchStrategy::Random => {
            Some(keys.swap_remove((OsRng.next_u32() as usize) % keys.len()))
        }
        GatewayDispatchStrategy::RoundRobin => {
            let index = match ROUND_ROBIN_CURSORS.lock() {
                Ok(mut cursors) => {
                    let cursor = cursors.entry(profile.id.clone()).or_insert(0);
                    let index = *cursor as usize % keys.len();
                    *cursor = cursor.wrapping_add(1);
                    index
                }
                Err(_) => 0,
            };
            Some(keys.swap_remove(index))
        }
    }
}

pub fn upstream_secret_ref(profile_id: &str, key_id: &str) -> String {
    format!("gateway:{profile_id}:{key_id}")
}

pub fn local_secret_ref(profile_id: &str, key_id: &str) -> String {
    format!("gateway-local:{profile_id}:{key_id}")
}

pub fn is_expected_upstream_secret_ref(profile_id: &str, key: &GatewayUpstreamKey) -> bool {
    key.secret_ref == upstream_secret_ref(profile_id, &key.id)
}

pub fn is_expected_local_secret_ref(profile_id: &str, key: &GatewayLocalKey) -> bool {
    key.secret_ref == local_secret_ref(profile_id, &key.id)
}

/// Updates only in-memory health state. It contains no credential material and
/// deliberately resets after an application restart.
pub fn report_upstream_result(profile_id: &str, key_id: &str, status_code: Option<u16>) {
    let mut health = match KEY_HEALTH.lock() {
        Ok(health) => health,
        Err(_) => return,
    };
    let key = format!("{profile_id}:{key_id}");
    let entry = health.entry(key).or_default();
    match status_code {
        Some(200..=299) => *entry = KeyHealth::default(),
        Some(400 | 404 | 422) => {}
        Some(401 | 402 | 403) => {
            entry.consecutive_failures = CIRCUIT_FAILURE_THRESHOLD;
            entry.trip_count = entry.trip_count.saturating_add(1);
            entry.cooling_until = Some(Instant::now() + CIRCUIT_MAX_COOLDOWN);
        }
        Some(429) => open_circuit(entry),
        Some(500..=599) | None => {
            entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
            if entry.consecutive_failures >= CIRCUIT_FAILURE_THRESHOLD {
                open_circuit(entry);
            }
        }
        _ => {}
    }
}

fn upstream_key_is_available(profile_id: &str, key_id: &str) -> bool {
    let mut health = match KEY_HEALTH.lock() {
        Ok(health) => health,
        Err(_) => return true,
    };
    let key = format!("{profile_id}:{key_id}");
    let Some(entry) = health.get_mut(&key) else {
        return true;
    };
    match entry.cooling_until {
        Some(until) if Instant::now() < until => false,
        Some(_) => {
            entry.cooling_until = None;
            entry.consecutive_failures = 0;
            true
        }
        None => true,
    }
}

fn open_circuit(entry: &mut KeyHealth) {
    entry.trip_count = entry.trip_count.saturating_add(1);
    entry.consecutive_failures = 0;
    let multiplier = 1u32
        .checked_shl(entry.trip_count.saturating_sub(1).min(4))
        .unwrap_or(16);
    entry.cooling_until = Some(
        Instant::now()
            + CIRCUIT_BASE_COOLDOWN
                .saturating_mul(multiplier)
                .min(CIRCUIT_MAX_COOLDOWN),
    );
}

pub fn store_upstream_secret(secret_ref: &str, secret: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("security")
            .args([
                "add-generic-password",
                "-U",
                "-a",
                secret_ref,
                "-s",
                KEYRING_SERVICE,
                "-w",
                secret,
            ])
            .status()
            .map_err(|_| "ERR_GATEWAY_CREDENTIAL_STORE_UNAVAILABLE".to_string())?;
        return status
            .success()
            .then_some(())
            .ok_or_else(|| "ERR_GATEWAY_CREDENTIAL_STORE_UNAVAILABLE".to_string());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (secret_ref, secret);
        Err("ERR_GATEWAY_CREDENTIAL_STORE_UNAVAILABLE".to_string())
    }
}

pub fn load_upstream_secret(secret_ref: &str) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("security")
            .args([
                "find-generic-password",
                "-a",
                secret_ref,
                "-s",
                KEYRING_SERVICE,
                "-w",
            ])
            .output()
            .map_err(|_| "ERR_GATEWAY_CREDENTIAL_STORE_UNAVAILABLE".to_string())?;
        if !output.status.success() {
            return Err("ERR_GATEWAY_UPSTREAM_KEY_UNAVAILABLE".to_string());
        }
        return String::from_utf8(output.stdout)
            .map(|secret| secret.trim_end_matches('\n').to_string())
            .map_err(|_| "ERR_GATEWAY_UPSTREAM_KEY_UNAVAILABLE".to_string());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = secret_ref;
        Err("ERR_GATEWAY_CREDENTIAL_STORE_UNAVAILABLE".to_string())
    }
}

pub fn delete_upstream_secret(secret_ref: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("security")
            .args([
                "delete-generic-password",
                "-a",
                secret_ref,
                "-s",
                KEYRING_SERVICE,
            ])
            .output();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = secret_ref;
    }
}

fn validate_key_remark(remark: &str) -> Result<(), String> {
    if remark.chars().count() > 80 || remark.chars().any(char::is_control) {
        return Err("ERR_GATEWAY_KEY_REMARK_INVALID".to_string());
    }
    Ok(())
}

fn random_token(bytes: usize) -> String {
    let mut value = vec![0u8; bytes];
    OsRng.fill_bytes(&mut value);
    URL_SAFE_NO_PAD.encode(value)
}

fn hash_local_key(key: &str, salt: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(salt.as_bytes());
    digest.update([0]);
    digest.update(key.as_bytes());
    URL_SAFE_NO_PAD.encode(digest.finalize())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |result, (a, b)| result | (a ^ b))
        == 0
}

pub fn listener_address(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

fn next_profile_id() -> String {
    let timestamp = chrono::Utc::now().timestamp_millis();
    let sequence = NEXT_PROFILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("gateway-{timestamp:x}-{sequence:x}")
}

fn next_key_id(kind: &str) -> String {
    let sequence = NEXT_PROFILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "{kind}-{:x}-{:x}",
        chrono::Utc::now().timestamp_millis(),
        sequence
    )
}

fn is_valid_profile_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(base_url: &str) -> GatewayProfileInput {
        GatewayProfileInput {
            name: " DeepSeek ".to_string(),
            protocol: GatewayProtocol::OpenAiChatCompletions,
            base_url: base_url.to_string(),
            enabled: true,
            client_label: " Cursor ".to_string(),
            dispatch_strategy: GatewayDispatchStrategy::RoundRobin,
        }
    }

    #[test]
    fn creates_normalized_non_secret_profile() {
        let profile = create_profile(input("  https://api.deepseek.com/  ")).expect("profile");
        assert!(profile.id.starts_with("gateway-"));
        assert_eq!(profile.name, "DeepSeek");
        assert_eq!(profile.base_url, "https://api.deepseek.com");
        assert_eq!(profile.client_label, "Cursor");
    }

    #[test]
    fn rejects_credential_bearing_or_non_https_urls() {
        assert_eq!(
            create_profile(input("https://key@example.com/v1")).unwrap_err(),
            "ERR_GATEWAY_BASE_URL_INVALID"
        );
        assert_eq!(
            create_profile(input("file:///tmp/upstream")).unwrap_err(),
            "ERR_GATEWAY_BASE_URL_INVALID"
        );
        assert_eq!(
            create_profile(input("http://api.example.com/v1")).unwrap_err(),
            "ERR_GATEWAY_BASE_URL_INVALID"
        );
    }

    #[test]
    fn rejects_local_and_non_public_literal_upstreams() {
        for base_url in [
            "https://localhost/v1",
            "https://localhost./v1",
            "https://127.0.0.1/v1",
            "https://10.0.0.1/v1",
            "https://172.16.0.1/v1",
            "https://192.168.1.1/v1",
            "https://169.254.10.20/v1",
            "https://0.0.0.0/v1",
            "https://100.64.0.1/v1",
            "https://[::1]/v1",
            "https://[::]/v1",
            "https://[fc00::1]/v1",
            "https://[fe80::1]/v1",
            "https://[::ffff:127.0.0.1]/v1",
        ] {
            assert_eq!(
                create_profile(input(base_url)).unwrap_err(),
                "ERR_GATEWAY_BASE_URL_INVALID",
                "{base_url} should be rejected"
            );
        }
    }

    #[test]
    fn accepts_public_https_upstream() {
        let profile = create_profile(input("https://api.deepseek.com/v1")).expect("profile");
        assert_eq!(profile.base_url, "https://api.deepseek.com/v1");
    }

    #[test]
    fn preserves_protocol_without_conversion() {
        let profile = create_profile(GatewayProfileInput {
            protocol: GatewayProtocol::AnthropicMessages,
            ..input("https://api.anthropic.com")
        })
        .expect("profile");
        assert_eq!(profile.protocol, GatewayProtocol::AnthropicMessages);
    }

    #[test]
    fn generated_local_key_is_one_way_but_verifiable() {
        let (metadata, generated) = create_local_key(
            "gateway-test",
            GatewayLocalKeyInput {
                remark: "Cursor".to_string(),
            },
        )
        .expect("local key");
        assert!(generated.key.starts_with("umg_local-"));
        assert!(verify_local_key(&generated.key, &metadata));
        assert!(!verify_local_key("umg_wrong_value", &metadata));
    }

    #[test]
    fn priority_strategy_selects_lowest_priority_value() {
        let mut profile = create_profile(input("https://api.deepseek.com")).unwrap();
        profile.dispatch_strategy = GatewayDispatchStrategy::PriorityFailover;
        profile.upstream_keys = vec![
            GatewayUpstreamKey {
                id: "a".to_string(),
                remark: String::new(),
                enabled: true,
                weight: 1,
                priority: 20,
                secret_ref: "a".to_string(),
            },
            GatewayUpstreamKey {
                id: "b".to_string(),
                remark: String::new(),
                enabled: true,
                weight: 1,
                priority: 1,
                secret_ref: "b".to_string(),
            },
        ];
        assert_eq!(select_upstream_key(&profile).unwrap().id, "b");
    }
}
