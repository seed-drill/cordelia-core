//! Replication engine -- coordinates culture dispatch and receive.

use cordelia_protocol::messages::{FetchedItem, ItemHeader};
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
pub enum OutboundAction {
    /// Send full item to all hot group peers (EagerPush).
    BroadcastItem {
        group_id: GroupId,
        item: FetchedItem,
    },
    /// Send header to all hot group peers (NotifyAndFetch).
    BroadcastHeader {
        group_id: GroupId,
        header: ItemHeader,
    },
    /// No action (Passive).
    None,
}

impl ReplicationEngine {
    pub fn new(config: ReplicationConfig, entity_id: String) -> Self {
        Self { config, entity_id }
    }

    /// Determine outbound action when a local write occurs.
    pub fn on_local_write(
        &self,
        group_id: &str,
        culture: &GroupCulture,
        item_id: &str,
        item_type: &str,
        data: &[u8],
        key_version: u32,
    ) -> OutboundAction {
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
                    parent_id: None,
                    is_copy: false,
                    updated_at: chrono::Utc::now().to_rfc3339(),
                },
            },
            ReplicationStrategy::NotifyAndFetch => OutboundAction::BroadcastHeader {
                group_id: group_id.to_string(),
                header: ItemHeader {
                    item_id: item_id.to_string(),
                    item_type: item_type.to_string(),
                    checksum: cs,
                    updated_at: chrono::Utc::now().to_rfc3339(),
                    author_id: self.entity_id.clone(),
                    is_deletion: false,
                },
            },
            ReplicationStrategy::Passive => OutboundAction::None,
        }
    }

    /// Process a received item from a remote peer.
    pub fn on_receive(
        &self,
        storage: &dyn Storage,
        item: &FetchedItem,
        our_groups: &[String],
    ) -> ReceiveOutcome {
        // 1. Validate group membership
        if !our_groups.contains(&item.group_id) {
            return ReceiveOutcome::Rejected(format!("not a member of group {}", item.group_id));
        }

        // 2. Validate checksum
        if !validate_checksum(item) {
            return ReceiveOutcome::Rejected("checksum mismatch".into());
        }

        // 3. Dedup: check if we already have this exact item
        if let Ok(Some(existing)) = storage.read_l2_item(&item.item_id) {
            if existing.checksum.as_deref() == Some(&item.checksum) {
                return ReceiveOutcome::Duplicate;
            }

            // Conflict resolution: last-writer-wins by updated_at
            if existing.updated_at >= item.updated_at {
                return ReceiveOutcome::Duplicate; // our version is newer or equal
            }
        }

        // 4. Store the item (encrypted blob, no decryption)
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

        // 5. Log access
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
            .sync_interval_secs(
                self.config.sync_interval_moderate_secs,
                self.config.sync_interval_taciturn_secs,
            )
            .unwrap_or(self.config.sync_interval_moderate_secs)
    }

    /// Max batch size for fetch requests.
    pub fn max_batch_size(&self) -> u32 {
        self.config.max_batch_size
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

        let action = engine.on_local_write("seed-drill", &culture, "item-1", "entity", b"blob", 1);

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
    fn test_on_local_write_moderate() {
        let engine = default_engine();
        let culture = GroupCulture::default();

        let action = engine.on_local_write("seed-drill", &culture, "item-1", "entity", b"blob", 1);

        match action {
            OutboundAction::BroadcastHeader { group_id, header } => {
                assert_eq!(group_id, "seed-drill");
                assert_eq!(header.item_id, "item-1");
            }
            _ => panic!("expected BroadcastHeader"),
        }
    }

    #[test]
    fn test_on_local_write_passive() {
        let engine = default_engine();
        let culture = GroupCulture {
            broadcast_eagerness: "taciturn".into(),
            ..Default::default()
        };

        let action = engine.on_local_write("seed-drill", &culture, "item-1", "entity", b"blob", 1);

        assert!(matches!(action, OutboundAction::None));
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

        let result = engine.on_receive(&db, &item, &["seed-drill".into()]);
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

        let result = engine.on_receive(&db, &item, &["seed-drill".into()]);
        assert_eq!(result, ReceiveOutcome::Stored);

        // Second receive of same item should be duplicate
        let result2 = engine.on_receive(&db, &item, &["seed-drill".into()]);
        assert_eq!(result2, ReceiveOutcome::Duplicate);
    }
}
