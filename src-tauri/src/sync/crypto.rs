use super::{
    SnapshotPackage, SyncKeyringState, UsageBatchPackage, WebDavClient, BATCH_SCHEMA_VERSION,
    KEYRING_FILE, KEY_LEN, NONCE_LEN, PBKDF2_ROUNDS, SNAPSHOT_SCHEMA_VERSION, SYNC_KEYRING_SCHEMA,
    WRAP_SALT_LEN,
};
pub(super) use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ring::{aead, pbkdf2, rand};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EncryptedPackage {
    pub(super) schema_version: u32,
    pub(super) algorithm: String,
    pub(super) kdf: String,
    pub(super) device_id: String,
    #[serde(default)]
    pub(super) dek_version: u32,
    pub(super) export_seq: i64,
    pub(super) nonce: String,
    pub(super) payload: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SyncKeyring {
    pub(super) schema_version: u32,
    pub(super) dek_version: u32,
    pub(super) algorithm: String,
    pub(super) kdf: String,
    pub(super) wrap_salt: String,
    pub(super) nonce: String,
    pub(super) wrapped_dek: String,
    pub(super) updated_at: i64,
}

pub(super) async fn load_or_create_keyring(
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

pub(super) async fn store_keyring(
    client: &WebDavClient,
    keyring: &SyncKeyring,
) -> Result<(), String> {
    client.ensure_meta_dir().await?;
    let bytes = serde_json::to_vec(keyring)
        .map_err(|e| format!("Failed to serialize sync keyring: {}", e))?;
    client.put(KEYRING_FILE, bytes).await
}

fn make_random_wrap_salt() -> Result<Vec<u8>, String> {
    let rng = rand::SystemRandom::new();
    let mut salt = vec![0_u8; WRAP_SALT_LEN];
    rand::SecureRandom::fill(&rng, &mut salt)
        .map_err(|_| "ERR_SYNC_WRAP_SALT_GENERATION_FAILED".to_string())?;
    Ok(salt)
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

pub(super) fn rewrap_keyring_dek(
    keyring: SyncKeyring,
    current_sync_password: &str,
    new_sync_password: &str,
) -> Result<SyncKeyring, String> {
    let dek = unwrap_dek(&keyring, current_sync_password)?;
    wrap_new_keyring(new_sync_password, dek, keyring.dek_version + 1)
}

pub(super) fn unwrap_dek(
    keyring: &SyncKeyring,
    sync_password: &str,
) -> Result<[u8; KEY_LEN], String> {
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

pub(super) async fn ensure_dek(
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

pub(super) fn encrypt_batch_package(
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

pub(super) fn encrypt_snapshot_package(
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

pub(super) fn decrypt_typed_package<T: DeserializeOwned>(
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

pub(super) fn make_random_id() -> Result<String, String> {
    let rng = rand::SystemRandom::new();
    let mut bytes = [0_u8; 12];
    rand::SecureRandom::fill(&rng, &mut bytes)
        .map_err(|_| "ERR_SYNC_RANDOM_ID_GENERATION_FAILED".to_string())?;
    Ok(bytes.iter().map(|byte| format!("{:02x}", byte)).collect())
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

pub(super) fn make_nonce() -> Result<Vec<u8>, String> {
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

pub(super) fn encrypt_bytes(
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
