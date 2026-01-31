//! Cordelia Crypto -- AES-256-GCM encryption, scrypt key derivation, Ed25519 identity.
//!
//! "What I cannot create, I do not understand." -- Richard Feynman
//!
//! Round-trip compatible with the TypeScript implementation:
//! - scrypt: N=16384, r=8, p=1, 32-byte key
//! - AES-256-GCM: 12-byte IV, 16-byte auth tag
//! - EncryptedPayload JSON format matches TS exactly

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub mod identity;

// Re-exports
pub use identity::NodeIdentity;

/// scrypt parameters matching TypeScript: N=16384, r=8, p=1
const SCRYPT_LOG_N: u8 = 14; // 2^14 = 16384
const SCRYPT_R: u32 = 8;
const SCRYPT_P: u32 = 1;
const KEY_LENGTH: usize = 32;
const IV_LENGTH: usize = 12;
const SALT_LENGTH: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("encryption failed: {0}")]
    EncryptionFailed(String),
    #[error("decryption failed: authentication tag mismatch")]
    DecryptionFailed,
    #[error("unsupported encryption version: {0}")]
    UnsupportedVersion(u32),
    #[error("key derivation failed: {0}")]
    KeyDerivationFailed(String),
    #[error("base64 decode error: {0}")]
    Base64Error(#[from] base64::DecodeError),
    #[error("identity error: {0}")]
    IdentityError(String),
    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Encrypted payload format -- matches TypeScript EncryptedPayload exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedPayload {
    pub _encrypted: bool,
    pub version: u32,
    pub iv: String,
    #[serde(rename = "authTag")]
    pub auth_tag: String,
    pub ciphertext: String,
}

impl EncryptedPayload {
    /// Check if a JSON value is an encrypted payload.
    pub fn is_encrypted(value: &serde_json::Value) -> bool {
        value.get("_encrypted") == Some(&serde_json::Value::Bool(true))
            && value.get("version") == Some(&serde_json::Value::Number(1.into()))
            && value.get("iv").and_then(|v| v.as_str()).is_some()
            && value.get("authTag").and_then(|v| v.as_str()).is_some()
            && value.get("ciphertext").and_then(|v| v.as_str()).is_some()
    }
}

/// Derive a 256-bit key from passphrase + salt using scrypt.
/// Parameters match TypeScript: N=16384, r=8, p=1.
pub fn derive_key(passphrase: &[u8], salt: &[u8]) -> Result<[u8; KEY_LENGTH], CryptoError> {
    let params = scrypt::Params::new(SCRYPT_LOG_N, SCRYPT_R, SCRYPT_P, KEY_LENGTH)
        .map_err(|e| CryptoError::KeyDerivationFailed(e.to_string()))?;

    let mut key = [0u8; KEY_LENGTH];
    scrypt::scrypt(passphrase, salt, &params, &mut key)
        .map_err(|e| CryptoError::KeyDerivationFailed(e.to_string()))?;

    Ok(key)
}

/// Generate a random salt (32 bytes).
pub fn generate_salt() -> [u8; SALT_LENGTH] {
    let rng = SystemRandom::new();
    let mut salt = [0u8; SALT_LENGTH];
    rng.fill(&mut salt).expect("system RNG failure");
    salt
}

/// AES-256-GCM encryption provider.
pub struct Aes256GcmProvider {
    key: Option<LessSafeKey>,
    rng: SystemRandom,
}

impl Aes256GcmProvider {
    pub fn new() -> Self {
        Self {
            key: None,
            rng: SystemRandom::new(),
        }
    }

    /// Derive key from passphrase + salt and unlock.
    pub fn unlock(&mut self, passphrase: &[u8], salt: &[u8]) -> Result<(), CryptoError> {
        let key_bytes = derive_key(passphrase, salt)?;
        let unbound = UnboundKey::new(&AES_256_GCM, &key_bytes)
            .map_err(|_| CryptoError::EncryptionFailed("invalid key".into()))?;
        self.key = Some(LessSafeKey::new(unbound));
        Ok(())
    }

    /// Unlock with raw key bytes (for testing / direct key use).
    pub fn unlock_with_key(&mut self, key_bytes: &[u8; KEY_LENGTH]) -> Result<(), CryptoError> {
        let unbound = UnboundKey::new(&AES_256_GCM, key_bytes)
            .map_err(|_| CryptoError::EncryptionFailed("invalid key".into()))?;
        self.key = Some(LessSafeKey::new(unbound));
        Ok(())
    }

    pub fn is_unlocked(&self) -> bool {
        self.key.is_some()
    }

    /// Encrypt plaintext, producing an EncryptedPayload compatible with TypeScript.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<EncryptedPayload, CryptoError> {
        let key = self
            .key
            .as_ref()
            .ok_or_else(|| CryptoError::EncryptionFailed("not unlocked".into()))?;

        let mut iv_bytes = [0u8; IV_LENGTH];
        self.rng
            .fill(&mut iv_bytes)
            .map_err(|_| CryptoError::EncryptionFailed("RNG failure".into()))?;

        let nonce =
            Nonce::try_assume_unique_for_key(&iv_bytes).map_err(|_| CryptoError::EncryptionFailed("nonce error".into()))?;

        // ring appends the auth tag to the ciphertext
        let mut in_out = plaintext.to_vec();
        key.seal_in_place_append_tag(nonce, Aad::empty(), &mut in_out)
            .map_err(|_| CryptoError::EncryptionFailed("seal failed".into()))?;

        // Split: ciphertext is everything except last 16 bytes (auth tag)
        let tag_start = in_out.len() - AES_256_GCM.tag_len();
        let ciphertext = &in_out[..tag_start];
        let auth_tag = &in_out[tag_start..];

        Ok(EncryptedPayload {
            _encrypted: true,
            version: 1,
            iv: BASE64.encode(iv_bytes),
            auth_tag: BASE64.encode(auth_tag),
            ciphertext: BASE64.encode(ciphertext),
        })
    }

    /// Decrypt an EncryptedPayload produced by either Rust or TypeScript.
    pub fn decrypt(&self, payload: &EncryptedPayload) -> Result<Vec<u8>, CryptoError> {
        if payload.version != 1 {
            return Err(CryptoError::UnsupportedVersion(payload.version));
        }

        let key = self
            .key
            .as_ref()
            .ok_or_else(|| CryptoError::EncryptionFailed("not unlocked".into()))?;

        let iv_bytes = BASE64.decode(&payload.iv)?;
        let auth_tag = BASE64.decode(&payload.auth_tag)?;
        let ciphertext = BASE64.decode(&payload.ciphertext)?;

        let nonce = Nonce::try_assume_unique_for_key(&iv_bytes)
            .map_err(|_| CryptoError::DecryptionFailed)?;

        // ring expects ciphertext + auth_tag concatenated
        let mut in_out = Vec::with_capacity(ciphertext.len() + auth_tag.len());
        in_out.extend_from_slice(&ciphertext);
        in_out.extend_from_slice(&auth_tag);

        let plaintext = key
            .open_in_place(nonce, Aad::empty(), &mut in_out)
            .map_err(|_| CryptoError::DecryptionFailed)?;

        Ok(plaintext.to_vec())
    }
}

