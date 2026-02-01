//! Protocol eras -- versioned parameter sets for the Cordelia network.
//!
//! An era defines all protocol-level timing and security parameters that peers
//! must agree on. Pool sizes (hot/warm/cold targets) are node-local decisions
//! and NOT part of the era.
//!
//! Currently hardcoded as ERA_0. When the Hard Fork Combinator (HFC) is
//! implemented, era transitions will be signaled via supermajority and
//! negotiated during handshake.

/// A protocol era: a named, versioned set of timing and security parameters.
///
/// All peers on the network must operate under the same era. Era transitions
/// are coordinated via the (future) HFC mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolEra {
    /// Era identifier. Monotonically increasing.
    pub id: u16,

    // -- Keepalive --
    /// Seconds between keepalive pings.
    pub keepalive_interval_secs: u64,
    /// Missed pings before declaring a peer dead.
    pub keepalive_miss_limit: u32,

    // -- Governor timing --
    /// Governor tick interval in seconds.
    pub tick_interval_secs: u64,
    /// Seconds of inactivity before demoting an active peer.
    pub dead_timeout_secs: u64,
    /// Seconds before demoting a hot peer with no items delivered.
    pub stale_timeout_secs: u64,
    /// Base ban duration in seconds (escalated on repeat offences).
    pub ban_base_duration_secs: u64,

    // -- Reconnect backoff --
    /// Base backoff in seconds: min(2^count * base, max).
    pub reconnect_backoff_base_secs: u64,
    /// Maximum backoff in seconds.
    pub reconnect_backoff_max_secs: u64,

    // -- Churn --
    /// Seconds between churn cycles (warm peer rotation).
    pub churn_interval_secs: u64,
    /// Fraction of warm peers to rotate per churn cycle (0.0 - 1.0).
    /// Stored as per-mille (parts per thousand) to avoid f64 in const.
    pub churn_per_mille: u32,

    // -- Peer sharing --
    /// Seconds between peer sharing rounds.
    pub peer_share_interval_secs: u64,

    // -- Transport --
    /// QUIC idle timeout in seconds (must be > keepalive interval).
    pub quic_idle_timeout_secs: u64,
    /// Maximum message size in bytes (wire limit per QUIC stream message).
    pub max_message_bytes: usize,
    /// Maximum encrypted blob size per individual memory item.
    pub max_item_bytes: usize,
    /// Maximum batch size for memory fetch.
    pub max_batch_size: u32,
    /// Pong response timeout in seconds.
    pub pong_timeout_secs: u64,

    // -- Replication --
    /// Eager-push anti-entropy interval in seconds (chatty groups).
    pub eager_push_interval_secs: u64,
    /// Anti-entropy sync interval for moderate groups (seconds).
    pub sync_interval_moderate_secs: u64,
    /// Anti-entropy sync interval for taciturn groups (seconds).
    pub sync_interval_taciturn_secs: u64,
    /// Days to retain tombstones before garbage collection.
    pub tombstone_retention_days: u32,
    /// Push retry backoff schedule in seconds (up to 4 steps).
    /// Items are re-pushed with increasing delay; no explicit ack.
    pub push_retry_backoffs: [u64; 4],
    /// Number of push retries to attempt (indexes into push_retry_backoffs).
    pub push_retry_count: u32,

    // -- Governor scheduling (in ticks, not seconds) --
    /// Group exchange interval in governor ticks.
    pub group_exchange_ticks: u64,
    /// Peer discovery interval in governor ticks.
    pub peer_discovery_ticks: u64,
    /// Bootnode retry interval in governor ticks.
    pub bootnode_retry_ticks: u64,
    /// Reconnect backoff exponent saturation: min(2^count, 2^cap).
    pub backoff_saturation_count: u32,
}

impl ProtocolEra {
    /// Churn fraction as f64 (convenience for governor).
    pub const fn churn_fraction(&self) -> f64 {
        self.churn_per_mille as f64 / 1000.0
    }
}

