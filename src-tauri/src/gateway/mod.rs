//! Local API gateway domain types and validation.
//!
//! Gateway profiles persist their credentials in the local settings document.
//! Old macOS Keychain references are read only during one-time migration.

pub mod audit;
pub mod probe;
pub mod rate_limit;

use crate::models::{
    GatewayAuthMode, GatewayDispatchStrategy, GatewayLocalKey, GatewayProfile, GatewayProtocol,
    GatewayUpstreamKey, GatewayUpstreamModel,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, Rng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

static NEXT_PROFILE_SEQUENCE: AtomicU64 = AtomicU64::new(1); // used by next_key_id
#[cfg(target_os = "macos")]
const KEYRING_SERVICE: &str = "com.usagemeter.gateway";
const CIRCUIT_FAILURE_THRESHOLD: u32 = 3;
const CIRCUIT_AUTH_FAILURE_THRESHOLD: u32 = 2;
const CIRCUIT_BASE_COOLDOWN: Duration = Duration::from_secs(30);
const CIRCUIT_MAX_COOLDOWN: Duration = Duration::from_secs(300);
const CIRCUIT_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default)]
struct KeyHealth {
    consecutive_failures: u32,
    consecutive_auth_failures: u32,
    trip_count: u32,
    cooling_until: Option<Instant>,
    half_open_probe_in_flight: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamOutcome {
    Success,
    InvalidCredentials,
    PaymentRequired,
    PermissionDenied,
    RateLimited,
    ClientError,
    ServerError,
    TransportError,
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
    #[serde(default)]
    pub upstream_secret: Option<String>,
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
    pub upstream_models: Vec<GatewayUpstreamModel>,
    pub credential_recovery: GatewayCredentialRecoveryView,
}

/// Non-sensitive recovery information for profiles imported from the v1
/// macOS Keychain representation. It never exposes credential references.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GatewayCredentialRecoveryView {
    pub upstream_key_required: bool,
    pub local_key_rotation_recommended: bool,
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
            protocol: profile.protocol,
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
            upstream_models: profile.upstream_models.clone(),
            credential_recovery: GatewayCredentialRecoveryView {
                upstream_key_required: profile.upstream_keys.iter().any(|key| {
                    key.secret.trim().is_empty()
                        && is_expected_upstream_secret_ref(&profile.id, key)
                }),
                local_key_rotation_recommended: profile.local_keys.iter().any(|key| {
                    key.secret.trim().is_empty() && is_expected_local_secret_ref(&profile.id, key)
                }),
            },
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
        upstream_models: Vec::new(),
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
        upstream_models: Vec::new(),
    };
    validate_profile(&profile)?;
    Ok(normalize_profile(profile))
}

pub fn validate_profile(profile: &GatewayProfile) -> Result<(), String> {
    validate_profile_with_resolver(profile, &DefaultHostResolver)
}

/// 校验 profile，主机名 → IP 的解析通过 `resolver` 注入，使测试可以提供
/// fake 解析结果而无需触发真实 DNS 查询。
fn validate_profile_with_resolver(
    profile: &GatewayProfile,
    resolver: &dyn HostResolver,
) -> Result<(), String> {
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

    // Additional DNS rebinding protection: resolve the hostname and check if any
    // resolved IP addresses point to private networks. This prevents attacks where
    // a public domain resolves to a private IP.
    for addr in resolver.resolve(host) {
        if is_disallowed_upstream_ip(addr) {
            log::warn!(
                "Gateway upstream {} resolves to disallowed IP: {}",
                host,
                addr
            );
            return Err("ERR_GATEWAY_BASE_URL_RESOLVES_TO_PRIVATE".to_string());
        }
    }
    // If DNS resolution fails (resolver returns no addresses), we allow it to
    // proceed - the actual HTTPS request will fail later with a proper network
    // error. This avoids rejecting valid configurations due to temporary DNS
    // issues during profile creation.

    Ok(())
}

/// 主机名 → IP 地址解析抽象，用于 DNS rebinding 保护检查。
trait HostResolver {
    fn resolve(&self, host: &str) -> Vec<IpAddr>;
}

/// 默认解析：使用系统 `to_socket_addrs`。解析失败返回空列表（= 放行）。
struct DefaultHostResolver;

