//! Cordelia Protocol -- wire types, message types, protocol eras.
//!
//! libp2p request-response JSON behaviours between peers.
//! Each mini-protocol is a separate request-response behaviour.

pub mod era;
pub mod messages;

pub use era::{ProtocolEra, CURRENT_ERA, ERA_0};
pub use messages::*;

/// Protocol magic number: 0xC0DE11A1
pub const PROTOCOL_MAGIC: u32 = 0xC0DE_11A1;

/// Minimum supported protocol version.
pub const VERSION_MIN: u16 = 1;

/// Maximum supported protocol version.
pub const VERSION_MAX: u16 = 1;

/// Keep-alive interval in seconds (sourced from current era).
pub const KEEPALIVE_INTERVAL_SECS: u64 = ERA_0.keepalive_interval_secs;

/// QUIC idle timeout in seconds (sourced from current era).
pub const QUIC_IDLE_TIMEOUT_SECS: u64 = ERA_0.quic_idle_timeout_secs;

/// Missed pings before declaring peer dead (sourced from current era).
pub const KEEPALIVE_MISS_LIMIT: u32 = ERA_0.keepalive_miss_limit;

/// Peer sharing interval in seconds (sourced from current era).
pub const PEER_SHARE_INTERVAL_SECS: u64 = ERA_0.peer_share_interval_secs;

/// Maximum batch size for memory fetch (sourced from current era).
pub const MAX_BATCH_SIZE: u32 = ERA_0.max_batch_size;

/// Maximum message size in bytes (sourced from current era).
pub const MAX_MESSAGE_BYTES: usize = ERA_0.max_message_bytes;

/// Maximum encrypted blob size per memory item (sourced from current era).
pub const MAX_ITEM_BYTES: usize = ERA_0.max_item_bytes;

/// Pong response timeout in seconds (sourced from current era).
pub const PONG_TIMEOUT_SECS: u64 = ERA_0.pong_timeout_secs;

/// Eager-push anti-entropy interval in seconds (sourced from current era).
pub const EAGER_PUSH_INTERVAL_SECS: u64 = ERA_0.eager_push_interval_secs;

/// Anti-entropy sync interval for taciturn groups (sourced from current era).
pub const SYNC_INTERVAL_TACITURN_SECS: u64 = ERA_0.sync_interval_taciturn_secs;

/// Tombstone retention in days (sourced from current era).
pub const TOMBSTONE_RETENTION_DAYS: u32 = ERA_0.tombstone_retention_days;

/// Group exchange interval in governor ticks (sourced from current era).
pub const GROUP_EXCHANGE_TICKS: u64 = ERA_0.group_exchange_ticks;

/// Peer discovery interval in governor ticks (sourced from current era).
pub const PEER_DISCOVERY_TICKS: u64 = ERA_0.peer_discovery_ticks;

/// Bootnode retry interval in governor ticks (sourced from current era).
pub const BOOTNODE_RETRY_TICKS: u64 = ERA_0.bootnode_retry_ticks;

/// Reconnect backoff saturation count (sourced from current era).
pub const BACKOFF_SATURATION_COUNT: u32 = ERA_0.backoff_saturation_count;

/// Group identifier (opaque string).
pub type GroupId = String;

/// Node identifier -- libp2p PeerId (multihash of Ed25519 public key).
pub type NodeId = libp2p::PeerId;

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("invalid magic: expected {expected:#010x}, got {got:#010x}")]
    InvalidMagic { expected: u32, got: u32 },
    #[error("version mismatch: peer offers {min}-{max}, we support {our_min}-{our_max}")]
    VersionMismatch {
        min: u16,
        max: u16,
        our_min: u16,
        our_max: u16,
    },
    #[error("message too large: {size} bytes (max {max})")]
    MessageTooLarge { size: usize, max: usize },
    #[error("codec error: {0}")]
    Codec(String),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
