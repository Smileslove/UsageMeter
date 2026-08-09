//! Gateway upstream connectivity probing.
//!
//! Provides model-list discovery and single-model availability tests against
//! a saved profile's upstream. Probes use an ephemeral request path that does
//! NOT flow through the gateway forwarder, so they never produce
//! [`UsageRecord`]s, never trip the circuit breaker, and never count toward
//! usage statistics. The probe client disables HTTP redirects so upstream
//! auth headers can never leak to a host outside the configured base URL.
//!
//! Credential injection delegates to [`crate::gateway::upstream_auth_header`],
//! the single source of truth shared with the gateway forwarder, so the two
//! paths can never diverge.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::models::{GatewayProfile, GatewayProtocol, GatewayUpstreamModel};
use crate::net::HttpClientFactory;

const PROBE_TIMEOUT_SECS: u64 = 15;
const ANTHROPIC_VERSION: &str = "2023-06-01";
const APP_USER_AGENT: &str = concat!("UsageMeter/", env!("CARGO_PKG_VERSION"));

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayUpstreamModelsResult {
    pub ok: bool,
    pub models: Vec<GatewayUpstreamModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayModelTestResult {
    pub ok: bool,
    pub model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
}

// ---------------------------------------------------------------------------
// Protocol adaptation (pure functions, unit-tested)
// ---------------------------------------------------------------------------

/// Returns the protocol's API base path (`/v1` for OpenAI/Anthropic,
/// `/v1beta` for Gemini).
pub fn protocol_base_path(protocol: GatewayProtocol) -> &'static str {
    match protocol {
        GatewayProtocol::OpenAiChatCompletions | GatewayProtocol::OpenAiResponses => "/v1",
        GatewayProtocol::AnthropicMessages => "/v1",
        GatewayProtocol::GeminiGenerateContent => "/v1beta",
    }
}

/// Trims whitespace and trailing slashes so downstream joins are predictable
/// regardless of how the user typed the base URL.
pub fn normalize_gateway_base_url(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_string()
}

/// Case-insensitive check for whether `base` already ends with `base_path`.
/// `base_path` always starts with `/`, so `ends_with` naturally enforces a
/// path-segment boundary (`/v1` never matches the tail of `/v1beta`, and
/// `/V1` is still recognized).
fn ends_with_base_path_ignoring_case(base: &str, base_path: &str) -> bool {
    base.trim_end_matches('/')
        .to_ascii_lowercase()
        .ends_with(base_path)
}

/// Joins `base_url`, a protocol `base_path` and a `suffix` into a full
/// endpoint. Tolerates a `base_url` that already ends with `base_path`
/// (e.g. `https://api.openai.com/v1`, `/V1`, or a trailing slash) without
/// doubling it.
fn join_endpoint(base_url: &str, base_path: &str, suffix: &str) -> String {
    let base = normalize_gateway_base_url(base_url);
    let base = if ends_with_base_path_ignoring_case(&base, base_path) {
        base
    } else {
        format!("{base}{base_path}")
    };
    format!("{base}{suffix}")
}

/// Normalizes a base URL and appends the protocol's base path unless it is
/// already present. Used by probing and model testing, where this app builds
/// the request path itself.
pub fn effective_gateway_base_url(base_url: &str, protocol: GatewayProtocol) -> String {
    join_endpoint(base_url, protocol_base_path(protocol), "")
}

/// Removes a redundant protocol base path from a base URL. Forwarded requests
/// carry the base path inside the client's own request path (e.g.
/// `/v1/messages`), so a base URL that already ends with `/v1` must not be
/// forwarded as `.../v1/v1/messages`. Non-matching custom paths are kept.
pub fn strip_redundant_base_path(base_url: &str, protocol: GatewayProtocol) -> String {
    let base_path = protocol_base_path(protocol);
    let base = normalize_gateway_base_url(base_url);
    if ends_with_base_path_ignoring_case(&base, base_path) {
        base[..base.len() - base_path.len()]
            .trim_end_matches('/')
            .to_string()
    } else {
        base
    }
}