/// Era 0: Genesis parameters.
///
/// Conservative timing suitable for a small initial network (3-20 nodes).
/// Stale timeout is generous (6 hours) because low-activity groups may have
/// long quiet periods where healthy peers deliver nothing.
pub const ERA_0: ProtocolEra = ProtocolEra {
    id: 0,

    // Keepalive
    keepalive_interval_secs: 15,
    keepalive_miss_limit: 3,

    // Governor timing
    tick_interval_secs: 10,
    dead_timeout_secs: 90,
    stale_timeout_secs: 6 * 3600, // 6 hours (was 30 min pre-era)
    ban_base_duration_secs: 3600,

    // Reconnect backoff: min(2^count * 30s, 15min)
    reconnect_backoff_base_secs: 30,
    reconnect_backoff_max_secs: 15 * 60,

    // Churn: rotate 20% of warm peers every hour
    churn_interval_secs: 3600,
    churn_per_mille: 200, // 0.2

    // Peer sharing
    peer_share_interval_secs: 300,

    // Transport
    quic_idle_timeout_secs: 300,
    max_message_bytes: 512 * 1024, // 512 KB -- backpressure on batch fetches
    max_item_bytes: 16 * 1024,     // 16 KB -- force high-density memories
    max_batch_size: 100,
    pong_timeout_secs: 10,

    // Replication
    eager_push_interval_secs: 60,
    sync_interval_moderate_secs: 300,
    sync_interval_taciturn_secs: 900,
    tombstone_retention_days: 7,
    push_retry_backoffs: [5, 15, 60, 300], // aggressive early, converges to anti-entropy
    push_retry_count: 4,

    // Governor scheduling
    group_exchange_ticks: 6,  // 60s at 10s tick
    peer_discovery_ticks: 3,  // 30s at 10s tick
    bootnode_retry_ticks: 30, // 5min at 10s tick
    backoff_saturation_count: 5,
};

/// The current active era. When HFC is implemented, this will be determined
/// by network consensus rather than hardcoded.
pub const CURRENT_ERA: &ProtocolEra = &ERA_0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_era_0_invariants() {
        let era = &ERA_0;
        assert_eq!(era.id, 0);
        // Keepalive interval must be less than idle timeout
        assert!(era.keepalive_interval_secs < era.quic_idle_timeout_secs);
        // Dead timeout should be a few keepalive intervals
        assert!(era.dead_timeout_secs > era.keepalive_interval_secs);
        // Stale timeout must be greater than dead timeout
        assert!(era.stale_timeout_secs > era.dead_timeout_secs);
        // Churn fraction must be in (0, 1)
        assert!(era.churn_per_mille > 0 && era.churn_per_mille < 1000);
    }

    #[test]
    fn test_churn_fraction() {
        assert!((ERA_0.churn_fraction() - 0.2).abs() < f64::EPSILON);
    }

    #[test]
    fn test_stale_timeout_is_6_hours() {
        assert_eq!(ERA_0.stale_timeout_secs, 21600);
    }

    #[test]
    fn test_message_fits_multiple_items() {
        // At least several max-size items must fit in one message.
        // The batch size (100) is a soft cap; the message size is the hard cap.
        // With large items, the fetch handler returns fewer per response.
        let worst_case_wire = ERA_0.max_item_bytes * 4 / 3 + 200; // base64 + JSON envelope
        let items_per_message = ERA_0.max_message_bytes / worst_case_wire;
        assert!(
            items_per_message >= 10,
            "must fit at least 10 max-size items per message, got {items_per_message}"
        );
    }

    #[test]
    fn test_max_item_bytes_is_16kb() {
        assert_eq!(ERA_0.max_item_bytes, 16 * 1024);
    }

    #[test]
    fn test_max_message_bytes_is_512kb() {
        assert_eq!(ERA_0.max_message_bytes, 512 * 1024);
    }

    #[test]
    fn test_push_retry_backoffs() {
        let era = &ERA_0;
        assert_eq!(era.push_retry_count, 4);
        assert_eq!(era.push_retry_backoffs, [5, 15, 60, 300]);
        // Final retry should align with anti-entropy interval
        let last = era.push_retry_backoffs[(era.push_retry_count - 1) as usize];
        assert_eq!(last, era.sync_interval_moderate_secs);
    }
}
