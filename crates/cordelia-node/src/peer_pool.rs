//! Peer pool -- thread-safe registry of active peer connections.
//!
//! Maps NodeId → PeerHandle (metadata only, no connection handles).
//! The Swarm task owns all connections; this pool tracks peer state.

use cordelia_governor::{GovernorActions, PeerState};
use cordelia_protocol::{GroupId, NodeId};
use libp2p::Multiaddr;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

type SharedGroups = Arc<RwLock<Vec<GroupId>>>;
type SharedRelayGroups = Arc<RwLock<HashSet<String>>>;

/// Handle to a connected peer (metadata only).
#[derive(Debug, Clone)]
pub struct PeerHandle {
    pub node_id: NodeId,
    pub addrs: Vec<Multiaddr>,
    pub state: PeerState,
    pub groups: Vec<GroupId>,
    pub group_intersection: Vec<GroupId>,
    /// RTT from libp2p ping, updated periodically.
    pub rtt_ms: Option<f64>,
    /// Negotiated protocol version from identify.
    pub protocol_version: u16,
    /// Whether this peer advertises itself as a relay.
    pub is_relay: bool,
    /// Cumulative items delivered to this peer via push/retry.
    pub items_delivered: u64,
}

/// Thread-safe pool of active peer connections.
///
/// For relay nodes, `relay_learned_groups` extends the effective group set
/// used when computing `group_intersection`. This allows dynamic relays to
/// find anti-entropy sync targets for groups they've learned from peers,
/// not just groups they're formally members of.
#[derive(Clone)]
pub struct PeerPool {
    inner: Arc<RwLock<HashMap<NodeId, PeerHandle>>>,
    our_groups: SharedGroups,
    relay_learned_groups: Option<SharedRelayGroups>,
}

