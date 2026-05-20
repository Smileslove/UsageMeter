use crate::local_usage::{ensure_local_usage_synced, SyncExportData};
use crate::models::SyncSettings;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use reqwest::{Client, Method, StatusCode, Url};
use ring::{aead, pbkdf2, rand};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;

const ROOT_DIR: &str = "UsageMeter";
const META_DIR: &str = "meta";
const KEYRING_FILE: &str = "meta/keyring.json";
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const PBKDF2_ROUNDS: u32 = 120_000;

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
    let client = WebDavClient::new(settings, credentials.password)?;
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
    if let Err(err) = sync_now_inner(settings.clone(), credentials).await {
        persist_failure(&err);
        return Err(err);
    }
    get_status(&settings)
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
    let client = WebDavClient::new(settings, credentials.password)?;
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
    let export_seq = db
        .get_webdav_sync_state("last_export_seq")?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0)
        + 1;
    db.upsert_webdav_sync_state("last_export_seq", &export_seq.to_string())?;

    let data = db.get_sync_export_data()?;
    let exported_request_count = data.requests.len() as u64;
    let exported_at = chrono::Utc::now().timestamp();
    let client = WebDavClient::new(settings, credentials.password)?;
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
    let dek = ensure_dek(&client, &mut keyring_state, &credentials.sync_password).await?;
    let dek_version = keyring_state
        .keyring()
        .map(|keyring| keyring.dek_version)
        .ok_or_else(|| "ERR_SYNC_KEYRING_MISSING".to_string())?;
    let package = SyncPackage {
        schema_version: 1,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        device_id: device_id.clone(),
        instance_id: Some(instance_id.clone()),
        export_seq,
        exported_at,
        data,
    };
    let encrypted = encrypt_package(&package, &dek, dek_version)?;
    let encrypted_bytes = serde_json::to_vec(&encrypted)
        .map_err(|e| format!("Failed to serialize encrypted sync package: {}", e))?;
    client
        .put(
            &format!("devices/{}/latest.json.enc", device_id),
            encrypted_bytes,
        )
        .await?;
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
        let Some(file) = select_latest_package(client, &device_id).await? else {
            continue;
        };
        let import_result =
            import_one_package(client, &device_id, &file, sync_password, keyring_state).await;
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

async fn import_one_package(
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

async fn assert_device_id_ownership(
    client: &WebDavClient,
    device_id: &str,
    instance_id: &str,
    previous_device_id: Option<&str>,
    sync_password: &str,
    keyring_state: &SyncKeyringState,
) -> Result<(), String> {
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

fn encrypt_package(
    package: &SyncPackage,
    dek: &[u8; KEY_LEN],
    dek_version: u32,
) -> Result<EncryptedPackage, String> {
    let plaintext = serde_json::to_vec(package)
        .map_err(|e| format!("Failed to serialize sync package: {}", e))?;
    let nonce = make_nonce()?;
    let payload = encrypt_bytes(&plaintext, dek, package.device_id.as_bytes(), &nonce)?;

    Ok(EncryptedPackage {
        schema_version: 1,
        algorithm: "chacha20-poly1305".to_string(),
        kdf: format!("pbkdf2-hmac-sha256:{}", PBKDF2_ROUNDS),
        device_id: package.device_id.clone(),
        dek_version,
        export_seq: package.export_seq,
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
