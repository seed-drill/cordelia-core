//! Cordelia Protocol -- wire types, message codec, mini-protocols.
//!
//! QUIC between peers. One bidirectional stream per mini-protocol.
//! 4-byte big-endian length prefix + serde JSON.

pub mod codec;
pub mod era;
pub mod messages;
pub mod tls;

pub use codec::MessageCodec;
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

/// Group identifier (opaque string).
pub type GroupId = String;

/// Node identifier (SHA-256 of Ed25519 pubkey).
pub type NodeId = [u8; 32];

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
