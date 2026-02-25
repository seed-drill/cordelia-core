//! Replication engine -- coordinates culture dispatch and receive.

use cordelia_protocol::messages::FetchedItem;
use cordelia_protocol::GroupId;
use cordelia_storage::{L2ItemWrite, Storage};

use crate::{
    checksum, validate_checksum, GroupCulture, ReceiveOutcome, ReplicationConfig,
    ReplicationStrategy,
};

/// The replication engine -- coordinates outbound and inbound replication.
pub struct ReplicationEngine {
    config: ReplicationConfig,
    entity_id: String,
}

/// Outbound action to send to peers.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum OutboundAction {
    /// Send full item to all hot group peers (EagerPush).
    BroadcastItem {
        group_id: GroupId,
        item: FetchedItem,
    },
    /// No action (Passive).
    None,
}

impl ReplicationEngine {
    pub fn new(config: ReplicationConfig, entity_id: String) -> Self {
        Self { config, entity_id }
    }

    /// Determine outbound action when a local write occurs.
    #[allow(clippy::too_many_arguments)]
    pub fn on_local_write(
        &self,
        group_id: &str,
        culture: &GroupCulture,
        item_id: &str,
        item_type: &str,
        data: &[u8],
        key_version: u32,
        parent_id: Option<String>,
        is_copy: bool,
    ) -> OutboundAction {
        // Enforce item size limit before replication dispatch
        if data.len() > cordelia_protocol::MAX_ITEM_BYTES {
            tracing::warn!(
                item_id,
                size = data.len(),
                max = cordelia_protocol::MAX_ITEM_BYTES,
                "Conditions Not Met: item exceeds size limit, suppressing outbound replication"
            );
            return OutboundAction::None;
        }

        let strategy = culture.strategy();
        let cs = checksum(data);

        match strategy {
            ReplicationStrategy::EagerPush => OutboundAction::BroadcastItem {
                group_id: group_id.to_string(),
                item: FetchedItem {
                    item_id: item_id.to_string(),
                    item_type: item_type.to_string(),
                    encrypted_blob: data.to_vec(),
                    checksum: cs,
                    author_id: self.entity_id.clone(),
                    group_id: group_id.to_string(),
                    key_version,
                    parent_id,
                    is_copy,
                    updated_at: chrono::Utc::now().to_rfc3339(),
                },
            },
            ReplicationStrategy::Passive => OutboundAction::None,
        }
    }

