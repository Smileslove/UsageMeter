use crate::cursor::events::{digest, parse_csv, CsvPreview, MAX_IMPORT_BYTES};
use crate::local_usage::database::cursor::CursorBatch;
use tauri::Emitter;

fn read_csv(path: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|_| "cursor_import_file_unavailable")?;
    let mut bytes = Vec::new();
    file.take(MAX_IMPORT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cursor_import_file_unavailable")?;
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err("cursor_import_too_large".into());
    }
    Ok(bytes)
}

#[tauri::command]
pub async fn preview_cursor_csv_import(path: String) -> Result<CsvPreview, String> {
    tokio::task::spawn_blocking(move || parse_csv(&read_csv(&path)?).map(|(preview, _)| preview))
        .await
        .map_err(|_| "cursor_import_failed")?
}

#[tauri::command]
pub async fn import_cursor_usage_csv(
    app: tauri::AppHandle,
    path: String,
    content_hash: String,
    namespace: String,
    account_key: Option<String>,
) -> Result<CursorBatch, String> {
    let batch = tokio::task::spawn_blocking(move || {
        if namespace.trim().is_empty() || namespace.len() > 128 {
            return Err("cursor_invalid_import_scope".into());
        }
        let (preview, events) = parse_csv(&read_csv(&path)?)?;
        if preview.content_hash != content_hash {
            return Err("cursor_import_file_changed".into());
        }
        let start = preview.first_timestamp_ms.ok_or("cursor_empty_import")?;
        let end = preview
            .last_timestamp_ms
            .ok_or("cursor_empty_import")?
            .checked_add(1)
            .ok_or("cursor_invalid_timestamp")?;
        // This is an observed event interval, not a claim of complete exported history.
        // Offline imports never silently inherit the currently signed-in Cursor account.
        let account_key = if let Some(key) = account_key {
            let active = crate::cursor::sync::current_auth().map_err(|failure| failure.code)?;
            if key != active.account_key {
                return Err("cursor_identity_mismatch".into());
            }
            key
        } else {
            digest(format!("cursor-csv:{}", namespace.trim()).as_bytes())
        };
        let batch_id = digest(format!("{account_key}:{}", preview.content_hash).as_bytes());
        let batch = CursorBatch {
            batch_id,
            account_key,
            source: "csv".into(),
            range_start_ms: start,
            range_end_ms: end,
            event_count: events.len(),
            imported_at_ms: chrono::Utc::now().timestamp_millis(),
        };
        crate::local_usage::get_local_usage_db()?.publish_cursor_batch(&batch, &events)?;
        Ok::<_, String>(batch)
    })
    .await
    .map_err(|_| "cursor_import_failed")??;
    let _ = app.emit("local_usage_synced", ());
    Ok(batch)
}

#[tauri::command]
pub async fn list_cursor_import_batches() -> Result<Vec<CursorBatch>, String> {
    tokio::task::spawn_blocking(|| crate::local_usage::get_local_usage_db()?.list_cursor_batches())
        .await
        .map_err(|_| "cursor_database_error")?
}

#[tauri::command]
pub async fn revoke_cursor_import_batch(
    app: tauri::AppHandle,
    batch_id: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        crate::local_usage::get_local_usage_db()?.revoke_cursor_batch(&batch_id)
    })
    .await
    .map_err(|_| "cursor_database_error")??;
    let _ = app.emit("local_usage_synced", ());
    Ok(())
}

#[tauri::command]
pub async fn get_cursor_status() -> Result<crate::cursor::sync::CursorStatus, String> {
    tokio::task::spawn_blocking(crate::cursor::sync::status)
        .await
        .map_err(|_| "cursor_database_error")?
}

#[tauri::command]
pub async fn sync_cursor_usage(
    app: tauri::AppHandle,
    start_ms: Option<i64>,
) -> Result<
    crate::local_usage::database::cursor::CursorSyncState,
    crate::cursor::client::CursorFailure,
> {
    let result = crate::cursor::sync::synchronize(true, start_ms).await?;
    let _ = app.emit("local_usage_synced", ());
    Ok(result)
}

#[tauri::command]
pub async fn select_cursor_csv(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("CSV", &["csv"])
        .pick_file(move |file| {
            let path = file
                .and_then(|file| file.into_path().ok())
                .map(|path| path.to_string_lossy().into_owned());
            let _ = sender.send(path);
        });
    receiver
        .await
        .map_err(|_| "cursor_import_file_unavailable".into())
}
