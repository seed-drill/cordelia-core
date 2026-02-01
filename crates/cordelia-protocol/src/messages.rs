//! Wire message types for all mini-protocols.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

use crate::GroupId;

// ============================================================================
// Envelope -- wraps all messages for stream demuxing
// ============================================================================

/// Top-level message envelope sent on QUIC streams.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Message {
    // Handshake (stream 0)
    HandshakePropose(HandshakePropose),
    HandshakeAccept(HandshakeAccept),

    // Keep-alive
    Ping(Ping),
    Pong(Pong),

    // Peer sharing
    PeerShareRequest(PeerShareRequest),
    PeerShareResponse(PeerShareResponse),

    // Memory sync
    SyncRequest(SyncRequest),
    SyncResponse(SyncResponse),

    // Memory fetch
    FetchRequest(FetchRequest),
    FetchResponse(FetchResponse),

    // Memory push ack
    PushAck(PushAck),

    // Group exchange
    GroupExchange(GroupExchange),
    GroupExchangeResponse(GroupExchangeResponse),
}

// ============================================================================
// Handshake
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakePropose {
    pub magic: u32,
    pub version_min: u16,
    pub version_max: u16,
    #[serde(with = "hex_bytes_32")]
    pub node_id: [u8; 32],
    pub timestamp: u64,
    pub groups: Vec<GroupId>,
    /// Protocol era this node is operating under. Old nodes omit this (defaults to 0).
    #[serde(default)]
    pub era: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeAccept {
    pub version: u16, // 0 = rejected
    #[serde(with = "hex_bytes_32")]
    pub node_id: [u8; 32],
    pub timestamp: u64,
    pub groups: Vec<GroupId>,
    pub reject_reason: Option<String>,
    /// The remote address we observe for the proposing peer (NAT hairpin avoidance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_addr: Option<SocketAddr>,
    /// Protocol era this node is operating under. Old nodes omit this (defaults to 0).
    #[serde(default)]
    pub era: u16,
}

// ============================================================================
// Keep-Alive
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ping {
    pub seq: u64,
    pub sent_at_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pong {
    pub seq: u64,
    pub sent_at_ns: u64,
    pub recv_at_ns: u64,
    /// The remote address we observe for the pinging peer (NAT hairpin avoidance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_addr: Option<SocketAddr>,
}

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
    #[serde(with = "hex_bytes_32")]
    pub node_id: [u8; 32],
    pub addrs: Vec<SocketAddr>,
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
// Memory Push Ack
// ============================================================================

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

/// Serialize/deserialize [u8; 32] as hex string.
mod hex_bytes_32 {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        let mut arr = [0u8; 32];
        if bytes.len() != 32 {
            return Err(serde::de::Error::custom(format!(
                "expected 32 bytes, got {}",
                bytes.len()
            )));
        }
        arr.copy_from_slice(&bytes);
        Ok(arr)
    }
}

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
    fn test_handshake_roundtrip() {
        let msg = Message::HandshakePropose(HandshakePropose {
            magic: crate::PROTOCOL_MAGIC,
            version_min: 1,
            version_max: 1,
            node_id: [0xAA; 32],
            timestamp: 1234567890,
            groups: vec!["seed-drill".into()],
            era: crate::ERA_0.id,
        });

        let json = serde_json::to_string(&msg).unwrap();
        let decoded: Message = serde_json::from_str(&json).unwrap();

        match decoded {
            Message::HandshakePropose(h) => {
                assert_eq!(h.magic, crate::PROTOCOL_MAGIC);
                assert_eq!(h.node_id, [0xAA; 32]);
            }
            _ => panic!("wrong variant"),
        }
    }

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
    fn test_all_message_variants_serialize() {
        let messages = vec![
            Message::Ping(Ping {
                seq: 1,
                sent_at_ns: 123,
            }),
            Message::Pong(Pong {
                seq: 1,
                sent_at_ns: 123,
                recv_at_ns: 456,
                observed_addr: None,
            }),
            Message::PeerShareRequest(PeerShareRequest { max_peers: 10 }),
            Message::SyncRequest(SyncRequest {
                group_id: "g1".into(),
                since: None,
                limit: 100,
            }),
            Message::FetchRequest(FetchRequest {
                item_ids: vec!["a".into(), "b".into()],
            }),
        ];

        for msg in &messages {
            let json = serde_json::to_string(msg).unwrap();
            let _: Message = serde_json::from_str(&json).unwrap();
        }
    }
}