impl PeerPool {
    pub fn new(our_groups: SharedGroups) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            our_groups,
            relay_learned_groups: None,
        }
    }

    /// Create a pool for a relay node. Learned groups are included when
    /// computing `group_intersection`, enabling anti-entropy sync for
    /// groups discovered via group exchange.
    pub fn new_relay(our_groups: SharedGroups, learned: SharedRelayGroups) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            our_groups,
            relay_learned_groups: Some(learned),
        }
    }

    /// Compute group intersection: peer's groups that overlap with our effective
    /// group set (shared_groups + relay_learned_groups for relay nodes).
    async fn compute_intersection(&self, peer_groups: &[GroupId]) -> Vec<GroupId> {
        let our_groups = self.our_groups.read().await;
        let learned = if let Some(ref relay) = self.relay_learned_groups {
            Some(relay.read().await)
        } else {
            None
        };

        peer_groups
            .iter()
            .filter(|g| {
                our_groups.contains(g)
                    || learned.as_ref().is_some_and(|set| set.contains(g.as_str()))
            })
            .cloned()
            .collect()
    }

    /// Insert a new peer after successful identify exchange.
    pub async fn insert(
        &self,
        node_id: NodeId,
        addrs: Vec<Multiaddr>,
        peer_groups: Vec<GroupId>,
        state: PeerState,
        protocol_version: u16,
        is_relay: bool,
    ) {
        let group_intersection = self.compute_intersection(&peer_groups).await;

        let state_name = state.name();
        let handle = PeerHandle {
            node_id,
            addrs,
            state,
            groups: peer_groups,
            group_intersection,
            rtt_ms: None,
            protocol_version,
            is_relay,
            items_delivered: 0,
        };

        let pool_size = {
            let mut pool = self.inner.write().await;
            pool.insert(node_id, handle);
            pool.len()
        };
        tracing::info!(
            %node_id,
            state = state_name,
            is_relay,
            pool_size,
            "pool: peer added"
        );
    }

    /// Remove a peer from the pool.
    pub async fn remove(&self, node_id: &NodeId) -> Option<PeerHandle> {
        let mut pool = self.inner.write().await;
        let removed = pool.remove(node_id);
        if removed.is_some() {
            tracing::info!(
                %node_id,
                pool_size = pool.len(),
                "pool: peer removed"
            );
        }
        removed
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

    /// Get all active peers (Hot or Warm) that share a given group.
    /// Used by push retries to maximise coverage in small meshes.
    pub async fn active_peers_for_group(&self, group_id: &str) -> Vec<PeerHandle> {
        self.inner
            .read()
            .await
            .values()
            .filter(|h| h.state.is_active() && h.group_intersection.contains(&group_id.to_string()))
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

    /// Get all active peers (Hot or Warm) that share a given group OR are relays.
    /// Used by push dispatch: relays forward items even without group membership.
    pub async fn active_peers_for_group_or_relays(&self, group_id: &str) -> Vec<PeerHandle> {
        self.inner
            .read()
            .await
            .values()
            .filter(|h| {
                h.state.is_active()
                    && (h.is_relay || h.group_intersection.contains(&group_id.to_string()))
            })
            .cloned()
            .collect()
    }

    /// Get a random hot peer for a group, including relays.
    /// Used by anti-entropy sync on relays where group membership is irrelevant.
    /// Prefers peers with group_intersection match over relay-only matches.
    /// Falls back to warm peers, then to relay-only matches.
    pub async fn random_hot_peer_for_group_or_relays(&self, group_id: &str) -> Option<PeerHandle> {
        let pool = self.inner.read().await;
        let gid = group_id.to_string();

        // Priority 1: Hot peers with group_intersection match (most likely to have items)
        let mut peers: Vec<&PeerHandle> = pool
            .values()
            .filter(|h| h.state == PeerState::Hot && h.group_intersection.contains(&gid))
            .collect();

        // Priority 2: Active (warm) peers with group_intersection match
        if peers.is_empty() {
            peers = pool
                .values()
                .filter(|h| h.state.is_active() && h.group_intersection.contains(&gid))
                .collect();
        }

        // Priority 3: Any hot relay peer (may have relayed items without group membership)
        if peers.is_empty() {
            peers = pool
                .values()
                .filter(|h| h.state == PeerState::Hot && h.is_relay)
                .collect();
        }

        // Priority 4: Any active relay peer
        if peers.is_empty() {
            peers = pool
                .values()
                .filter(|h| h.state.is_active() && h.is_relay)
                .collect();
        }

        if peers.is_empty() {
            return None;
        }
        use rand::Rng;
        let idx = rand::thread_rng().gen_range(0..peers.len());
        Some(peers[idx].clone())
    }

    /// Get only relay peers (for gossip: only share relays in peer-share responses).
    pub async fn relay_peers(&self) -> Vec<PeerHandle> {
        self.inner
            .read()
            .await
            .values()
            .filter(|h| h.state.is_active() && h.is_relay)
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

    #[allow(dead_code)]
    /// Whether the pool is empty.
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }

    /// Update a peer's groups and recompute group_intersection.
    pub async fn update_peer_groups(&self, node_id: &NodeId, groups: Vec<GroupId>) {
        let intersection = self.compute_intersection(&groups).await;
        if let Some(handle) = self.inner.write().await.get_mut(node_id) {
            handle.group_intersection = intersection;
            handle.groups = groups;
        }
    }

    /// Update a peer's addresses (from identify).
    pub async fn update_addrs(&self, node_id: &NodeId, addrs: Vec<Multiaddr>) {
        if let Some(handle) = self.inner.write().await.get_mut(node_id) {
            handle.addrs = addrs;
        }
    }

    /// Record items delivered to a peer (push or retry).
    pub async fn record_items_delivered(&self, node_id: &NodeId, count: u64) {
        if let Some(handle) = self.inner.write().await.get_mut(node_id) {
            handle.items_delivered += count;
        }
    }

    /// Update a peer's RTT (from ping events).
    pub async fn update_rtt(&self, node_id: &NodeId, rtt_ms: f64) {
        if let Some(handle) = self.inner.write().await.get_mut(node_id) {
            handle.rtt_ms = Some(rtt_ms);
        }
    }

    #[allow(dead_code)]
    /// Set a peer's relay flag.
    pub async fn set_relay(&self, node_id: &NodeId, is_relay: bool) {
        if let Some(handle) = self.inner.write().await.get_mut(node_id) {
            handle.is_relay = is_relay;
        }
    }

    #[allow(dead_code)]
    /// Update peer state.
    pub async fn set_state(&self, node_id: &NodeId, state: PeerState) {
        if let Some(handle) = self.inner.write().await.get_mut(node_id) {
            handle.state = state;
        }
    }

    /// Apply governor actions: update states and remove disconnected peers.
    /// Note: actual connection teardown is handled by the swarm task via SwarmCommand::Disconnect.
    pub async fn apply_governor_actions(&self, actions: &GovernorActions) -> Vec<NodeId> {
        let mut pool = self.inner.write().await;
        let mut disconnected = Vec::new();

        // Apply state transitions
        for (node_id, from, to) in &actions.transitions {
            if let Some(handle) = pool.get_mut(node_id) {
                match to.as_str() {
                    "hot" => handle.state = PeerState::Hot,
                    "warm" => handle.state = PeerState::Warm,
                    _ => {}
                }
                tracing::debug!(
                    peer = %node_id,
                    from,
                    to,
                    "pool: state transition applied"
                );
            } else {
                tracing::warn!(
                    peer = %node_id,
                    from,
                    to,
                    "pool: governor transition for peer not in pool"
                );
            }
        }

        // Remove disconnected peers from pool (caller sends SwarmCommand::Disconnect)
        for node_id in &actions.disconnect {
            if pool.remove(node_id).is_some() {
                tracing::debug!(peer = %node_id, "pool: peer removed by governor");
                disconnected.push(*node_id);
            }
        }

        disconnected
    }

    /// Get a random hot peer for a group (for anti-entropy sync).
    /// Falls back to warm peers if no hot peers available (small mesh).
    pub async fn random_hot_peer_for_group(&self, group_id: &str) -> Option<PeerHandle> {
        let mut peers = self.hot_peers_for_group(group_id).await;
        if peers.is_empty() {
            peers = self.active_peers_for_group(group_id).await;
        }
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
                cordelia_api::PeerDetail {
                    node_id: h.node_id.to_base58(),
                    addrs: h.addrs.iter().map(|a| a.to_string()).collect(),
                    state: h.state.name().to_string(),
                    rtt_ms: h.rtt_ms,
                    items_delivered: h.items_delivered,
                    groups: h.groups.clone(),
                    group_intersection: h.group_intersection.clone(),
                    is_relay: h.is_relay,
                    protocol_version: h.protocol_version,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::PeerId;

    fn test_peer_id() -> PeerId {
        PeerId::random()
    }

    #[tokio::test]
    async fn test_peer_pool_new() {
        let groups = Arc::new(RwLock::new(vec!["g1".into()]));
        let pool = PeerPool::new(groups);
        assert_eq!(pool.len().await, 0);
    }

    #[tokio::test]
    async fn test_intersection_without_relay() {
        let our_groups = Arc::new(RwLock::new(vec!["g1".into(), "g2".into()]));
        let pool = PeerPool::new(our_groups);

        let peer = test_peer_id();
        pool.insert(
            peer,
            vec![],
            vec!["g2".into(), "g3".into()],
            PeerState::Hot,
            1,
            false,
        )
        .await;

        let handle = pool.get(&peer).await.unwrap();
        assert_eq!(handle.group_intersection, vec!["g2".to_string()]);
    }

    #[tokio::test]
    async fn test_intersection_includes_relay_learned_groups() {
        let our_groups = Arc::new(RwLock::new(vec!["g1".into()]));
        let learned = Arc::new(RwLock::new(HashSet::from(["g-learned".to_string()])));
        let pool = PeerPool::new_relay(our_groups, learned);

        let peer = test_peer_id();
        pool.insert(
            peer,
            vec![],
            vec!["g1".into(), "g-learned".into(), "g-unknown".into()],
            PeerState::Hot,
            1,
            false,
        )
        .await;

        let handle = pool.get(&peer).await.unwrap();
        // g1 matches shared_groups, g-learned matches relay_learned_groups
        assert!(handle.group_intersection.contains(&"g1".to_string()));
        assert!(handle.group_intersection.contains(&"g-learned".to_string()));
        assert!(!handle.group_intersection.contains(&"g-unknown".to_string()));
    }

    #[tokio::test]
    async fn test_update_peer_groups_uses_relay_learned() {
        let our_groups = Arc::new(RwLock::new(vec!["g1".into()]));
        let learned = Arc::new(RwLock::new(HashSet::new()));
        let pool = PeerPool::new_relay(our_groups, learned.clone());

        let peer = test_peer_id();
        pool.insert(peer, vec![], vec!["g-new".into()], PeerState::Hot, 1, false)
            .await;

        // Initially no intersection (g-new not in our_groups or learned)
        let handle = pool.get(&peer).await.unwrap();
        assert!(handle.group_intersection.is_empty());

        // Simulate group exchange: relay learns g-new
        learned.write().await.insert("g-new".to_string());

        // Recompute intersection (as group exchange does)
        pool.update_peer_groups(&peer, vec!["g-new".into()]).await;
        let handle = pool.get(&peer).await.unwrap();
        assert_eq!(handle.group_intersection, vec!["g-new".to_string()]);
    }
}
