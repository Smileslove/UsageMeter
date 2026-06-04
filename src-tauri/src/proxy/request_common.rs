use super::openai_forwarder::OpenAiForwarder;
use super::source_detector::{
    detect_source_info, normalize_base_url, register_source_to_settings, SourceRegistrationResult,
};
use super::source_registry::{ProxySourceHandle, ProxySourceRegistry};
use super::types::{ProxyState, RequestContext};
use crate::commands::{load_settings, save_settings_internal};
use crate::models::AppSettings;
use http_body_util::BodyExt;
use hyper::{Request, Response, StatusCode};
use std::fs;
use std::sync::Arc;
use std::time::SystemTime;
use tauri::Emitter;

pub type BoxBody = http_body_util::combinators::UnsyncBoxBody<bytes::Bytes, std::io::Error>;
pub type ProxyResponse = Response<BoxBody>;
pub type ProxyErrorResponse = Box<ProxyResponse>;
pub type HandlerResult = Result<Response<BoxBody>, hyper::Error>;

#[derive(Debug, Clone)]
pub(crate) struct ClientRoute {
    pub normalized_path: String,
    pub client_tool: String,
    pub proxy_profile_id: Option<String>,
    pub detection_method: String,
    pub target_base_url: Option<String>,
    pub provider_id: Option<String>,
    pub source_id: Option<String>,
}

fn trim_path_prefix(prefix: &str) -> String {
    prefix.trim().trim_matches('/').to_string()
}

fn strip_prefix_path(path: &str, prefix: &str) -> Option<String> {
    let clean_prefix = trim_path_prefix(prefix);
    if clean_prefix.is_empty() {
        return None;
    }
    let full_prefix = format!("/{clean_prefix}");
    if path == full_prefix {
        return Some("/".to_string());
    }
    path.strip_prefix(&(full_prefix + "/"))
        .map(|rest| format!("/{rest}"))
}

pub(crate) fn settings_file_mtime() -> Option<SystemTime> {
    let path = AppSettings::settings_path().ok()?;
    fs::metadata(path).ok()?.modified().ok()
}

pub(crate) fn detect_client_route(path: &str, settings: &AppSettings) -> ClientRoute {
    for profile in settings
        .client_tools
        .profiles
        .iter()
        .filter(|profile| profile.enabled)
    {
        if let Some(normalized_path) = strip_prefix_path(path, &profile.path_prefix) {
            let (normalized_path, provider_id, source_id, detection_method) =
                if profile.tool == "opencode" {
                    let (normalized_path, provider_id, source_id) =
                        strip_opencode_provider_source_path(&normalized_path);
                    let detection_method = if provider_id.is_some() && source_id.is_some() {
                        "path_prefix_provider_source".to_string()
                    } else if source_id.is_some() {
                        "path_prefix_source".to_string()
                    } else {
                        "path_prefix".to_string()
                    };
                    (normalized_path, provider_id, source_id, detection_method)
                } else {
                    let (normalized_path, source_id) = strip_source_handle_path(&normalized_path);
                    let detection_method = if source_id.is_some() {
                        "path_prefix_source".to_string()
                    } else {
                        "path_prefix".to_string()
                    };
                    (normalized_path, None, source_id, detection_method)
                };
            return ClientRoute {
                normalized_path,
                client_tool: profile.tool.clone(),
                proxy_profile_id: Some(profile.id.clone()),
                detection_method,
                target_base_url: profile.target_base_url.clone(),
                provider_id,
                source_id,
            };
        }
    }

    ClientRoute {
        normalized_path: path.to_string(),
        client_tool: "unknown".to_string(),
        proxy_profile_id: None,
        detection_method: "unmatched_path".to_string(),
        target_base_url: None,
        provider_id: None,
        source_id: None,
    }
}

pub(crate) async fn get_settings_snapshot(state: &Arc<ProxyState>) -> AppSettings {
    state.settings_snapshot.read().await.clone()
}