    /// Process a received item from a remote peer.
    ///
    /// `relay_accepts`: if Some, this node is a relay and the predicate determines
    /// whether the relay accepts items for a given group_id. If None, normal
    /// group membership check applies (non-relay behaviour).
    pub fn on_receive(
        &self,
        storage: &dyn Storage,
        item: &FetchedItem,
        our_groups: &[String],
        relay_accepts: Option<&dyn Fn(&str) -> bool>,
    ) -> ReceiveOutcome {
        // 1. Validate item size (backpressure: reject oversized blobs at P2P boundary)
        if item.encrypted_blob.len() > cordelia_protocol::MAX_ITEM_BYTES {
            return ReceiveOutcome::Rejected(format!(
                "Not My Problem, Entirely Yours: item {} bytes exceeds {} byte limit -- condense your thoughts",
                item.encrypted_blob.len(),
                cordelia_protocol::MAX_ITEM_BYTES
            ));
        }

        // 2. Validate group membership (or relay acceptance)
        let accepted = if let Some(relay_check) = relay_accepts {
            // Relay: use posture-based acceptance predicate
            relay_check(&item.group_id)
        } else {
            // Non-relay: must be a member of the group
            our_groups.contains(&item.group_id)
        };

        if !accepted {
            return ReceiveOutcome::Rejected(format!(
                "Outside Context Problem: not a member of group '{}'",
                item.group_id
            ));
        }

        // 3. Handle tombstones: delete the local copy
        if item.item_type == "__tombstone__" {
            match storage.delete_l2_item(&item.item_id) {
                Ok(true) => {
                    tracing::info!(
                        item_id = item.item_id,
                        group_id = item.group_id,
                        "repl: tombstone applied -- item deleted"
                    );
                    return ReceiveOutcome::Stored; // Treated as a successful receive
                }
                Ok(false) => {
                    return ReceiveOutcome::Duplicate; // Already gone
                }
                Err(e) => {
                    return ReceiveOutcome::Rejected(format!("tombstone delete failed: {e}"));
                }
            }
        }

        // 4. Validate checksum
        if !validate_checksum(item) {
            return ReceiveOutcome::Rejected(
                "integrity violation: checksum mismatch -- item corrupted or tampered".into(),
            );
        }

        // 5. Dedup: check if we already have this exact item
        if let Ok(Some(existing)) = storage.read_l2_item(&item.item_id) {
            if existing.checksum.as_deref() == Some(&item.checksum) {
                return ReceiveOutcome::Duplicate;
            }

            // Conflict resolution: last-writer-wins by updated_at
            if existing.updated_at >= item.updated_at {
                return ReceiveOutcome::Duplicate; // our version is newer or equal
            }
        }

        // 6. Store the item (encrypted blob, no decryption)
        let write = L2ItemWrite {
            id: item.item_id.clone(),
            item_type: item.item_type.clone(),
            data: item.encrypted_blob.clone(),
            owner_id: None,
            visibility: "group".into(),
            group_id: Some(item.group_id.clone()),
            author_id: Some(item.author_id.clone()),
            key_version: item.key_version as i32,
            parent_id: item.parent_id.clone(),
            is_copy: item.is_copy,
        };

        if let Err(e) = storage.write_l2_item(&write) {
            return ReceiveOutcome::Rejected(format!("storage error: {e}"));
        }

        // 7. Log access
        let _ = storage.log_access(&cordelia_storage::AccessLogEntry {
            entity_id: item.author_id.clone(),
            action: "replicate_receive".into(),
            resource_type: "l2_item".into(),
            resource_id: Some(item.item_id.clone()),
            group_id: Some(item.group_id.clone()),
            detail: None,
        });

        ReceiveOutcome::Stored
    }

    /// Get the anti-entropy sync interval for a group culture.
    pub fn sync_interval(&self, culture: &GroupCulture) -> u64 {
        culture
            .strategy()
            .sync_interval_secs(self.config.sync_interval_taciturn_secs)
            .unwrap_or(cordelia_protocol::EAGER_PUSH_INTERVAL_SECS)
    }

    /// Max batch size for fetch requests.
    pub fn max_batch_size(&self) -> u32 {
        self.config.max_batch_size
    }

    /// Tombstone retention period for GC.
    pub fn tombstone_retention_days(&self) -> u32 {
        self.config.tombstone_retention_days
    }

    /// Access the replication config.
    pub fn config(&self) -> &ReplicationConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_engine() -> ReplicationEngine {
        ReplicationEngine::new(ReplicationConfig::default(), "russell".into())
    }

    #[test]
    fn test_on_local_write_eager() {
        let engine = default_engine();
        let culture = GroupCulture {
            broadcast_eagerness: "chatty".into(),
            ..Default::default()
        };

        let action = engine.on_local_write(
            "seed-drill",
            &culture,
            "item-1",
            "entity",
            b"blob",
            1,
            None,
            false,
        );

        match action {
            OutboundAction::BroadcastItem { group_id, item } => {
                assert_eq!(group_id, "seed-drill");
                assert_eq!(item.item_id, "item-1");
                assert_eq!(item.encrypted_blob, b"blob");
            }
            _ => panic!("expected BroadcastItem"),
        }
    }

