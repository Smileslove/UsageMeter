use crate::local_usage::{
    ensure_local_usage_synced, SyncExportData, SyncExportRequest, SyncExportSession,
    SyncOutboxBatch,
};
use crate::models::SyncSettings;
use crate::net::HttpClientFactory;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use reqwest::header::RETRY_AFTER;
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode, Url};
use ring::{aead, pbkdf2, rand};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
const WRAP_SALT_LEN: usize = 32;
const SYNC_KEYRING_SCHEMA: u32 = 1;
const NONCE_LEN: usize = 12;
const PBKDF2_ROUNDS: u32 = 120_000;
const BATCH_SCHEMA_VERSION: u32 = 2;
const SNAPSHOT_SCHEMA_VERSION: u32 = 2;
const SNAPSHOT_INTERVAL_BATCHES: i64 = 100;
const BATCH_RETENTION_AFTER_SNAPSHOT: i64 = 20;
const AUTO_SYNC_MIN_INTERVAL_SECONDS: u64 = 60;
const MAX_BATCHES_PER_SYNC: usize = 20;
const HTTP_MAX_ATTEMPTS: u32 = 3;
const HTTP_RETRY_BASE_DELAY_MS: u64 = 1_000;
const HTTP_RETRY_AFTER_CAP_SECONDS: u64 = 30;

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
    pub local_request_count: u64,
    pub total_request_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotateSyncPasswordPayload {
    pub current_sync_password: String,
    pub new_sync_password: String,
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

/// 所有参与字段级合并的设置字段名，顺序与 field_value() 一致
const SHARED_SETTING_FIELDS: &[&str] = &[
    "locale",
    "timezone",
    "summary_window",
    "theme",
    "model_pricing",
    "source_aware",
    "currency",
];

impl SharedSettingsPayload {
    /// 将指定字段序列化为可比较的字符串，用于判断是否发生变化
    fn field_value(&self, field: &str) -> String {
        match field {
            "locale" => self.locale.clone(),
            "timezone" => self.timezone.clone(),
            "summary_window" => self.summary_window.clone(),
            "theme" => self.theme.clone(),
            "model_pricing" => serde_json::to_string(&self.model_pricing).unwrap_or_default(),
            "source_aware" => serde_json::to_string(&self.source_aware).unwrap_or_default(),
            "currency" => serde_json::to_string(&self.currency).unwrap_or_default(),
            _ => String::new(),
        }
    }
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
    /// 各字段最后修改时间（unix timestamp），向后兼容：老文档缺失时视为 0
    #[serde(default)]
    field_timestamps: HashMap<String, i64>,
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
    let _ = load_or_create_keyring(&client, &credentials.sync_password).await?;
    assert_device_id_ownership(
        &client,
        &device_id,
        &instance_id,
        previous_device_id.as_deref(),
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

pub fn spawn_background_sync_loop() {
    tauri::async_runtime::spawn(async move {
        let mut startup_attempted = false;
        let mut prev_auto_sync_active = false;
        loop {
            let iteration_start = tokio::time::Instant::now();
            let settings = match crate::commands::load_settings() {
                Ok(app_settings) => app_settings.sync,
                Err(err) => {
                    eprintln!("[UsageMeter] Failed to load settings for background sync: {err}");
                    tokio::time::sleep(Duration::from_secs(AUTO_SYNC_MIN_INTERVAL_SECONDS)).await;
                    continue;
                }
            };

            let auto_sync_active = settings.enabled && settings.auto_sync;

            // 检测 auto_sync 从关闭到开启的跳变，重置启动标志以触发即时同步
            if auto_sync_active && !prev_auto_sync_active {
                startup_attempted = false;
            }
            prev_auto_sync_active = auto_sync_active;

            if auto_sync_active && !startup_attempted {
                let _ = run_auto_sync_once(settings.clone(), false).await;
                startup_attempted = true;
            }

            let sleep_seconds = if auto_sync_active {
                (settings.interval_minutes.max(1) * 60).max(AUTO_SYNC_MIN_INTERVAL_SECONDS)
            } else {
                AUTO_SYNC_MIN_INTERVAL_SECONDS
            };

            // 扣除本次迭代（含启动同步）已消耗的时间，避免间隔漂移
            let elapsed = iteration_start.elapsed();
            let adjusted_sleep = Duration::from_secs(sleep_seconds).saturating_sub(elapsed);
            tokio::time::sleep(adjusted_sleep).await;

            if auto_sync_active {
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

    // 初始 export_seq：取本地已上传、远端 manifest、legacy 三者最大值 +1
    let mut next_export_seq = local_last_uploaded_seq.max(legacy_last_export_seq).max(
        remote_manifest
            .as_ref()
            .map(|manifest| manifest.latest_batch_seq)
            .unwrap_or(0),
    ) + 1;

    let exported_at = chrono::Utc::now().timestamp();
    let mut effective_latest_batch_seq = local_last_uploaded_seq.max(legacy_last_export_seq);
    let mut total_exported_request_count: u64 = 0;

    // 循环上传，直到 outbox 清空或达到单次同步批次上限
    for _ in 0..MAX_BATCHES_PER_SYNC {
        let SyncOutboxBatch {
            request_events,
            session_events,
        } = db.reserve_sync_outbox_batch(&device_id, next_export_seq, 1000, 250)?;

        if request_events.is_empty() && session_events.is_empty() {
            break;
        }

        let batch_exported_at = chrono::Utc::now().timestamp();
        let batch_request_count = request_events.len();
        let batch_session_count = session_events.len();
        let package = UsageBatchPackage {
            schema_version: BATCH_SCHEMA_VERSION,
            package_type: "usage_batch".to_string(),
            device_id: device_id.clone(),
            instance_id: instance_id.clone(),
            batch_seq: next_export_seq,
            prev_batch_seq: next_export_seq - 1,
            exported_at: batch_exported_at,
            request_events,
            session_events,
        };
        let encrypted = encrypt_batch_package(&package, &dek, dek_version)?;
        let encrypted_bytes = serde_json::to_vec(&encrypted)
            .map_err(|e| format!("Failed to serialize encrypted sync batch: {}", e))?;
        let remote_path = client.batch_path(&device_id, next_export_seq);
        if let Err(err) = client.put(&remote_path, encrypted_bytes).await {
            let _ = db.release_sync_outbox_batch(next_export_seq);
            return Err(err);
        }
        if let Err(err) = db.mark_sync_outbox_batch_uploaded(
            next_export_seq,
            &remote_path,
            batch_request_count,
            batch_session_count,
        ) {
            // mark 失败：batch 文件已在服务端，但本地状态未更新；
            // 释放 batched_seq 锁，下次 sync 会重新上传（服务端做幂等 upsert）
            let _ = db.release_sync_outbox_batch(next_export_seq);
            return Err(err);
        }
        total_exported_request_count += batch_request_count as u64;
        effective_latest_batch_seq = next_export_seq;
        next_export_seq += 1;
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
        &total_exported_request_count.to_string(),
    )?;
    db.upsert_webdav_sync_state("last_imported_requests", &imported_requests.to_string())?;
    // 清理已上传的 outbox 行，防止表无限增长
    let _ = db.prune_uploaded_outbox();
    Ok(())
}

pub fn get_status(settings: &SyncSettings) -> Result<SyncStatus, String> {
    let db = ensure_local_usage_synced()?;
    let last_error = db
        .get_webdav_sync_state("last_error")?
        .filter(|value| !value.trim().is_empty());
    let local_request_count = db.count_local_request_facts()?;
    let remote_request_count = db.count_remote_request_facts()?;
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
        local_request_count,
        total_request_count: local_request_count + remote_request_count,
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
        let Some(manifest) = load_manifest(client, &device_id).await? else {
            continue;
        };
        let import_result =
            import_device_batches(client, &manifest, sync_password, keyring_state).await;
        match import_result {
            Ok(request_count) => imported_requests += request_count,
            Err(ref err) if err == "ERR_SYNC_BATCH_PRUNED_RETRY" => {
                // 批次已被远端清理，重置游标为 0，下次同步将从快照重建
                let _ = db.upsert_import_cursor(
                    &device_id,
                    Some(&manifest.instance_id),
                    0,
                    "retry",
                    Some(err),
                );
                eprintln!(
                    "[UsageMeter] WebDAV sync device {} batches pruned, cursor reset to 0 for snapshot recovery",
                    device_id
                );
            }
            Err(err) => {
                db.upsert_webdav_sync_state(&format!("failed:{}:manifest", device_id), &err)?;
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
    }

    Ok(imported_requests)
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
    // Fast-forward via snapshot whenever cursor lags behind it, not only on first import:
    // pruning removes batches older than (snapshot_seq - BATCH_RETENTION_AFTER_SNAPSHOT),
    // so a stale consumer's missing batches must be recovered through the snapshot.
    if cursor < manifest.latest_snapshot_seq {
        match import_device_snapshot(client, manifest, sync_password, keyring_state).await? {
            Some(snapshot) => {
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
            None => {
                if cursor == 0 {
                    return Ok(imported_requests);
                }
            }
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
        let bytes = match client.get_optional(&remote_path).await? {
            Some(bytes) => bytes,
            None => {
                if manifest.latest_snapshot_seq > cursor {
                    return Err("ERR_SYNC_BATCH_PRUNED_RETRY".to_string());
                }
                return Err(format!(
                    "ERR_SYNC_BATCH_MISSING: device {} batch {}",
                    manifest.device_id, batch_seq
                ));
            }
        };
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
    let snapshot: SnapshotPackage =
        decrypt_typed_package(&encrypted, sync_password, keyring_state)?;
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
) -> Result<(), String> {
    let Some(manifest) = load_manifest(client, device_id).await? else {
        return Ok(());
    };
    if manifest.device_id != device_id {
        return Err("ERR_SYNC_DEVICE_ID_CONFLICT".to_string());
    }
    if manifest.instance_id == instance_id {
        return Ok(());
    }
    if previous_device_id == Some(device_id) {
        return Ok(());
    }
    Err("ERR_SYNC_DEVICE_ID_CONFLICT".to_string())
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
        .get_optional(&format!(
            "{}/{}",
            client.device_path(device_id),
            MANIFEST_FILE
        ))
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
    let batch_files = client
        .list_files(&client.batch_dir_path(&manifest.device_id))
        .await?;
    for file in batch_files {
        let Some(seq) = parse_batch_seq(&file) else {
            continue;
        };
        if seq < keep_from {
            let _ = client
                .delete(&format!(
                    "{}/{}",
                    client.batch_dir_path(&manifest.device_id),
                    file
                ))
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
                .delete(&format!(
                    "{}/{}",
                    client.snapshot_dir_path(&manifest.device_id),
                    file
                ))
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
        .put(
            &format!("{}/{}", client.device_path(device_id), MANIFEST_FILE),
            bytes,
        )
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
    let now = chrono::Utc::now().timestamp();

    let local_version = db
        .get_webdav_sync_state("shared_settings_version")?
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);

    // 加载本地各字段的时间戳（首次为空，默认 0）
    let mut local_field_timestamps: HashMap<String, i64> = SHARED_SETTING_FIELDS
        .iter()
        .map(|&field| {
            let ts = db
                .get_webdav_sync_state(&format!("shared_field_ts:{}", field))
                .ok()
                .flatten()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0);
            (field.to_string(), ts)
        })
        .collect();

    let remote_document = load_shared_settings_document(client, credentials).await?;

    // --- 拉取阶段：逐字段比较时间戳，取较新的值 ---
    if let Some(document) = remote_document.as_ref() {
        if document.version > local_version {
            let mut changed = false;
            // 判断是否是升级前的老文档（没有 field_timestamps）
            let is_legacy_doc = document.field_timestamps.is_empty();
            for &field in SHARED_SETTING_FIELDS {
                let local_ts = local_field_timestamps.get(field).copied().unwrap_or(0);
                let remote_ts = if is_legacy_doc {
                    // 老文档没有字段级时间戳：
                    // - 本地未曾修改该字段（local_ts == 0）→ 使用文档整体时间戳，正常拉取
                    // - 本地已有修改（local_ts > 0）→ 置为 0，不覆盖本地更改
                    if local_ts == 0 {
                        document.updated_at
                    } else {
                        0
                    }
                } else {
                    // 新格式文档：取字段时间戳，缺失视为 0（该字段未曾被修改过）
                    document.field_timestamps.get(field).copied().unwrap_or(0)
                };
                if remote_ts > local_ts {
                    apply_shared_settings_field(&mut app_settings, field, &document.payload);
                    local_field_timestamps.insert(field.to_string(), remote_ts);
                    changed = true;
                }
            }
            if changed {
                crate::commands::save_settings_internal(app_settings.clone())
                    .map_err(String::from)?;
                // 持久化本地字段时间戳
                for (field, ts) in &local_field_timestamps {
                    db.upsert_webdav_sync_state(
                        &format!("shared_field_ts:{}", field),
                        &ts.to_string(),
                    )?;
                }
            }
            db.upsert_webdav_sync_state("shared_settings_version", &document.version.to_string())?;
        }
    }

    // --- 推送阶段：检测本地相对于远端的变化，更新对应字段时间戳 ---
    let current_version = db
        .get_webdav_sync_state("shared_settings_version")?
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    let local_payload = extract_shared_settings_payload(&app_settings);

    // 对比远端 payload，为本地有主动改动的字段打上当前时间戳。
    // 拉取阶段已把远端更新的字段同步到 app_settings，
    // 若此时某字段仍与远端不同，说明本地值更新，需要标记为 now 以便推送。
    match remote_document.as_ref() {
        Some(document) => {
            for &field in SHARED_SETTING_FIELDS {
                if local_payload.field_value(field) != document.payload.field_value(field) {
                    // 本地值与远端不同（且经过拉取阶段仍未被覆盖）：标记为本地最新
                    local_field_timestamps.insert(field.to_string(), now);
                }
            }
        }
        None => {
            // 远端没有文档：所有字段都视为本地最新
            for &field in SHARED_SETTING_FIELDS {
                local_field_timestamps.insert(field.to_string(), now);
            }
        }
    }

    // 判断是否需要推送
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
        let version =
            current_version.max(remote_document.as_ref().map(|d| d.version).unwrap_or(0)) + 1;
        let document = SharedSettingsDocument {
            schema_version: 1,
            document_type: "shared_settings".to_string(),
            updated_at: now,
            updated_by_device_id: device_id.to_string(),
            version,
            payload: local_payload,
            field_timestamps: local_field_timestamps.clone(),
        };
        store_shared_settings_document(client, &document, &dek, keyring.dek_version).await?;
        // 持久化本地字段时间戳
        for (field, ts) in &local_field_timestamps {
            db.upsert_webdav_sync_state(&format!("shared_field_ts:{}", field), &ts.to_string())?;
        }
        db.upsert_webdav_sync_state("shared_settings_version", &version.to_string())?;
    }

    Ok(())
}

/// 将远端文档中单个字段的值应用到本地设置
fn apply_shared_settings_field(
    settings: &mut crate::models::AppSettings,
    field: &str,
    payload: &SharedSettingsPayload,
) {
    match field {
        "locale" => settings.locale = payload.locale.clone(),
        "timezone" => settings.timezone = payload.timezone.clone(),
        "summary_window" => settings.summary_window = payload.summary_window.clone(),
        "theme" => settings.theme = payload.theme.clone(),
        "model_pricing" => settings.model_pricing = payload.model_pricing.clone(),
        "source_aware" => settings.source_aware = payload.source_aware.clone(),
        "currency" => settings.currency = payload.currency.clone(),
        _ => {}
    }
}

fn extract_shared_settings_payload(settings: &crate::models::AppSettings) -> SharedSettingsPayload {
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

fn make_random_wrap_salt() -> Result<Vec<u8>, String> {
    let rng = rand::SystemRandom::new();
    let mut salt = vec![0_u8; WRAP_SALT_LEN];
    rand::SecureRandom::fill(&rng, &mut salt)
        .map_err(|_| "ERR_SYNC_WRAP_SALT_GENERATION_FAILED".to_string())?;
    Ok(salt)
}

async fn store_keyring(client: &WebDavClient, keyring: &SyncKeyring) -> Result<(), String> {
    client.ensure_meta_dir().await?;
    let bytes = serde_json::to_vec(keyring)
        .map_err(|e| format!("Failed to serialize sync keyring: {}", e))?;
    client.put(KEYRING_FILE, bytes).await
}

fn wrap_new_keyring(
    sync_password: &str,
    dek: [u8; KEY_LEN],
    dek_version: u32,
) -> Result<SyncKeyring, String> {
    let salt_bytes = make_random_wrap_salt()?;
    let nonce = make_nonce()?;
    let wrapping_key = derive_key(sync_password, &salt_bytes);
    let wrapped_dek = encrypt_bytes(&dek, &wrapping_key, &salt_bytes, &nonce)?;
    Ok(SyncKeyring {
        schema_version: SYNC_KEYRING_SCHEMA,
        dek_version,
        algorithm: "chacha20-poly1305".to_string(),
        kdf: format!("pbkdf2-hmac-sha256:{}", PBKDF2_ROUNDS),
        wrap_salt: BASE64.encode(&salt_bytes),
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
    wrap_new_keyring(new_sync_password, dek, keyring.dek_version + 1)
}

fn unwrap_dek(keyring: &SyncKeyring, sync_password: &str) -> Result<[u8; KEY_LEN], String> {
    if keyring.schema_version != SYNC_KEYRING_SCHEMA {
        return Err("ERR_SYNC_KEYRING_SCHEMA_UNSUPPORTED".to_string());
    }
    let nonce = BASE64
        .decode(&keyring.nonce)
        .map_err(|e| format!("Failed to decode keyring nonce: {}", e))?;
    let cipher = BASE64
        .decode(&keyring.wrapped_dek)
        .map_err(|e| format!("Failed to decode wrapped DEK: {}", e))?;
    let salt_bytes = BASE64
        .decode(&keyring.wrap_salt)
        .map_err(|e| format!("Failed to decode keyring wrap salt: {}", e))?;
    let wrapping_key = derive_key(sync_password, &salt_bytes);
    let plaintext = decrypt_bytes(cipher, &wrapping_key, &salt_bytes, &nonce)?;
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
            let keyring = wrap_new_keyring(sync_password, dek, 1)?;
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

fn derive_key(password: &str, salt: &[u8]) -> [u8; KEY_LEN] {
    let mut key = [0_u8; KEY_LEN];
    let rounds = NonZeroU32::new(PBKDF2_ROUNDS).expect("PBKDF2_ROUNDS must be non-zero");
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        rounds,
        salt,
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
        let client = HttpClientFactory::global().webdav();
        Ok(Self {
            client,
            base_url: format!("{}/{}", root, root_dir),
            root_dir,
            device_dir,
            username: settings.username,
            password,
        })
    }

    async fn check_access(&self) -> Result<(), String> {
        let builder = self
            .client
            .request(Method::OPTIONS, self.root_url())
            .basic_auth(&self.username, Some(&self.password));
        let response = self.send_with_retry(builder).await?;
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
        let builder = self
            .client
            .request(method, self.url(path))
            .basic_auth(&self.username, Some(&self.password));
        let response = self.send_with_retry(builder).await?;
        if response.status().is_success()
            || response.status() == StatusCode::METHOD_NOT_ALLOWED
            || response.status() == StatusCode::CONFLICT
        {
            return Ok(());
        }
        Err(format!("ERR_WEBDAV_MKCOL_FAILED: {}", response.status()))
    }

    async fn put(&self, path: &str, bytes: Vec<u8>) -> Result<(), String> {
        let builder = self
            .client
            .put(self.url(path))
            .basic_auth(&self.username, Some(&self.password))
            .body(bytes);
        let response = self.send_with_retry(builder).await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("ERR_WEBDAV_PUT_FAILED: {}", response.status()))
        }
    }

    async fn delete(&self, path: &str) -> Result<(), String> {
        let builder = self
            .client
            .delete(self.url(path))
            .basic_auth(&self.username, Some(&self.password));
        let response = self.send_with_retry(builder).await?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(format!("ERR_WEBDAV_DELETE_FAILED: {}", response.status()))
        }
    }

    #[allow(dead_code)]
    async fn get(&self, path: &str) -> Result<Vec<u8>, String> {
        let builder = self
            .client
            .get(self.url(path))
            .basic_auth(&self.username, Some(&self.password));
        let response = self.send_with_retry(builder).await?;
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
        let builder = self
            .client
            .get(self.url(path))
            .basic_auth(&self.username, Some(&self.password));
        let response = self.send_with_retry(builder).await?;
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
        let builder = self
            .client
            .request(method, self.url(path))
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "1")
            .body(
                r#"<?xml version="1.0" encoding="utf-8"?><propfind xmlns="DAV:"><prop><resourcetype/></prop></propfind>"#,
            );
        let response = self.send_with_retry(builder).await?;
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
        format!(
            "{}/{:012}.json.enc",
            self.batch_dir_path(device_id),
            batch_seq
        )
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
        let builder = self
            .client
            .request(method, url.to_string())
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "0")
            .body(
                r#"<?xml version="1.0" encoding="utf-8"?><propfind xmlns="DAV:"><prop><resourcetype/></prop></propfind>"#,
            );
        let response = self.send_with_retry(builder).await?;
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

    async fn send_with_retry(&self, builder: RequestBuilder) -> Result<Response, String> {
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            let request = builder
                .try_clone()
                .ok_or_else(|| "ERR_WEBDAV_REQUEST_BUILD_FAILED".to_string())?;
            match request.send().await {
                Ok(response) => {
                    let status = response.status();
                    if should_retry_status(status) && attempt < HTTP_MAX_ATTEMPTS {
                        let delay = retry_delay_for_response(&response, attempt);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Ok(response);
                }
                Err(err) => {
                    if is_transient_send_error(&err) && attempt < HTTP_MAX_ATTEMPTS {
                        let delay = retry_delay_for_attempt(attempt);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(format!("ERR_WEBDAV_REQUEST_FAILED: {}", err));
                }
            }
        }
    }
}

struct WebDavEntry {
    name: String,
    is_collection: bool,
}

fn should_retry_status(status: StatusCode) -> bool {
    if status.is_server_error() {
        return true;
    }
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS
    )
}

fn is_transient_send_error(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request()
}

fn retry_delay_for_attempt(attempt: u32) -> Duration {
    let exp = attempt.saturating_sub(1).min(4);
    let base_ms = HTTP_RETRY_BASE_DELAY_MS.saturating_mul(4u64.pow(exp));
    let jittered = apply_jitter(base_ms);
    Duration::from_millis(jittered)
}

fn retry_delay_for_response(response: &Response, attempt: u32) -> Duration {
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        if let Some(value) = response.headers().get(RETRY_AFTER) {
            if let Ok(text) = value.to_str() {
                if let Ok(secs) = text.trim().parse::<u64>() {
                    let capped = secs.min(HTTP_RETRY_AFTER_CAP_SECONDS);
                    return Duration::from_secs(capped.max(1));
                }
            }
        }
    }
    retry_delay_for_attempt(attempt)
}

fn apply_jitter(base_ms: u64) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let mut hasher = DefaultHasher::new();
    nanos.hash(&mut hasher);
    let entropy = hasher.finish();
    let spread = (base_ms / 5).max(1);
    let offset = entropy % (spread * 2 + 1);
    base_ms.saturating_add(offset).saturating_sub(spread)
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
