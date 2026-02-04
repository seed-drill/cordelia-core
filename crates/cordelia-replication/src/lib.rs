//! Cordelia Replication -- culture-aware replication engine.
//!
//! Dispatches based on group culture:
//!   - EagerPush (chatty): send full item to all hot group peers
//!   - Passive (taciturn): do nothing, peers discover on periodic sync
//!
//! "moderate" is accepted as a culture string for backward compatibility
//! but maps to EagerPush (chatty). See docs/design/replication-routing.md
//! Section 10 for rationale.
//!
//! Anti-entropy sync runs per-group at culture-determined intervals.

use cordelia_protocol::messages::{FetchedItem, ItemHeader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub mod engine;

pub use engine::ReplicationEngine;

/// Replication strategy derived from group culture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ReplicationStrategy {
    /// Chatty: send full item to all hot group peers immediately.
    EagerPush,
    /// Taciturn: do nothing, peers discover via periodic sync.
    Passive,
}

/// Anti-entropy sync intervals per strategy.
impl ReplicationStrategy {
    pub fn sync_interval_secs(&self, taciturn_secs: u64) -> Option<u64> {
        match self {
            // Chatty: real-time push + fast anti-entropy safety net
            ReplicationStrategy::EagerPush => Some(cordelia_protocol::EAGER_PUSH_INTERVAL_SECS),
            ReplicationStrategy::Passive => Some(taciturn_secs),
        }
    }
}

/// Configuration for the replication engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationConfig {
    pub sync_interval_taciturn_secs: u64,
    pub tombstone_retention_days: u32,
    pub max_batch_size: u32,
}

impl Default for ReplicationConfig {
    fn default() -> Self {
        Self {
            sync_interval_taciturn_secs: cordelia_protocol::SYNC_INTERVAL_TACITURN_SECS,
            tombstone_retention_days: cordelia_protocol::TOMBSTONE_RETENTION_DAYS,
            max_batch_size: cordelia_protocol::MAX_BATCH_SIZE,
        }
    }
}

/// Group culture configuration (parsed from groups.culture JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupCulture {
    #[serde(default = "default_eagerness")]
    pub broadcast_eagerness: String,
    pub ttl_default: Option<u64>,
    #[serde(default)]
    pub notification_policy: Option<String>,
    #[serde(default)]
    pub departure_policy: Option<String>,
}

fn default_eagerness() -> String {
    "chatty".into()
}

impl GroupCulture {
    /// Determine replication strategy from culture.
    ///
    /// "moderate" is accepted for backward compatibility but maps to
    /// EagerPush (chatty). See docs/design/replication-routing.md Section 10.
    pub fn strategy(&self) -> ReplicationStrategy {
        match self.broadcast_eagerness.as_str() {
            "chatty" | "moderate" => ReplicationStrategy::EagerPush,
            "taciturn" => ReplicationStrategy::Passive,
            _ => ReplicationStrategy::EagerPush, // safe default
        }
    }
}

impl Default for GroupCulture {
    fn default() -> Self {
        Self {
            broadcast_eagerness: "chatty".into(),
            ttl_default: None,
            notification_policy: None,
            departure_policy: None,
        }
    }
}

/// Outcome of processing a received item.
#[derive(Debug, PartialEq)]
pub enum ReceiveOutcome {
    /// Item stored (new or updated).
    Stored,
    /// Item already exists with same checksum -- skipped.
    Duplicate,
    /// Item rejected (not a member of the group, or private).
    Rejected(String),
}

/// Compute SHA-256 checksum of data.
pub fn checksum(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Validate that a received item's checksum matches its blob.
pub fn validate_checksum(item: &FetchedItem) -> bool {
    checksum(&item.encrypted_blob) == item.checksum
}

/// Compare local and remote headers to find items needing fetch.
pub fn diff_headers(local: &[ItemHeader], remote: &[ItemHeader]) -> Vec<String> {
    let local_map: std::collections::HashMap<&str, &str> = local
        .iter()
        .map(|h| (h.item_id.as_str(), h.checksum.as_str()))
        .collect();

    remote
        .iter()
        .filter(|r| {
            match local_map.get(r.item_id.as_str()) {
                None => true, // unknown item
                Some(local_checksum) => {
                    // Different checksum -- need to fetch (last-writer-wins by updated_at)
                    *local_checksum != r.checksum.as_str()
                }
            }
        })
        .map(|r| r.item_id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_culture_strategy() {
        let chatty = GroupCulture {
            broadcast_eagerness: "chatty".into(),
            ..Default::default()
        };
        assert_eq!(chatty.strategy(), ReplicationStrategy::EagerPush);

        // "moderate" maps to EagerPush (deprecated, backward compat)
        let moderate = GroupCulture {
            broadcast_eagerness: "moderate".into(),
            ..Default::default()
        };
        assert_eq!(moderate.strategy(), ReplicationStrategy::EagerPush);

        // Default culture is chatty
        let default = GroupCulture::default();
        assert_eq!(default.strategy(), ReplicationStrategy::EagerPush);

        let taciturn = GroupCulture {
            broadcast_eagerness: "taciturn".into(),
            ..Default::default()
        };
        assert_eq!(taciturn.strategy(), ReplicationStrategy::Passive);
    }

    #[test]
    fn test_validate_checksum() {
        let data = b"test blob";
        let item = FetchedItem {
            item_id: "test".into(),
            item_type: "entity".into(),
            encrypted_blob: data.to_vec(),
            checksum: checksum(data),
            author_id: "russell".into(),
            group_id: "seed-drill".into(),
            key_version: 1,
            parent_id: None,
            is_copy: false,
            updated_at: "2026-01-29T00:00:00Z".into(),
        };

        assert!(validate_checksum(&item));

        let bad = FetchedItem {
            checksum: "wrong".into(),
            ..item
        };
        assert!(!validate_checksum(&bad));
    }

    #[test]
    fn test_diff_headers() {
        let local = vec![
            ItemHeader {
                item_id: "a".into(),
                item_type: "entity".into(),
                checksum: "hash_a".into(),
                updated_at: "2026-01-01".into(),
                author_id: "r".into(),
                is_deletion: false,
            },
            ItemHeader {
                item_id: "b".into(),
                item_type: "entity".into(),
                checksum: "hash_b".into(),
                updated_at: "2026-01-01".into(),
                author_id: "r".into(),
                is_deletion: false,
            },
        ];

        let remote = vec![
            ItemHeader {
                item_id: "a".into(),
                item_type: "entity".into(),
                checksum: "hash_a".into(), // same
                updated_at: "2026-01-01".into(),
                author_id: "r".into(),
                is_deletion: false,
            },
            ItemHeader {
                item_id: "b".into(),
                item_type: "entity".into(),
                checksum: "hash_b_new".into(), // changed
                updated_at: "2026-01-02".into(),
                author_id: "r".into(),
                is_deletion: false,
            },
            ItemHeader {
                item_id: "c".into(),
                item_type: "learning".into(),
                checksum: "hash_c".into(), // new
                updated_at: "2026-01-01".into(),
                author_id: "m".into(),
                is_deletion: false,
            },
        ];

        let needed = diff_headers(&local, &remote);
        assert_eq!(needed, vec!["b", "c"]);
    }

    #[test]
    fn test_sync_intervals() {
        assert_eq!(
            ReplicationStrategy::EagerPush.sync_interval_secs(900),
            Some(60)
        );
        assert_eq!(
            ReplicationStrategy::Passive.sync_interval_secs(900),
            Some(900)
        );
    }
}