/// Computes the upstream base URL for a forwarded request. When the client's
/// request path already carries the protocol base path (e.g. `/v1/messages`),
/// any matching base path in `base_url` is stripped so it is not doubled;
/// otherwise the base URL is kept as entered, because the profile supplies
/// the base path in that case.
pub fn forwarding_base_url(
    base_url: &str,
    protocol: GatewayProtocol,
    request_path: &str,
) -> String {
    let base = normalize_gateway_base_url(base_url);
    if request_path
        .to_ascii_lowercase()
        .starts_with(protocol_base_path(protocol))
    {
        strip_redundant_base_path(&base, protocol)
    } else {
        base
    }
}

/// A representative operation path per protocol, used by the base URL preview
/// so users can see where a real request would go.
pub fn sample_request_path(protocol: GatewayProtocol) -> &'static str {
    match protocol {
        GatewayProtocol::OpenAiChatCompletions => "/chat/completions",
        GatewayProtocol::OpenAiResponses => "/responses",
        GatewayProtocol::AnthropicMessages => "/messages",
        GatewayProtocol::GeminiGenerateContent => "/models",
    }
}

/// Result of the base URL preview command: the effective upstream URL after
/// normalization and base-path handling, plus whether the base path was
/// appended automatically.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayBaseUrlPreview {
    pub base_path: &'static str,
    pub effective_base_url: String,
    pub base_path_added: bool,
    pub sample_request_url: String,
}

/// The model-list discovery endpoint for a protocol.
pub fn models_endpoint(base_url: &str, protocol: GatewayProtocol) -> String {
    join_endpoint(base_url, protocol_base_path(protocol), "/models")
}

/// Builds the `(url, body)` for a minimal single-model test request.
///
/// Each request is intentionally tiny (`max_tokens`/`max_output_tokens` = 1,
/// prompt "ping") to minimize quota consumption while still exercising the
/// model selection path on the upstream.
fn test_request(base_url: &str, protocol: GatewayProtocol, model_id: &str) -> (String, Value) {
    match protocol {
        GatewayProtocol::OpenAiChatCompletions => {
            let url = join_endpoint(base_url, "/v1", "/chat/completions");
            (
                url,
                json!({
                    "model": model_id,
                    "messages": [{"role": "user", "content": "ping"}],
                    "max_tokens": 1,
                    "stream": false
                }),
            )
        }
        GatewayProtocol::OpenAiResponses => {
            let url = join_endpoint(base_url, "/v1", "/responses");
            (
                url,
                json!({
                    "model": model_id,
                    "input": "ping",
                    "max_output_tokens": 1
                }),
            )
        }
        GatewayProtocol::AnthropicMessages => {
            let url = join_endpoint(base_url, "/v1", "/messages");
            (
                url,
                json!({
                    "model": model_id,
                    "messages": [{"role": "user", "content": "ping"}],
                    "max_tokens": 1
                }),
            )
        }
        GatewayProtocol::GeminiGenerateContent => {
            // Gemini encodes the model id in the path. Callers pass ids from
            // the model list or validated by `is_safe_model_id`, so the value
            // is guaranteed path-safe.
            let suffix = format!("/models/{model_id}:generateContent");
            let url = join_endpoint(base_url, "/v1beta", &suffix);
            (
                url,
                json!({
                    "contents": [{"parts": [{"text": "ping"}]}],
                    "generationConfig": {"maxOutputTokens": 1}
                }),
            )
        }
    }
}

/// Model ids are embedded in the Gemini request path, so probe inputs are
/// restricted to path-safe characters. Shared with [`super::is_safe_model_id`].
fn is_safe_model_id(model_id: &str) -> bool {
    super::is_safe_model_id(model_id)
}

