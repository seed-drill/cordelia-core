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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupExchange {
    pub groups: Vec<GroupId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupExchangeResponse {
    pub groups: Vec<GroupId>,
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
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: GroupExchange = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.groups, vec!["g1", "g2"]);
    }
}
