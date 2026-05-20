use crate::local_usage::{
    ensure_local_usage_synced, SyncExportData, SyncExportRequest, SyncExportSession,
    SyncOutboxBatch,
};
use crate::models::SyncSettings;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use reqwest::{Client, Method, StatusCode, Url};
use ring::{aead, pbkdf2, rand};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::Mutex;

const ROOT_DIR: &str = "UsageMeter";
const META_DIR: &str = "meta";
const KEYRING_FILE: &str = "meta/keyring.json";
const MANIFEST_FILE: &str = "manifest.json";
const BATCH_DIR: &str = "batches";
const SNAPSHOT_DIR: &str = "snapshots";
const SHARED_DIR: &str = "shared";
const SHARED_SETTINGS_DIR: &str = "shared/settings";
const SHARED_SETTINGS_FILE: &str = "shared/settings/profile.json.enc";
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const PBKDF2_ROUNDS: u32 = 120_000;
const BATCH_SCHEMA_VERSION: u32 = 2;
const SNAPSHOT_SCHEMA_VERSION: u32 = 2;
const SNAPSHOT_INTERVAL_BATCHES: i64 = 100;
const BATCH_RETENTION_AFTER_SNAPSHOT: i64 = 20;
const AUTO_SYNC_MIN_INTERVAL_SECONDS: u64 = 60;

