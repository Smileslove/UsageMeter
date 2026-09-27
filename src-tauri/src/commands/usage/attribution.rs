use crate::models::AppSettings;

const MANUAL_ATTRIBUTION_MAX_TARGETS: usize = 1_000;

fn validate_target_count(target_count: usize) -> Result<(), String> {
    if target_count == 0 || target_count > MANUAL_ATTRIBUTION_MAX_TARGETS {
        return Err("ERR_MANUAL_ATTRIBUTION_INVALID_TARGETS".to_string());
    }
    Ok(())
}

fn validate_time_range(start_epoch: i64, end_epoch: i64) -> Result<(), String> {
    if start_epoch < 0 || end_epoch <= start_epoch {
        return Err("ERR_MANUAL_ATTRIBUTION_INVALID_TIME_RANGE".to_string());
    }
    Ok(())
}

fn validate_time_range_target_count(target_count: usize) -> Result<(), String> {
    if target_count == 0 {
        return Err("ERR_MANUAL_ATTRIBUTION_EMPTY_TIME_RANGE".to_string());
    }
    validate_target_count(target_count)
}

fn validate_source_id(settings: &AppSettings, source_id: Option<&str>) -> Result<(), String> {
    let Some(source_id) = source_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    if settings
        .source_aware
        .sources
        .iter()
        .any(|source| source.id == source_id)
    {
        Ok(())
    } else {
        Err("ERR_MANUAL_ATTRIBUTION_SOURCE_NOT_FOUND".to_string())
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualAttributionRequest {
    pub request_keys: Vec<String>,
    /// None intentionally means "manually leave unattributed".
    pub source_id: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualAttributionTimeRange {
    /// Unix seconds, inclusive.
    pub start_epoch: i64,
    /// Unix seconds, exclusive.
    pub end_epoch: i64,
}

#[tauri::command]
pub async fn set_manual_request_attribution(
    request: ManualAttributionRequest,
    settings: AppSettings,
) -> Result<u64, String> {
    validate_target_count(request.request_keys.len())?;
    validate_source_id(&settings, request.source_id.as_deref())?;
    let changed = tauri::async_runtime::spawn_blocking(move || {
        let database = crate::local_usage::get_local_usage_db()?;
        database.set_manual_attribution_overrides(
            &request.request_keys,
            request.source_id.as_deref(),
            chrono::Utc::now().timestamp_millis(),
        )
    })
    .await
    .map_err(|error| format!("Task error: {error}"))??;
    crate::unified_usage::clear_runtime_caches();
    Ok(changed)
}

#[tauri::command]
pub async fn clear_manual_request_attribution(request_keys: Vec<String>) -> Result<u64, String> {
    validate_target_count(request_keys.len())?;
    let removed = tauri::async_runtime::spawn_blocking(move || {
        crate::local_usage::get_local_usage_db()?.clear_manual_attribution_overrides(&request_keys)
    })
    .await
    .map_err(|error| format!("Task error: {error}"))??;
    if removed > 0 {
        crate::unified_usage::clear_runtime_caches();
    }
    Ok(removed)
}

#[tauri::command]
pub async fn set_manual_session_attribution(
    session_id: String,
    source_id: Option<String>,
    settings: AppSettings,
) -> Result<u64, String> {
    validate_source_id(&settings, source_id.as_deref())?;
    let request_keys = crate::unified_usage::get_manual_attribution_request_keys_for_session(
        &settings,
        &session_id,
    )
    .await?;
    validate_target_count(request_keys.len())?;
    let changed = tauri::async_runtime::spawn_blocking(move || {
        crate::local_usage::get_local_usage_db()?.set_manual_attribution_overrides(
            &request_keys,
            source_id.as_deref(),
            chrono::Utc::now().timestamp_millis(),
        )
    })
    .await
    .map_err(|error| format!("Task error: {error}"))??;
    crate::unified_usage::clear_runtime_caches();
    Ok(changed)
}

#[tauri::command]
pub async fn clear_manual_session_attribution(
    session_id: String,
    settings: AppSettings,
) -> Result<u64, String> {
    let request_keys = crate::unified_usage::get_manual_attribution_request_keys_for_session(
        &settings,
        &session_id,
    )
    .await?;
    validate_target_count(request_keys.len())?;
    let removed = tauri::async_runtime::spawn_blocking(move || {
        crate::local_usage::get_local_usage_db()?.clear_manual_attribution_overrides(&request_keys)
    })
    .await
    .map_err(|error| format!("Task error: {error}"))??;
    if removed > 0 {
        crate::unified_usage::clear_runtime_caches();
    }
    Ok(removed)
}

#[tauri::command]
pub async fn set_manual_time_range_attribution(
    range: ManualAttributionTimeRange,
    source_id: Option<String>,
    settings: AppSettings,
) -> Result<u64, String> {
    validate_time_range(range.start_epoch, range.end_epoch)?;
    validate_source_id(&settings, source_id.as_deref())?;
    let request_keys = crate::unified_usage::get_manual_attribution_request_keys_for_time_range(
        &settings,
        range.start_epoch,
        range.end_epoch,
    )
    .await?;
    validate_time_range_target_count(request_keys.len())?;
    let changed = tauri::async_runtime::spawn_blocking(move || {
        crate::local_usage::get_local_usage_db()?.set_manual_attribution_overrides(
            &request_keys,
            source_id.as_deref(),
            chrono::Utc::now().timestamp_millis(),
        )
    })
    .await
    .map_err(|error| format!("Task error: {error}"))??;
    crate::unified_usage::clear_runtime_caches();
    Ok(changed)
}

#[tauri::command]
pub async fn clear_manual_time_range_attribution(
    range: ManualAttributionTimeRange,
    settings: AppSettings,
) -> Result<u64, String> {
    validate_time_range(range.start_epoch, range.end_epoch)?;
    let request_keys = crate::unified_usage::get_manual_attribution_request_keys_for_time_range(
        &settings,
        range.start_epoch,
        range.end_epoch,
    )
    .await?;
    validate_time_range_target_count(request_keys.len())?;
    let removed = tauri::async_runtime::spawn_blocking(move || {
        crate::local_usage::get_local_usage_db()?.clear_manual_attribution_overrides(&request_keys)
    })
    .await
    .map_err(|error| format!("Task error: {error}"))??;
    if removed > 0 {
        crate::unified_usage::clear_runtime_caches();
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_attribution_target_limit_is_strict() {
        assert!(validate_target_count(1).is_ok());
        assert!(validate_target_count(MANUAL_ATTRIBUTION_MAX_TARGETS).is_ok());
        assert!(validate_target_count(0).is_err());
        assert!(validate_target_count(MANUAL_ATTRIBUTION_MAX_TARGETS + 1).is_err());
    }

    #[test]
    fn manual_attribution_time_range_is_nonempty_and_nonnegative() {
        assert!(validate_time_range(100, 101).is_ok());
        assert!(validate_time_range(-1, 10).is_err());
        assert!(validate_time_range(100, 100).is_err());
        assert!(validate_time_range(100, 99).is_err());
        assert_eq!(
            validate_time_range_target_count(0).unwrap_err(),
            "ERR_MANUAL_ATTRIBUTION_EMPTY_TIME_RANGE"
        );
    }
}