    #[test]
    fn test_on_local_write_moderate_maps_to_chatty() {
        let engine = default_engine();
        // "moderate" is deprecated and maps to EagerPush (chatty)
        let culture = GroupCulture {
            broadcast_eagerness: "moderate".into(),
            ..Default::default()
        };

        let action = engine.on_local_write(
            "seed-drill",
            &culture,
            "item-1",
            "entity",
            b"blob",
            1,
            None,
            false,
        );

        match action {
            OutboundAction::BroadcastItem { group_id, item } => {
                assert_eq!(group_id, "seed-drill");
                assert_eq!(item.item_id, "item-1");
            }
            _ => panic!("expected BroadcastItem (moderate maps to chatty)"),
        }
    }

    #[test]
    fn test_on_local_write_oversized_suppressed() {
        let engine = default_engine();
        let culture = GroupCulture {
            broadcast_eagerness: "chatty".into(),
            ..Default::default()
        };

        let oversized = vec![0u8; cordelia_protocol::MAX_ITEM_BYTES + 1];
        let action = engine.on_local_write(
            "seed-drill",
            &culture,
            "big-item",
            "entity",
            &oversized,
            1,
            None,
            false,
        );

        assert!(
            matches!(action, OutboundAction::None),
            "oversized item should suppress outbound replication"
        );
    }

    #[test]
    fn test_on_local_write_passive() {
        let engine = default_engine();
        let culture = GroupCulture {
            broadcast_eagerness: "taciturn".into(),
            ..Default::default()
        };

        let action = engine.on_local_write(
            "seed-drill",
            &culture,
            "item-1",
            "entity",
            b"blob",
            1,
            None,
            false,
        );

        assert!(matches!(action, OutboundAction::None));
    }

    #[test]
    fn test_on_local_write_propagates_cow_fields() {
        let engine = default_engine();
        let culture = GroupCulture {
            broadcast_eagerness: "chatty".into(),
            ..Default::default()
        };

        let action = engine.on_local_write(
            "seed-drill",
            &culture,
            "copy-1",
            "entity",
            b"blob",
            1,
            Some("parent-abc".into()),
            true,
        );

        match action {
            OutboundAction::BroadcastItem { item, .. } => {
                assert_eq!(item.parent_id.as_deref(), Some("parent-abc"));
                assert!(item.is_copy);
            }
            _ => panic!("expected BroadcastItem"),
        }
    }

