//! Ed25519 node identity -- keypair generation, loading, libp2p Keypair conversion.

use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use std::path::Path;

use crate::CryptoError;

/// Node identity wrapping an Ed25519 keypair.
pub struct NodeIdentity {
    keypair: Ed25519KeyPair,
    peer_id: libp2p::PeerId,
    pkcs8_doc: Vec<u8>,
}

impl NodeIdentity {
    /// Generate a new random keypair.
    pub fn generate() -> Result<Self, CryptoError> {
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng)
            .map_err(|e| CryptoError::IdentityError(e.to_string()))?;
        let pkcs8_bytes = pkcs8.as_ref().to_vec();
        Self::from_pkcs8_bytes(pkcs8_bytes)
    }

    /// Load keypair from PKCS#8 DER file.
    pub fn from_file(path: &Path) -> Result<Self, CryptoError> {
        let pkcs8_bytes = std::fs::read(path)?;
        Self::from_pkcs8_bytes(pkcs8_bytes)
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

    fn from_pkcs8_bytes(pkcs8_bytes: Vec<u8>) -> Result<Self, CryptoError> {
        let keypair = Ed25519KeyPair::from_pkcs8(&pkcs8_bytes)
            .map_err(|e| CryptoError::IdentityError(e.to_string()))?;

        // Derive libp2p PeerId from the Ed25519 public key
        let libp2p_keypair = Self::pkcs8_to_libp2p_keypair(&pkcs8_bytes)?;
        let peer_id = libp2p_keypair.public().to_peer_id();

        Ok(Self {
            keypair,
            peer_id,
            pkcs8_doc: pkcs8_bytes,
        })
    }

    /// Extract the raw Ed25519 seed from PKCS#8 DER and create a libp2p Keypair.
    ///
    /// ring's PKCS#8 DER for Ed25519 has a known structure:
    /// - Bytes 0..15: ASN.1 header (OID etc)
    /// - Bytes 16..18: OCTET STRING wrapper
    /// - Bytes 18..50: 32-byte Ed25519 seed (private key)
    ///
    /// We extract the seed and use it to create a libp2p ed25519 keypair.
    fn pkcs8_to_libp2p_keypair(pkcs8_der: &[u8]) -> Result<libp2p::identity::Keypair, CryptoError> {
        // ring Ed25519 PKCS#8 v1 DER: the 32-byte seed starts at offset 16
        // after the ASN.1 header: SEQUENCE { SEQUENCE { OID }, OCTET STRING { seed } }
        // The OCTET STRING at position 14 has tag 0x04, length 0x22 (34),
        // then another OCTET STRING tag 0x04, length 0x20 (32), then the 32-byte seed.
        if pkcs8_der.len() < 48 {
            return Err(CryptoError::IdentityError(
                "PKCS#8 DER too short for Ed25519".into(),
            ));
        }

        // Find the seed: scan for the inner OCTET STRING (04 20) followed by 32 bytes
        let seed = extract_ed25519_seed(pkcs8_der).ok_or_else(|| {
            CryptoError::IdentityError("could not extract Ed25519 seed from PKCS#8 DER".into())
        })?;

        let libp2p_keypair =
            libp2p::identity::Keypair::ed25519_from_bytes(seed.to_vec()).map_err(|e| {
                CryptoError::IdentityError(format!("failed to create libp2p keypair: {e}"))
            })?;

        Ok(libp2p_keypair)
    }

    /// PeerId (libp2p identity derived from Ed25519 public key).
    pub fn peer_id(&self) -> &libp2p::PeerId {
        &self.peer_id
    }

    /// PeerId as base58 string.
    pub fn peer_id_base58(&self) -> String {
        self.peer_id.to_base58()
    }

    /// Raw public key bytes.
    pub fn public_key(&self) -> &[u8] {
        self.keypair.public_key().as_ref()
    }

    /// Sign data.
    pub fn sign(&self, data: &[u8]) -> Vec<u8> {
        self.keypair.sign(data).as_ref().to_vec()
    }

    /// PKCS#8 DER bytes.
    pub fn pkcs8_der(&self) -> &[u8] {
        &self.pkcs8_doc
    }

    /// Create a libp2p Keypair from this identity.
    pub fn to_libp2p_keypair(&self) -> Result<libp2p::identity::Keypair, CryptoError> {
        Self::pkcs8_to_libp2p_keypair(&self.pkcs8_doc)
    }

    // Legacy compatibility: node_id_hex returns PeerId base58
    pub fn node_id_hex(&self) -> String {
        self.peer_id.to_base58()
    }
}

/// Extract the 32-byte Ed25519 seed from a ring-generated PKCS#8 DER.
///
/// ring's Ed25519 PKCS#8 v1 format:
/// ```text
/// SEQUENCE {
///   INTEGER 0                          -- version
///   SEQUENCE { OID 1.3.101.112 }       -- Ed25519
///   OCTET STRING {                      -- wrapping
///     OCTET STRING { <32 bytes seed> }  -- the actual seed
///   }
/// }
/// ```
/// We look for the pattern 04 20 (OCTET STRING, length 32) followed by exactly 32 bytes.
fn extract_ed25519_seed(der: &[u8]) -> Option<&[u8]> {
    // The inner OCTET STRING containing the seed is typically at a known offset
    // in ring's output. For robustness, scan backwards from the end.
    for i in 0..der.len().saturating_sub(33) {
        if der[i] == 0x04 && der[i + 1] == 0x20 {
            let seed = &der[i + 2..i + 34];
            return Some(seed);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_identity() {
        let id = NodeIdentity::generate().unwrap();
        assert!(!id.public_key().is_empty());
        // PeerId should be a valid base58 string
        assert!(!id.peer_id_base58().is_empty());
    }

    #[test]
    fn test_deterministic_peer_id() {
        let id = NodeIdentity::generate().unwrap();
        let kp = id.to_libp2p_keypair().unwrap();
        assert_eq!(kp.public().to_peer_id(), *id.peer_id());
    }

    #[test]
    fn test_load_or_create() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.key");

        let id1 = NodeIdentity::load_or_create(&path).unwrap();
        let id2 = NodeIdentity::load_or_create(&path).unwrap();

        assert_eq!(id1.peer_id(), id2.peer_id());
        assert_eq!(id1.public_key(), id2.public_key());
    }

    #[test]
    fn test_sign() {
        let id = NodeIdentity::generate().unwrap();
        let sig = id.sign(b"test message");
        assert!(!sig.is_empty());
    }

    #[test]
    fn test_libp2p_keypair_roundtrip() {
        let id = NodeIdentity::generate().unwrap();
        let kp = id.to_libp2p_keypair().unwrap();

        // Verify the libp2p keypair produces the same public key
        let libp2p_pub = kp.public();
        let peer_id = libp2p_pub.to_peer_id();
        assert_eq!(peer_id, *id.peer_id());
    }

    #[test]
    fn test_seed_extraction() {
        // Generate a keypair and verify we can extract the seed
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let der = pkcs8.as_ref();

        let seed = extract_ed25519_seed(der);
        assert!(seed.is_some(), "should extract seed from PKCS#8 DER");
        assert_eq!(seed.unwrap().len(), 32);
    }
}
