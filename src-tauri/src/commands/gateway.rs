//! Tauri commands for the non-secret local API gateway profiles.

use crate::gateway::{
    self, GatewayLocalKeyInput, GatewayProfileInput, GatewayProfileView, GatewayStatus,
    GatewayUpstreamKeyInput, GatewayUpstreamKeyUpdateInput, GeneratedGatewayLocalKey,
};
use crate::models::AppSettings;
use tauri::State;

use super::usage::ProxyState;
use super::{load_settings_blocking, save_settings_internal};

#[tauri::command]
pub async fn list_gateway_profiles() -> Result<Vec<GatewayProfileView>, String> {
    let mut settings = load_settings_blocking()?;
    let mut changed = false;
    for profile in &mut settings.gateway.profiles {
        if profile.local_keys.is_empty() {
            let (metadata, generated) = gateway::create_local_key(
                &profile.id,
                GatewayLocalKeyInput {
                    remark: String::new(),
                },
            )?;
            gateway::store_upstream_secret(&metadata.secret_ref, &generated.key)?;
            profile.local_keys.push(metadata);
            profile.auth_mode = crate::models::GatewayAuthMode::ManagedKeys;
            changed = true;
        }
    }
    if changed {
        save_settings_internal(settings.clone()).map_err(String::from)?;
    }
    Ok(settings
        .gateway
        .profiles
        .iter()
        .map(GatewayProfileView::from)
        .collect())
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
) -> Result<GatewayProfileView, String> {
    let mut profile = gateway::create_profile(input)?;
    let (local_metadata, local_generated) = gateway::create_local_key(
        &profile.id,
        GatewayLocalKeyInput {
            remark: String::new(),
        },
    )?;
    gateway::store_upstream_secret(&local_metadata.secret_ref, &local_generated.key)?;
    profile.local_keys.push(local_metadata);
    profile.auth_mode = crate::models::GatewayAuthMode::ManagedKeys;
    let mut settings = load_settings_blocking()?;
    settings.gateway.profiles.push(profile.clone());
    save_settings_internal(settings.clone()).map_err(String::from)?;
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(GatewayProfileView::from(&profile))
}

#[tauri::command]
pub async fn update_gateway_profile(
    id: String,
    input: GatewayProfileInput,
    state: State<'_, ProxyState>,
) -> Result<GatewayProfileView, String> {
    let mut profile = gateway::update_profile(&id, input)?;
    let mut settings = load_settings_blocking()?;
    let existing = settings
        .gateway
        .profiles
        .iter_mut()
        .find(|candidate| candidate.id == id)
        .ok_or_else(|| "ERR_GATEWAY_PROFILE_NOT_FOUND".to_string())?;
    // Updating a route must never discard its managed credentials.
    profile.auth_mode = existing.auth_mode.clone();
    profile.upstream_keys = existing.upstream_keys.clone();
    profile.local_keys = existing.local_keys.clone();
    *existing = profile.clone();
    save_settings_internal(settings.clone()).map_err(String::from)?;
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(GatewayProfileView::from(&profile))
}

#[tauri::command]
pub async fn delete_gateway_profile(
    id: String,
    state: State<'_, ProxyState>,
) -> Result<(), String> {
    let mut settings = load_settings_blocking()?;
    let original_len = settings.gateway.profiles.len();
    let removed = settings
        .gateway
        .profiles
        .iter()
        .find(|profile| profile.id == id)
        .cloned();
    settings.gateway.profiles.retain(|profile| profile.id != id);
    if settings.gateway.profiles.len() == original_len {
        return Err("ERR_GATEWAY_PROFILE_NOT_FOUND".to_string());
    }
    save_settings_internal(settings.clone()).map_err(String::from)?;
    if let Some(profile) = removed {
        for key in profile.upstream_keys {
            if gateway::is_expected_upstream_secret_ref(&profile.id, &key) {
                gateway::delete_upstream_secret(&key.secret_ref);
            }
        }
        for key in profile.local_keys {
            if gateway::is_expected_local_secret_ref(&profile.id, &key) {
                gateway::delete_upstream_secret(&key.secret_ref);
            }
        }
    }
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(())
}