impl HostResolver for DefaultHostResolver {
    fn resolve(&self, host: &str) -> Vec<IpAddr> {
        format!("{}:443", host)
            .to_socket_addrs()
            .map(|addrs| addrs.map(|addr| addr.ip()).collect())
            .unwrap_or_default()
    }
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

/// Upper bound on the model list persisted with a profile, keeping the
/// settings document bounded even when an upstream exposes a huge catalog.
pub const MAX_UPSTREAM_MODELS: usize = 200;

/// Model ids are embedded in the Gemini request path, so model ids are
/// restricted to path-safe characters. Shared by the probe path and by
/// [`normalize_upstream_models`].
pub fn is_safe_model_id(model_id: &str) -> bool {
    !model_id.is_empty()
        && model_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Normalizes a model list before persisting it with a profile:
/// drops ids that are unsafe for request paths, de-duplicates by `id`
/// preserving first-seen order, and caps the list at [`MAX_UPSTREAM_MODELS`].
pub fn normalize_upstream_models(models: Vec<GatewayUpstreamModel>) -> Vec<GatewayUpstreamModel> {
    let mut seen = std::collections::HashSet::new();
    models
        .into_iter()
        .filter(|model| is_safe_model_id(&model.id) && seen.insert(model.id.clone()))
        .take(MAX_UPSTREAM_MODELS)
        .collect()
}

pub fn create_upstream_key(
    _profile_id: &str,
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
    let secret = input.secret.trim().to_string();
    Ok((
        GatewayUpstreamKey {
            id,
            remark: input.remark.trim().to_string(),
            enabled: input.enabled,
            weight: input.weight,
            priority: input.priority,
            secret: secret.clone(),
            // New records intentionally leave the v1 keychain reference empty.
            secret_ref: String::new(),
        },
        secret,
    ))
}

pub fn create_local_key(
    _profile_id: &str,
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
        secret: key.clone(),
        // New records intentionally leave the v1 keychain reference empty.
        secret_ref: String::new(),
        created_at_ms: chrono::Utc::now().timestamp_millis(),
        last_used_at_ms: None,
        expires_at_ms: None, // Default: never expires
        max_requests: None,  // Default: unlimited requests
        request_count: 0,
    };
    let generated = GeneratedGatewayLocalKey {
        id,
        remark: metadata.remark.clone(),
        key,
    };
    Ok((metadata, generated))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalKeyVerifyError {
    Disabled,
    Expired,
    QuotaExceeded,
    Invalid,
}

#[allow(dead_code)]
pub fn verify_local_key(key: &str, candidate: &GatewayLocalKey) -> bool {
    matches!(verify_local_key_with_expiry(key, candidate), Ok(()))
}

pub fn verify_local_key_with_expiry(
    key: &str,
    candidate: &GatewayLocalKey,
) -> Result<(), LocalKeyVerifyError> {
    if !candidate.enabled {
        return Err(LocalKeyVerifyError::Disabled);
    }

    if !key.starts_with("umg_") {
        return Err(LocalKeyVerifyError::Invalid);
    }

    // Check expiration
    if let Some(expires_at) = candidate.expires_at_ms {
        let now = chrono::Utc::now().timestamp_millis();
        if now > expires_at {
            return Err(LocalKeyVerifyError::Expired);
        }
    }

    // Check quota
    if let Some(max_requests) = candidate.max_requests {
        if candidate.request_count >= max_requests {
            return Err(LocalKeyVerifyError::QuotaExceeded);
        }
    }

    // v2 stores the raw value for cross-platform reveal; v1 records contain
    // only a salted verifier. Keep both paths valid during migration.
    let verified = if !candidate.secret_hash.is_empty() && !candidate.secret_salt.is_empty() {
        let actual = hash_local_key(key, &candidate.secret_salt);
        constant_time_eq(actual.as_bytes(), candidate.secret_hash.as_bytes())
    } else if !candidate.secret.is_empty() {
        // Last-resort compatibility for malformed/imported records that have
        // a raw value but no verifier. Normal v2 records always take the hash
        // branch above.
        constant_time_eq(key.as_bytes(), candidate.secret.as_bytes())
    } else {
        false
    };
    if !verified {
        return Err(LocalKeyVerifyError::Invalid);
    }

    Ok(())
}

/// Returns the upstream auth header `(name, value)` for a protocol:
/// - OpenAI: `Authorization: Bearer {secret}`
/// - Anthropic: `x-api-key: {secret}`
/// - Gemini: `x-goog-api-key: {secret}`
///
/// Single source of truth shared by the gateway forwarder
/// (`proxy/handlers/gateway.rs`) and the connectivity probe
/// (`gateway/probe.rs`) so credential injection can never diverge.
pub fn upstream_auth_header(protocol: GatewayProtocol, secret: &str) -> (&'static str, String) {
    match protocol {
        GatewayProtocol::OpenAiChatCompletions | GatewayProtocol::OpenAiResponses => {
            ("authorization", format!("Bearer {secret}"))
        }
        GatewayProtocol::AnthropicMessages => ("x-api-key", secret.to_string()),
        GatewayProtocol::GeminiGenerateContent => ("x-goog-api-key", secret.to_string()),
    }
}

pub fn select_upstream_key(profile: &GatewayProfile) -> Option<&GatewayUpstreamKey> {
    select_upstream_key_excluding(profile, &[])
}

pub fn select_upstream_key_excluding<'a>(
    profile: &'a GatewayProfile,
    excluded_key_ids: &[String],
) -> Option<&'a GatewayUpstreamKey> {
    let mut candidates: Vec<&GatewayUpstreamKey> = profile
        .upstream_keys
        .iter()
        .filter(|key| {
            key.enabled
                && !excluded_key_ids.iter().any(|id| id == &key.id)
                && upstream_key_available(&profile.id, &key.id)
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }

    match profile.dispatch_strategy {
        GatewayDispatchStrategy::PriorityFailover => {
            candidates.sort_by_key(|key| key.priority);
            candidates.into_iter().next()
        }
        GatewayDispatchStrategy::RoundRobin => {
            let mut cursors = ROUND_ROBIN_CURSORS.lock().ok()?;
            let cursor = cursors.entry(profile.id.clone()).or_default();
            let selected = candidates[(*cursor as usize) % candidates.len()];
            *cursor = cursor.wrapping_add(1);
            Some(selected)
        }
        GatewayDispatchStrategy::Random => {
            let index = OsRng.gen_range(0..candidates.len());
            candidates.into_iter().nth(index)
        }
        GatewayDispatchStrategy::Weighted => {
            let total_weight: u64 = candidates
                .iter()
                .map(|key| u64::from(key.weight.max(1)))
                .sum();
            let mut draw = OsRng.next_u64() % total_weight;
            candidates.into_iter().find(|key| {
                let weight = u64::from(key.weight.max(1));
                if draw < weight {
                    true
                } else {
                    draw -= weight;
                    false
                }
            })
        }
    }
}

fn upstream_key_available(profile_id: &str, key_id: &str) -> bool {
    let Ok(health) = KEY_HEALTH.lock() else {
        return true;
    };
    health
        .get(&format!("{profile_id}:{key_id}"))
        .and_then(|entry| entry.cooling_until)
        .map(|until| until <= Instant::now())
        .unwrap_or(true)
}

pub fn upstream_secret_ref(profile_id: &str, key_id: &str) -> String {
    format!("gateway:{profile_id}:{key_id}")
}

pub fn local_secret_ref(profile_id: &str, key_id: &str) -> String {
    format!("gateway-local:{profile_id}:{key_id}")
}

pub fn is_expected_upstream_secret_ref(profile_id: &str, key: &GatewayUpstreamKey) -> bool {
    !key.secret_ref.is_empty() && key.secret_ref == upstream_secret_ref(profile_id, &key.id)
}

pub fn is_expected_local_secret_ref(profile_id: &str, key: &GatewayLocalKey) -> bool {
    !key.secret_ref.is_empty() && key.secret_ref == local_secret_ref(profile_id, &key.id)
}

/// Returns a v2 upstream secret without consulting an operating-system store.
#[allow(dead_code)]
pub fn local_upstream_secret(key: &GatewayUpstreamKey) -> Option<&str> {
    (!key.secret.is_empty()).then_some(key.secret.as_str())
}

/// Returns a v2 local client key without consulting an operating-system store.
#[allow(dead_code)]
pub fn local_client_secret(key: &GatewayLocalKey) -> Option<&str> {
    (!key.secret.is_empty()).then_some(key.secret.as_str())
}

/// Updates only in-memory health state. It contains no credential material and
/// deliberately resets after an application restart.
///
/// Request-specific authorization failures (403) and ordinary client errors do
/// not poison a credential. Invalid credentials require two consecutive 401s;
/// transport/server failures require three consecutive failures. Rate limiting
/// uses a short fixed cooldown so a busy key can rejoin rotation promptly.
pub fn report_upstream_outcome(profile_id: &str, key_id: &str, outcome: UpstreamOutcome) {
    let mut health = match KEY_HEALTH.lock() {
        Ok(health) => health,
        Err(_) => return,
    };
    let key = format!("{profile_id}:{key_id}");
    let entry = health.entry(key).or_default();
    entry.half_open_probe_in_flight = false;
    match outcome {
        UpstreamOutcome::Success => *entry = KeyHealth::default(),
        UpstreamOutcome::InvalidCredentials => {
            entry.consecutive_auth_failures = entry.consecutive_auth_failures.saturating_add(1);
            entry.consecutive_failures = 0;
            if entry.consecutive_auth_failures >= CIRCUIT_AUTH_FAILURE_THRESHOLD {
                open_circuit(entry);
            }
        }
        UpstreamOutcome::PaymentRequired => {
            // Payment/quota state is credential scoped, but may be fixed quickly.
            entry.consecutive_auth_failures = 0;
            entry.consecutive_failures = 0;
            open_circuit(entry);
        }
        UpstreamOutcome::RateLimited => {
            entry.consecutive_auth_failures = 0;
            entry.consecutive_failures = 0;
            entry.half_open_probe_in_flight = false;
            entry.cooling_until = Some(Instant::now() + CIRCUIT_RATE_LIMIT_COOLDOWN);
        }
        UpstreamOutcome::ServerError | UpstreamOutcome::TransportError => {
            entry.consecutive_auth_failures = 0;
            entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
            if entry.consecutive_failures >= CIRCUIT_FAILURE_THRESHOLD {
                open_circuit(entry);
            }
        }
        UpstreamOutcome::PermissionDenied | UpstreamOutcome::ClientError => {
            // 403 commonly means model/organization/request permission rather than
            // an invalid API key. Do not remove the key from future routing.
            entry.consecutive_auth_failures = 0;
            entry.consecutive_failures = 0;
        }
    }
}

pub fn clear_upstream_runtime_state(profile_id: &str, key_id: &str) {
    if let Ok(mut health) = KEY_HEALTH.lock() {
        health.remove(&format!("{profile_id}:{key_id}"));
    }
}

pub fn clear_profile_runtime_state(profile_id: &str) {
    if let Ok(mut health) = KEY_HEALTH.lock() {
        let prefix = format!("{profile_id}:");
        health.retain(|key, _| !key.starts_with(&prefix));
    }
    if let Ok(mut cursors) = ROUND_ROBIN_CURSORS.lock() {
        cursors.remove(profile_id);
    }
}

fn open_circuit(entry: &mut KeyHealth) {
    // Cap trip_count to prevent potential overflow and limit exponential backoff
    entry.trip_count = entry.trip_count.saturating_add(1).min(10);
    entry.consecutive_failures = 0;
    entry.consecutive_auth_failures = 0;
    entry.half_open_probe_in_flight = false;
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

/// Reads a v1 macOS Keychain entry. It is intentionally not used by new
/// gateway operations; callers should migrate the returned value into `secret`.
pub fn load_legacy_keychain_secret(secret_ref: &str) -> Result<String, String> {
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
        String::from_utf8(output.stdout)
            .map(|secret| secret.trim_end_matches('\n').to_string())
            .map_err(|_| "ERR_GATEWAY_UPSTREAM_KEY_UNAVAILABLE".to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = secret_ref;
        Err("ERR_GATEWAY_CREDENTIAL_STORE_UNAVAILABLE".to_string())
    }
}

/// Best-effort cleanup for v1 Keychain records after a successful migration.
/// New v2 credentials never call this function.
pub fn delete_legacy_keychain_secret(secret_ref: &str) {
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
    // Compact, unguessable routing id: `p` + 6 random lowercase alphanumerics
    // (36^6 ≈ 2.2e9 space). It is an opaque string used only for routing and
    // statistics attribution; nothing parses its internal structure, so this
    // format may evolve without migrating existing profiles.
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = [0u8; 6];
    OsRng.fill_bytes(&mut bytes);
    let mut id = String::with_capacity(7);
    id.push('p');
    for byte in bytes {
        id.push(ALPHABET[(byte as usize) % ALPHABET.len()] as char);
    }
    id
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
            upstream_secret: None,
        }
    }