pub(crate) async fn store_settings_snapshot(
    state: &Arc<ProxyState>,
    settings: AppSettings,
    mtime: Option<SystemTime>,
) {
    *state.settings_snapshot.write().await = settings;
    *state.settings_file_mtime.write().await = mtime;
}

pub(crate) async fn persist_proxy_settings(
    state: &Arc<ProxyState>,
    settings: AppSettings,
) -> Result<(), String> {
    save_settings_internal(settings.clone()).map_err(String::from)?;
    store_settings_snapshot(state, settings, settings_file_mtime()).await;
    Ok(())
}

pub(crate) async fn refresh_settings_snapshot_if_needed(state: &Arc<ProxyState>) {
    let current_mtime = settings_file_mtime();
    {
        let known_mtime = state.settings_file_mtime.read().await;
        if *known_mtime == current_mtime {
            return;
        }
    }

    match load_settings() {
        Ok(settings) => {
            store_settings_snapshot(state, settings, current_mtime).await;
        }
        Err(err) => {
            eprintln!("[proxy] Failed to reload settings snapshot: {}", err);
        }
    }
}

pub(crate) async fn register_source_for_runtime(
    state: &Arc<ProxyState>,
    api_key: &str,
    target_base_url: &str,
) -> SourceRegistrationResult {
    let mut settings = state.settings_snapshot.write().await;
    let result = register_source_to_settings(&mut settings, api_key, target_base_url);
    let settings_to_persist = if result.is_new {
        Some(settings.clone())
    } else {
        None
    };
    drop(settings);

    if let Some(settings) = settings_to_persist {
        if let Err(err) = persist_proxy_settings(state, settings).await {
            eprintln!("[proxy] Failed to persist source state: {}", err);
        }
    }

    result
}

pub(crate) fn strip_source_handle_path(path: &str) -> (String, Option<String>) {
    let clean_path = path.trim();
    let Some(rest) = clean_path.strip_prefix("/source/") else {
        return (path.to_string(), None);
    };

    let mut parts = rest.splitn(2, '/');
    let source_id = parts.next().unwrap_or_default().trim();
    if source_id.is_empty() {
        return (path.to_string(), None);
    }

    let normalized_path = parts
        .next()
        .map(|tail| format!("/{tail}"))
        .unwrap_or_else(|| "/".to_string());
    (normalized_path, Some(source_id.to_string()))
}

pub(crate) fn strip_opencode_provider_source_path(
    path: &str,
) -> (String, Option<String>, Option<String>) {
    let clean_path = path.trim();
    if let Some(rest) = clean_path.strip_prefix("/provider/") {
        let mut parts = rest.splitn(4, '/');
        let provider_id = parts.next().unwrap_or_default().trim();
        let marker = parts.next().unwrap_or_default().trim();
        let source_id = parts.next().unwrap_or_default().trim();
        if provider_id.is_empty() || marker != "source" || source_id.is_empty() {
            return (path.to_string(), None, None);
        }
        let normalized_path = parts
            .next()
            .map(|tail| format!("/{tail}"))
            .unwrap_or_else(|| "/".to_string());
        return (
            normalized_path,
            Some(provider_id.to_string()),
            Some(source_id.to_string()),
        );
    }

    let (normalized_path, source_id) = strip_source_handle_path(clean_path);
    (normalized_path, None, source_id)
}

pub(crate) fn resolve_route_source(
    client_route_source_id: Option<&str>,
    active_source_id: Option<&str>,
) -> Result<Option<ProxySourceHandle>, String> {
    let registry = ProxySourceRegistry::new();
    if let Some(id) = client_route_source_id {
        return registry
            .get(id)
            .map(Some)
            .ok_or_else(|| format!("Proxy source handle '{}' was not found", id));
    }

    Ok(active_source_id.and_then(|id| registry.get(id)))
}

pub(crate) fn resolve_target_base_url(
    source_base_url: Option<&str>,
    route_target_base_url: Option<&str>,
    tool: &str,
) -> Result<String, String> {
    source_base_url
        .or(route_target_base_url)
        .map(str::to_string)
        .ok_or_else(|| {
            format!(
                "{tool} proxy target base URL is not configured; refusing to guess an upstream target"
            )
        })
}