#[tauri::command]
pub async fn create_gateway_upstream_key(
    profile_id: String,
    input: GatewayUpstreamKeyInput,
    state: State<'_, ProxyState>,
) -> Result<GatewayProfileView, String> {
    let mut settings = load_settings_blocking()?;
    let profile = settings
        .gateway
        .profiles
        .iter_mut()
        .find(|item| item.id == profile_id)
        .ok_or_else(|| "ERR_GATEWAY_PROFILE_NOT_FOUND".to_string())?;
    let (key, secret) = gateway::create_upstream_key(&profile_id, input)?;
    gateway::store_upstream_secret(&key.secret_ref, &secret)?;
    profile.upstream_keys.push(key);
    profile.auth_mode = crate::models::GatewayAuthMode::ManagedKeys;
    let result = profile.clone();
    if let Err(error) = save_settings_internal(settings.clone()) {
        if let Some(key) = result.upstream_keys.last() {
            gateway::delete_upstream_secret(&key.secret_ref);
        }
        return Err(error.into());
    }
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(GatewayProfileView::from(&result))
}

#[tauri::command]
pub async fn delete_gateway_upstream_key(
    profile_id: String,
    key_id: String,
    state: State<'_, ProxyState>,
) -> Result<GatewayProfileView, String> {
    let mut settings = load_settings_blocking()?;
    let profile = settings
        .gateway
        .profiles
        .iter_mut()
        .find(|item| item.id == profile_id)
        .ok_or_else(|| "ERR_GATEWAY_PROFILE_NOT_FOUND".to_string())?;
    let index = profile
        .upstream_keys
        .iter()
        .position(|key| key.id == key_id)
        .ok_or_else(|| "ERR_GATEWAY_UPSTREAM_KEY_NOT_FOUND".to_string())?;
    let removed = profile.upstream_keys.remove(index);
    let result = profile.clone();
    save_settings_internal(settings.clone()).map_err(String::from)?;
    if gateway::is_expected_upstream_secret_ref(&profile_id, &removed) {
        gateway::delete_upstream_secret(&removed.secret_ref);
    }
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(GatewayProfileView::from(&result))
}

#[tauri::command]
pub async fn set_gateway_upstream_key_enabled(
    profile_id: String,
    key_id: String,
    enabled: bool,
    state: State<'_, ProxyState>,
) -> Result<GatewayProfileView, String> {
    let mut settings = load_settings_blocking()?;
    let profile = settings
        .gateway
        .profiles
        .iter_mut()
        .find(|item| item.id == profile_id)
        .ok_or_else(|| "ERR_GATEWAY_PROFILE_NOT_FOUND".to_string())?;
    let key = profile
        .upstream_keys
        .iter_mut()
        .find(|key| key.id == key_id)
        .ok_or_else(|| "ERR_GATEWAY_UPSTREAM_KEY_NOT_FOUND".to_string())?;
    key.enabled = enabled;
    let result = profile.clone();
    save_settings_internal(settings.clone()).map_err(String::from)?;
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(GatewayProfileView::from(&result))
}

