//! Peer pool -- thread-safe registry of active peer connections.
//!
//! Maps NodeId → PeerHandle (quinn::Connection + metadata).
//! Used by mini-protocols, governor task, and replication task.

use cordelia_governor::{GovernorActions, PeerState};
use cordelia_protocol::{GroupId, NodeId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Handle to a connected peer.
#[derive(Debug, Clone)]
pub struct PeerHandle {
    pub connection: quinn::Connection,
    pub node_id: NodeId,
    pub state: PeerState,
    pub groups: Vec<GroupId>,
    pub group_intersection: Vec<GroupId>,
    /// Negotiated protocol version from handshake.
    /// Used for future version-specific message handling (R3-022).
    pub protocol_version: u16,
}

/// Thread-safe pool of active peer connections.
#[derive(Clone)]
pub struct PeerPool {
    inner: Arc<RwLock<HashMap<NodeId, PeerHandle>>>,
    our_groups: Arc<Vec<GroupId>>,
}

impl PeerPool {
    pub fn new(our_groups: Vec<GroupId>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            our_groups: Arc::new(our_groups),
        }
    }

    /// Insert a new peer connection after successful handshake.
    pub async fn insert(
        &self,
        node_id: NodeId,
        connection: quinn::Connection,
        peer_groups: Vec<GroupId>,
        state: PeerState,
        protocol_version: u16,
    ) {
        let group_intersection: Vec<GroupId> = peer_groups
            .iter()
            .filter(|g| self.our_groups.contains(g))
            .cloned()
            .collect();

        let handle = PeerHandle {
            connection,
            node_id,
            state,
            groups: peer_groups,
            group_intersection,
            protocol_version,
        };

        self.inner.write().await.insert(node_id, handle);
    }

    /// Remove a peer from the pool.
    pub async fn remove(&self, node_id: &NodeId) -> Option<PeerHandle> {
        self.inner.write().await.remove(node_id)
    }

    /// Get a clone of a peer handle.
    pub async fn get(&self, node_id: &NodeId) -> Option<PeerHandle> {
        self.inner.read().await.get(node_id).cloned()
    }

    /// Get all hot peers that share a given group.
    pub async fn hot_peers_for_group(&self, group_id: &str) -> Vec<PeerHandle> {
        self.inner
            .read()
            .await
            .values()
            .filter(|h| {
                h.state == PeerState::Hot && h.group_intersection.contains(&group_id.to_string())
            })
            .cloned()
            .collect()
    }

    /// Get all connected peers (Warm or Hot).
    pub async fn active_peers(&self) -> Vec<PeerHandle> {
        self.inner
            .read()
            .await
            .values()
            .filter(|h| h.state.is_active())
            .cloned()
            .collect()
    }

    /// Count peers by state.
    pub async fn peer_count_by_state(&self) -> (usize, usize) {
        let pool = self.inner.read().await;
        let warm = pool.values().filter(|h| h.state == PeerState::Warm).count();
        let hot = pool.values().filter(|h| h.state == PeerState::Hot).count();
        (warm, hot)
    }

    #[allow(dead_code)]
    /// Total connected peer count.
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    /// Update a peer's groups and recompute group_intersection.
    pub async fn update_peer_groups(&self, node_id: &NodeId, groups: Vec<GroupId>) {
        if let Some(handle) = self.inner.write().await.get_mut(node_id) {
            handle.group_intersection = groups
                .iter()
                .filter(|g| self.our_groups.contains(g))
                .cloned()
                .collect();
            handle.groups = groups;
        }
    }

    #[allow(dead_code)]
    /// Update peer state.
    pub async fn set_state(&self, node_id: &NodeId, state: PeerState) {
        if let Some(handle) = self.inner.write().await.get_mut(node_id) {
            handle.state = state;
        }
    }

    /// Apply governor actions: update states and disconnect peers.
    pub async fn apply_governor_actions(&self, actions: &GovernorActions) {
        let mut pool = self.inner.write().await;

        // Apply state transitions
        for (node_id, from, to) in &actions.transitions {
            if let Some(handle) = pool.get_mut(node_id) {
                match to.as_str() {
                    "hot" => handle.state = PeerState::Hot,
                    "warm" => handle.state = PeerState::Warm,
                    _ => {}
                }
            } else {
                tracing::warn!(
                    peer = hex::encode(node_id),
                    from,
                    to,
                    "governor transition for peer not in pool"
                );
            }
        }

        // Disconnect peers
        for node_id in &actions.disconnect {
            if let Some(handle) = pool.remove(node_id) {
                handle
                    .connection
                    .close(quinn::VarInt::from_u32(0), b"governor disconnect");
            }
        }
    }

    /// Get a random hot peer for a group (for anti-entropy sync).
    pub async fn random_hot_peer_for_group(&self, group_id: &str) -> Option<PeerHandle> {
        let peers = self.hot_peers_for_group(group_id).await;
        if peers.is_empty() {
            return None;
        }
        use rand::Rng;
        let idx = rand::thread_rng().gen_range(0..peers.len());
        Some(peers[idx].clone())
    }

    /// Get details of all connected peers for the API.
    pub async fn peer_details(&self) -> Vec<cordelia_api::PeerDetail> {
        self.inner
            .read()
            .await
            .values()
            .map(|h| {
                let addr = h.connection.remote_address().to_string();
                let rtt = h.connection.rtt();
                cordelia_api::PeerDetail {
                    node_id: hex::encode(h.node_id),
                    addrs: vec![addr],
                    state: h.state.name().to_string(),
                    rtt_ms: Some(rtt.as_secs_f64() * 1000.0),
                    items_delivered: 0, // TODO: track this per-connection
                    groups: h.groups.clone(),
                    protocol_version: h.protocol_version,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_peer_pool_new() {
        let pool = PeerPool::new(vec!["g1".into()]);
        assert_eq!(pool.len().await, 0);
    }
}
