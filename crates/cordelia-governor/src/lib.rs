//! Cordelia Governor -- peer state machine, promotion/demotion, churn.
//!
//! Background tokio task, ticks every 10s.
//! Manages Cold → Warm → Hot peer lifecycle with adversarial demotion.

use cordelia_protocol::{GroupId, NodeId, ERA_0};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// Governor tick interval (sourced from current era).
pub const TICK_INTERVAL: Duration = Duration::from_secs(ERA_0.tick_interval_secs);

/// Time before demoting inactive peers (sourced from current era).
const DEAD_TIMEOUT: Duration = Duration::from_secs(ERA_0.dead_timeout_secs);

/// Time before demoting stale hot peers with no items delivered (sourced from current era).
const STALE_TIMEOUT: Duration = Duration::from_secs(ERA_0.stale_timeout_secs);

/// Default ban duration (sourced from current era).
const DEFAULT_BAN_DURATION: Duration = Duration::from_secs(ERA_0.ban_base_duration_secs);

/// Dial policy controls which peers the governor will attempt to connect to.
#[derive(Debug, Clone)]
pub enum DialPolicy {
    /// Dial any discovered peer (relay behaviour).
    All,
    /// Only dial peers marked as relays or bootnodes (personal node behaviour).
    RelaysOnly,
    /// Only dial specific trusted relay node IDs (keeper behaviour).
    TrustedOnly(Vec<NodeId>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernorTargets {
    pub hot_min: usize,
    pub hot_max: usize,
    pub warm_min: usize,
    pub warm_max: usize,
    pub cold_max: usize,
    pub churn_interval_secs: u64,
    pub churn_fraction: f64,
}

impl Default for GovernorTargets {
    fn default() -> Self {
        Self {
            hot_min: 2,
            hot_max: 20,
            warm_min: 10,
            warm_max: 50,
            cold_max: 100,
            churn_interval_secs: ERA_0.churn_interval_secs,
            churn_fraction: ERA_0.churn_fraction(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PeerState {
    Cold,
    Warm,
    Hot,
    Banned {
        until: Instant,
        reason: String,
        escalation: u32,
    },
}

impl PeerState {
    pub fn is_active(&self) -> bool {
        matches!(self, PeerState::Warm | PeerState::Hot)
    }

    pub fn is_banned(&self) -> bool {
        matches!(self, PeerState::Banned { .. })
    }

    pub fn name(&self) -> &'static str {
        match self {
            PeerState::Cold => "cold",
            PeerState::Warm => "warm",
            PeerState::Hot => "hot",
            PeerState::Banned { .. } => "banned",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub node_id: NodeId,
    pub addrs: Vec<SocketAddr>,
    pub state: PeerState,
    pub groups: Vec<GroupId>,
    pub rtt_ms: Option<f64>,
    pub last_activity: Instant,
    pub items_delivered: u64,
    pub connected_since: Option<Instant>,
    pub demoted_at: Option<Instant>,
    pub disconnect_count: u32,
    pub last_disconnected: Option<Instant>,
    /// Whether this peer is a relay/bootnode (eligible for dial under restricted policies).
    pub is_relay: bool,
}

impl PeerInfo {
    pub fn new(node_id: NodeId, addrs: Vec<SocketAddr>, groups: Vec<GroupId>) -> Self {
        Self {
            node_id,
            addrs,
            state: PeerState::Cold,
            groups,
            rtt_ms: None,
            last_activity: Instant::now(),
            items_delivered: 0,
            connected_since: None,
            demoted_at: None,
            disconnect_count: 0,
            last_disconnected: None,
            is_relay: false,
        }
    }

    /// Performance score: items delivered per second, weighted by RTT.
    pub fn score(&self) -> f64 {
        let elapsed = self
            .connected_since
            .map(|s| s.elapsed().as_secs_f64())
            .unwrap_or(1.0)
            .max(1.0);

        let throughput = self.items_delivered as f64 / elapsed;
        let rtt_factor = self.rtt_ms.map(|r| 1.0 / (1.0 + r / 100.0)).unwrap_or(0.5);

        throughput * rtt_factor
    }

    /// Whether this peer has any groups in common with the given set.
    pub fn has_group_overlap(&self, groups: &[GroupId]) -> bool {
        self.groups.iter().any(|g| groups.contains(g))
    }
}

/// Peer governor managing the peer state machine.
pub struct Governor {
    peers: HashMap<NodeId, PeerInfo>,
    targets: GovernorTargets,
    our_groups: Vec<GroupId>,
    last_churn: Instant,
    dial_policy: DialPolicy,
}

/// Actions the governor wants the node to take after a tick.
#[derive(Debug, Default)]
pub struct GovernorActions {
    /// Peers to connect to (Cold → Warm promotion).
    pub connect: Vec<NodeId>,
    /// Peers to disconnect from.
    pub disconnect: Vec<NodeId>,
    /// State transitions that occurred.
    pub transitions: Vec<(NodeId, String, String)>, // (node_id, from, to)
}

impl Governor {
    pub fn new(targets: GovernorTargets, our_groups: Vec<GroupId>) -> Self {
        Self::with_dial_policy(targets, our_groups, DialPolicy::All)
    }

    pub fn with_dial_policy(
        targets: GovernorTargets,
        our_groups: Vec<GroupId>,
        dial_policy: DialPolicy,
    ) -> Self {
        Self {
            peers: HashMap::new(),
            targets,
            our_groups,
            last_churn: Instant::now(),
            dial_policy,
        }
    }

    /// Add or update a known peer.
    pub fn add_peer(&mut self, node_id: NodeId, addrs: Vec<SocketAddr>, groups: Vec<GroupId>) {
        self.peers
            .entry(node_id)
            .and_modify(|p| {
                p.addrs = addrs.clone();
                p.groups = groups.clone();
            })
            .or_insert_with(|| PeerInfo::new(node_id, addrs, groups));
    }

    /// Record that a peer sent us a keep-alive response.
    pub fn record_activity(&mut self, node_id: &NodeId, rtt_ms: Option<f64>) {
        if let Some(peer) = self.peers.get_mut(node_id) {
            peer.last_activity = Instant::now();
            if let Some(rtt) = rtt_ms {
                peer.rtt_ms = Some(rtt);
            }
        }
    }

    /// Record that a peer delivered items.
    pub fn record_items_delivered(&mut self, node_id: &NodeId, count: u64) {
        if let Some(peer) = self.peers.get_mut(node_id) {
            peer.items_delivered += count;
            peer.last_activity = Instant::now();
        }
    }

    /// Mark peer as connected (Warm state).
    pub fn mark_connected(&mut self, node_id: &NodeId) {
        if let Some(peer) = self.peers.get_mut(node_id) {
            if peer.state == PeerState::Cold {
                peer.state = PeerState::Warm;
                peer.connected_since = Some(Instant::now());
                peer.last_activity = Instant::now();
            }
        }
    }

    /// Mark peer as disconnected (back to Cold) with reconnect backoff.
    /// Called when QUIC connection drops to keep governor in sync with pool.
    /// Tracks disconnect count for exponential backoff: min(2^count * 30s, 15min).
    pub fn mark_disconnected(&mut self, node_id: &NodeId) {
        if let Some(peer) = self.peers.get_mut(node_id) {
            if peer.state.is_active() {
                peer.state = PeerState::Cold;
                peer.connected_since = None;
                peer.disconnect_count += 1;
                peer.last_disconnected = Some(Instant::now());
            }
        }
    }

    /// Mark a dial attempt as failed for backoff tracking.
    /// Unlike mark_disconnected, works on Cold peers (pre-connection failures).
    pub fn mark_dial_failed(&mut self, node_id: &NodeId) {
        if let Some(peer) = self.peers.get_mut(node_id) {
            peer.disconnect_count += 1;
            peer.last_disconnected = Some(Instant::now());
        }
    }

    /// Backoff duration for a peer based on disconnect count.
    /// Exponential: min(2^count * base, max). Zero if never disconnected.
    /// Base and max sourced from current era.
    fn reconnect_backoff(disconnect_count: u32) -> Duration {
        if disconnect_count == 0 {
            return Duration::ZERO;
        }
        let base = ERA_0.reconnect_backoff_base_secs;
        let max = ERA_0.reconnect_backoff_max_secs;
        let secs = base.saturating_mul(1u64 << disconnect_count.min(ERA_0.backoff_saturation_count));
        Duration::from_secs(secs.min(max))
    }

    /// Replace a peer's node ID (e.g. after handshake reveals real identity).
    /// Moves all peer state from `old` to `new`. Returns true if replaced.
    /// Replace a peer's node ID (e.g. after handshake reveals real identity).
    /// Moves all peer state from `old` to `new`, preserving relay flag. Returns true if replaced.
    pub fn replace_node_id(&mut self, old: &NodeId, new: NodeId, groups: Vec<GroupId>) -> bool {
        if let Some(mut peer) = self.peers.remove(old) {
            peer.node_id = new;
            peer.groups = groups;
            // is_relay is preserved from the old entry (bootnode seeding sets it)
            self.peers.insert(new, peer);
            true
        } else {
            false
        }
    }

    /// Ban a peer for protocol violation.
    pub fn ban_peer(&mut self, node_id: &NodeId, reason: String) {
        if let Some(peer) = self.peers.get_mut(node_id) {
            let escalation = match &peer.state {
                PeerState::Banned { escalation, .. } => escalation + 1,
                _ => 1,
            };
            let duration = DEFAULT_BAN_DURATION * escalation;
            peer.state = PeerState::Banned {
                until: Instant::now() + duration,
                reason,
                escalation,
            };
            peer.connected_since = None;
        }
    }

    /// Mark a peer as a relay node.
    pub fn set_peer_relay(&mut self, node_id: &NodeId, is_relay: bool) {
        if let Some(peer) = self.peers.get_mut(node_id) {
            peer.is_relay = is_relay;
        }
    }

    /// Check if a peer is dialable under the current policy.
    fn is_dialable(&self, peer: &PeerInfo) -> bool {
        match &self.dial_policy {
            DialPolicy::All => true,
            DialPolicy::RelaysOnly => peer.is_relay,
            DialPolicy::TrustedOnly(trusted) => trusted.contains(&peer.node_id),
        }
    }

    /// Get a peer's current state.
    pub fn peer_state(&self, node_id: &NodeId) -> Option<&PeerState> {
        self.peers.get(node_id).map(|p| &p.state)
    }

    /// Get peer info.
    pub fn peer_info(&self, node_id: &NodeId) -> Option<&PeerInfo> {
        self.peers.get(node_id)
    }

    /// Get all hot peers for a specific group.
    pub fn hot_peers_for_group(&self, group_id: &str) -> Vec<&PeerInfo> {
        self.peers
            .values()
            .filter(|p| p.state == PeerState::Hot && p.groups.contains(&group_id.to_string()))
            .collect()
    }

    /// Get counts by state.
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let mut hot = 0;
        let mut warm = 0;
        let mut cold = 0;
        let mut banned = 0;
        for p in self.peers.values() {
            match p.state {
                PeerState::Hot => hot += 1,
                PeerState::Warm => warm += 1,
                PeerState::Cold => cold += 1,
                PeerState::Banned { .. } => banned += 1,
            }
        }
        (hot, warm, cold, banned)
    }

    /// Run one governor tick. Returns actions for the node to execute.
    pub fn tick(&mut self) -> GovernorActions {
        let mut actions = GovernorActions::default();

        // 1. Unban expired bans
        self.unban_expired(&mut actions);

        // 2. Reap dead peers
        self.reap_dead(&mut actions);

        // 3. Promote Cold → Warm if needed
        self.promote_cold_to_warm(&mut actions);

        // 4. Promote Warm → Hot if needed
        self.promote_warm_to_hot(&mut actions);

        // 5. Demote excess Hot → Warm
        self.demote_excess_hot(&mut actions);

        // 6. Periodic churn
        self.churn(&mut actions);

        // 7. Evict excess cold
        self.evict_excess_cold(&mut actions);

        actions
    }

    fn unban_expired(&mut self, actions: &mut GovernorActions) {
        let now = Instant::now();
        for peer in self.peers.values_mut() {
            if let PeerState::Banned { until, .. } = &peer.state {
                if now >= *until {
                    let from = peer.state.name().to_string();
                    peer.state = PeerState::Cold;
                    actions
                        .transitions
                        .push((peer.node_id, from, "cold".into()));
                }
            }
        }
    }

    fn reap_dead(&mut self, actions: &mut GovernorActions) {
        let now = Instant::now();
        let dead_ids: Vec<NodeId> = self
            .peers
            .values()
            .filter(|p| p.state.is_active() && now.duration_since(p.last_activity) > DEAD_TIMEOUT)
            .map(|p| p.node_id)
            .collect();

        for id in dead_ids {
            if let Some(peer) = self.peers.get_mut(&id) {
                let from = peer.state.name().to_string();
                match peer.state {
                    PeerState::Hot => {
                        peer.state = PeerState::Warm;
                        peer.connected_since = None;
                        peer.demoted_at = Some(Instant::now());
                        actions.transitions.push((id, from, "warm".into()));
                    }
                    PeerState::Warm => {
                        peer.state = PeerState::Cold;
                        peer.connected_since = None;
                        actions.disconnect.push(id);
                        actions.transitions.push((id, from, "cold".into()));
                    }
                    _ => {}
                }
            }
        }
    }

    fn promote_cold_to_warm(&mut self, actions: &mut GovernorActions) {
        let (_, warm, _, _) = self.counts();
        if warm >= self.targets.warm_min {
            return;
        }

        let needed = self.targets.warm_min - warm;
        let now = Instant::now();
        let mut candidates: Vec<NodeId> = self
            .peers
            .values()
            .filter(|p| {
                matches!(p.state, PeerState::Cold) && self.is_dialable(p) && {
                    // Skip peers still in reconnect backoff
                    let backoff = Self::reconnect_backoff(p.disconnect_count);
                    p.last_disconnected
                        .is_none_or(|t| now.duration_since(t) >= backoff)
                }
            })
            .map(|p| (p.node_id, p.has_group_overlap(&self.our_groups)))
            .collect::<Vec<_>>()
            .into_iter()
            // Prefer peers with group overlap
            .sorted_by_key(|(_, overlap)| std::cmp::Reverse(*overlap))
            .into_iter()
            .map(|(id, _)| id)
            .take(needed)
            .collect();

        // If no sorting available, just take first N
        if candidates.is_empty() {
            candidates = self
                .peers
                .values()
                .filter(|p| {
                    matches!(p.state, PeerState::Cold) && self.is_dialable(p) && {
                        let backoff = Self::reconnect_backoff(p.disconnect_count);
                        p.last_disconnected
                            .is_none_or(|t| now.duration_since(t) >= backoff)
                    }
                })
                .take(needed)
                .map(|p| p.node_id)
                .collect();
        }

        for id in candidates {
            actions.connect.push(id);
            // Note: actual state transition happens when mark_connected() is called
        }
    }

    fn promote_warm_to_hot(&mut self, actions: &mut GovernorActions) {
        let (hot, _, _, _) = self.counts();
        if hot >= self.targets.hot_min {
            // Check if any warm peer outperforms worst hot
            let worst_hot_score = self
                .peers
                .values()
                .filter(|p| p.state == PeerState::Hot)
                .map(|p| p.score())
                .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or(f64::MAX);

            let best_warm = self
                .peers
                .values()
                .filter(|p| {
                    p.state == PeerState::Warm
                        && p.demoted_at.is_none_or(|d| d.elapsed() > DEAD_TIMEOUT)
                })
                .max_by(|a, b| {
                    a.score()
                        .partial_cmp(&b.score())
                        .unwrap_or(std::cmp::Ordering::Equal)
                });

            if let Some(warm) = best_warm {
                if hot < self.targets.hot_max && warm.score() > worst_hot_score {
                    let id = warm.node_id;
                    if let Some(peer) = self.peers.get_mut(&id) {
                        peer.state = PeerState::Hot;
                        peer.disconnect_count = 0; // stable connection, reset backoff
                        actions.transitions.push((id, "warm".into(), "hot".into()));
                    }
                }
            }
            return;
        }

        // Need more hot peers
        let needed = self.targets.hot_min - hot;
        let mut warm_peers: Vec<(NodeId, f64)> = self
            .peers
            .values()
            .filter(|p| {
                p.state == PeerState::Warm
                    && p.demoted_at.is_none_or(|d| d.elapsed() > DEAD_TIMEOUT)
            })
            .map(|p| (p.node_id, p.score()))
            .collect();

        warm_peers.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        for (id, _) in warm_peers.into_iter().take(needed) {
            if let Some(peer) = self.peers.get_mut(&id) {
                peer.state = PeerState::Hot;
                peer.disconnect_count = 0; // stable connection, reset backoff
                actions.transitions.push((id, "warm".into(), "hot".into()));
            }
        }
    }

    fn demote_excess_hot(&mut self, actions: &mut GovernorActions) {
        let (hot, _, _, _) = self.counts();
        if hot <= self.targets.hot_max {
            return;
        }

        let excess = hot - self.targets.hot_max;

        // Demote stale (no items for STALE_TIMEOUT) first, then worst performers
        let mut hot_peers: Vec<(NodeId, f64, bool)> = self
            .peers
            .values()
            .filter(|p| p.state == PeerState::Hot)
            .map(|p| {
                let is_stale = p.last_activity.elapsed() > STALE_TIMEOUT;
                (p.node_id, p.score(), is_stale)
            })
            .collect();

        // Sort: stale first, then by worst score
        hot_peers.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        });

        for (id, _, _) in hot_peers.into_iter().take(excess) {
            if let Some(peer) = self.peers.get_mut(&id) {
                peer.state = PeerState::Warm;
                actions.transitions.push((id, "hot".into(), "warm".into()));
            }
        }
    }

    fn churn(&mut self, actions: &mut GovernorActions) {
        if self.last_churn.elapsed() < Duration::from_secs(self.targets.churn_interval_secs) {
            return;
        }
        self.last_churn = Instant::now();

        let (_, warm, cold, _) = self.counts();
        let churn_count = (warm as f64 * self.targets.churn_fraction).ceil() as usize;

        if churn_count == 0 || cold == 0 {
            return;
        }

        // Demote random warm → cold
        let warm_ids: Vec<NodeId> = self
            .peers
            .values()
            .filter(|p| p.state == PeerState::Warm)
            .take(churn_count)
            .map(|p| p.node_id)
            .collect();

        for id in &warm_ids {
            if let Some(peer) = self.peers.get_mut(id) {
                peer.state = PeerState::Cold;
                peer.connected_since = None;
                actions.disconnect.push(*id);
                actions
                    .transitions
                    .push((*id, "warm".into(), "cold".into()));
            }
        }

        // Promote random cold → warm (to replace), filtered by dial policy
        let cold_ids: Vec<NodeId> = self
            .peers
            .values()
            .filter(|p| matches!(p.state, PeerState::Cold) && self.is_dialable(p))
            .take(churn_count)
            .map(|p| p.node_id)
            .collect();

        for id in cold_ids {
            actions.connect.push(id);
        }
    }

    fn evict_excess_cold(&mut self, _actions: &mut GovernorActions) {
        let (_, _, cold, _) = self.counts();
        if cold <= self.targets.cold_max {
            return;
        }

        // Remove oldest cold peers
        let excess = cold - self.targets.cold_max;
        let mut cold_peers: Vec<(NodeId, Instant)> = self
            .peers
            .values()
            .filter(|p| matches!(p.state, PeerState::Cold))
            .map(|p| (p.node_id, p.last_activity))
            .collect();

        cold_peers.sort_by_key(|(_, t)| *t);

        for (id, _) in cold_peers.into_iter().take(excess) {
            self.peers.remove(&id);
        }
    }

    /// All known peers.
    pub fn all_peers(&self) -> impl Iterator<Item = &PeerInfo> {
        self.peers.values()
    }
}

/// Extension trait for sorting iterators (avoiding external dep).
trait SortedBy: Iterator + Sized {
    fn sorted_by_key<K: Ord, F: FnMut(&Self::Item) -> K>(self, f: F) -> Vec<Self::Item> {
        let mut v: Vec<Self::Item> = self.collect();
        v.sort_by_key(f);
        v
    }
}

impl<I: Iterator> SortedBy for I {}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node_id(byte: u8) -> NodeId {
        [byte; 32]
    }

    fn make_addr() -> Vec<SocketAddr> {
        vec!["127.0.0.1:9474".parse().unwrap()]
    }

    #[test]
    fn test_add_peer() {
        let mut gov = Governor::new(GovernorTargets::default(), vec!["g1".into()]);
        gov.add_peer(make_node_id(1), make_addr(), vec!["g1".into()]);
        assert_eq!(gov.counts(), (0, 0, 1, 0));
    }

    #[test]
    fn test_promote_to_warm() {
        let mut gov = Governor::new(GovernorTargets::default(), vec!["g1".into()]);
        for i in 0..15 {
            gov.add_peer(make_node_id(i), make_addr(), vec!["g1".into()]);
        }

        let actions = gov.tick();
        // Should want to connect to peers to reach warm_min (10)
        assert!(!actions.connect.is_empty());
    }

    #[test]
    fn test_promote_warm_to_hot() {
        let targets = GovernorTargets {
            hot_min: 2,
            warm_min: 0,
            ..Default::default()
        };
        let mut gov = Governor::new(targets, vec!["g1".into()]);

        for i in 0..5 {
            let id = make_node_id(i);
            gov.add_peer(id, make_addr(), vec!["g1".into()]);
            gov.mark_connected(&id);
        }

        let (hot, warm, _, _) = gov.counts();
        assert_eq!(hot, 0);
        assert_eq!(warm, 5);

        let actions = gov.tick();
        let (hot, _, _, _) = gov.counts();
        assert!(hot >= 2, "should have promoted to hot");
        assert!(!actions.transitions.is_empty());
    }

    #[test]
    fn test_ban_peer() {
        let mut gov = Governor::new(GovernorTargets::default(), vec![]);
        let id = make_node_id(1);
        gov.add_peer(id, make_addr(), vec![]);

        gov.ban_peer(&id, "protocol violation".into());
        assert!(gov.peer_state(&id).unwrap().is_banned());

        // Escalation
        gov.ban_peer(&id, "repeat offense".into());
        match gov.peer_state(&id).unwrap() {
            PeerState::Banned { escalation, .. } => assert_eq!(*escalation, 2),
            _ => panic!("should be banned"),
        }
    }

    #[test]
    fn test_hot_peers_for_group() {
        let mut gov = Governor::new(GovernorTargets::default(), vec!["g1".into(), "g2".into()]);

        let id1 = make_node_id(1);
        let id2 = make_node_id(2);
        gov.add_peer(id1, make_addr(), vec!["g1".into()]);
        gov.add_peer(id2, make_addr(), vec!["g2".into()]);

        // Force to hot
        gov.peers.get_mut(&id1).unwrap().state = PeerState::Hot;
        gov.peers.get_mut(&id2).unwrap().state = PeerState::Hot;

        let g1_hot = gov.hot_peers_for_group("g1");
        assert_eq!(g1_hot.len(), 1);
        assert_eq!(g1_hot[0].node_id, id1);
    }

    #[test]
    fn test_replace_node_id() {
        let mut gov = Governor::new(GovernorTargets::default(), vec!["g1".into()]);
        let old_id = make_node_id(99);
        let new_id = make_node_id(1);
        gov.add_peer(old_id, make_addr(), vec![]);

        assert!(gov.peer_info(&old_id).is_some());
        assert!(gov.peer_info(&new_id).is_none());

        let replaced = gov.replace_node_id(&old_id, new_id, vec!["g1".into()]);
        assert!(replaced);
        assert!(gov.peer_info(&old_id).is_none());
        assert!(gov.peer_info(&new_id).is_some());
        assert_eq!(
            gov.peer_info(&new_id).unwrap().groups,
            vec!["g1".to_string()]
        );
    }

    #[test]
    fn test_peer_score() {
        let mut peer = PeerInfo::new(make_node_id(1), make_addr(), vec![]);
        peer.connected_since = Some(Instant::now() - Duration::from_secs(100));
        peer.items_delivered = 50;
        peer.rtt_ms = Some(10.0);

        let score = peer.score();
        assert!(score > 0.0);
    }

    #[test]
    fn test_mark_disconnected() {
        let mut gov = Governor::new(GovernorTargets::default(), vec!["g1".into()]);
        let id = make_node_id(1);
        gov.add_peer(id, make_addr(), vec!["g1".into()]);

        // Cold peer: mark_disconnected should be a no-op
        gov.mark_disconnected(&id);
        assert_eq!(*gov.peer_state(&id).unwrap(), PeerState::Cold);
        assert_eq!(gov.peer_info(&id).unwrap().disconnect_count, 0);

        // Warm peer: should go back to Cold with disconnect tracking
        gov.mark_connected(&id);
        assert_eq!(*gov.peer_state(&id).unwrap(), PeerState::Warm);
        gov.mark_disconnected(&id);
        assert_eq!(*gov.peer_state(&id).unwrap(), PeerState::Cold);
        assert!(gov.peer_info(&id).unwrap().connected_since.is_none());
        assert_eq!(gov.peer_info(&id).unwrap().disconnect_count, 1);
        assert!(gov.peer_info(&id).unwrap().last_disconnected.is_some());

        // Hot peer: should go back to Cold, increment count
        gov.mark_connected(&id);
        gov.peers.get_mut(&id).unwrap().state = PeerState::Hot;
        gov.mark_disconnected(&id);
        assert_eq!(*gov.peer_state(&id).unwrap(), PeerState::Cold);
        assert_eq!(gov.peer_info(&id).unwrap().disconnect_count, 2);
    }

    #[test]
    fn test_reconnect_backoff_values() {
        assert_eq!(Governor::reconnect_backoff(0), Duration::ZERO);
        assert_eq!(Governor::reconnect_backoff(1), Duration::from_secs(60));
        assert_eq!(Governor::reconnect_backoff(2), Duration::from_secs(120));
        assert_eq!(Governor::reconnect_backoff(3), Duration::from_secs(240));
        assert_eq!(Governor::reconnect_backoff(4), Duration::from_secs(480));
        // Capped at 15 minutes (900s) from count=5 onward (30*32=960 > 900)
        assert_eq!(Governor::reconnect_backoff(5), Duration::from_secs(900));
        assert_eq!(Governor::reconnect_backoff(6), Duration::from_secs(900));
        assert_eq!(Governor::reconnect_backoff(99), Duration::from_secs(900));
    }

    #[test]
    fn test_backoff_prevents_immediate_reconnect() {
        let targets = GovernorTargets {
            warm_min: 5,
            ..Default::default()
        };
        let mut gov = Governor::new(targets, vec!["g1".into()]);

        let id = make_node_id(1);
        gov.add_peer(id, make_addr(), vec!["g1".into()]);

        // Connect then disconnect
        gov.mark_connected(&id);
        gov.mark_disconnected(&id);

        // Immediate tick should NOT try to reconnect (in 60s backoff)
        let actions = gov.tick();
        assert!(
            !actions.connect.contains(&id),
            "peer in backoff must not be reconnected"
        );
    }

    #[test]
    fn test_backoff_allows_reconnect_after_expiry() {
        let targets = GovernorTargets {
            warm_min: 5,
            ..Default::default()
        };
        let mut gov = Governor::new(targets, vec!["g1".into()]);

        let id = make_node_id(1);
        gov.add_peer(id, make_addr(), vec!["g1".into()]);

        // Connect then disconnect, but set last_disconnected in the past
        gov.mark_connected(&id);
        gov.mark_disconnected(&id);
        gov.peers.get_mut(&id).unwrap().last_disconnected =
            Some(Instant::now() - Duration::from_secs(120));

        // Tick should try to reconnect (60s backoff expired)
        let actions = gov.tick();
        assert!(
            actions.connect.contains(&id),
            "peer past backoff should be reconnected"
        );
    }

    #[test]
    fn test_hot_promotion_resets_disconnect_count() {
        let targets = GovernorTargets {
            hot_min: 1,
            warm_min: 0,
            ..Default::default()
        };
        let mut gov = Governor::new(targets, vec!["g1".into()]);

        let id = make_node_id(1);
        gov.add_peer(id, make_addr(), vec!["g1".into()]);
        gov.mark_connected(&id);

        // Simulate prior disconnects
        gov.peers.get_mut(&id).unwrap().disconnect_count = 3;

        let _actions = gov.tick();
        let peer = gov.peer_info(&id).unwrap();
        assert_eq!(peer.state, PeerState::Hot);
        assert_eq!(
            peer.disconnect_count, 0,
            "hot promotion should reset backoff"
        );
    }

    #[test]
    fn test_no_oscillation_after_reap() {
        let targets = GovernorTargets {
            hot_min: 2,
            warm_min: 0,
            ..Default::default()
        };
        let mut gov = Governor::new(targets, vec!["g1".into()]);

        // Create 3 warm peers, promote 2 to hot
        for i in 0..3 {
            let id = make_node_id(i);
            gov.add_peer(id, make_addr(), vec!["g1".into()]);
            gov.mark_connected(&id);
        }
        // Force two to Hot
        gov.peers.get_mut(&make_node_id(0)).unwrap().state = PeerState::Hot;
        gov.peers.get_mut(&make_node_id(1)).unwrap().state = PeerState::Hot;

        // Simulate dead timeout on peer 0 so reap_dead demotes it
        gov.peers.get_mut(&make_node_id(0)).unwrap().last_activity =
            Instant::now() - Duration::from_secs(100);

        let actions = gov.tick();

        // Peer 0 should have been demoted Hot -> Warm
        let peer0 = gov.peer_info(&make_node_id(0)).unwrap();
        assert_eq!(
            peer0.state,
            PeerState::Warm,
            "peer 0 should be demoted to Warm"
        );
        assert!(peer0.demoted_at.is_some(), "demoted_at should be set");

        // Despite hot_min=2 and only 1 hot, peer 0 should NOT have been re-promoted
        // because of hysteresis (demoted_at within DEAD_TIMEOUT)
        let _hot_count = actions
            .transitions
            .iter()
            .filter(|(_, _, to)| to == "hot")
            .count();
        // Peer 2 (warm, never demoted) could be promoted, but peer 0 must not be
        let peer0_promoted = actions
            .transitions
            .iter()
            .any(|(id, _, to)| *id == make_node_id(0) && to == "hot");
        assert!(
            !peer0_promoted,
            "recently demoted peer must not be re-promoted"
        );
    }

    #[test]
    fn test_dial_policy_all() {
        let targets = GovernorTargets {
            warm_min: 5,
            ..Default::default()
        };
        let mut gov = Governor::with_dial_policy(targets, vec!["g1".into()], DialPolicy::All);

        let relay_id = make_node_id(1);
        let personal_id = make_node_id(2);
        gov.add_peer(relay_id, make_addr(), vec!["g1".into()]);
        gov.set_peer_relay(&relay_id, true);
        gov.add_peer(personal_id, make_addr(), vec!["g1".into()]);

        let actions = gov.tick();
        assert!(
            actions.connect.contains(&relay_id),
            "relay should be in connect with DialPolicy::All"
        );
        assert!(
            actions.connect.contains(&personal_id),
            "personal should be in connect with DialPolicy::All"
        );
    }

    #[test]
    fn test_dial_policy_relays_only() {
        let targets = GovernorTargets {
            warm_min: 5,
            ..Default::default()
        };
        let mut gov =
            Governor::with_dial_policy(targets, vec!["g1".into()], DialPolicy::RelaysOnly);

        let relay_id = make_node_id(1);
        let personal_id = make_node_id(2);
        gov.add_peer(relay_id, make_addr(), vec!["g1".into()]);
        gov.set_peer_relay(&relay_id, true);
        gov.add_peer(personal_id, make_addr(), vec!["g1".into()]);

        let actions = gov.tick();
        assert!(
            actions.connect.contains(&relay_id),
            "relay should be in connect with DialPolicy::RelaysOnly"
        );
        assert!(
            !actions.connect.contains(&personal_id),
            "personal should NOT be in connect with DialPolicy::RelaysOnly"
        );
    }

    #[test]
    fn test_dial_policy_trusted_only() {
        let trusted_id = make_node_id(1);
        let untrusted_id = make_node_id(2);

        let targets = GovernorTargets {
            warm_min: 5,
            ..Default::default()
        };
        let mut gov = Governor::with_dial_policy(
            targets,
            vec!["g1".into()],
            DialPolicy::TrustedOnly(vec![trusted_id]),
        );

        gov.add_peer(trusted_id, make_addr(), vec!["g1".into()]);
        gov.set_peer_relay(&trusted_id, true);
        gov.add_peer(untrusted_id, make_addr(), vec!["g1".into()]);
        gov.set_peer_relay(&untrusted_id, true); // relay but not trusted

        let actions = gov.tick();
        assert!(
            actions.connect.contains(&trusted_id),
            "trusted relay should be in connect"
        );
        assert!(
            !actions.connect.contains(&untrusted_id),
            "untrusted relay should NOT be in connect with TrustedOnly"
        );
    }

    #[test]
    fn test_relay_flag_preserved_on_replace() {
        let mut gov = Governor::new(GovernorTargets::default(), vec!["g1".into()]);
        let old_id = make_node_id(99);
        let new_id = make_node_id(1);
        gov.add_peer(old_id, make_addr(), vec![]);
        gov.set_peer_relay(&old_id, true);

        gov.replace_node_id(&old_id, new_id, vec!["g1".into()]);
        assert!(
            gov.peer_info(&new_id).unwrap().is_relay,
            "relay flag should be preserved after replace_node_id"
        );
    }

    #[test]
    fn test_reap_then_promote_after_cooldown() {
        let targets = GovernorTargets {
            hot_min: 1,
            warm_min: 0,
            ..Default::default()
        };
        let mut gov = Governor::new(targets, vec!["g1".into()]);

        let id = make_node_id(1);
        gov.add_peer(id, make_addr(), vec!["g1".into()]);
        gov.mark_connected(&id);

        // Set demoted_at in the past (beyond DEAD_TIMEOUT)
        gov.peers.get_mut(&id).unwrap().demoted_at =
            Some(Instant::now() - Duration::from_secs(100));

        // Peer is Warm with expired cooldown -- should be eligible for promotion
        let _actions = gov.tick();
        let peer = gov.peer_info(&id).unwrap();
        assert_eq!(
            peer.state,
            PeerState::Hot,
            "peer should be promoted after cooldown expires"
        );
    }
}