#[tauri::command]
pub async fn update_gateway_upstream_key(
    profile_id: String,
    key_id: String,
    input: GatewayUpstreamKeyUpdateInput,
    state: State<'_, ProxyState>,
) -> Result<GatewayProfileView, String> {
    if input.weight == 0 {
        return Err("ERR_GATEWAY_UPSTREAM_KEY_WEIGHT_INVALID".to_string());
    }
    let mut settings = load_settings_blocking()?;
    let profile = settings
        .gateway
        .profiles
        .iter_mut()
        .find(|item| item.id == profile_id)
        .ok_or_else(|| "ERR_GATEWAY_PROFILE_NOT_FOUND".to_string())?;
    let key = profile
        .upstream_keys
        .iter_mut()
        .find(|key| key.id == key_id)
        .ok_or_else(|| "ERR_GATEWAY_UPSTREAM_KEY_NOT_FOUND".to_string())?;
    key.enabled = input.enabled;
    key.weight = input.weight;
    key.priority = input.priority;
    let result = profile.clone();
    save_settings_internal(settings.clone()).map_err(String::from)?;
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(GatewayProfileView::from(&result))
}

#[tauri::command]
pub async fn create_gateway_local_key(
    profile_id: String,
    input: GatewayLocalKeyInput,
    state: State<'_, ProxyState>,
) -> Result<GeneratedGatewayLocalKey, String> {
    let mut settings = load_settings_blocking()?;
    let profile = settings
        .gateway
        .profiles
        .iter_mut()
        .find(|item| item.id == profile_id)
        .ok_or_else(|| "ERR_GATEWAY_PROFILE_NOT_FOUND".to_string())?;
    // A gateway profile represents one upstream and therefore has exactly one
    // client credential. Revoke the existing key before creating another one.
    if !profile.local_keys.is_empty() {
        return Err("ERR_GATEWAY_LOCAL_KEY_ALREADY_EXISTS".to_string());
    }
    let (metadata, generated) = gateway::create_local_key(&profile_id, input)?;
    gateway::store_upstream_secret(&metadata.secret_ref, &generated.key)?;
    let secret_ref = metadata.secret_ref.clone();
    profile.local_keys.push(metadata);
    profile.auth_mode = crate::models::GatewayAuthMode::ManagedKeys;
    let _ = profile;
    if let Err(error) = save_settings_internal(settings.clone()) {
        gateway::delete_upstream_secret(&secret_ref);
        return Err(error.into());
    }
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(generated)
}

#[tauri::command]
pub async fn revoke_gateway_local_key(
    profile_id: String,
    key_id: String,
    state: State<'_, ProxyState>,
) -> Result<GatewayProfileView, String> {
    let mut settings = load_settings_blocking()?;
    let profile = settings
        .gateway
        .profiles
        .iter_mut()
        .find(|item| item.id == profile_id)
        .ok_or_else(|| "ERR_GATEWAY_PROFILE_NOT_FOUND".to_string())?;
    let index = profile
        .local_keys
        .iter()
        .position(|key| key.id == key_id)
        .ok_or_else(|| "ERR_GATEWAY_LOCAL_KEY_NOT_FOUND".to_string())?;
    let removed = profile.local_keys.remove(index);
    let result = profile.clone();
    save_settings_internal(settings.clone()).map_err(String::from)?;
    if gateway::is_expected_local_secret_ref(&profile_id, &removed) {
        gateway::delete_upstream_secret(&removed.secret_ref);
    }
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(GatewayProfileView::from(&result))
}

#[tauri::command]
pub async fn reveal_gateway_local_key(
    profile_id: String,
    key_id: String,
) -> Result<String, String> {
    let settings = load_settings_blocking()?;
    let profile = settings
        .gateway
        .profiles
        .iter()
        .find(|item| item.id == profile_id)
        .ok_or_else(|| "ERR_GATEWAY_PROFILE_NOT_FOUND".to_string())?;
    let key = profile
        .local_keys
        .iter()
        .find(|key| key.id == key_id && key.enabled)
        .ok_or_else(|| "ERR_GATEWAY_LOCAL_KEY_NOT_FOUND".to_string())?;
    if !gateway::is_expected_local_secret_ref(&profile_id, key) {
        return Err("ERR_GATEWAY_LOCAL_KEY_UNAVAILABLE".to_string());
    }
    gateway::load_upstream_secret(&key.secret_ref)
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