    #[test]
    fn creates_normalized_non_secret_profile() {
        // 使用公网 IP 字面量，避免测试触发真实 DNS 解析。
        let profile = create_profile(input("  https://8.8.8.8/  ")).expect("profile");
        assert_eq!(profile.id.len(), 7);
        assert!(profile.id.starts_with('p'));
        assert!(profile
            .id
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit()));
        assert_eq!(profile.name, "DeepSeek");
        assert_eq!(profile.base_url, "https://8.8.8.8");
        assert_eq!(profile.client_label, "Cursor");
    }

    #[test]
    fn normalize_models_dedupes_filters_and_caps() {
        let model = |id: &str| GatewayUpstreamModel {
            id: id.to_string(),
            name: None,
            owned_by: None,
        };
        let many: Vec<_> = (0..250).map(|index| model(&format!("m-{index}"))).collect();
        let normalized = normalize_upstream_models(vec![
            model("gemini-1.5-pro"),
            model("a/b"),            // unsafe for the request path
            model("gemini-1.5-pro"), // duplicate, first-seen wins
            model("deepseek-chat"),
        ]);
        assert_eq!(
            normalized
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["gemini-1.5-pro", "deepseek-chat"]
        );
        assert_eq!(normalize_upstream_models(many).len(), MAX_UPSTREAM_MODELS);
        assert!(normalize_upstream_models(vec![model("")]).is_empty());
        assert!(normalize_upstream_models(vec![model("a?b")]).is_empty());
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
        // 公网 IP 字面量，不走真实 DNS。
        let profile = create_profile(input("https://8.8.8.8/v1")).expect("profile");
        assert_eq!(profile.base_url, "https://8.8.8.8/v1");
    }

    struct FakeResolver(Vec<IpAddr>);
    impl HostResolver for FakeResolver {
        fn resolve(&self, _host: &str) -> Vec<IpAddr> {
            self.0.clone()
        }
    }

    fn profile_with_base_url(base_url: &str) -> GatewayProfile {
        GatewayProfile {
            id: "gateway-test".to_string(),
            name: "Test Profile".to_string(),
            protocol: GatewayProtocol::OpenAiChatCompletions,
            base_url: base_url.to_string(),
            enabled: true,
            client_label: String::new(),
            auth_mode: GatewayAuthMode::ClientPassthrough,
            dispatch_strategy: GatewayDispatchStrategy::RoundRobin,
            upstream_keys: Vec::new(),
            local_keys: Vec::new(),
            upstream_models: Vec::new(),
        }
    }

    #[test]
    fn rejects_profile_resolving_to_private_ip() {
        let profile = profile_with_base_url("https://public.example.com/v1");
        for ip in [
            "127.0.0.1".parse::<IpAddr>().unwrap(),
            "10.1.2.3".parse::<IpAddr>().unwrap(),
            "192.168.1.1".parse::<IpAddr>().unwrap(),
            "169.254.10.20".parse::<IpAddr>().unwrap(),
            "::1".parse::<IpAddr>().unwrap(),
            "fc00::1".parse::<IpAddr>().unwrap(),
            "::ffff:127.0.0.1".parse::<IpAddr>().unwrap(),
        ] {
            assert_eq!(
                validate_profile_with_resolver(&profile, &FakeResolver(vec![ip])),
                Err("ERR_GATEWAY_BASE_URL_RESOLVES_TO_PRIVATE".to_string()),
                "resolved {ip} should be rejected"
            );
        }
    }

    #[test]
    fn accepts_profile_resolving_to_public_ip() {
        let profile = profile_with_base_url("https://public.example.com/v1");
        assert_eq!(
            validate_profile_with_resolver(
                &profile,
                &FakeResolver(vec!["8.8.8.8".parse::<IpAddr>().unwrap()]),
            ),
            Ok(())
        );
    }

    #[test]
    fn accepts_profile_when_resolution_fails() {
        // 解析失败（空结果）应放行，与生产行为一致：由实际 HTTPS 请求报错。
        let profile = profile_with_base_url("https://public.example.com/v1");
        assert_eq!(
            validate_profile_with_resolver(&profile, &FakeResolver(vec![])),
            Ok(())
        );
    }

    #[test]
    fn rejects_profile_when_any_resolved_ip_is_private() {
        // 一个私网 IP 混在公网解析结果中也要拒绝。
        let profile = profile_with_base_url("https://public.example.com/v1");
        assert_eq!(
            validate_profile_with_resolver(
                &profile,
                &FakeResolver(vec![
                    "8.8.8.8".parse::<IpAddr>().unwrap(),
                    "10.0.0.5".parse::<IpAddr>().unwrap(),
                ]),
            ),
            Err("ERR_GATEWAY_BASE_URL_RESOLVES_TO_PRIVATE".to_string())
        );
    }

    #[test]
    fn preserves_protocol_without_conversion() {
        let profile = create_profile(GatewayProfileInput {
            protocol: GatewayProtocol::AnthropicMessages,
            ..input("https://8.8.8.8")
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
    fn generated_local_key_keeps_v2_secret_for_direct_reads() {
        let (metadata, generated) = create_local_key(
            "gateway-test",
            GatewayLocalKeyInput {
                remark: String::new(),
            },
        )
        .expect("local key");
        assert_eq!(metadata.secret, generated.key);
        assert!(metadata.secret_ref.is_empty());
        assert_eq!(local_client_secret(&metadata), Some(generated.key.as_str()));
    }

    #[test]
    fn legacy_local_key_verifies_from_hash_when_secret_is_absent() {
        let (mut metadata, generated) = create_local_key(
            "gateway-test",
            GatewayLocalKeyInput {
                remark: String::new(),
            },
        )
        .expect("local key");
        metadata.secret.clear();
        assert!(verify_local_key(&generated.key, &metadata));
        assert_eq!(local_client_secret(&metadata), None);
    }

    #[test]
    fn profile_view_marks_legacy_credentials_that_need_manual_recovery() {
        let mut profile = create_profile(input("https://8.8.8.8")).expect("profile");
        profile.id = "gateway-legacy".to_string();
        profile.upstream_keys = vec![GatewayUpstreamKey {
            id: "upstream-legacy".to_string(),
            remark: String::new(),
            enabled: false,
            weight: 1,
            priority: 0,
            secret: String::new(),
            secret_ref: upstream_secret_ref(&profile.id, "upstream-legacy"),
        }];
        let (mut local_key, _) = create_local_key(
            &profile.id,
            GatewayLocalKeyInput {
                remark: String::new(),
            },
        )
        .expect("local key");
        local_key.secret.clear();
        local_key.secret_ref = local_secret_ref(&profile.id, &local_key.id);
        profile.local_keys = vec![local_key];

        let view = GatewayProfileView::from(&profile);

        assert!(view.credential_recovery.upstream_key_required);
        assert!(view.credential_recovery.local_key_rotation_recommended);
    }

    #[test]
    fn upstream_key_persists_v2_secret_without_keychain_reference() {
        let (key, returned_secret) = create_upstream_key(
            "gateway-test",
            GatewayUpstreamKeyInput {
                remark: "primary".to_string(),
                secret: " sk-test ".to_string(),
                enabled: true,
                weight: 1,
                priority: 0,
            },
        )
        .expect("upstream key");
        assert_eq!(key.secret, "sk-test");
        assert_eq!(returned_secret, "sk-test");
        assert!(key.secret_ref.is_empty());
        assert_eq!(local_upstream_secret(&key), Some("sk-test"));
    }

    fn health_test_profile(profile_id: &str, key_id: &str) -> GatewayProfile {
        let mut profile = create_profile(input("https://8.8.8.8")).unwrap();
        profile.id = profile_id.to_string();
        profile.upstream_keys = vec![GatewayUpstreamKey {
            id: key_id.to_string(),
            remark: String::new(),
            enabled: true,
            weight: 1,
            priority: 0,
            secret: "legacy-secret".to_string(),
            secret_ref: upstream_secret_ref(profile_id, key_id),
        }];
        profile
    }

    #[test]
    fn permission_denied_does_not_poison_key_health() {
        let profile = health_test_profile("gateway-health-403", "key-403");
        report_upstream_outcome(
            &profile.id,
            &profile.upstream_keys[0].id,
            UpstreamOutcome::PermissionDenied,
        );
        assert!(select_upstream_key(&profile).is_some());
        clear_profile_runtime_state(&profile.id);
    }

    #[test]
    fn invalid_credentials_require_two_consecutive_failures() {
        let profile = health_test_profile("gateway-health-401", "key-401");
        let key_id = &profile.upstream_keys[0].id;
        report_upstream_outcome(&profile.id, key_id, UpstreamOutcome::InvalidCredentials);
        assert!(select_upstream_key(&profile).is_some());
        report_upstream_outcome(&profile.id, key_id, UpstreamOutcome::InvalidCredentials);
        assert!(select_upstream_key(&profile).is_none());
        clear_profile_runtime_state(&profile.id);
    }

    #[test]
    fn server_errors_require_three_consecutive_failures() {
        let profile = health_test_profile("gateway-health-500", "key-500");
        let key_id = &profile.upstream_keys[0].id;
        report_upstream_outcome(&profile.id, key_id, UpstreamOutcome::ServerError);
        report_upstream_outcome(&profile.id, key_id, UpstreamOutcome::ServerError);
        assert!(select_upstream_key(&profile).is_some());
        report_upstream_outcome(&profile.id, key_id, UpstreamOutcome::ServerError);
        assert!(select_upstream_key(&profile).is_none());
        clear_profile_runtime_state(&profile.id);
    }

    #[test]
    fn priority_failover_selects_the_lowest_priority_key_first() {
        let mut profile = create_profile(input("https://8.8.8.8")).unwrap();
        profile.dispatch_strategy = GatewayDispatchStrategy::PriorityFailover;
        profile.upstream_keys = vec![
            GatewayUpstreamKey {
                id: "a".to_string(),
                remark: String::new(),
                enabled: true,
                weight: 1,
                priority: 20,
                secret: String::new(),
                secret_ref: "a".to_string(),
            },
            GatewayUpstreamKey {
                id: "b".to_string(),
                remark: String::new(),
                enabled: true,
                weight: 1,
                priority: 1,
                secret: String::new(),
                secret_ref: "b".to_string(),
            },
        ];
        assert_eq!(select_upstream_key(&profile).unwrap().id, "b");
    }

    #[test]
    fn selection_skips_keys_in_cooldown() {
        let mut profile = health_test_profile("gateway-health-selection", "key-a");
        profile.dispatch_strategy = GatewayDispatchStrategy::PriorityFailover;
        profile.upstream_keys.push(GatewayUpstreamKey {
            id: "key-b".to_string(),
            remark: String::new(),
            enabled: true,
            weight: 1,
            priority: 1,
            secret: "backup-secret".to_string(),
            secret_ref: upstream_secret_ref(&profile.id, "key-b"),
        });
        report_upstream_outcome(&profile.id, "key-a", UpstreamOutcome::PaymentRequired);
        assert_eq!(select_upstream_key(&profile).unwrap().id, "key-b");
        clear_profile_runtime_state(&profile.id);
    }

    #[test]
    fn upstream_auth_header_uses_bearer_for_openai_and_raw_for_others() {
        let (name, value) = upstream_auth_header(GatewayProtocol::OpenAiChatCompletions, "sk-test");
        assert_eq!(name, "authorization");
        assert_eq!(value, "Bearer sk-test");

        let (name, value) = upstream_auth_header(GatewayProtocol::OpenAiResponses, "sk-test");
        assert_eq!(name, "authorization");
        assert_eq!(value, "Bearer sk-test");

        let (name, value) = upstream_auth_header(GatewayProtocol::AnthropicMessages, "ant-key");
        assert_eq!(name, "x-api-key");
        assert_eq!(value, "ant-key");

        let (name, value) = upstream_auth_header(GatewayProtocol::GeminiGenerateContent, "gem-key");
        assert_eq!(name, "x-goog-api-key");
        assert_eq!(value, "gem-key");
    }
}
