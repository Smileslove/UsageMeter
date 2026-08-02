//! Tauri commands for the non-secret local API gateway profiles.

use crate::gateway::{self, GatewayProfileInput, GatewayStatus};
use crate::models::{AppSettings, GatewayProfile};
use tauri::State;

use super::usage::ProxyState;
use super::{load_settings_blocking, save_settings_internal};

#[tauri::command]
pub async fn list_gateway_profiles() -> Result<Vec<GatewayProfile>, String> {
    let settings = load_settings_blocking()?;
    Ok(settings.gateway.profiles)
}

async fn refresh_running_proxy_settings(state: &ProxyState, settings: &AppSettings) {
    let server_guard = state.server.read().await;
    if let Some(server) = server_guard.as_ref() {
        server.update_settings_snapshot(settings.clone()).await;
    }
}

#[tauri::command]
pub async fn create_gateway_profile(
    input: GatewayProfileInput,
    state: State<'_, ProxyState>,
) -> Result<GatewayProfile, String> {
    let profile = gateway::create_profile(input)?;
    let mut settings = load_settings_blocking()?;
    settings.gateway.profiles.push(profile.clone());
    save_settings_internal(settings.clone()).map_err(String::from)?;
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(profile)
}

#[tauri::command]
pub async fn update_gateway_profile(
    id: String,
    input: GatewayProfileInput,
    state: State<'_, ProxyState>,
) -> Result<GatewayProfile, String> {
    let profile = gateway::update_profile(&id, input)?;
    let mut settings = load_settings_blocking()?;
    let existing = settings
        .gateway
        .profiles
        .iter_mut()
        .find(|candidate| candidate.id == id)
        .ok_or_else(|| "ERR_GATEWAY_PROFILE_NOT_FOUND".to_string())?;
    *existing = profile.clone();
    save_settings_internal(settings.clone()).map_err(String::from)?;
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(profile)
}

#[tauri::command]
pub async fn delete_gateway_profile(
    id: String,
    state: State<'_, ProxyState>,
) -> Result<(), String> {
    let mut settings = load_settings_blocking()?;
    let original_len = settings.gateway.profiles.len();
    settings.gateway.profiles.retain(|profile| profile.id != id);
    if settings.gateway.profiles.len() == original_len {
        return Err("ERR_GATEWAY_PROFILE_NOT_FOUND".to_string());
    }
    save_settings_internal(settings.clone()).map_err(String::from)?;
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(())
}

#[tauri::command]
pub async fn get_gateway_status(state: State<'_, ProxyState>) -> Result<GatewayStatus, String> {
    let settings = load_settings_blocking()?;
    let server_guard = state.server.read().await;
    let proxy_running = if let Some(server) = server_guard.as_ref() {
        server.is_running().await
    } else {
        false
    };
    let enabled_profile_count = settings
        .gateway
        .profiles
        .iter()
        .filter(|profile| profile.enabled)
        .count();

    Ok(GatewayStatus {
        proxy_running,
        // The shared local proxy owns the strict /gateway/<profile-id>/ routes.
        routing_active: proxy_running,
        enabled_profile_count,
        listener_address: proxy_running.then(|| gateway::listener_address(settings.proxy.port)),
    })
}