static SYNC_JOB_LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavCredentials {
    pub password: String,
    pub sync_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub enabled: bool,
    pub last_sync_at: Option<i64>,
    pub last_status: String,
    pub last_error: Option<String>,
    pub uploaded_requests: u64,
    pub imported_requests: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotateSyncPasswordPayload {
    pub current_sync_password: String,
    pub new_sync_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncPackage {
    schema_version: u32,
    app_version: String,
    device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    instance_id: Option<String>,
    export_seq: i64,
    exported_at: i64,
    data: SyncExportData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceManifest {
    schema_version: u32,
    device_id: String,
    instance_id: String,
    app_version: String,
    latest_batch_seq: i64,
    latest_snapshot_seq: i64,
    latest_exported_at: i64,
    batch_count: i64,
    snapshot_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageBatchPackage {
    schema_version: u32,
    package_type: String,
    device_id: String,
    instance_id: String,
    batch_seq: i64,
    prev_batch_seq: i64,
    exported_at: i64,
    request_events: Vec<SyncExportRequest>,
    session_events: Vec<SyncExportSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotPackage {
    schema_version: u32,
    package_type: String,
    device_id: String,
    instance_id: String,
    covered_until_batch_seq: i64,
    exported_at: i64,
    data: SyncExportData,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct SharedSettingsPayload {
    locale: String,
    timezone: String,
    summary_window: String,
    theme: String,
    model_pricing: crate::models::ModelPricingSettings,
    source_aware: crate::models::SourceAwareSettings,
    currency: crate::models::CurrencySettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SharedSettingsDocument {
    schema_version: u32,
    document_type: String,
    updated_at: i64,
    updated_by_device_id: String,
    version: i64,
    payload: SharedSettingsPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EncryptedPackage {
    schema_version: u32,
    algorithm: String,
    kdf: String,
    device_id: String,
    #[serde(default)]
    dek_version: u32,
    export_seq: i64,
    nonce: String,
    payload: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncKeyring {
    schema_version: u32,
    dek_version: u32,
    algorithm: String,
    kdf: String,
    wrap_salt: String,
    nonce: String,
    wrapped_dek: String,
    updated_at: i64,
}

enum SyncKeyringState {
    Ready(SyncKeyring),
    Missing,
}

impl SyncKeyringState {
    fn keyring(&self) -> Option<&SyncKeyring> {
        match self {
            Self::Ready(keyring) => Some(keyring),
            Self::Missing => None,
        }
    }
}

fn sync_job_lock() -> &'static Arc<Mutex<()>> {
    SYNC_JOB_LOCK.get_or_init(|| Arc::new(Mutex::new(())))
}

pub async fn test_connection(
    settings: SyncSettings,
    credentials: WebDavCredentials,
) -> Result<(), String> {
    validate_config(&settings, &credentials)?;
    let db = ensure_local_usage_synced()?;
    let previous_device_id = db
        .get_webdav_sync_state("device_id")?
        .map(|value| crate::models::normalize_sync_device_id(&value))
        .filter(|value| !value.trim().is_empty());
    let device_id = resolve_device_id(&settings, db.as_ref())?;
    let instance_id = get_or_create_instance_id(db.as_ref())?;
    let webdav_password = credentials.password.clone();
    let client = WebDavClient::new(settings, webdav_password)?;
    client.check_access().await?;
    client.ensure_base_dirs(&device_id).await?;
    let keyring_state = load_or_create_keyring(&client, &credentials.sync_password).await?;
    assert_device_id_ownership(
        &client,
        &device_id,
        &instance_id,
        previous_device_id.as_deref(),
        &credentials.sync_password,
        &keyring_state,
    )
    .await
}

pub async fn sync_now(
    settings: SyncSettings,
    credentials: WebDavCredentials,
) -> Result<SyncStatus, String> {
    let _guard = sync_job_lock().lock().await;
    if let Err(err) = sync_now_inner(settings.clone(), credentials).await {
        persist_failure(&err);
        return Err(err);
    }
    get_status(&settings)
}

pub async fn sync_on_quit(settings: SyncSettings) -> Result<(), String> {
    if !settings.enabled || !settings.sync_on_quit {
        return Ok(());
    }
    let _guard = sync_job_lock().lock().await;
    let credentials = WebDavCredentials {
        password: settings.password.clone(),
        sync_password: settings.sync_password.clone(),
    };
    if credentials.password.is_empty() || credentials.sync_password.len() < 8 {
        return Ok(());
    }
    let _ = sync_now_inner(settings, credentials).await;
    Ok(())
}

pub fn spawn_background_sync_loop() {
    tauri::async_runtime::spawn(async move {
        let mut startup_attempted = false;
        loop {
            let settings = match crate::commands::load_settings() {
                Ok(app_settings) => app_settings.sync,
                Err(err) => {
                    eprintln!("[UsageMeter] Failed to load settings for background sync: {err}");
                    tokio::time::sleep(Duration::from_secs(AUTO_SYNC_MIN_INTERVAL_SECONDS)).await;
                    continue;
                }
            };

            if settings.enabled && !startup_attempted && settings.sync_on_startup {
                let _ = run_auto_sync_once(settings.clone(), false).await;
                startup_attempted = true;
            } else if settings.enabled {
                startup_attempted = true;
            }

            let sleep_seconds = if settings.enabled {
                (settings.interval_minutes.max(1) * 60).max(AUTO_SYNC_MIN_INTERVAL_SECONDS)
            } else {
                AUTO_SYNC_MIN_INTERVAL_SECONDS
            };
            tokio::time::sleep(Duration::from_secs(sleep_seconds)).await;

            if settings.enabled {
                let _ = run_auto_sync_once(settings, true).await;
            }
        }
    });
}

async fn run_auto_sync_once(
    settings: SyncSettings,
    enforce_min_interval: bool,
) -> Result<(), String> {
    if !settings.enabled {
        return Ok(());
    }
    let _guard = sync_job_lock().lock().await;
    let db = ensure_local_usage_synced()?;
    let credentials = WebDavCredentials {
        password: settings.password.clone(),
        sync_password: settings.sync_password.clone(),
    };
    if credentials.password.is_empty() || credentials.sync_password.len() < 8 {
        return Ok(());
    }
    if enforce_min_interval {
        if let Some(last_sync_at) = db
            .get_webdav_sync_state("last_sync_at")?
            .and_then(|value| value.parse::<i64>().ok())
        {
            let elapsed = chrono::Utc::now().timestamp() - last_sync_at;
            if elapsed >= 0 && elapsed < AUTO_SYNC_MIN_INTERVAL_SECONDS as i64 {
                return Ok(());
            }
        }
    }
    let _ = sync_now_inner(settings, credentials).await;
    Ok(())
}

pub async fn rotate_sync_password(
    settings: SyncSettings,
    credentials: WebDavCredentials,
    payload: RotateSyncPasswordPayload,
) -> Result<(), String> {
    validate_webdav_config(&settings, &credentials.password)?;
    if payload.current_sync_password.len() < 8 {
        return Err("ERR_SYNC_PASSWORD_TOO_SHORT".to_string());
    }
    if payload.new_sync_password.len() < 8 {
        return Err("ERR_SYNC_PASSWORD_TOO_SHORT".to_string());
    }

    let db = ensure_local_usage_synced()?;
    let previous_device_id = db
        .get_webdav_sync_state("device_id")?
        .map(|value| crate::models::normalize_sync_device_id(&value))
        .filter(|value| !value.trim().is_empty());
    let device_id = resolve_device_id(&settings, db.as_ref())?;
    let instance_id = get_or_create_instance_id(db.as_ref())?;
    let client = WebDavClient::new(settings, credentials.password.clone())?;
    client.check_access().await?;
    client.ensure_base_dirs(&device_id).await?;

    let keyring_state = load_or_create_keyring(&client, &payload.current_sync_password).await?;
    assert_device_id_ownership(
        &client,
        &device_id,
        &instance_id,
        previous_device_id.as_deref(),
        &payload.current_sync_password,
        &keyring_state,
    )
    .await?;

    let keyring = match keyring_state {
        SyncKeyringState::Ready(keyring) => keyring,
        SyncKeyringState::Missing => {
            return Err("ERR_SYNC_PASSWORD_ROTATION_REQUIRES_SYNC".to_string())
        }
    };

    let keyring = rewrap_keyring_dek(
        keyring,
        &payload.current_sync_password,
        &payload.new_sync_password,
    )?;
    store_keyring(&client, &keyring).await?;
    Ok(())
}

async fn sync_now_inner(
    settings: SyncSettings,
    credentials: WebDavCredentials,
) -> Result<(), String> {
    validate_config(&settings, &credentials)?;
    let db = ensure_local_usage_synced()?;
    let previous_device_id = db
        .get_webdav_sync_state("device_id")?
        .map(|value| crate::models::normalize_sync_device_id(&value))
        .filter(|value| !value.trim().is_empty());
    let device_id = resolve_device_id(&settings, db.as_ref())?;
    let instance_id = get_or_create_instance_id(db.as_ref())?;
    let client = WebDavClient::new(settings, credentials.password.clone())?;
    client.ensure_base_dirs(&device_id).await?;
    let mut keyring_state = load_or_create_keyring(&client, &credentials.sync_password).await?;
    assert_device_id_ownership(
        &client,
        &device_id,
        &instance_id,
        previous_device_id.as_deref(),
        &credentials.sync_password,
        &keyring_state,
    )
    .await?;
    db.seed_sync_outbox_from_local(&device_id)?;
    let dek = ensure_dek(&client, &mut keyring_state, &credentials.sync_password).await?;
    let dek_version = keyring_state
        .keyring()
        .map(|keyring| keyring.dek_version)
        .ok_or_else(|| "ERR_SYNC_KEYRING_MISSING".to_string())?;
    sync_shared_settings(&client, &device_id, &instance_id, &credentials).await?;

    let remote_manifest = load_manifest(&client, &device_id).await?;
    let local_last_uploaded_seq = db.get_last_uploaded_batch_seq()?;
    let legacy_last_export_seq = db
        .get_webdav_sync_state("last_export_seq")?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let export_seq = local_last_uploaded_seq
        .max(legacy_last_export_seq)
        .max(remote_manifest.as_ref().map(|manifest| manifest.latest_batch_seq).unwrap_or(0))
        + 1;

    let SyncOutboxBatch {
        request_events,
        session_events,
    } = db.reserve_sync_outbox_batch(&device_id, export_seq, 1000, 250)?;
    let exported_request_count = request_events.len() as u64;
    let exported_at = chrono::Utc::now().timestamp();
    let mut effective_latest_batch_seq = local_last_uploaded_seq.max(legacy_last_export_seq);

    if !request_events.is_empty() || !session_events.is_empty() {
        let package = UsageBatchPackage {
            schema_version: BATCH_SCHEMA_VERSION,
            package_type: "usage_batch".to_string(),
            device_id: device_id.clone(),
            instance_id: instance_id.clone(),
            batch_seq: export_seq,
            prev_batch_seq: export_seq - 1,
            exported_at,
            request_events,
            session_events,
        };
        let encrypted = encrypt_batch_package(&package, &dek, dek_version)?;
        let encrypted_bytes = serde_json::to_vec(&encrypted)
            .map_err(|e| format!("Failed to serialize encrypted sync batch: {}", e))?;
        let remote_path = client.batch_path(&device_id, export_seq);
        if let Err(err) = client.put(&remote_path, encrypted_bytes).await {
            let _ = db.release_sync_outbox_batch(export_seq);
            return Err(err);
        }
        db.mark_sync_outbox_batch_uploaded(
            export_seq,
            &remote_path,
            exported_request_count as usize,
            package.session_events.len(),
        )?;
        effective_latest_batch_seq = export_seq;
    }

    if effective_latest_batch_seq > 0
        && remote_manifest
            .as_ref()
            .map(|manifest| manifest.latest_batch_seq < effective_latest_batch_seq)
            .unwrap_or(true)
    {
        let mut manifest = DeviceManifest {
            schema_version: BATCH_SCHEMA_VERSION,
            device_id: device_id.clone(),
            instance_id: instance_id.clone(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            latest_batch_seq: effective_latest_batch_seq,
            latest_snapshot_seq: remote_manifest
                .as_ref()
                .map(|manifest| manifest.latest_snapshot_seq)
                .unwrap_or(0),
            latest_exported_at: exported_at,
            batch_count: remote_manifest
                .as_ref()
                .map(|manifest| manifest.batch_count)
                .unwrap_or(0)
                .max(effective_latest_batch_seq),
            snapshot_count: remote_manifest
                .as_ref()
                .map(|manifest| manifest.snapshot_count)
                .unwrap_or(0),
        };
        if should_create_snapshot(&manifest) {
            create_and_store_snapshot(
                &client,
                &device_id,
                &instance_id,
                &mut manifest,
                &dek,
                dek_version,
            )
            .await?;
            prune_old_sync_artifacts(&client, &manifest).await?;
        }
        store_manifest(&client, &device_id, &manifest).await?;
    }

    if let Some(keyring) = keyring_state.keyring() {
        store_keyring(&client, keyring).await?;
    }

    let imported_requests = import_remote_packages(
        &client,
        &device_id,
        &credentials.sync_password,
        &mut keyring_state,
    )
    .await?;

    db.upsert_webdav_sync_state("last_sync_at", &exported_at.to_string())?;
    db.upsert_webdav_sync_state("last_status", "success")?;
    db.upsert_webdav_sync_state("last_error", "")?;
    db.upsert_webdav_sync_state(
        "last_uploaded_requests",
        &exported_request_count.to_string(),
    )?;
    db.upsert_webdav_sync_state("last_imported_requests", &imported_requests.to_string())?;
    Ok(())
}

pub fn get_status(settings: &SyncSettings) -> Result<SyncStatus, String> {
    let db = ensure_local_usage_synced()?;
    let last_error = db
        .get_webdav_sync_state("last_error")?
        .filter(|value| !value.trim().is_empty());
    Ok(SyncStatus {
        enabled: settings.enabled,
        last_sync_at: db
            .get_webdav_sync_state("last_sync_at")?
            .and_then(|value| value.parse::<i64>().ok()),
        last_status: db
            .get_webdav_sync_state("last_status")?
            .unwrap_or_else(|| "idle".to_string()),
        last_error,
        uploaded_requests: db
            .get_webdav_sync_state("last_uploaded_requests")?
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0),
        imported_requests: db
            .get_webdav_sync_state("last_imported_requests")?
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0),
    })
}

async fn import_remote_packages(
    client: &WebDavClient,
    own_device_id: &str,
    sync_password: &str,
    keyring_state: &mut SyncKeyringState,
) -> Result<u64, String> {
    let db = ensure_local_usage_synced()?;
    let mut imported_requests = 0_u64;
    let devices = client.list_dirs(client.device_dir()).await?;

    for device_id in devices {
        if device_id == own_device_id || device_id.trim().is_empty() {
            continue;
        }
        let manifest = load_manifest(client, &device_id).await?;
        if let Some(manifest) = manifest {
            let import_result =
                import_device_batches(client, &manifest, sync_password, keyring_state).await;
            match import_result {
                Ok(request_count) => imported_requests += request_count,
                Err(err) => {
                    db.upsert_webdav_sync_state(
                        &format!("failed:{}:manifest", device_id),
                        &err,
                    )?;
                    let _ = db.upsert_import_cursor(
                        &device_id,
                        Some(&manifest.instance_id),
                        db.get_import_cursor(&device_id).unwrap_or(0),
                        "failed",
                        Some(&err),
                    );
                    eprintln!(
                        "[UsageMeter] Skipped WebDAV sync device {} via manifest: {}",
                        device_id, err
                    );
                }
            }
            continue;
        }

        let Some(file) = select_latest_package(client, &device_id).await? else {
            continue;
        };
        let import_result =
            import_one_legacy_package(client, &device_id, &file, sync_password, keyring_state)
                .await;
        match import_result {
            Ok(request_count) => imported_requests += request_count,
            Err(err) => {
                db.upsert_webdav_sync_state(&format!("failed:{}:{}", device_id, file), &err)?;
                eprintln!(
                    "[UsageMeter] Skipped WebDAV sync package {}/{}: {}",
                    device_id, file, err
                );
            }
        }
    }

    Ok(imported_requests)
}

async fn select_latest_package(
    client: &WebDavClient,
    device_id: &str,
) -> Result<Option<String>, String> {
    let files = client.list_files(&client.device_path(device_id)).await?;
    if files.iter().any(|file| file == "latest.json.enc") {
        return Ok(Some("latest.json.enc".to_string()));
    }

    Ok(latest_snapshot_file(files))
}

fn latest_snapshot_file(files: Vec<String>) -> Option<String> {
    files
        .into_iter()
        .filter_map(|file| parse_snapshot_seq(&file).map(|seq| (seq, file)))
        .max_by_key(|(seq, _)| *seq)
        .map(|(_, file)| file)
}

async fn import_one_legacy_package(
    client: &WebDavClient,
    listed_device_id: &str,
    file: &str,
    sync_password: &str,
    keyring_state: &SyncKeyringState,
) -> Result<u64, String> {
    let db = ensure_local_usage_synced()?;
    let bytes = client
        .get(&format!(
            "{}/{}",
            client.device_path(listed_device_id),
            file
        ))
        .await?;
    let encrypted: EncryptedPackage = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Failed to parse encrypted sync package: {}", e))?;
    let package = decrypt_package(&encrypted, sync_password, keyring_state)?;
    if package.device_id.trim().is_empty() {
        return Err("ERR_SYNC_DEVICE_ID_EMPTY".to_string());
    }

    if db.package_imported(&package.device_id, package.export_seq)? {
        return Ok(0);
    }

    let request_count = package.data.requests.len() as u64;
    db.import_remote_sync_data(&package.device_id, package.export_seq, &package.data)?;
    Ok(request_count)
}

async fn import_device_batches(
    client: &WebDavClient,
    manifest: &DeviceManifest,
    sync_password: &str,
    keyring_state: &SyncKeyringState,
) -> Result<u64, String> {
    let db = ensure_local_usage_synced()?;
    let mut imported_requests = 0_u64;
    let mut cursor = db.get_import_cursor(&manifest.device_id)?;
    if cursor == 0 && manifest.latest_snapshot_seq > 0 {
        if let Some(snapshot) = import_device_snapshot(client, manifest, sync_password, keyring_state).await? {
            imported_requests += snapshot.requests.len() as u64;
            db.import_remote_sync_data(
                &manifest.device_id,
                manifest.latest_snapshot_seq,
                &snapshot,
            )?;
            cursor = manifest.latest_snapshot_seq;
            db.upsert_import_cursor(
                &manifest.device_id,
                Some(&manifest.instance_id),
                cursor,
                "ready",
                None,
            )?;
        }
    }

    if manifest.latest_batch_seq <= cursor {
        let _ = db.upsert_import_cursor(
            &manifest.device_id,
            Some(&manifest.instance_id),
            cursor,
            "ready",
            None,
        );
        return Ok(0);
    }

    for batch_seq in (cursor + 1)..=manifest.latest_batch_seq {
        let remote_path = client.batch_path(&manifest.device_id, batch_seq);
        let bytes = client.get(&remote_path).await?;
        let encrypted: EncryptedPackage = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Failed to parse encrypted sync batch: {}", e))?;
        let package: UsageBatchPackage =
            decrypt_typed_package(&encrypted, sync_password, keyring_state)?;
        if package.device_id != manifest.device_id {
            return Err("ERR_SYNC_DEVICE_ID_CONFLICT".to_string());
        }
        if package.instance_id != manifest.instance_id {
            return Err("ERR_SYNC_DEVICE_ID_CONFLICT".to_string());
        }
        if package.batch_seq != batch_seq {
            return Err("ERR_SYNC_BATCH_SEQ_MISMATCH".to_string());
        }
        if batch_seq > 1 && package.prev_batch_seq != batch_seq - 1 {
            return Err("ERR_SYNC_BATCH_CHAIN_BROKEN".to_string());
        }

        let export_data = SyncExportData {
            sessions: package.session_events.clone(),
            requests: package.request_events.clone(),
        };
        imported_requests += export_data.requests.len() as u64;
        db.import_remote_sync_data(&package.device_id, package.batch_seq, &export_data)?;
        db.upsert_import_cursor(
            &manifest.device_id,
            Some(&manifest.instance_id),
            batch_seq,
            "ready",
            None,
        )?;
    }

    Ok(imported_requests)
}

async fn import_device_snapshot(
    client: &WebDavClient,
    manifest: &DeviceManifest,
    sync_password: &str,
    keyring_state: &SyncKeyringState,
) -> Result<Option<SyncExportData>, String> {
    if manifest.latest_snapshot_seq <= 0 {
        return Ok(None);
    }
    let bytes = client
        .get_optional(&client.snapshot_path(&manifest.device_id, manifest.latest_snapshot_seq))
        .await?;
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let encrypted: EncryptedPackage = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Failed to parse encrypted sync snapshot: {}", e))?;
    let snapshot: SnapshotPackage = decrypt_typed_package(&encrypted, sync_password, keyring_state)?;
    if snapshot.device_id != manifest.device_id {
        return Err("ERR_SYNC_DEVICE_ID_CONFLICT".to_string());
    }
    if snapshot.instance_id != manifest.instance_id {
        return Err("ERR_SYNC_DEVICE_ID_CONFLICT".to_string());
    }
    if snapshot.covered_until_batch_seq != manifest.latest_snapshot_seq {
        return Err("ERR_SYNC_SNAPSHOT_SEQ_MISMATCH".to_string());
    }
    Ok(Some(snapshot.data))
}

async fn assert_device_id_ownership(
    client: &WebDavClient,
    device_id: &str,
    instance_id: &str,
    previous_device_id: Option<&str>,
    sync_password: &str,
    keyring_state: &SyncKeyringState,
) -> Result<(), String> {
    if let Some(manifest) = load_manifest(client, device_id).await? {
        if manifest.device_id != device_id {
            return Err("ERR_SYNC_DEVICE_ID_CONFLICT".to_string());
        }
        if manifest.instance_id == instance_id {
            return Ok(());
        }
        if previous_device_id == Some(device_id) {
            return Ok(());
        }
        return Err("ERR_SYNC_DEVICE_ID_CONFLICT".to_string());
    }

    let latest_path = format!("{}/latest.json.enc", client.device_path(device_id));
    let bytes = match client.get_optional(&latest_path).await? {
        Some(bytes) => bytes,
        None => return Ok(()),
    };

    let encrypted: EncryptedPackage = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Failed to parse encrypted sync package: {}", e))?;
    let package = decrypt_package_for_ownership(&encrypted, sync_password, keyring_state)?;
    let remote_device_id = crate::models::normalize_sync_device_id(&package.device_id);
    if remote_device_id != device_id {
        return Err("ERR_SYNC_DEVICE_ID_CONFLICT".to_string());
    }

    match package.instance_id.as_deref() {
        Some(remote_instance_id) if remote_instance_id == instance_id => Ok(()),
        Some(_) => Err("ERR_SYNC_DEVICE_ID_CONFLICT".to_string()),
        None if previous_device_id == Some(device_id) => Ok(()),
        None => Err("ERR_SYNC_DEVICE_ID_CONFLICT".to_string()),
    }
}

fn parse_snapshot_seq(file: &str) -> Option<i64> {
    file.strip_prefix("snapshot-")?
        .strip_suffix(".json.enc")?
        .parse::<i64>()
        .ok()
}

fn parse_batch_seq(file: &str) -> Option<i64> {
    file.strip_suffix(".json.enc")?.parse::<i64>().ok()
}

async fn load_manifest(
    client: &WebDavClient,
    device_id: &str,
) -> Result<Option<DeviceManifest>, String> {
    let Some(bytes) = client
        .get_optional(&format!("{}/{}", client.device_path(device_id), MANIFEST_FILE))
        .await?
    else {
        return Ok(None);
    };
    let manifest: DeviceManifest = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Failed to parse sync manifest: {}", e))?;
    Ok(Some(manifest))
}

fn should_create_snapshot(manifest: &DeviceManifest) -> bool {
    manifest.latest_batch_seq > 0
        && manifest.latest_batch_seq >= manifest.latest_snapshot_seq + SNAPSHOT_INTERVAL_BATCHES
}

async fn create_and_store_snapshot(
    client: &WebDavClient,
    device_id: &str,
    instance_id: &str,
    manifest: &mut DeviceManifest,
    dek: &[u8; KEY_LEN],
    dek_version: u32,
) -> Result<(), String> {
    let db = ensure_local_usage_synced()?;
    let data = db.get_sync_export_data()?;
    let snapshot = SnapshotPackage {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        package_type: "usage_snapshot".to_string(),
        device_id: device_id.to_string(),
        instance_id: instance_id.to_string(),
        covered_until_batch_seq: manifest.latest_batch_seq,
        exported_at: chrono::Utc::now().timestamp(),
        data,
    };
    let encrypted = encrypt_snapshot_package(&snapshot, dek, dek_version)?;
    let bytes = serde_json::to_vec(&encrypted)
        .map_err(|e| format!("Failed to serialize encrypted sync snapshot: {}", e))?;
    client
        .put(
            &client.snapshot_path(device_id, manifest.latest_batch_seq),
            bytes,
        )
        .await?;
    manifest.latest_snapshot_seq = manifest.latest_batch_seq;
    manifest.snapshot_count += 1;
    Ok(())
}

async fn prune_old_sync_artifacts(
    client: &WebDavClient,
    manifest: &DeviceManifest,
) -> Result<(), String> {
    if manifest.latest_snapshot_seq <= 0 {
        return Ok(());
    }

    let keep_from = (manifest.latest_snapshot_seq - BATCH_RETENTION_AFTER_SNAPSHOT).max(0);
    let batch_files = client.list_files(&client.batch_dir_path(&manifest.device_id)).await?;
    for file in batch_files {
        let Some(seq) = parse_batch_seq(&file) else {
            continue;
        };
        if seq < keep_from {
            let _ = client
                .delete(&format!("{}/{}", client.batch_dir_path(&manifest.device_id), file))
                .await;
        }
    }

    let snapshot_files = client
        .list_files(&client.snapshot_dir_path(&manifest.device_id))
        .await?;
    for file in snapshot_files {
        let Some(seq) = parse_snapshot_seq(&file) else {
            continue;
        };
        if seq < manifest.latest_snapshot_seq {
            let _ = client
                .delete(&format!("{}/{}", client.snapshot_dir_path(&manifest.device_id), file))
                .await;
        }
    }

    Ok(())
}

async fn store_manifest(
    client: &WebDavClient,
    device_id: &str,
    manifest: &DeviceManifest,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(manifest)
        .map_err(|e| format!("Failed to serialize sync manifest: {}", e))?;
    client
        .put(&format!("{}/{}", client.device_path(device_id), MANIFEST_FILE), bytes)
        .await
}

async fn sync_shared_settings(
    client: &WebDavClient,
    device_id: &str,
    _instance_id: &str,
    credentials: &WebDavCredentials,
) -> Result<(), String> {
    let mut app_settings = crate::commands::load_settings()?;
    let db = ensure_local_usage_synced()?;
    let local_version = db
        .get_webdav_sync_state("shared_settings_version")?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);

    let remote_document = load_shared_settings_document(client, credentials).await?;
    if let Some(document) = remote_document.as_ref() {
        if document.version > local_version {
            apply_shared_settings_payload(&mut app_settings, &document.payload);
            crate::commands::save_settings(app_settings.clone())?;
            db.upsert_webdav_sync_state("shared_settings_version", &document.version.to_string())?;
        }
    }

    let current_version = db
        .get_webdav_sync_state("shared_settings_version")?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let local_payload = extract_shared_settings_payload(&app_settings);
    let should_push = match remote_document.as_ref() {
        Some(document) => {
            current_version > document.version
                || (current_version == document.version
                    && !shared_settings_payload_matches(&local_payload, &document.payload))
        }
        None => true,
    };
    if should_push {
        let keyring_state = load_or_create_keyring(client, &credentials.sync_password).await?;
        let keyring = keyring_state
            .keyring()
            .ok_or_else(|| "ERR_SYNC_KEYRING_MISSING".to_string())?;
        let dek = unwrap_dek(keyring, &credentials.sync_password)?;
        let version = current_version.max(remote_document.as_ref().map(|d| d.version).unwrap_or(0)) + 1;
        let document = SharedSettingsDocument {
            schema_version: 1,
            document_type: "shared_settings".to_string(),
            updated_at: chrono::Utc::now().timestamp(),
            updated_by_device_id: device_id.to_string(),
            version,
            payload: local_payload,
        };
        store_shared_settings_document(client, &document, &dek, keyring.dek_version).await?;
        db.upsert_webdav_sync_state("shared_settings_version", &version.to_string())?;
    }

    Ok(())
}

fn extract_shared_settings_payload(
    settings: &crate::models::AppSettings,
) -> SharedSettingsPayload {
    SharedSettingsPayload {
        locale: settings.locale.clone(),
        timezone: settings.timezone.clone(),
        summary_window: settings.summary_window.clone(),
        theme: settings.theme.clone(),
        model_pricing: settings.model_pricing.clone(),
        source_aware: settings.source_aware.clone(),
        currency: settings.currency.clone(),
    }
}

fn apply_shared_settings_payload(
    settings: &mut crate::models::AppSettings,
    payload: &SharedSettingsPayload,
) {
    settings.locale = payload.locale.clone();
    settings.timezone = payload.timezone.clone();
    settings.summary_window = payload.summary_window.clone();
    settings.theme = payload.theme.clone();
    settings.model_pricing = payload.model_pricing.clone();
    settings.source_aware = payload.source_aware.clone();
    settings.currency = payload.currency.clone();
}

fn shared_settings_payload_matches(
    left: &SharedSettingsPayload,
    right: &SharedSettingsPayload,
) -> bool {
    left == right
}

async fn load_shared_settings_document(
    client: &WebDavClient,
    credentials: &WebDavCredentials,
) -> Result<Option<SharedSettingsDocument>, String> {
    let Some(bytes) = client.get_optional(SHARED_SETTINGS_FILE).await? else {
        return Ok(None);
    };
    let encrypted: EncryptedPackage = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Failed to parse encrypted shared settings: {}", e))?;
    let keyring_state = load_or_create_keyring(client, &credentials.sync_password).await?;
    let document: SharedSettingsDocument =
        decrypt_typed_package(&encrypted, &credentials.sync_password, &keyring_state)?;
    Ok(Some(document))
}

async fn store_shared_settings_document(
    client: &WebDavClient,
    document: &SharedSettingsDocument,
    dek: &[u8; KEY_LEN],
    dek_version: u32,
) -> Result<(), String> {
    client.mkcol(SHARED_DIR).await?;
    client.mkcol(SHARED_SETTINGS_DIR).await?;
    let plaintext = serde_json::to_vec(document)
        .map_err(|e| format!("Failed to serialize shared settings document: {}", e))?;
    let nonce = make_nonce()?;
    let payload = encrypt_bytes(&plaintext, dek, b"shared_settings", &nonce)?;
    let encrypted = EncryptedPackage {
        schema_version: 2,
        algorithm: "chacha20-poly1305".to_string(),
        kdf: format!("pbkdf2-hmac-sha256:{}", PBKDF2_ROUNDS),
        device_id: "shared_settings".to_string(),
        dek_version,
        export_seq: document.version,
        nonce: BASE64.encode(nonce),
        payload: BASE64.encode(payload),
    };
    let bytes = serde_json::to_vec(&encrypted)
        .map_err(|e| format!("Failed to serialize encrypted shared settings: {}", e))?;
    client.put(SHARED_SETTINGS_FILE, bytes).await
}

fn persist_failure(err: &str) {
    if let Ok(db) = ensure_local_usage_synced() {
        let _ = db.upsert_webdav_sync_state("last_status", "failed");
        let _ = db.upsert_webdav_sync_state("last_error", err);
    }
}

fn validate_config(settings: &SyncSettings, credentials: &WebDavCredentials) -> Result<(), String> {
    validate_webdav_config(settings, &credentials.password)?;
    if credentials.sync_password.len() < 8 {
        return Err("ERR_SYNC_PASSWORD_TOO_SHORT".to_string());
    }
    Ok(())
}

fn validate_webdav_config(settings: &SyncSettings, webdav_password: &str) -> Result<(), String> {
    if settings.url.trim().is_empty() {
        return Err("ERR_WEBDAV_URL_REQUIRED".to_string());
    }
    if settings.username.trim().is_empty() {
        return Err("ERR_WEBDAV_USERNAME_REQUIRED".to_string());
    }
    if webdav_password.is_empty() {
        return Err("ERR_WEBDAV_PASSWORD_REQUIRED".to_string());
    }
    Ok(())
}

fn resolve_device_id(
    settings: &SyncSettings,
    db: &crate::local_usage::LocalUsageDatabase,
) -> Result<String, String> {
    let configured = crate::models::normalize_sync_device_id(&settings.device_id);
    if !configured.is_empty() {
        crate::models::validate_sync_device_id(&configured)?;
        db.upsert_webdav_sync_state("device_id", &configured)?;
        return Ok(configured);
    }
    get_or_create_device_id(db)
}

fn get_or_create_device_id(db: &crate::local_usage::LocalUsageDatabase) -> Result<String, String> {
    if let Some(existing) = db.get_webdav_sync_state("device_id")? {
        if !existing.trim().is_empty() {
            let normalized = crate::models::normalize_sync_device_id(&existing);
            crate::models::validate_sync_device_id(&normalized)?;
            return Ok(normalized);
        }
    }

    let device_id =
        crate::models::normalize_sync_device_id(&crate::models::default_sync_device_id());
    crate::models::validate_sync_device_id(&device_id)?;
    db.upsert_webdav_sync_state("device_id", &device_id)?;
    Ok(device_id)
}

fn get_or_create_instance_id(
    db: &crate::local_usage::LocalUsageDatabase,
) -> Result<String, String> {
    if let Some(existing) = db.get_webdav_sync_state("instance_id")? {
        let normalized = crate::models::normalize_sync_device_id(&existing);
        if !normalized.is_empty() {
            return Ok(normalized);
        }
    }

    let instance_id = format!(
        "{}-{}",
        crate::models::normalize_sync_device_id(&crate::models::default_sync_device_id()),
        make_random_id()?
    );
    db.upsert_webdav_sync_state("instance_id", &instance_id)?;
    Ok(instance_id)
}

async fn load_or_create_keyring(
    client: &WebDavClient,
    sync_password: &str,
) -> Result<SyncKeyringState, String> {
    if let Some(bytes) = client.get_optional(KEYRING_FILE).await? {
        let keyring: SyncKeyring = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Failed to parse sync keyring: {}", e))?;
        let _ = unwrap_dek(&keyring, sync_password)?;
        return Ok(SyncKeyringState::Ready(keyring));
    }

    Ok(SyncKeyringState::Missing)
}

async fn store_keyring(client: &WebDavClient, keyring: &SyncKeyring) -> Result<(), String> {
    client.ensure_meta_dir().await?;
    let bytes = serde_json::to_vec(keyring)
        .map_err(|e| format!("Failed to serialize sync keyring: {}", e))?;
    client.put(KEYRING_FILE, bytes).await
}

fn wrap_new_keyring(
    sync_password: &str,
    wrap_salt: &str,
    dek: [u8; KEY_LEN],
    dek_version: u32,
) -> Result<SyncKeyring, String> {
    let nonce = make_nonce()?;
    let wrapping_key = derive_key(sync_password, wrap_salt);
    let wrapped_dek = encrypt_bytes(&dek, &wrapping_key, wrap_salt.as_bytes(), &nonce)?;
    Ok(SyncKeyring {
        schema_version: 1,
        dek_version,
        algorithm: "chacha20-poly1305".to_string(),
        kdf: format!("pbkdf2-hmac-sha256:{}", PBKDF2_ROUNDS),
        wrap_salt: wrap_salt.to_string(),
        nonce: BASE64.encode(nonce),
        wrapped_dek: BASE64.encode(wrapped_dek),
        updated_at: chrono::Utc::now().timestamp(),
    })
}

fn rewrap_keyring_dek(
    keyring: SyncKeyring,
    current_sync_password: &str,
    new_sync_password: &str,
) -> Result<SyncKeyring, String> {
    let dek = unwrap_dek(&keyring, current_sync_password)?;
    wrap_new_keyring(
        new_sync_password,
        &format!("dek-v{}", keyring.dek_version + 1),
        dek,
        keyring.dek_version + 1,
    )
}

fn unwrap_dek(keyring: &SyncKeyring, sync_password: &str) -> Result<[u8; KEY_LEN], String> {
    let nonce = BASE64
        .decode(&keyring.nonce)
        .map_err(|e| format!("Failed to decode keyring nonce: {}", e))?;
    let cipher = BASE64
        .decode(&keyring.wrapped_dek)
        .map_err(|e| format!("Failed to decode wrapped DEK: {}", e))?;
    let wrapping_key = derive_key(sync_password, &keyring.wrap_salt);
    let plaintext = decrypt_bytes(cipher, &wrapping_key, &keyring.wrap_salt, &nonce)?;
    plaintext
        .as_slice()
        .try_into()
        .map_err(|_| "ERR_SYNC_DEK_INVALID".to_string())
}

async fn ensure_dek(
    client: &WebDavClient,
    keyring_state: &mut SyncKeyringState,
    sync_password: &str,
) -> Result<[u8; KEY_LEN], String> {
    match keyring_state {
        SyncKeyringState::Ready(keyring) => unwrap_dek(keyring, sync_password),
        SyncKeyringState::Missing => {
            let dek = make_random_key()?;
            let keyring = wrap_new_keyring(sync_password, "dek-v1", dek, 1)?;
            store_keyring(client, &keyring).await?;
            *keyring_state = SyncKeyringState::Ready(keyring);
            Ok(dek)
        }
    }
}

fn encrypt_batch_package(
    package: &UsageBatchPackage,
    dek: &[u8; KEY_LEN],
    dek_version: u32,
) -> Result<EncryptedPackage, String> {
    let plaintext = serde_json::to_vec(package)
        .map_err(|e| format!("Failed to serialize sync batch package: {}", e))?;
    let nonce = make_nonce()?;
    let payload = encrypt_bytes(&plaintext, dek, package.device_id.as_bytes(), &nonce)?;

    Ok(EncryptedPackage {
        schema_version: BATCH_SCHEMA_VERSION,
        algorithm: "chacha20-poly1305".to_string(),
        kdf: format!("pbkdf2-hmac-sha256:{}", PBKDF2_ROUNDS),
        device_id: package.device_id.clone(),
        dek_version,
        export_seq: package.batch_seq,
        nonce: BASE64.encode(nonce),
        payload: BASE64.encode(payload),
    })
}

fn encrypt_snapshot_package(
    package: &SnapshotPackage,
    dek: &[u8; KEY_LEN],
    dek_version: u32,
) -> Result<EncryptedPackage, String> {
    let plaintext = serde_json::to_vec(package)
        .map_err(|e| format!("Failed to serialize sync snapshot package: {}", e))?;
    let nonce = make_nonce()?;
    let payload = encrypt_bytes(&plaintext, dek, package.device_id.as_bytes(), &nonce)?;

    Ok(EncryptedPackage {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        algorithm: "chacha20-poly1305".to_string(),
        kdf: format!("pbkdf2-hmac-sha256:{}", PBKDF2_ROUNDS),
        device_id: package.device_id.clone(),
        dek_version,
        export_seq: package.covered_until_batch_seq,
        nonce: BASE64.encode(nonce),
        payload: BASE64.encode(payload),
    })
}

fn decrypt_package_with_keyring(
    encrypted: &EncryptedPackage,
    sync_password: &str,
    keyring: &SyncKeyring,
) -> Result<SyncPackage, String> {
    if encrypted.schema_version != 1 {
        return Err("ERR_SYNC_SCHEMA_UNSUPPORTED".to_string());
    }
    if encrypted.dek_version == 0 {
        return Err("ERR_SYNC_LEGACY_PACKAGE_UNSUPPORTED".to_string());
    }
    let dek = unwrap_dek(keyring, sync_password)?;
    let nonce = BASE64
        .decode(&encrypted.nonce)
        .map_err(|e| format!("Failed to decode sync nonce: {}", e))?;
    let cipher = BASE64
        .decode(&encrypted.payload)
        .map_err(|e| format!("Failed to decode sync payload: {}", e))?;
    let plaintext = decrypt_bytes(cipher, &dek, &encrypted.device_id, &nonce)?;
    serde_json::from_slice(&plaintext).map_err(|e| format!("Failed to parse sync package: {}", e))
}

fn decrypt_package_for_ownership(
    encrypted: &EncryptedPackage,
    sync_password: &str,
    keyring_state: &SyncKeyringState,
) -> Result<SyncPackage, String> {
    match keyring_state {
        SyncKeyringState::Ready(keyring) => {
            decrypt_package_with_keyring(encrypted, sync_password, keyring)
        }
        SyncKeyringState::Missing => Err("ERR_SYNC_KEYRING_MISSING".to_string()),
    }
}

fn decrypt_package(
    encrypted: &EncryptedPackage,
    sync_password: &str,
    keyring_state: &SyncKeyringState,
) -> Result<SyncPackage, String> {
    match keyring_state {
        SyncKeyringState::Ready(keyring) => {
            decrypt_package_with_keyring(encrypted, sync_password, keyring)
        }
        SyncKeyringState::Missing => Err("ERR_SYNC_KEYRING_MISSING".to_string()),
    }
}

fn decrypt_typed_package<T: DeserializeOwned>(
    encrypted: &EncryptedPackage,
    sync_password: &str,
    keyring_state: &SyncKeyringState,
) -> Result<T, String> {
    if encrypted.schema_version != BATCH_SCHEMA_VERSION
        && encrypted.schema_version != SNAPSHOT_SCHEMA_VERSION
    {
        return Err("ERR_SYNC_SCHEMA_UNSUPPORTED".to_string());
    }
    let keyring = match keyring_state {
        SyncKeyringState::Ready(keyring) => keyring,
        SyncKeyringState::Missing => return Err("ERR_SYNC_KEYRING_MISSING".to_string()),
    };
    if encrypted.dek_version == 0 {
        return Err("ERR_SYNC_LEGACY_PACKAGE_UNSUPPORTED".to_string());
    }
    let dek = unwrap_dek(keyring, sync_password)?;
    let nonce = BASE64
        .decode(&encrypted.nonce)
        .map_err(|e| format!("Failed to decode sync nonce: {}", e))?;
    let cipher = BASE64
        .decode(&encrypted.payload)
        .map_err(|e| format!("Failed to decode sync payload: {}", e))?;
    let plaintext = decrypt_bytes(cipher, &dek, &encrypted.device_id, &nonce)?;
    serde_json::from_slice(&plaintext)
        .map_err(|e| format!("Failed to parse typed sync package: {}", e))
}

fn derive_key(password: &str, salt: &str) -> [u8; KEY_LEN] {
    let mut key = [0_u8; KEY_LEN];
    let rounds = NonZeroU32::new(PBKDF2_ROUNDS).expect("PBKDF2_ROUNDS must be non-zero");
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        rounds,
        salt.as_bytes(),
        password.as_bytes(),
        &mut key,
    );
    key
}

fn make_nonce() -> Result<Vec<u8>, String> {
    let rng = rand::SystemRandom::new();
    let mut nonce = vec![0_u8; NONCE_LEN];
    rand::SecureRandom::fill(&rng, &mut nonce)
        .map_err(|_| "ERR_SYNC_NONCE_GENERATION_FAILED".to_string())?;
    Ok(nonce)
}

fn make_random_key() -> Result<[u8; KEY_LEN], String> {
    let rng = rand::SystemRandom::new();
    let mut key = [0_u8; KEY_LEN];
    rand::SecureRandom::fill(&rng, &mut key)
        .map_err(|_| "ERR_SYNC_DEK_GENERATION_FAILED".to_string())?;
    Ok(key)
}

fn make_random_id() -> Result<String, String> {
    let rng = rand::SystemRandom::new();
    let mut bytes = [0_u8; 12];
    rand::SecureRandom::fill(&rng, &mut bytes)
        .map_err(|_| "ERR_SYNC_RANDOM_ID_GENERATION_FAILED".to_string())?;
    Ok(bytes.iter().map(|byte| format!("{:02x}", byte)).collect())
}

fn encrypt_bytes(
    plaintext: &[u8],
    key: &[u8; KEY_LEN],
    aad: &[u8],
    nonce: &[u8],
) -> Result<Vec<u8>, String> {
    let mut buffer = plaintext.to_vec();
    let unbound_key = aead::UnboundKey::new(&aead::CHACHA20_POLY1305, key)
        .map_err(|_| "ERR_SYNC_ENCRYPT_FAILED".to_string())?;
    let sealing_key = aead::LessSafeKey::new(unbound_key);
    let nonce_array: [u8; NONCE_LEN] = nonce
        .try_into()
        .map_err(|_| "ERR_SYNC_NONCE_INVALID".to_string())?;
    sealing_key
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce_array),
            aead::Aad::from(aad),
            &mut buffer,
        )
        .map_err(|_| "ERR_SYNC_ENCRYPT_FAILED".to_string())?;
    Ok(buffer)
}

fn decrypt_bytes(
    mut cipher: Vec<u8>,
    key: &[u8; KEY_LEN],
    aad: impl AsRef<[u8]>,
    nonce: &[u8],
) -> Result<Vec<u8>, String> {
    let unbound_key = aead::UnboundKey::new(&aead::CHACHA20_POLY1305, key)
        .map_err(|_| "ERR_SYNC_DECRYPT_FAILED".to_string())?;
    let opening_key = aead::LessSafeKey::new(unbound_key);
    let nonce_array: [u8; NONCE_LEN] = nonce
        .try_into()
        .map_err(|_| "ERR_SYNC_NONCE_INVALID".to_string())?;
    let plaintext = opening_key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce_array),
            aead::Aad::from(aad.as_ref()),
            &mut cipher,
        )
        .map_err(|_| "ERR_SYNC_DECRYPT_FAILED".to_string())?;
    Ok(plaintext.to_vec())
}

struct WebDavClient {
    client: Client,
    base_url: String,
    root_dir: String,
    device_dir: String,
    username: String,
    password: String,
}

impl WebDavClient {
    fn new(settings: SyncSettings, password: String) -> Result<Self, String> {
        Self::new_with_root(
            settings,
            password,
            ROOT_DIR.to_string(),
            "devices".to_string(),
        )
    }

    fn new_with_root(
        settings: SyncSettings,
        password: String,
        root_dir: String,
        device_dir: String,
    ) -> Result<Self, String> {
        let root = settings.url.trim().trim_end_matches('/');
        if !url_is_allowed(root) {
            return Err("ERR_WEBDAV_URL_INVALID".to_string());
        }
        Ok(Self {
            client: Client::new(),
            base_url: format!("{}/{}", root, root_dir),
            root_dir,
            device_dir,
            username: settings.username,
            password,
        })
    }

    async fn check_access(&self) -> Result<(), String> {
        let response = self
            .client
            .request(Method::OPTIONS, self.root_url())
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .map_err(|e| format!("ERR_WEBDAV_REQUEST_FAILED: {}", e))?;
        if response.status().is_success() {
            return Ok(());
        }
        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            let _ = self.propfind_root().await?;
            return Ok(());
        }
        Err(format!("ERR_WEBDAV_AUTH_FAILED: {}", response.status()))
    }

    async fn ensure_base_dirs(&self, device_id: &str) -> Result<(), String> {
        self.mkcol("").await?;
        self.mkcol(self.device_dir()).await?;
        if !device_id.trim().is_empty() {
            self.mkcol(&self.device_path(device_id)).await?;
            self.mkcol(&self.batch_dir_path(device_id)).await?;
            self.mkcol(&self.snapshot_dir_path(device_id)).await?;
        }
        Ok(())
    }

    async fn ensure_meta_dir(&self) -> Result<(), String> {
        self.mkcol("").await?;
        self.mkcol(META_DIR).await
    }

    async fn mkcol(&self, path: &str) -> Result<(), String> {
        let method = Method::from_bytes(b"MKCOL").map_err(|e| e.to_string())?;
        let response = self
            .client
            .request(method, self.url(path))
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .map_err(|e| format!("ERR_WEBDAV_REQUEST_FAILED: {}", e))?;
        if response.status().is_success()
            || response.status() == StatusCode::METHOD_NOT_ALLOWED
            || response.status() == StatusCode::CONFLICT
        {
            return Ok(());
        }
        Err(format!("ERR_WEBDAV_MKCOL_FAILED: {}", response.status()))
    }

    async fn put(&self, path: &str, bytes: Vec<u8>) -> Result<(), String> {
        let response = self
            .client
            .put(self.url(path))
            .basic_auth(&self.username, Some(&self.password))
            .body(bytes)
            .send()
            .await
            .map_err(|e| format!("ERR_WEBDAV_REQUEST_FAILED: {}", e))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("ERR_WEBDAV_PUT_FAILED: {}", response.status()))
        }
    }

    async fn delete(&self, path: &str) -> Result<(), String> {
        let response = self
            .client
            .delete(self.url(path))
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .map_err(|e| format!("ERR_WEBDAV_REQUEST_FAILED: {}", e))?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(format!("ERR_WEBDAV_DELETE_FAILED: {}", response.status()))
        }
    }

    async fn get(&self, path: &str) -> Result<Vec<u8>, String> {
        let response = self
            .client
            .get(self.url(path))
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .map_err(|e| format!("ERR_WEBDAV_REQUEST_FAILED: {}", e))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("ERR_WEBDAV_GET_FAILED: {}", status));
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|e| format!("ERR_WEBDAV_READ_FAILED: {}", e))
    }

    async fn get_optional(&self, path: &str) -> Result<Option<Vec<u8>>, String> {
        let response = self
            .client
            .get(self.url(path))
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .map_err(|e| format!("ERR_WEBDAV_REQUEST_FAILED: {}", e))?;
        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(format!("ERR_WEBDAV_GET_FAILED: {}", status));
        }
        response
            .bytes()
            .await
            .map(|bytes| Some(bytes.to_vec()))
            .map_err(|e| format!("ERR_WEBDAV_READ_FAILED: {}", e))
    }

    async fn list_dirs(&self, path: &str) -> Result<Vec<String>, String> {
        let entries = self.propfind(path).await?;
        Ok(entries
            .into_iter()
            .filter(|entry| entry.is_collection)
            .map(|entry| entry.name)
            .collect())
    }

    async fn list_files(&self, path: &str) -> Result<Vec<String>, String> {
        let entries = self.propfind(path).await?;
        Ok(entries
            .into_iter()
            .filter(|entry| !entry.is_collection)
            .map(|entry| entry.name)
            .collect())
    }

    async fn propfind(&self, path: &str) -> Result<Vec<WebDavEntry>, String> {
        let method = Method::from_bytes(b"PROPFIND").map_err(|e| e.to_string())?;
        let response = self
            .client
            .request(method, self.url(path))
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "1")
            .body(
                r#"<?xml version="1.0" encoding="utf-8"?><propfind xmlns="DAV:"><prop><resourcetype/></prop></propfind>"#,
            )
            .send()
            .await
            .map_err(|e| format!("ERR_WEBDAV_REQUEST_FAILED: {}", e))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if !response.status().is_success() && response.status().as_u16() != 207 {
            return Err(format!("ERR_WEBDAV_PROPFIND_FAILED: {}", response.status()));
        }
        let text = response
            .text()
            .await
            .map_err(|e| format!("ERR_WEBDAV_READ_FAILED: {}", e))?;
        Ok(extract_propfind_entries(&text)
            .into_iter()
            .filter_map(|entry| {
                let name = leaf_name(&entry.href)?;
                if name == self.root_dir || name == path.trim_matches('/') {
                    return None;
                }
                Some(WebDavEntry {
                    name,
                    is_collection: entry.is_collection,
                })
            })
            .collect())
    }

    fn url(&self, path: &str) -> String {
        let clean = path.trim_matches('/');
        if clean.is_empty() {
            self.base_url.clone()
        } else {
            format!("{}/{}", self.base_url, clean)
        }
    }

    fn root_url(&self) -> String {
        self.base_url
            .strip_suffix(&format!("/{}", self.root_dir))
            .unwrap_or(&self.base_url)
            .to_string()
    }

    fn device_dir(&self) -> &str {
        &self.device_dir
    }

    fn device_path(&self, device_id: &str) -> String {
        format!("{}/{}", self.device_dir, device_id)
    }

    fn batch_dir_path(&self, device_id: &str) -> String {
        format!("{}/{}", self.device_path(device_id), BATCH_DIR)
    }

    fn snapshot_dir_path(&self, device_id: &str) -> String {
        format!("{}/{}", self.device_path(device_id), SNAPSHOT_DIR)
    }

    fn batch_path(&self, device_id: &str, batch_seq: i64) -> String {
        format!("{}/{:012}.json.enc", self.batch_dir_path(device_id), batch_seq)
    }

    fn snapshot_path(&self, device_id: &str, batch_seq: i64) -> String {
        format!(
            "{}/snapshot-{:012}.json.enc",
            self.snapshot_dir_path(device_id),
            batch_seq
        )
    }

    async fn propfind_root(&self) -> Result<Vec<String>, String> {
        self.propfind_absolute(&self.root_url()).await
    }

    async fn propfind_absolute(&self, url: &str) -> Result<Vec<String>, String> {
        let method = Method::from_bytes(b"PROPFIND").map_err(|e| e.to_string())?;
        let response = self
            .client
            .request(method, url.to_string())
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "0")
            .body(
                r#"<?xml version="1.0" encoding="utf-8"?><propfind xmlns="DAV:"><prop><resourcetype/></prop></propfind>"#,
            )
            .send()
            .await
            .map_err(|e| format!("ERR_WEBDAV_REQUEST_FAILED: {}", e))?;
        if !response.status().is_success() && response.status().as_u16() != 207 {
            return Err(format!("ERR_WEBDAV_PROPFIND_FAILED: {}", response.status()));
        }
        let text = response
            .text()
            .await
            .map_err(|e| format!("ERR_WEBDAV_READ_FAILED: {}", e))?;
        Ok(extract_hrefs(&text)
            .into_iter()
            .filter_map(|href| leaf_name(&href))
            .collect())
    }
}

