//! Ed25519 node identity -- keypair generation, loading, self-signed X.509 for QUIC TLS.

use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use std::path::Path;

use crate::{node_id_from_pubkey, CryptoError};

/// Node identity wrapping an Ed25519 keypair.
pub struct NodeIdentity {
    keypair: Ed25519KeyPair,
    node_id: [u8; 32],
    pkcs8_doc: Vec<u8>,
}

impl NodeIdentity {
    /// Generate a new random keypair.
    pub fn generate() -> Result<Self, CryptoError> {
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng)
            .map_err(|e| CryptoError::IdentityError(e.to_string()))?;
        let pkcs8_bytes = pkcs8.as_ref().to_vec();
        let keypair = Ed25519KeyPair::from_pkcs8(&pkcs8_bytes)
            .map_err(|e| CryptoError::IdentityError(e.to_string()))?;
        let node_id = node_id_from_pubkey(keypair.public_key().as_ref());

        Ok(Self {
            keypair,
            node_id,
            pkcs8_doc: pkcs8_bytes,
        })
    }

    /// Load keypair from PKCS#8 DER file.
    pub fn from_file(path: &Path) -> Result<Self, CryptoError> {
        let pkcs8_bytes = std::fs::read(path)?;
        let keypair = Ed25519KeyPair::from_pkcs8(&pkcs8_bytes)
            .map_err(|e| CryptoError::IdentityError(e.to_string()))?;
        let node_id = node_id_from_pubkey(keypair.public_key().as_ref());

        Ok(Self {
            keypair,
            node_id,
            pkcs8_doc: pkcs8_bytes,
        })
    }

    /// Load or create keypair at path.
    pub fn load_or_create(path: &Path) -> Result<Self, CryptoError> {
        if path.exists() {
            Self::from_file(path)
        } else {
            let identity = Self::generate()?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, &identity.pkcs8_doc)?;
            Ok(identity)
        }
    }

    /// Node ID (SHA-256 of Ed25519 public key).
    pub fn node_id(&self) -> &[u8; 32] {
        &self.node_id
    }

    /// Node ID as hex string.
    pub fn node_id_hex(&self) -> String {
        hex::encode(self.node_id)
    }

    /// Raw public key bytes.
    pub fn public_key(&self) -> &[u8] {
        self.keypair.public_key().as_ref()
    }

    /// Sign data.
    pub fn sign(&self, data: &[u8]) -> Vec<u8> {
        self.keypair.sign(data).as_ref().to_vec()
    }

    /// PKCS#8 DER bytes (for TLS certificate generation).
    pub fn pkcs8_der(&self) -> &[u8] {
        &self.pkcs8_doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_identity() {
        let id = NodeIdentity::generate().unwrap();
        assert_eq!(id.node_id().len(), 32);
        assert!(!id.public_key().is_empty());
    }

    #[test]
    fn test_deterministic_node_id() {
        let id = NodeIdentity::generate().unwrap();
        let recomputed = node_id_from_pubkey(id.public_key());
        assert_eq!(id.node_id(), &recomputed);
    }

    #[test]
    fn test_load_or_create() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.key");

        let id1 = NodeIdentity::load_or_create(&path).unwrap();
        let id2 = NodeIdentity::load_or_create(&path).unwrap();

        assert_eq!(id1.node_id(), id2.node_id());
        assert_eq!(id1.public_key(), id2.public_key());
    }

    #[test]
    fn test_sign() {
        let id = NodeIdentity::generate().unwrap();
        let sig = id.sign(b"test message");
        assert!(!sig.is_empty());
    }
}
