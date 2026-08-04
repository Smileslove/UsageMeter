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
    let ((profiles, migrated_secret_refs), _) = update_settings_internal(|settings| {
        let mut migrated_secret_refs = Vec::new();
        let mut migration_pending = false;
        for profile in &mut settings.gateway.profiles {
            // Profiles are single-upstream by design. Keep the first record for
            // compatibility with older settings, without touching any external
            // credential store.
            for extra in profile.upstream_keys.drain(1..) {
                if gateway::is_expected_upstream_secret_ref(&profile.id, &extra) {
                    migrated_secret_refs.push(extra.secret_ref);
                }
            }
            // Migrate v1 Keychain-backed upstream credentials once when the
            // legacy reference is still present. New profiles never enter this
            // path because they persist `secret` directly in settings.
            for key in &mut profile.upstream_keys {
                if key.secret.trim().is_empty() && !key.secret_ref.trim().is_empty() {
                    if !gateway::is_expected_upstream_secret_ref(&profile.id, key) {
                        key.secret_ref.clear();
                    } else if let Ok(secret) = gateway::load_legacy_keychain_secret(&key.secret_ref)
                    {
                        key.secret = secret;
                        migrated_secret_refs.push(key.secret_ref.clone());
                        key.secret_ref.clear();
                    } else {
                        // Keep the profile visible, but prevent an empty
                        // credential from being selected for forwarding.
                        key.enabled = false;
                        migration_pending = true;
                    }
                }
            }
            if profile.local_keys.is_empty() {
                let (metadata, generated) = gateway::create_local_key(
                    &profile.id,
                    GatewayLocalKeyInput {
                        remark: String::new(),
                    },
                )?;
                let mut metadata = metadata;
                metadata.secret = generated.key.clone();
                profile.local_keys.push(metadata);
                profile.auth_mode = crate::models::GatewayAuthMode::ManagedKeys;
            } else {
                // Migrate an existing v1 local key when possible. The verifier
                // remains usable even if the legacy raw value is unavailable.
                for key in &mut profile.local_keys {
                    if key.secret.trim().is_empty() && !key.secret_ref.trim().is_empty() {
                        if !gateway::is_expected_local_secret_ref(&profile.id, key) {
                            key.secret_ref.clear();
                        } else if let Ok(secret) =
                            gateway::load_legacy_keychain_secret(&key.secret_ref)
                        {
                            key.secret = secret;
                            migrated_secret_refs.push(key.secret_ref.clone());
                            key.secret_ref.clear();
                        } else {
                            migration_pending = true;
                        }
                    }
                }
            }
        }
        if !migration_pending {
            settings.gateway.storage_version = crate::models::default_gateway_storage_version();
        }
        Ok((
            settings
                .gateway
                .profiles
                .iter()
                .map(GatewayProfileView::from)
                .collect(),
            migrated_secret_refs,
        ))
    })
    .map_err(String::from)?;
    for secret_ref in migrated_secret_refs {
        gateway::delete_legacy_keychain_secret(&secret_ref);
    }
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
    let upstream_secret = input
        .upstream_secret
        .clone()
        .filter(|value| !value.trim().is_empty());
    let mut profile = gateway::create_profile(input)?;
    let (local_metadata, local_generated) = gateway::create_local_key(
        &profile.id,
        GatewayLocalKeyInput {
            remark: String::new(),
        },
    )?;
    let mut local_metadata = local_metadata;
    local_metadata.secret = local_generated.key;

    if let Some(secret) = upstream_secret {
        let (upstream_key, normalized_secret) = gateway::create_upstream_key(
            &profile.id,
            GatewayUpstreamKeyInput {
                remark: String::new(),
                secret: secret.clone(),
                enabled: true,
                weight: 1,
                priority: 0,
            },
        )?;
        let mut upstream_key = upstream_key;
        upstream_key.secret = normalized_secret;
        profile.upstream_keys.push(upstream_key);
    }

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
    let replacement_secret = input
        .upstream_secret
        .clone()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string());
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
        if let Some(secret) = replacement_secret {
            let key = updated_profile
                .upstream_keys
                .first_mut()
                .ok_or_else(|| "ERR_GATEWAY_UPSTREAM_KEY_NOT_FOUND".to_string())?;
            if secret.chars().count() > 4096 {
                return Err("ERR_GATEWAY_UPSTREAM_KEY_INVALID".to_string());
            }
            key.secret = secret;
            key.secret_ref.clear();
        }
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
    let (key, secret) = gateway::create_upstream_key(&profile_id, input)?;
    let mut key = key;
    key.secret = secret;

    let key_id = key.id.clone();
    let profile_id_clone = profile_id.clone();
    let update = update_settings_internal(move |settings| {
        let profile = settings
            .gateway
            .profiles
            .iter_mut()
            .find(|item| item.id == profile_id_clone)
            .ok_or_else(|| "ERR_GATEWAY_PROFILE_NOT_FOUND".to_string())?;
        if !profile.upstream_keys.is_empty() {
            return Err("ERR_GATEWAY_UPSTREAM_KEY_ALREADY_EXISTS".to_string());
        }
        profile.upstream_keys.push(key);
        profile.auth_mode = crate::models::GatewayAuthMode::ManagedKeys;
        Ok(GatewayProfileView::from(&*profile))
    });
    let (view, settings) = update.map_err(String::from)?;

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
    let mut metadata = metadata;
    metadata.secret = generated.key.clone();
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
        Err(error) => return Err(error.into()),
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
    let ((_removed, view), settings) = update_settings_internal(move |settings| {
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
    if !key.secret.trim().is_empty() {
        return Ok(key.secret.clone());
    }
    Err("ERR_GATEWAY_LOCAL_KEY_REGENERATE_REQUIRED".to_string())
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
