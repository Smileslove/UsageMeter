use super::{
    client::{read_auth, CursorAuth, CursorClient, CursorFailure},
    events::digest,
};
use crate::local_usage::database::cursor::{CursorBatch, CursorSyncState};
use serde::Serialize;
use std::time::Duration;
use tauri::Emitter;

// ponytail: one installed Cursor account, one mutex. Per-account locks only with multi-account UI.
static SYNC_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) fn current_auth() -> Result<CursorAuth, CursorFailure> {
    let path =
        crate::session::cursor_reader::configured_database_path().ok_or("cursor_not_found")?;
    read_auth(&path, chrono::Utc::now().timestamp())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CursorStatus {
    pub metadata_status: String,
    pub account_sync_available: bool,
    pub usage_event_count: u64,
    pub unknown_cost_events: u64,
    pub incomplete_usage_events: u64,
    pub local_available: bool,
    pub auth_status: String,
    pub account_key: Option<String>,
    pub account_sync_enabled: bool,
    pub sync: Option<CursorSyncState>,
}

pub(crate) fn status() -> Result<CursorStatus, String> {
    let settings = crate::settings::load_settings_blocking()?;
    let local_available =
        crate::session::cursor_reader::configured_database_path().is_some_and(|path| path.exists());
    let (auth_status, account_key) = match current_auth() {
        Ok(auth) => ("ready".into(), Some(auth.account_key)),
        Err(failure) => (failure.code, None),
    };
    let sync = match &account_key {
        Some(key) => crate::local_usage::get_local_usage_db()?.get_cursor_sync_state(key)?,
        None => None,
    };
    let (usage_event_count, unknown_cost_events, incomplete_usage_events) =
        crate::local_usage::get_local_usage_db()?.cursor_quality_counts()?;
    Ok(CursorStatus {
        metadata_status: crate::local_usage::get_local_usage_db()?
            .get_local_sync_state("cursor_metadata_status")?
            .unwrap_or_else(|| "unknown".into()),
        account_sync_available: super::ACCOUNT_API_CONTRACT_VERIFIED,
        usage_event_count,
        unknown_cost_events,
        incomplete_usage_events,
        local_available,
        auth_status,
        account_key,
        account_sync_enabled: settings.cursor.account_sync_enabled,
        sync,
    })
}

pub(crate) async fn synchronize(
    manual: bool,
    start_ms: Option<i64>,
) -> Result<CursorSyncState, CursorFailure> {
    if !super::ACCOUNT_API_CONTRACT_VERIFIED {
        return Err("cursor_contract_unverified".into());
    }
    let requested_at = chrono::Utc::now().timestamp_millis();
    let _guard = SYNC_LOCK.lock().await;
    if !crate::settings::load_settings_blocking()?
        .cursor
        .account_sync_enabled
    {
        return Err("cursor_sync_disabled".into());
    }
    let auth = tokio::task::spawn_blocking(current_auth)
        .await
        .map_err(|_| "cursor_auth_unavailable")??;
    let db = crate::local_usage::get_local_usage_db()?;
    if let Some(state) = db.get_cursor_sync_state(&auth.account_key)? {
        // Another window may have completed this same refresh while this caller waited.
        if start_ms.is_none()
            && state
                .last_success_ms
                .is_some_and(|time| time >= requested_at)
        {
            return Ok(state);
        }
        if state
            .next_retry_ms
            .is_some_and(|time| time > chrono::Utc::now().timestamp_millis())
        {
            return Err(CursorFailure {
                code: state
                    .error_code
                    .unwrap_or_else(|| "cursor_rate_limited".into()),
                next_retry_ms: state.next_retry_ms,
            });
        }
    }
    let today = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .ok_or("cursor_invalid_timestamp")?
        .and_utc()
        .timestamp_millis();
    let end_ms = today + 24 * 60 * 60 * 1000;
    let start_ms = start_ms.unwrap_or(today - 30 * 24 * 60 * 60 * 1000);
    if start_ms < 0 || start_ms >= end_ms {
        return Err("cursor_invalid_timestamp".into());
    }
    db.record_cursor_sync_attempt(&auth.account_key, "syncing", None)?;
    let mut result = async {
        let events = CursorClient::new()
            .fetch_events(&auth, start_ms, end_ms, manual)
            .await?;
        let active = tokio::task::spawn_blocking(current_auth)
            .await
            .map_err(|_| "cursor_auth_unavailable")??;
        if active.account_key != auth.account_key {
            return Err("cursor_identity_mismatch".into());
        }
        if !crate::settings::load_settings_blocking()?
            .cursor
            .account_sync_enabled
        {
            return Err("cursor_sync_disabled".into());
        }
        let batch = CursorBatch {
            batch_id: digest(
                format!("cursor-api:{}:{start_ms}:{end_ms}", auth.account_key).as_bytes(),
            ),
            account_key: auth.account_key.clone(),
            source: "api".into(),
            range_start_ms: start_ms,
            range_end_ms: end_ms,
            event_count: events.len(),
            imported_at_ms: chrono::Utc::now().timestamp_millis(),
        };
        let writer = db.clone();
        tokio::task::spawn_blocking(move || writer.publish_cursor_batch(&batch, &events))
            .await
            .map_err(|_| "cursor_database_error")??;
        db.get_cursor_sync_state(&auth.account_key)?
            .ok_or_else(|| CursorFailure::from("cursor_database_error"))
    }
    .await;
    if let Err(failure) = &mut result {
        if failure.next_retry_ms.is_none()
            && matches!(
                failure.code.as_str(),
                "cursor_network_error" | "cursor_service_unavailable" | "cursor_sync_timeout"
            )
        {
            failure.next_retry_ms = Some(chrono::Utc::now().timestamp_millis() + 60_000);
        }
        let state = match failure.code.as_str() {
            "cursor_expired" => "expired",
            "cursor_rate_limited" => "rate_limited",
            "cursor_identity_mismatch" => "identity_mismatch",
            "cursor_sync_incomplete"
            | "cursor_sync_count_changed"
            | "cursor_ambiguous_duplicate_page" => "partial",
            "cursor_schema_unsupported" | "cursor_range_unsupported" => "schema_unsupported",
            "cursor_permission_denied" => "permission_denied",
            _ => "stale",
        };
        db.record_cursor_sync_attempt(&auth.account_key, state, Some(failure))?;
    }
    result
}

pub(crate) fn start_background_sync(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            if crate::settings::load_settings_blocking()
                .is_ok_and(|settings| settings.cursor.account_sync_enabled)
                && super::ACCOUNT_API_CONTRACT_VERIFIED
                && synchronize(false, None).await.is_ok()
            {
                let _ = app.emit("local_usage_synced", ());
            }
            tokio::time::sleep(Duration::from_secs(5 * 60)).await;
        }
    });
}