impl Default for Aes256GcmProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// SHA-256 hash of data, returned as hex string.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Compute node ID from Ed25519 public key (SHA-256 of pubkey bytes).
pub fn node_id_from_pubkey(pubkey: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(pubkey);
    let result = hasher.finalize();
    let mut id = [0u8; 32];
    id.copy_from_slice(&result);
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_round_trip() {
        let mut provider = Aes256GcmProvider::new();
        let salt = generate_salt();
        provider.unlock(b"test-passphrase", &salt).unwrap();

        let plaintext = b"hello cordelia";
        let encrypted = provider.encrypt(plaintext).unwrap();

        assert!(encrypted._encrypted);
        assert_eq!(encrypted.version, 1);

        let decrypted = provider.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_unique_iv_per_encryption() {
        let mut provider = Aes256GcmProvider::new();
        let salt = generate_salt();
        provider.unlock(b"test", &salt).unwrap();

        let e1 = provider.encrypt(b"same data").unwrap();
        let e2 = provider.encrypt(b"same data").unwrap();

        assert_ne!(e1.iv, e2.iv);
        assert_ne!(e1.ciphertext, e2.ciphertext);
    }

    #[test]
    fn test_wrong_key_fails() {
        let salt = generate_salt();

        let mut enc = Aes256GcmProvider::new();
        enc.unlock(b"key-one", &salt).unwrap();
        let payload = enc.encrypt(b"secret").unwrap();

        let mut dec = Aes256GcmProvider::new();
        dec.unlock(b"key-two", &salt).unwrap();
        assert!(dec.decrypt(&payload).is_err());
    }

    #[test]
    fn test_tampered_ciphertext_fails() {
        let mut provider = Aes256GcmProvider::new();
        let salt = generate_salt();
        provider.unlock(b"test", &salt).unwrap();

        let mut payload = provider.encrypt(b"data").unwrap();
        // Tamper with ciphertext
        let mut ct = BASE64.decode(&payload.ciphertext).unwrap();
        if let Some(b) = ct.first_mut() {
            *b ^= 0xff;
        }
        payload.ciphertext = BASE64.encode(&ct);

        assert!(provider.decrypt(&payload).is_err());
    }

    #[test]
    fn test_json_data_round_trip() {
        let mut provider = Aes256GcmProvider::new();
        let salt = generate_salt();
        provider.unlock(b"test", &salt).unwrap();

        let data = serde_json::json!({
            "name": "Russell",
            "version": 1,
            "tags": ["founder", "engineer"]
        });
        let plaintext = serde_json::to_vec(&data).unwrap();
        let encrypted = provider.encrypt(&plaintext).unwrap();
        let decrypted = provider.decrypt(&encrypted).unwrap();
        let recovered: serde_json::Value = serde_json::from_slice(&decrypted).unwrap();
        assert_eq!(data, recovered);
    }

    #[test]
    fn test_derive_key_deterministic() {
        let salt = [0x42u8; 32];
        let k1 = derive_key(b"same-pass", &salt).unwrap();
        let k2 = derive_key(b"same-pass", &salt).unwrap();
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_encrypted_payload_detection() {
        let val = serde_json::json!({
            "_encrypted": true,
            "version": 1,
            "iv": "AAAA",
            "authTag": "BBBB",
            "ciphertext": "CCCC"
        });
        assert!(EncryptedPayload::is_encrypted(&val));

        let plain = serde_json::json!({"name": "test"});
        assert!(!EncryptedPayload::is_encrypted(&plain));
    }

    #[test]
    fn test_sha256_hex() {
        let hash = sha256_hex(b"hello");
        assert_eq!(hash.len(), 64);
        // Known SHA-256 of "hello"
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
