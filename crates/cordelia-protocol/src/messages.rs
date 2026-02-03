//! Wire message types for all mini-protocols.
//!
//! Each protocol uses a separate request/response pair via libp2p request-response.
//! No more Message wrapper enum -- each behaviour has its own types.

use serde::{Deserialize, Serialize};

use crate::GroupId;

// ============================================================================
// Peer Sharing
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerShareRequest {
    pub max_peers: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerShareResponse {
    pub peers: Vec<PeerAddress>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerAddress {
    /// PeerId as base58 string (libp2p standard encoding).
    pub peer_id: String,
    /// Multiaddr strings.
    pub addrs: Vec<String>,
    pub last_seen: u64,
    pub groups: Vec<GroupId>,
    /// Node role: "relay", "personal", or "keeper". Empty = unknown (treat as personal).
    #[serde(default)]
    pub role: String,
}

// ============================================================================
// Memory Sync
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRequest {
    pub group_id: GroupId,
    pub since: Option<String>, // ISO8601
    pub limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResponse {
    pub items: Vec<ItemHeader>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemHeader {
    pub item_id: String,
    pub item_type: String,
    pub checksum: String,
    pub updated_at: String,
    pub author_id: String,
    pub is_deletion: bool,
}

// ============================================================================
// Memory Fetch
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchRequest {
    pub item_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResponse {
    pub items: Vec<FetchedItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedItem {
    pub item_id: String,
    pub item_type: String,
    /// Opaque encrypted blob -- NEVER decrypted by P2P layer.
    #[serde(with = "base64_bytes")]
    pub encrypted_blob: Vec<u8>,
    pub checksum: String,
    pub author_id: String,
    pub group_id: GroupId,
    pub key_version: u32,
    pub parent_id: Option<String>,
    pub is_copy: bool,
    pub updated_at: String,
}

// ============================================================================
// Memory Push (pusher = request initiator, receiver sends ack)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryPushRequest {
    pub items: Vec<FetchedItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushAck {
    pub stored: u32,
    pub rejected: u32,
}

// ============================================================================
// Group Exchange
// ============================================================================

/// Lightweight group metadata for protocol-level propagation.
/// Carries culture (replication policy) but NOT membership or name.
/// Name is display metadata -- distributed out-of-band by the portal.
/// Group IDs should be UUIDs (opaque). See R4-030 design doc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupDescriptor {
    pub id: String,
    /// Culture JSON (replication policy). Max 4KB.
    pub culture: String,
    /// ISO 8601 timestamp of last update.
    pub updated_at: String,
    /// SHA-256 of canonical(id + culture) for integrity verification.
    pub checksum: String,
    /// Entity ID of the group owner (signs the descriptor).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    /// Hex-encoded Ed25519 public key of the owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_pubkey: Option<String>,
    /// Hex-encoded Ed25519 signature over canonical(id + culture + updated_at).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl GroupDescriptor {
    /// Compute the canonical checksum for a group descriptor.
    pub fn compute_checksum(id: &str, culture: &str) -> String {
        use sha2::{Digest, Sha256};
        let canonical = format!("{}\n{}", id, culture);
        let hash = Sha256::digest(canonical.as_bytes());
        hex::encode(hash)
    }

    /// Verify this descriptor's checksum matches its contents.
    pub fn verify_checksum(&self) -> bool {
        self.checksum == Self::compute_checksum(&self.id, &self.culture)
    }

    /// Canonical payload for signing: id + culture + updated_at.
    pub fn signing_payload(&self) -> Vec<u8> {
        format!("{}\n{}\n{}", self.id, self.culture, self.updated_at)
            .into_bytes()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupExchange {
    pub groups: Vec<GroupId>,
    /// Optional group metadata descriptors (ERA_0 v1.1 extension).
    /// Old peers ignore this field; new peers populate it alongside `groups`.
    #[serde(default)]
    pub descriptors: Option<Vec<GroupDescriptor>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupExchangeResponse {
    pub groups: Vec<GroupId>,
    /// Optional group metadata descriptors (ERA_0 v1.1 extension).
    #[serde(default)]
    pub descriptors: Option<Vec<GroupDescriptor>>,
}

// ============================================================================
// Serde helpers
// ============================================================================

/// Serialize/deserialize Vec<u8> as base64 string.
mod base64_bytes {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        STANDARD.decode(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fetched_item_blob_base64() {
        let item = FetchedItem {
            item_id: "test".into(),
            item_type: "entity".into(),
            encrypted_blob: vec![1, 2, 3, 4],
            checksum: "abc".into(),
            author_id: "russell".into(),
            group_id: "seed-drill".into(),
            key_version: 1,
            parent_id: None,
            is_copy: false,
            updated_at: "2026-01-29T00:00:00Z".into(),
        };

        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("AQIDBA==")); // base64 of [1,2,3,4]

        let decoded: FetchedItem = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.encrypted_blob, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_peer_share_roundtrip() {
        let req = PeerShareRequest { max_peers: 10 };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: PeerShareRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.max_peers, 10);
    }

    #[test]
    fn test_sync_request_roundtrip() {
        let req = SyncRequest {
            group_id: "g1".into(),
            since: None,
            limit: 100,
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: SyncRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.group_id, "g1");
        assert_eq!(decoded.limit, 100);
    }

    #[test]
    fn test_memory_push_request_roundtrip() {
        let req = MemoryPushRequest {
            items: vec![FetchedItem {
                item_id: "x".into(),
                item_type: "entity".into(),
                encrypted_blob: vec![42],
                checksum: "abc".into(),
                author_id: "russell".into(),
                group_id: "g1".into(),
                key_version: 1,
                parent_id: None,
                is_copy: false,
                updated_at: "2026-01-29T00:00:00Z".into(),
            }],
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: MemoryPushRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.items.len(), 1);
        assert_eq!(decoded.items[0].item_id, "x");
    }

    #[test]
    fn test_group_exchange_roundtrip() {
        let req = GroupExchange {
            groups: vec!["g1".into(), "g2".into()],
            descriptors: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: GroupExchange = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.groups, vec!["g1", "g2"]);
        assert!(decoded.descriptors.is_none());
    }

    #[test]
    fn test_group_exchange_with_descriptors() {
        let checksum = GroupDescriptor::compute_checksum("g1", r#"{"broadcast_eagerness":"moderate"}"#);
        let req = GroupExchange {
            groups: vec!["g1".into()],
            descriptors: Some(vec![GroupDescriptor {
                id: "g1".into(),
                culture: r#"{"broadcast_eagerness":"moderate"}"#.into(),
                updated_at: "2026-02-03T00:00:00Z".into(),
                checksum: checksum.clone(),
                owner_id: None,
                owner_pubkey: None,
                signature: None,
            }]),
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: GroupExchange = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.groups, vec!["g1"]);
        let descs = decoded.descriptors.unwrap();
        assert_eq!(descs.len(), 1);
        assert!(descs[0].verify_checksum());
        // No name on wire, no signature fields when unsigned
        assert!(!json.contains("name"));
        assert!(!json.contains("owner_id"));
        assert!(!json.contains("signature"));
    }

    #[test]
    fn test_group_exchange_backward_compat() {
        // Old peers send just groups, no descriptors field
        let old_json = r#"{"groups":["g1","g2"]}"#;
        let decoded: GroupExchange = serde_json::from_str(old_json).unwrap();
        assert_eq!(decoded.groups, vec!["g1", "g2"]);
        assert!(decoded.descriptors.is_none());
    }

    #[test]
    fn test_group_descriptor_checksum() {
        let checksum = GroupDescriptor::compute_checksum("test", "{}");
        let desc = GroupDescriptor {
            id: "test".into(),
            culture: "{}".into(),
            updated_at: "2026-02-03T00:00:00Z".into(),
            checksum,
            owner_id: None,
            owner_pubkey: None,
            signature: None,
        };
        assert!(desc.verify_checksum());

        // Tampered culture should fail
        let bad = GroupDescriptor {
            culture: r#"{"broadcast_eagerness":"chatty"}"#.into(),
            ..desc
        };
        assert!(!bad.verify_checksum());
    }

    #[test]
    fn test_group_descriptor_signing_payload() {
        let desc = GroupDescriptor {
            id: "g1".into(),
            culture: "{}".into(),
            updated_at: "2026-02-03T00:00:00Z".into(),
            checksum: "ignored".into(),
            owner_id: None,
            owner_pubkey: None,
            signature: None,
        };
        let payload = desc.signing_payload();
        assert_eq!(payload, b"g1\n{}\n2026-02-03T00:00:00Z");
    }
}
