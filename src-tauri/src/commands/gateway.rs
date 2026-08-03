//! Tauri commands for the non-secret local API gateway profiles.

use crate::gateway::{
    self, GatewayLocalKeyInput, GatewayProfileInput, GatewayProfileView, GatewayStatus,
    GatewayUpstreamKeyInput, GatewayUpstreamKeyUpdateInput, GeneratedGatewayLocalKey,
};
use crate::models::AppSettings;
use tauri::State;

use super::usage::ProxyState;
use super::{load_settings_blocking, update_settings_internal};

#[tauri::command]
pub async fn list_gateway_profiles() -> Result<Vec<GatewayProfileView>, String> {
    let (profiles, _) = update_settings_internal(|settings| {
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
            }
        }
        Ok(settings
            .gateway
            .profiles
            .iter()
            .map(GatewayProfileView::from)
            .collect())
    })
    .map_err(String::from)?;
    Ok(profiles)
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
    use crate::gateway::compensation::{CompensationScope, DeleteSecretAction};

    let compensation = CompensationScope::new();

    let mut profile = gateway::create_profile(input)?;
    let (local_metadata, local_generated) = gateway::create_local_key(
        &profile.id,
        GatewayLocalKeyInput {
            remark: String::new(),
        },
    )?;
    gateway::store_upstream_secret(&local_metadata.secret_ref, &local_generated.key)?;

    // Register compensation: delete the secret if settings update fails
    compensation.register(Box::new(DeleteSecretAction {
        secret_ref: local_metadata.secret_ref.clone(),
    }));

    profile.local_keys.push(local_metadata);
    profile.auth_mode = crate::models::GatewayAuthMode::ManagedKeys;

    let profile_to_insert = profile.clone();
    let update = update_settings_internal(move |settings| {
        if settings
            .gateway
            .profiles
            .iter()
            .any(|candidate| candidate.id == profile_to_insert.id)
        {
            return Err("ERR_GATEWAY_PROFILE_ALREADY_EXISTS".to_string());
        }
        settings.gateway.profiles.push(profile_to_insert.clone());
        Ok(GatewayProfileView::from(&profile_to_insert))
    });
    let (view, settings) = update.map_err(String::from)?;

    // Success: commit the transaction
    compensation.commit();

    refresh_running_proxy_settings(&state, &settings).await;

    // Audit log
    crate::gateway::audit::log_audit(crate::gateway::audit::GatewayAuditEvent {
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        event_type: crate::gateway::audit::GatewayAuditEventType::ProfileCreated,
        profile_id: profile.id.clone(),
        actor: None,
        details: serde_json::json!({
            "name": profile.name,
            "protocol": format!("{:?}", profile.protocol),
        }),
        result: crate::gateway::audit::AuditResult::Success,
    });

    Ok(view)
}

#[tauri::command]
pub async fn update_gateway_profile(
    id: String,
    input: GatewayProfileInput,
    state: State<'_, ProxyState>,
) -> Result<GatewayProfileView, String> {
    let mut updated_profile = gateway::update_profile(&id, input)?;
    let (view, settings) = update_settings_internal(move |settings| {
        let existing = settings
            .gateway
            .profiles
            .iter_mut()
            .find(|candidate| candidate.id == id)
            .ok_or_else(|| "ERR_GATEWAY_PROFILE_NOT_FOUND".to_string())?;
        // Updating a route must never discard its managed credentials.
        updated_profile.auth_mode = existing.auth_mode.clone();
        updated_profile.upstream_keys = existing.upstream_keys.clone();
        updated_profile.local_keys = existing.local_keys.clone();
        *existing = updated_profile.clone();
        Ok(GatewayProfileView::from(&updated_profile))
    })
    .map_err(String::from)?;
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(view)
}

#[tauri::command]
pub async fn delete_gateway_profile(
    id: String,
    state: State<'_, ProxyState>,
) -> Result<(), String> {
    let (removed, settings) = update_settings_internal(move |settings| {
        let index = settings
            .gateway
            .profiles
            .iter()
            .position(|profile| profile.id == id)
            .ok_or_else(|| "ERR_GATEWAY_PROFILE_NOT_FOUND".to_string())?;
        Ok(settings.gateway.profiles.remove(index))
    })
    .map_err(String::from)?;

    for key in removed.upstream_keys {
        if gateway::is_expected_upstream_secret_ref(&removed.id, &key) {
            gateway::delete_upstream_secret(&key.secret_ref);
        }
    }
    for key in removed.local_keys {
        if gateway::is_expected_local_secret_ref(&removed.id, &key) {
            gateway::delete_upstream_secret(&key.secret_ref);
        }
    }
    gateway::clear_profile_runtime_state(&removed.id);
    refresh_running_proxy_settings(&state, &settings).await;

    // Audit log
    crate::gateway::audit::log_audit(crate::gateway::audit::GatewayAuditEvent {
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        event_type: crate::gateway::audit::GatewayAuditEventType::ProfileDeleted,
        profile_id: removed.id.clone(),
        actor: None,
        details: serde_json::json!({
            "name": removed.name,
        }),
        result: crate::gateway::audit::AuditResult::Success,
    });

    Ok(())
}