    #[test]
    fn test_on_receive_rejected_oversized() {
        let engine = default_engine();
        let dir = tempfile::tempdir().unwrap();
        let db = cordelia_storage::SqliteStorage::create_new(&dir.path().join("test.db")).unwrap();

        // Create a blob larger than max_item_bytes (16 KB)
        let oversized = vec![0u8; cordelia_protocol::MAX_ITEM_BYTES + 1];
        let item = FetchedItem {
            item_id: "big".into(),
            item_type: "entity".into(),
            encrypted_blob: oversized.clone(),
            checksum: checksum(&oversized),
            author_id: "attacker".into(),
            group_id: "seed-drill".into(),
            key_version: 1,
            parent_id: None,
            is_copy: false,
            updated_at: "2026-02-01T00:00:00Z".into(),
        };

        let result = engine.on_receive(&db, &item, &["seed-drill".into()], None);
        match result {
            ReceiveOutcome::Rejected(reason) => {
                assert!(
                    reason.contains("Not My Problem"),
                    "expected NMPEY rejection: {reason}"
                );
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn test_on_receive_max_size_item_accepted() {
        let engine = default_engine();
        let dir = tempfile::tempdir().unwrap();
        let db = cordelia_storage::SqliteStorage::create_new(&dir.path().join("test.db")).unwrap();

        // Exactly at limit should be accepted
        let data = vec![0xABu8; cordelia_protocol::MAX_ITEM_BYTES];
        let item = FetchedItem {
            item_id: "just-right".into(),
            item_type: "entity".into(),
            encrypted_blob: data.clone(),
            checksum: checksum(&data),
            author_id: "russell".into(),
            group_id: "seed-drill".into(),
            key_version: 1,
            parent_id: None,
            is_copy: false,
            updated_at: "2026-02-01T00:00:00Z".into(),
        };

        let result = engine.on_receive(&db, &item, &["seed-drill".into()], None);
        assert_eq!(result, ReceiveOutcome::Stored);
    }

    #[test]
    fn test_on_receive_rejected_not_member() {
        let engine = default_engine();
        let dir = tempfile::tempdir().unwrap();
        let db = cordelia_storage::SqliteStorage::create_new(&dir.path().join("test.db")).unwrap();

        let item = FetchedItem {
            item_id: "test".into(),
            item_type: "entity".into(),
            encrypted_blob: b"blob".to_vec(),
            checksum: checksum(b"blob"),
            author_id: "martin".into(),
            group_id: "unknown-group".into(),
            key_version: 1,
            parent_id: None,
            is_copy: false,
            updated_at: "2026-01-29T00:00:00Z".into(),
        };

        let result = engine.on_receive(&db, &item, &["seed-drill".into()], None);
        assert!(matches!(result, ReceiveOutcome::Rejected(_)));
    }

    #[test]
    fn test_on_receive_stored() {
        let engine = default_engine();
        let dir = tempfile::tempdir().unwrap();
        let db = cordelia_storage::SqliteStorage::create_new(&dir.path().join("test.db")).unwrap();

        let data = b"encrypted-blob";
        let item = FetchedItem {
            item_id: "test-1".into(),
            item_type: "entity".into(),
            encrypted_blob: data.to_vec(),
            checksum: checksum(data),
            author_id: "martin".into(),
            group_id: "seed-drill".into(),
            key_version: 1,
            parent_id: None,
            is_copy: false,
            updated_at: "2026-01-29T00:00:00Z".into(),
        };

        let result = engine.on_receive(&db, &item, &["seed-drill".into()], None);
        assert_eq!(result, ReceiveOutcome::Stored);

        // Second receive of same item should be duplicate
        let result2 = engine.on_receive(&db, &item, &["seed-drill".into()], None);
        assert_eq!(result2, ReceiveOutcome::Duplicate);
    }

    #[test]
    fn test_on_receive_relay_accepts_any_group() {
        let engine = default_engine();
        let dir = tempfile::tempdir().unwrap();
        let db = cordelia_storage::SqliteStorage::create_new(&dir.path().join("test.db")).unwrap();

        let data = b"relay-blob";
        let item = FetchedItem {
            item_id: "relay-1".into(),
            item_type: "entity".into(),
            encrypted_blob: data.to_vec(),
            checksum: checksum(data),
            author_id: "alpha-agent".into(),
            group_id: "alpha-internal".into(),
            key_version: 1,
            parent_id: None,
            is_copy: false,
            updated_at: "2026-02-01T00:00:00Z".into(),
        };

        // Non-relay: rejected (not a member)
        let result = engine.on_receive(&db, &item, &[], None);
        assert!(matches!(result, ReceiveOutcome::Rejected(_)));

        // Transparent relay: accepts anything
        let transparent = |_: &str| true;
        let result = engine.on_receive(&db, &item, &[], Some(&transparent));
        assert_eq!(result, ReceiveOutcome::Stored);
    }

    #[test]
    fn test_on_receive_relay_rejects_unknown_group() {
        let engine = default_engine();
        let dir = tempfile::tempdir().unwrap();
        let db = cordelia_storage::SqliteStorage::create_new(&dir.path().join("test.db")).unwrap();

        let data = b"edge-blob";
        let item = FetchedItem {
            item_id: "edge-1".into(),
            item_type: "entity".into(),
            encrypted_blob: data.to_vec(),
            checksum: checksum(data),
            author_id: "alpha-agent".into(),
            group_id: "alpha-internal".into(),
            key_version: 1,
            parent_id: None,
            is_copy: false,
            updated_at: "2026-02-01T00:00:00Z".into(),
        };

        // Dynamic edge relay that only knows about "shared-xorg"
        let dynamic = |group_id: &str| group_id == "shared-xorg";
        let result = engine.on_receive(&db, &item, &[], Some(&dynamic));
        assert!(matches!(result, ReceiveOutcome::Rejected(_)));

        // Same relay accepts "shared-xorg"
        let xorg_item = FetchedItem {
            item_id: "edge-2".into(),
            group_id: "shared-xorg".into(),
            ..item
        };
        let result = engine.on_receive(&db, &xorg_item, &[], Some(&dynamic));
        assert_eq!(result, ReceiveOutcome::Stored);
    }

    #[test]
    fn test_on_receive_tombstone_deletes_item() {
        let engine = default_engine();
        let dir = tempfile::tempdir().unwrap();
        let db = cordelia_storage::SqliteStorage::create_new(&dir.path().join("test.db")).unwrap();

        // First, store an item
        let data = b"will-be-deleted";
        let item = FetchedItem {
            item_id: "doomed-1".into(),
            item_type: "entity".into(),
            encrypted_blob: data.to_vec(),
            checksum: checksum(data),
            author_id: "russell".into(),
            group_id: "seed-drill".into(),
            key_version: 1,
            parent_id: None,
            is_copy: false,
            updated_at: "2026-02-01T00:00:00Z".into(),
        };
        let result = engine.on_receive(&db, &item, &["seed-drill".into()], None);
        assert_eq!(result, ReceiveOutcome::Stored);

        // Verify item exists
        assert!(db.read_l2_item("doomed-1").unwrap().is_some());

        // Send tombstone
        let tombstone = FetchedItem {
            item_id: "doomed-1".into(),
            item_type: "__tombstone__".into(),
            encrypted_blob: Vec::new(),
            checksum: checksum(b""),
            author_id: "russell".into(),
            group_id: "seed-drill".into(),
            key_version: 1,
            parent_id: None,
            is_copy: false,
            updated_at: "2026-02-09T00:00:00Z".into(),
        };
        let result = engine.on_receive(&db, &tombstone, &["seed-drill".into()], None);
        assert_eq!(result, ReceiveOutcome::Stored);

        // Verify item is deleted
        assert!(db.read_l2_item("doomed-1").unwrap().is_none());
    }

    #[test]
    fn test_on_receive_tombstone_for_missing_item() {
        let engine = default_engine();
        let dir = tempfile::tempdir().unwrap();
        let db = cordelia_storage::SqliteStorage::create_new(&dir.path().join("test.db")).unwrap();

        // Tombstone for item that doesn't exist locally
        let tombstone = FetchedItem {
            item_id: "never-existed".into(),
            item_type: "__tombstone__".into(),
            encrypted_blob: Vec::new(),
            checksum: checksum(b""),
            author_id: "russell".into(),
            group_id: "seed-drill".into(),
            key_version: 1,
            parent_id: None,
            is_copy: false,
            updated_at: "2026-02-09T00:00:00Z".into(),
        };
        let result = engine.on_receive(&db, &tombstone, &["seed-drill".into()], None);
        assert_eq!(result, ReceiveOutcome::Duplicate); // Already gone
    }

    #[test]
    fn test_on_receive_tombstone_rejected_not_member() {
        let engine = default_engine();
        let dir = tempfile::tempdir().unwrap();
        let db = cordelia_storage::SqliteStorage::create_new(&dir.path().join("test.db")).unwrap();

        // Tombstone for a group we're not a member of
        let tombstone = FetchedItem {
            item_id: "foreign-1".into(),
            item_type: "__tombstone__".into(),
            encrypted_blob: Vec::new(),
            checksum: checksum(b""),
            author_id: "attacker".into(),
            group_id: "foreign-group".into(),
            key_version: 1,
            parent_id: None,
            is_copy: false,
            updated_at: "2026-02-09T00:00:00Z".into(),
        };
        let result = engine.on_receive(&db, &tombstone, &["seed-drill".into()], None);
        assert!(matches!(result, ReceiveOutcome::Rejected(_)));
    }
}