pub(crate) fn append_query(path: &str, query: Option<&str>) -> String {
    match query {
        Some(query) if !query.is_empty() => format!("{path}?{query}"),
        _ => path.to_string(),
    }
}

pub(crate) fn full<T: Into<bytes::Bytes>>(chunk: T) -> BoxBody {
    http_body_util::Full::new(chunk.into())
        .map_err(|never| match never {})
        .boxed_unsync()
}

pub(crate) fn json_error_response(
    status: StatusCode,
    error_type: &str,
    message: &str,
) -> ProxyResponse {
    let body = serde_json::json!({ "error": { "type": error_type, "message": message } });
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(full(serde_json::to_string(&body).unwrap_or_default()))
        .unwrap()
}

pub(crate) async fn build_request_base_url(
    state: &Arc<ProxyState>,
    auth_token: Option<&str>,
    target_base_url: &str,
) -> (String, Option<String>) {
    if let Some(api_key) = auth_token {
        let settings = get_settings_snapshot(state).await;
        let sources = settings.source_aware.sources;
        let (prefix, base_url, _) = detect_source_info(api_key, target_base_url, &sources);
        (prefix, base_url)
    } else {
        (String::new(), normalize_base_url(target_base_url))
    }
}

pub(crate) async fn resolve_registered_request_base_url(
    state: &Arc<ProxyState>,
    auth_token: Option<&str>,
    target_base_url: &str,
) -> (String, Option<String>) {
    if let Some(api_key) = auth_token {
        let result = register_source_for_runtime(state, api_key, target_base_url).await;
        if result.is_new {
            emit_source_detected(state).await;
        }
        (result.prefix, result.base_url)
    } else {
        build_request_base_url(state, None, target_base_url).await
    }
}

pub(crate) async fn emit_source_detected(state: &Arc<ProxyState>) {
    if let Some(ref app_handle) = *state.app_handle.read().await {
        let _ = app_handle.emit("source_detected", ());
    }
}

pub(crate) fn resolve_registry_source_handle<T, Get, Touch>(
    source_id: Option<&str>,
    tool_name: &str,
    get: Get,
    touch: Touch,
) -> Result<Option<T>, ProxyErrorResponse>
where
    Get: Fn(&str) -> Option<T>,
    Touch: Fn(&str),
{
    match source_id {
        Some(id) => match get(id) {
            Some(handle) => {
                touch(id);
                Ok(Some(handle))
            }
            None => Err(Box::new(json_error_response(
                StatusCode::BAD_GATEWAY,
                "proxy_source_not_found",
                &format!("{tool_name} proxy source handle '{}' was not found", id),
            ))),
        },
        None => Ok(None),
    }
}

pub(crate) async fn get_openai_forwarder(
    state: &Arc<ProxyState>,
) -> Result<Arc<OpenAiForwarder>, ProxyErrorResponse> {
    let openai_forwarder = {
        let guard = state.openai_forwarder.read().await;
        guard.clone()
    };
    openai_forwarder.ok_or_else(|| {
        Box::new(json_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "proxy_error",
            "OpenAI forwarder not initialized",
        ))
    })
}

pub(crate) fn apply_request_identity(
    mut context: RequestContext,
    client_route: &ClientRoute,
    api_key_prefix: String,
    request_base_url: Option<String>,
    target_base_url: String,
) -> RequestContext {
    context.api_key_prefix = if api_key_prefix.is_empty() {
        None
    } else {
        Some(api_key_prefix)
    };
    context.request_base_url = request_base_url;
    context.client_tool = client_route.client_tool.clone();
    context.proxy_profile_id = client_route.proxy_profile_id.clone();
    context.client_detection_method = client_route.detection_method.clone();
    context.target_base_url = Some(target_base_url);
    context
}

pub(crate) async fn collect_body(
    req: Request<hyper::body::Incoming>,
) -> Result<(hyper::HeaderMap, bytes::Bytes), hyper::Error> {
    let headers = req.headers().clone();
    let body = req.collect().await?.to_bytes();
    Ok((headers, body))
}
