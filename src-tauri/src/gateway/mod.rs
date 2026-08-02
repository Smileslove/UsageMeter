//! Local API gateway domain types and validation.
//!
//! Gateway profiles are intentionally non-secret.  A profile only identifies a
//! compatible upstream and the client label used for later usage attribution.

use crate::models::{GatewayProfile, GatewayProtocol};
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_PROFILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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

pub fn listener_address(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

fn next_profile_id() -> String {
    let timestamp = chrono::Utc::now().timestamp_millis();
    let sequence = NEXT_PROFILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("gateway-{timestamp:x}-{sequence:x}")
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
}