struct WebDavEntry {
    name: String,
    is_collection: bool,
}

struct PropfindEntry {
    href: String,
    is_collection: bool,
}

fn url_is_allowed(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    match parsed.scheme() {
        "https" => true,
        "http" => matches!(
            parsed.host_str(),
            Some("localhost") | Some("127.0.0.1") | Some("::1")
        ),
        _ => false,
    }
}

fn extract_hrefs(xml: &str) -> Vec<String> {
    let mut result = Vec::new();
    let lower = xml.to_lowercase();
    let mut offset = 0_usize;
    while let Some(relative_start) = lower[offset..].find("<") {
        let tag_start = offset + relative_start;
        let Some(tag_end_rel) = lower[tag_start..].find('>') else {
            break;
        };
        let tag_end = tag_start + tag_end_rel;
        let tag = lower[tag_start + 1..tag_end].trim();
        let tag_name = tag
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_start_matches('/');
        let local_name = tag_name.rsplit(':').next().unwrap_or(tag_name);
        offset = tag_end + 1;
        if local_name != "href" || tag.starts_with('/') {
            continue;
        }
        let Some(close_rel) = lower[offset..].find("</") else {
            break;
        };
        let raw = xml[offset..offset + close_rel].trim();
        if !raw.is_empty() {
            result.push(xml_unescape(raw));
        }
        offset += close_rel;
    }
    result
}