/// Parses a model-list response body into normalized model entries.
///
/// - OpenAI / Anthropic: `{"data":[{"id":"...","owned_by":"..."}]}`
///   (Anthropic additionally exposes `display_name`).
/// - Gemini: `{"models":[{"name":"models/gemini-1.5-pro","displayName":"..."}]}`
fn parse_models(protocol: GatewayProtocol, body: &Value) -> Vec<GatewayUpstreamModel> {
    match protocol {
        GatewayProtocol::GeminiGenerateContent => body
            .get("models")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let raw = item.get("name")?.as_str()?;
                        // Gemini returns `models/<id>`; strip the prefix to
                        // expose the id used in `:generateContent` paths.
                        let id = raw.strip_prefix("models/").unwrap_or(raw).to_string();
                        let name = item
                            .get("displayName")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        Some(GatewayUpstreamModel {
                            id,
                            name,
                            owned_by: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => body
            .get("data")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let id = item.get("id")?.as_str()?.to_string();
                        let name = item
                            .get("display_name")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        let owned_by = item
                            .get("owned_by")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        Some(GatewayUpstreamModel { id, name, owned_by })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Maps an HTTP status code to a stable error-kind string consumed by the
/// frontend i18n layer. Kept in sync with [`crate::gateway::UpstreamOutcome`]
/// semantics.
fn classify_status(status: reqwest::StatusCode) -> &'static str {
    match status.as_u16() {
        200..=299 => "success",
        401 => "invalid_credentials",
        402 => "payment_required",
        403 => "permission_denied",
        429 => "rate_limited",
        400..=499 => "client_error",
        500..=599 => "server_error",
        _ => "client_error",
    }
}

/// Returns the secret of the profile's selected upstream key, or `None` when
/// no enabled key with a non-empty secret is available.
fn resolve_upstream_secret(profile: &GatewayProfile) -> Option<&str> {
    let key = super::select_upstream_key(profile)?;
    let secret = key.secret.trim();
    if secret.is_empty() {
        None
    } else {
        Some(secret)
    }
}

/// Builds a one-off reqwest client that reuses the current global proxy
/// configuration but disables HTTP redirects, so an upstream 3xx can never
/// relay auth headers to an off-site host.
fn build_probe_client() -> Result<reqwest::Client, String> {
    let builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(PROBE_TIMEOUT_SECS))
        .user_agent(APP_USER_AGENT)
        .redirect(reqwest::redirect::Policy::none());
    let builder = HttpClientFactory::global().apply_proxy_to_builder(builder);
    builder
        .build()
        .map_err(|e| format!("ERR_GATEWAY_PROBE_CLIENT_BUILD: {e}"))
}

// ---------------------------------------------------------------------------
// Probe entry points
// ---------------------------------------------------------------------------

/// Queries the upstream `/models` endpoint and returns the normalized list.
pub async fn probe_upstream_models(profile: &GatewayProfile) -> GatewayUpstreamModelsResult {
    let Some(secret) = resolve_upstream_secret(profile) else {
        return GatewayUpstreamModelsResult {
            ok: false,
            models: Vec::new(),
            latency_ms: None,
            error_kind: Some("credential_missing".to_string()),
            error_detail: None,
        };
    };

    let client = match build_probe_client() {
        Ok(client) => client,
        Err(err) => {
            return GatewayUpstreamModelsResult {
                ok: false,
                models: Vec::new(),
                latency_ms: None,
                error_kind: Some("transport_error".to_string()),
                error_detail: Some(err),
            };
        }
    };

    let url = models_endpoint(&profile.base_url, profile.protocol);
    let (auth_name, auth_value) = super::upstream_auth_header(profile.protocol, secret);
    let mut request = client.get(&url).header(auth_name, auth_value);
    if profile.protocol == GatewayProtocol::AnthropicMessages {
        request = request.header("anthropic-version", ANTHROPIC_VERSION);
    }

    let start = Instant::now();
    let response = request.send().await;
    let latency_ms = start.elapsed().as_millis() as u64;

    match response {
        Ok(resp) => {
            let status = resp.status();
            let kind = classify_status(status);
            if kind == "success" {
                match resp.json::<Value>().await {
                    Ok(body) => {
                        let models = parse_models(profile.protocol, &body);
                        GatewayUpstreamModelsResult {
                            ok: true,
                            models,
                            latency_ms: Some(latency_ms),
                            error_kind: None,
                            error_detail: None,
                        }
                    }
                    Err(err) => GatewayUpstreamModelsResult {
                        ok: false,
                        models: Vec::new(),
                        latency_ms: Some(latency_ms),
                        error_kind: Some("client_error".to_string()),
                        error_detail: Some(format!("invalid response body: {err}")),
                    },
                }
            } else {
                GatewayUpstreamModelsResult {
                    ok: false,
                    models: Vec::new(),
                    latency_ms: Some(latency_ms),
                    error_kind: Some(kind.to_string()),
                    error_detail: Some(status.to_string()),
                }
            }
        }
        Err(err) => {
            let kind = if err.is_timeout() {
                "timeout"
            } else {
                "transport_error"
            };
            GatewayUpstreamModelsResult {
                ok: false,
                models: Vec::new(),
                latency_ms: None,
                error_kind: Some(kind.to_string()),
                error_detail: Some(err.to_string()),
            }
        }
    }
}

/// Sends a minimal request to verify a specific model is usable on the
/// upstream.
pub async fn test_upstream_model(
    profile: &GatewayProfile,
    model_id: &str,
) -> GatewayModelTestResult {
    if !is_safe_model_id(model_id) {
        return GatewayModelTestResult {
            ok: false,
            model_id: model_id.to_string(),
            latency_ms: None,
            http_status: None,
            error_kind: Some("invalid_model_id".to_string()),
            error_detail: None,
        };
    }

    let Some(secret) = resolve_upstream_secret(profile) else {
        return GatewayModelTestResult {
            ok: false,
            model_id: model_id.to_string(),
            latency_ms: None,
            http_status: None,
            error_kind: Some("credential_missing".to_string()),
            error_detail: None,
        };
    };

    let client = match build_probe_client() {
        Ok(client) => client,
        Err(err) => {
            return GatewayModelTestResult {
                ok: false,
                model_id: model_id.to_string(),
                latency_ms: None,
                http_status: None,
                error_kind: Some("transport_error".to_string()),
                error_detail: Some(err),
            };
        }
    };

    let (url, body) = test_request(&profile.base_url, profile.protocol, model_id);
    let (auth_name, auth_value) = super::upstream_auth_header(profile.protocol, secret);
    let mut request = client.post(&url).json(&body).header(auth_name, auth_value);
    if profile.protocol == GatewayProtocol::AnthropicMessages {
        request = request.header("anthropic-version", ANTHROPIC_VERSION);
    }

    let start = Instant::now();
    let response = request.send().await;
    let latency_ms = start.elapsed().as_millis() as u64;

    match response {
        Ok(resp) => {
            let status = resp.status();
            let kind = classify_status(status);
            GatewayModelTestResult {
                ok: kind == "success",
                model_id: model_id.to_string(),
                latency_ms: Some(latency_ms),
                http_status: Some(status.as_u16()),
                error_kind: if kind == "success" {
                    None
                } else {
                    Some(kind.to_string())
                },
                error_detail: if kind == "success" {
                    None
                } else {
                    Some(status.to_string())
                },
            }
        }
        Err(err) => {
            let kind = if err.is_timeout() {
                "timeout"
            } else {
                "transport_error"
            };
            GatewayModelTestResult {
                ok: false,
                model_id: model_id.to_string(),
                latency_ms: None,
                http_status: None,
                error_kind: Some(kind.to_string()),
                error_detail: Some(err.to_string()),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_path_matches_each_protocol() {
        assert_eq!(
            protocol_base_path(GatewayProtocol::OpenAiChatCompletions),
            "/v1"
        );
        assert_eq!(protocol_base_path(GatewayProtocol::OpenAiResponses), "/v1");
        assert_eq!(
            protocol_base_path(GatewayProtocol::AnthropicMessages),
            "/v1"
        );
        assert_eq!(
            protocol_base_path(GatewayProtocol::GeminiGenerateContent),
            "/v1beta"
        );
    }

    #[test]
    fn join_endpoint_appends_base_path_when_missing() {
        assert_eq!(
            join_endpoint("https://api.openai.com", "/v1", "/models"),
            "https://api.openai.com/v1/models"
        );
    }

    #[test]
    fn join_endpoint_skips_base_path_when_already_present() {
        assert_eq!(
            join_endpoint("https://api.openai.com/v1", "/v1", "/models"),
            "https://api.openai.com/v1/models"
        );
    }

    #[test]
    fn join_endpoint_does_not_match_v1beta_against_v1() {
        assert_eq!(
            join_endpoint(
                "https://generativelanguage.googleapis.com/v1beta",
                "/v1",
                "/models"
            ),
            "https://generativelanguage.googleapis.com/v1beta/v1/models"
        );
    }

    #[test]
    fn join_endpoint_handles_trailing_slash_and_case() {
        assert_eq!(
            join_endpoint("https://api.openai.com/v1/", "/v1", "/models"),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            join_endpoint("https://api.openai.com/V1", "/v1", "/models"),
            "https://api.openai.com/V1/models"
        );
    }

    #[test]
    fn normalize_base_url_trims_whitespace_and_trailing_slashes() {
        assert_eq!(
            normalize_gateway_base_url("  https://api.openai.com/v1/  "),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_gateway_base_url("https://api.openai.com//"),
            "https://api.openai.com"
        );
    }

    #[test]
    fn effective_base_url_appends_base_path_when_missing() {
        assert_eq!(
            effective_gateway_base_url(
                "https://api.openai.com",
                GatewayProtocol::OpenAiChatCompletions
            ),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            effective_gateway_base_url(
                "https://generativelanguage.googleapis.com",
                GatewayProtocol::GeminiGenerateContent
            ),
            "https://generativelanguage.googleapis.com/v1beta"
        );
    }

    #[test]
    fn effective_base_url_dedupes_case_insensitive_and_trailing_slash() {
        assert_eq!(
            effective_gateway_base_url(
                "https://api.openai.com/v1",
                GatewayProtocol::OpenAiChatCompletions
            ),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            effective_gateway_base_url(
                "https://api.openai.com/v1/",
                GatewayProtocol::OpenAiChatCompletions
            ),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            effective_gateway_base_url(
                "https://api.openai.com/V1",
                GatewayProtocol::OpenAiChatCompletions
            ),
            "https://api.openai.com/V1"
        );
    }

    #[test]
    fn strip_redundant_base_path_removes_protocol_base_path() {
        assert_eq!(
            strip_redundant_base_path(
                "https://api.openai.com/v1",
                GatewayProtocol::OpenAiChatCompletions
            ),
            "https://api.openai.com"
        );
        assert_eq!(
            strip_redundant_base_path(
                "https://api.openai.com/v1/",
                GatewayProtocol::OpenAiChatCompletions
            ),
            "https://api.openai.com"
        );
        assert_eq!(
            strip_redundant_base_path(
                "https://api.openai.com/V1",
                GatewayProtocol::OpenAiChatCompletions
            ),
            "https://api.openai.com"
        );
    }

    #[test]
    fn strip_redundant_base_path_keeps_host_root_and_custom_paths() {
        assert_eq!(
            strip_redundant_base_path(
                "https://api.openai.com",
                GatewayProtocol::OpenAiChatCompletions
            ),
            "https://api.openai.com"
        );
        assert_eq!(
            strip_redundant_base_path(
                "https://gateway.example.com/custom",
                GatewayProtocol::AnthropicMessages
            ),
            "https://gateway.example.com/custom"
        );
        // `/v1beta` must not be mistaken for `/v1`.
        assert_eq!(
            strip_redundant_base_path(
                "https://generativelanguage.googleapis.com/v1beta",
                GatewayProtocol::OpenAiChatCompletions
            ),
            "https://generativelanguage.googleapis.com/v1beta"
        );
    }

    #[test]
    fn forwarding_base_url_strips_only_when_client_path_carries_base_path() {
        // Client path has `/v1` -> strip the profile's `/v1` to avoid doubling.
        assert_eq!(
            forwarding_base_url(
                "https://api.openai.com/v1",
                GatewayProtocol::OpenAiChatCompletions,
                "/v1/chat/completions"
            ),
            "https://api.openai.com"
        );
        // Client path has `/v1`, profile base has none -> keep as entered.
        assert_eq!(
            forwarding_base_url(
                "https://api.openai.com",
                GatewayProtocol::OpenAiChatCompletions,
                "/v1/chat/completions"
            ),
            "https://api.openai.com"
        );
        // Client path lacks the base path -> the profile base URL supplies it.
        assert_eq!(
            forwarding_base_url(
                "https://gateway.example.com/v1",
                GatewayProtocol::OpenAiChatCompletions,
                "/chat/completions"
            ),
            "https://gateway.example.com/v1"
        );
        // Gemini client paths carry `/v1beta`.
        assert_eq!(
            forwarding_base_url(
                "https://generativelanguage.googleapis.com/v1beta",
                GatewayProtocol::GeminiGenerateContent,
                "/v1beta/models/gemini-2.0-flash:generateContent"
            ),
            "https://generativelanguage.googleapis.com"
        );
    }

    #[test]
    fn models_endpoint_resolves_per_protocol() {
        assert_eq!(
            models_endpoint(
                "https://api.openai.com",
                GatewayProtocol::OpenAiChatCompletions
            ),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            models_endpoint(
                "https://api.anthropic.com/v1",
                GatewayProtocol::AnthropicMessages
            ),
            "https://api.anthropic.com/v1/models"
        );
        assert_eq!(
            models_endpoint(
                "https://generativelanguage.googleapis.com",
                GatewayProtocol::GeminiGenerateContent
            ),
            "https://generativelanguage.googleapis.com/v1beta/models"
        );
    }

    #[test]
    fn test_request_builds_chat_completions_url_and_body() {
        let (url, body) = test_request(
            "https://api.openai.com",
            GatewayProtocol::OpenAiChatCompletions,
            "gpt-4o-mini",
        );
        assert_eq!(url, "https://api.openai.com/v1/chat/completions");
        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(body["max_tokens"], 1);
        assert_eq!(body["stream"], false);
    }

    #[test]
    fn test_request_builds_gemini_path_with_model_id() {
        let (url, body) = test_request(
            "https://generativelanguage.googleapis.com",
            GatewayProtocol::GeminiGenerateContent,
            "gemini-1.5-pro",
        );
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-pro:generateContent"
        );
        assert_eq!(body["contents"][0]["parts"][0]["text"], "ping");
    }

    #[test]
    fn test_request_respects_existing_base_path() {
        let (url, _) = test_request(
            "https://api.openai.com/v1",
            GatewayProtocol::OpenAiChatCompletions,
            "gpt-4o",
        );
        assert_eq!(url, "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn parse_openai_models_from_data_array() {
        let body = json!({
            "data": [
                {"id": "gpt-4o", "owned_by": "openai"},
                {"id": "gpt-4o-mini", "owned_by": "openai"}
            ]
        });
        let models = parse_models(GatewayProtocol::OpenAiChatCompletions, &body);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-4o");
        assert_eq!(models[0].owned_by.as_deref(), Some("openai"));
    }

    #[test]
    fn parse_anthropic_models_extracts_display_name() {
        let body = json!({
            "data": [
                {"id": "claude-3-opus-20240229", "display_name": "Claude 3 Opus"}
            ]
        });
        let models = parse_models(GatewayProtocol::AnthropicMessages, &body);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "claude-3-opus-20240229");
        assert_eq!(models[0].name.as_deref(), Some("Claude 3 Opus"));
    }

    #[test]
    fn parse_gemini_models_strips_models_prefix() {
        let body = json!({
            "models": [
                {"name": "models/gemini-1.5-pro", "displayName": "Gemini 1.5 Pro"},
                {"name": "models/gemini-1.5-flash", "displayName": "Gemini 1.5 Flash"}
            ]
        });
        let models = parse_models(GatewayProtocol::GeminiGenerateContent, &body);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gemini-1.5-pro");
        assert_eq!(models[0].name.as_deref(), Some("Gemini 1.5 Pro"));
    }

    #[test]
    fn parse_models_returns_empty_for_missing_array() {
        let body = json!({"unexpected": true});
        let models = parse_models(GatewayProtocol::OpenAiChatCompletions, &body);
        assert!(models.is_empty());
    }

    #[test]
    fn classify_status_maps_known_codes() {
        assert_eq!(classify_status(reqwest::StatusCode::OK), "success");
        assert_eq!(
            classify_status(reqwest::StatusCode::UNAUTHORIZED),
            "invalid_credentials"
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::PAYMENT_REQUIRED),
            "payment_required"
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::FORBIDDEN),
            "permission_denied"
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::TOO_MANY_REQUESTS),
            "rate_limited"
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::NOT_FOUND),
            "client_error"
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
            "server_error"
        );
    }
}