#[tauri::command]
pub async fn create_gateway_upstream_key(
    profile_id: String,
    input: GatewayUpstreamKeyInput,
    state: State<'_, ProxyState>,
) -> Result<GatewayProfileView, String> {
    use crate::gateway::compensation::{CompensationScope, DeleteSecretAction};

    let compensation = CompensationScope::new();

    let (key, secret) = gateway::create_upstream_key(&profile_id, input)?;
    gateway::store_upstream_secret(&key.secret_ref, &secret)?;

    // Register compensation: delete the secret if settings update fails
    compensation.register(Box::new(DeleteSecretAction {
        secret_ref: key.secret_ref.clone(),
    }));

    let key_id = key.id.clone();
    let profile_id_clone = profile_id.clone();
    let update = update_settings_internal(move |settings| {
        let profile = settings
            .gateway
            .profiles
            .iter_mut()
            .find(|item| item.id == profile_id_clone)
            .ok_or_else(|| "ERR_GATEWAY_PROFILE_NOT_FOUND".to_string())?;
        profile.upstream_keys.push(key);
        profile.auth_mode = crate::models::GatewayAuthMode::ManagedKeys;
        Ok(GatewayProfileView::from(&*profile))
    });
    let (view, settings) = update.map_err(String::from)?;

    // Success: commit the transaction
    compensation.commit();

    refresh_running_proxy_settings(&state, &settings).await;

    // Audit log
    crate::gateway::audit::log_audit(crate::gateway::audit::GatewayAuditEvent {
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        event_type: crate::gateway::audit::GatewayAuditEventType::UpstreamKeyAdded,
        profile_id: profile_id.clone(),
        actor: None,
        details: serde_json::json!({
            "key_id": key_id,
        }),
        result: crate::gateway::audit::AuditResult::Success,
    });

    Ok(view)
}

#[tauri::command]
pub async fn delete_gateway_upstream_key(
    profile_id: String,
    key_id: String,
    state: State<'_, ProxyState>,
) -> Result<GatewayProfileView, String> {
    let profile_id_for_update = profile_id.clone();
    let ((removed, view), settings) = update_settings_internal(move |settings| {
        let profile = settings
            .gateway
            .profiles
            .iter_mut()
            .find(|item| item.id == profile_id_for_update)
            .ok_or_else(|| "ERR_GATEWAY_PROFILE_NOT_FOUND".to_string())?;
        let index = profile
            .upstream_keys
            .iter()
            .position(|key| key.id == key_id)
            .ok_or_else(|| "ERR_GATEWAY_UPSTREAM_KEY_NOT_FOUND".to_string())?;
        let removed = profile.upstream_keys.remove(index);
        Ok((removed, GatewayProfileView::from(&*profile)))
    })
    .map_err(String::from)?;
    if gateway::is_expected_upstream_secret_ref(&profile_id, &removed) {
        gateway::delete_upstream_secret(&removed.secret_ref);
    }
    gateway::clear_upstream_runtime_state(&profile_id, &removed.id);
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(view)
}

#[tauri::command]
pub async fn set_gateway_upstream_key_enabled(
    profile_id: String,
    key_id: String,
    enabled: bool,
    state: State<'_, ProxyState>,
) -> Result<GatewayProfileView, String> {
    let (view, settings) = update_settings_internal(move |settings| {
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
        Ok(GatewayProfileView::from(&*profile))
    })
    .map_err(String::from)?;
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(view)
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
    let (view, settings) = update_settings_internal(move |settings| {
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
        Ok(GatewayProfileView::from(&*profile))
    })
    .map_err(String::from)?;
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(view)
}

#[tauri::command]
pub async fn create_gateway_local_key(
    profile_id: String,
    input: GatewayLocalKeyInput,
    state: State<'_, ProxyState>,
) -> Result<GeneratedGatewayLocalKey, String> {
    let (metadata, generated) = gateway::create_local_key(&profile_id, input)?;
    gateway::store_upstream_secret(&metadata.secret_ref, &generated.key)?;
    let secret_ref = metadata.secret_ref.clone();
    let update = update_settings_internal(move |settings| {
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
        profile.local_keys.push(metadata);
        profile.auth_mode = crate::models::GatewayAuthMode::ManagedKeys;
        Ok(())
    });
    let (_, settings) = match update {
        Ok(result) => result,
        Err(error) => {
            gateway::delete_upstream_secret(&secret_ref);
            return Err(error.into());
        }
    };
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(generated)
}

#[tauri::command]
pub async fn revoke_gateway_local_key(
    profile_id: String,
    key_id: String,
    state: State<'_, ProxyState>,
) -> Result<GatewayProfileView, String> {
    let profile_id_for_update = profile_id.clone();
    let ((removed, view), settings) = update_settings_internal(move |settings| {
        let profile = settings
            .gateway
            .profiles
            .iter_mut()
            .find(|item| item.id == profile_id_for_update)
            .ok_or_else(|| "ERR_GATEWAY_PROFILE_NOT_FOUND".to_string())?;
        let index = profile
            .local_keys
            .iter()
            .position(|key| key.id == key_id)
            .ok_or_else(|| "ERR_GATEWAY_LOCAL_KEY_NOT_FOUND".to_string())?;
        let removed = profile.local_keys.remove(index);
        Ok((removed, GatewayProfileView::from(&*profile)))
    })
    .map_err(String::from)?;
    if gateway::is_expected_local_secret_ref(&profile_id, &removed) {
        gateway::delete_upstream_secret(&removed.secret_ref);
    }
    refresh_running_proxy_settings(&state, &settings).await;
    Ok(view)
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