fn extract_propfind_entries(xml: &str) -> Vec<PropfindEntry> {
    let lower = xml.to_lowercase();
    let mut entries = Vec::new();
    let mut offset = 0_usize;

    while let Some(start_rel) = lower[offset..]
        .find("<d:response")
        .or_else(|| lower[offset..].find("<response"))
    {
        let response_start = offset + start_rel;
        let Some(close_rel) = lower[response_start..].find("</") else {
            break;
        };
        let response_end = response_start + close_rel;
        let chunk = &xml[response_start..response_end];
        let chunk_lower = chunk.to_lowercase();

        let href = extract_first_tag_text(chunk, "href");
        let is_collection =
            chunk_lower.contains("<collection") || chunk_lower.contains(":collection");

        if let Some(href) = href {
            entries.push(PropfindEntry {
                href,
                is_collection,
            });
        }

        offset = response_end + 2;
    }

    if entries.is_empty() {
        return extract_hrefs(xml)
            .into_iter()
            .map(|href| PropfindEntry {
                href,
                is_collection: false,
            })
            .collect();
    }

    entries
}

fn extract_first_tag_text(xml: &str, tag_name: &str) -> Option<String> {
    let lower = xml.to_lowercase();
    let mut offset = 0_usize;

    while let Some(relative_start) = lower[offset..].find('<') {
        let tag_start = offset + relative_start;
        let tag_end_rel = lower[tag_start..].find('>')?;
        let tag_end = tag_start + tag_end_rel;
        let tag = lower[tag_start + 1..tag_end].trim();
        let current = tag
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_start_matches('/');
        let local = current.rsplit(':').next().unwrap_or(current);
        offset = tag_end + 1;

        if local != tag_name || tag.starts_with('/') {
            continue;
        }

        let close_rel = lower[offset..].find("</")?;
        let raw = xml[offset..offset + close_rel].trim();
        if raw.is_empty() {
            return None;
        }
        return Some(xml_unescape(raw));
    }

    None
}

fn leaf_name(href: &str) -> Option<String> {
    let clean = href.trim_end_matches('/');
    let name = percent_decode(clean.rsplit('/').next()?.trim());
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' && idx + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[idx + 1]), hex_value(bytes[idx + 2])) {
                output.push((hi << 4) | lo);
                idx += 3;
                continue;
            }
        }
        output.push(bytes[idx]);
        idx += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
