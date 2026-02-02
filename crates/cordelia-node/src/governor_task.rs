//! Governor background task -- ticks every 10s, manages peer lifecycle.
//!
//! On each tick:
//!   1. governor.tick() -> GovernorActions
//!   2. Connect actions: send Dial via SwarmCommand
//!   3. Disconnect/demote: send Disconnect via SwarmCommand, update pool
//!   4. On startup: seed bootnodes as cold, attempt initial connections
//!
//! Also handles SwarmEvent2 events: connect/disconnect/ping/identify.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cordelia_governor::Governor;
use libp2p::{Multiaddr, PeerId};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};

use crate::config::BootnodeEntry;
use crate::peer_pool::PeerPool;
use crate::swarm_task::{SwarmCommand, SwarmEvent2};

#[allow(clippy::too_many_arguments)]
/// Run the governor loop until shutdown.
///
/// `relay_learned_groups`: if Some, this node is a dynamic relay and groups
/// discovered from non-relay peers via GroupExchange are collected here.
/// The replication/swarm tasks use this to decide which groups to forward.
pub async fn run_governor_loop(
    governor: Arc<Mutex<Governor>>,
    pool: PeerPool,
    cmd_tx: mpsc::Sender<SwarmCommand>,
    mut event_rx: broadcast::Receiver<SwarmEvent2>,
    bootnodes: Vec<BootnodeEntry>,
    shared_groups: Arc<RwLock<Vec<String>>>,
    relay_learned_groups: Option<Arc<RwLock<HashSet<String>>>>,
    our_peer_id: PeerId,
    mut shutdown: broadcast::Receiver<()>,
) {
    if bootnodes.is_empty() {
        tracing::warn!("no bootnodes configured -- node will only accept inbound connections");
    }

    // Seed bootnodes as cold peers (with retry for DNS resolution)
    let mut unresolved_bootnodes: Vec<BootnodeEntry> = Vec::new();
    // Track resolved bootnode addrs for persistent reconnection
    let mut resolved_bootnode_addrs: Vec<(BootnodeEntry, Multiaddr)> = Vec::new();
    {
        let mut gov = governor.lock().await;
        for boot in &bootnodes {
            if let Some(addr) = parse_bootnode_multiaddr(boot).await {
                seed_bootnode(&mut gov, &boot.addr, addr.clone());
                resolved_bootnode_addrs.push((boot.clone(), addr));
            } else {
                tracing::info!(bootnode = &boot.addr, "bootnode DNS pending, will retry");
                unresolved_bootnodes.push(boot.clone());
            }
        }
    }

    // Track outbound dials: addr -> placeholder PeerId (for bootnode replacement)
    let mut pending_dials: HashMap<Multiaddr, PeerId> = HashMap::new();

    let tick_interval = cordelia_governor::TICK_INTERVAL;
    let mut tick_count: u64 = 0;
    let group_exchange_every = cordelia_protocol::GROUP_EXCHANGE_TICKS;
    let peer_discovery_every = cordelia_protocol::PEER_DISCOVERY_TICKS;

    loop {
        tokio::select! {
            _ = tokio::time::sleep(tick_interval) => {
                // Sync dynamic groups into governor before tick
                let actions = {
                    let mut gov = governor.lock().await;
                    let current_groups = shared_groups.read().await.clone();
                    gov.set_groups(current_groups);
                    gov.tick()
                };

                // Apply state transitions and get disconnected peer list
                let disconnected = pool.apply_governor_actions(&actions).await;

                // Send disconnect commands to swarm
                for peer_id in disconnected {
                    if let Err(e) = cmd_tx.send(SwarmCommand::Disconnect(peer_id)).await {
                        tracing::warn!(%peer_id, "governor: disconnect command send failed: {e}");
                    }
                }

                // Log transitions (promote to INFO for hot promotion)
                for (node_id, from, to) in &actions.transitions {
                    if to == "hot" {
                        tracing::info!(
                            peer = %node_id,
                            from,
                            to,
                            "gov: peer promoted"
                        );
                    } else {
                        tracing::debug!(
                            peer = %node_id,
                            from,
                            to,
                            "gov: peer state transition"
                        );
                    }
                }

                // Connect to promoted peers
                for node_id in &actions.connect {
                    let addr = {
                        let gov = governor.lock().await;
                        gov.peer_info(node_id)
                            .and_then(|p| p.addrs.first().cloned())
                    };

                    if let Some(addr) = addr {
                        tracing::debug!(
                            peer = %node_id,
                            %addr,
                            "gov: initiating dial to promoted peer"
                        );
                        pending_dials.insert(addr.clone(), *node_id);
                        if let Err(e) = cmd_tx.send(SwarmCommand::Dial(addr.clone())).await {
                            tracing::warn!(%addr, "gov: failed to send dial command: {e}");
                        }
                    } else {
                        tracing::warn!(
                            peer = %node_id,
                            "gov: connect requested but no address known"
                        );
                    }
                }

                let (warm, hot) = pool.peer_count_by_state().await;
                let group_count = shared_groups.read().await.len();
                tracing::info!(
                    warm,
                    hot,
                    groups = group_count,
                    promoted = actions.connect.len(),
                    demoted = actions.disconnect.len(),
                    tick = tick_count,
                    "governor tick"
                );

                tick_count += 1;

                // Retry unresolved bootnodes (DNS may not have been ready at startup)
                if !unresolved_bootnodes.is_empty() && tick_count <= 6 {
                    let mut still_unresolved = Vec::new();
                    let mut gov = governor.lock().await;
                    for boot in unresolved_bootnodes.drain(..) {
                        if let Some(addr) = parse_bootnode_multiaddr(&boot).await {
                            tracing::info!(bootnode = &boot.addr, "bootnode resolved on retry");
                            seed_bootnode(&mut gov, &boot.addr, addr.clone());
                            resolved_bootnode_addrs.push((boot, addr));
                        } else {
                            still_unresolved.push(boot);
                        }
                    }
                    drop(gov);
                    unresolved_bootnodes = still_unresolved;
                    if unresolved_bootnodes.is_empty() {
                        tracing::info!("all bootnodes resolved");
                    }
                }

                // Persistent bootnode reconnection ("Conditions Of Acceptance"):
                // if any resolved bootnode is not in the active peer pool,
                // re-dial it. Bootnodes are critical infrastructure -- they
                // must stay connected. Like a Culture Mind making contact,
                // we keep trying whether the other end is ready or not.
                if tick_count > 0 && tick_count.is_multiple_of(6) && !resolved_bootnode_addrs.is_empty() {
                    let active_addrs: HashSet<Multiaddr> = pool
                        .active_peers()
                        .await
                        .into_iter()
                        .flat_map(|h| h.addrs)
                        .collect();

                    for (boot, addr) in &resolved_bootnode_addrs {
                        if !active_addrs.contains(addr) {
                            tracing::warn!(
                                bootnode = &boot.addr,
                                %addr,
                                "bootnode not connected, re-dialling"
                            );
                            // Re-resolve DNS in case IP changed
                            let dial_addr = if let Some(fresh) = parse_bootnode_multiaddr(boot).await {
                                fresh
                            } else {
                                addr.clone()
                            };
                            if let Err(e) = cmd_tx.send(SwarmCommand::Dial(dial_addr)).await {
                                tracing::warn!(bootnode = &boot.addr, "bootnode re-dial send failed: {e}");
                            }
                        }
                    }
                }

                // Periodic group exchange with active peers
                if tick_count.is_multiple_of(group_exchange_every) {
                    let active_peers = pool.active_peers().await;
                    let groups = shared_groups.read().await.clone();
                    for peer in active_peers {
                        let cmd_tx = cmd_tx.clone();
                        let pool = pool.clone();
                        let groups = groups.clone();
                        let relay_learned = relay_learned_groups.clone();
                        tokio::spawn(async move {
                            let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
                            let _ = cmd_tx.send(SwarmCommand::SendGroupExchange {
                                peer: peer.node_id,
                                request: cordelia_protocol::messages::GroupExchange {
                                    groups: groups.clone(),
                                },
                                response_tx: resp_tx,
                            }).await;

                            match resp_rx.await {
                                Ok(Ok(resp)) => {
                                    let old_intersection = peer.group_intersection.clone();
                                    pool.update_peer_groups(&peer.node_id, resp.groups.clone()).await;
                                    let new_handle = pool.get(&peer.node_id).await;
                                    if let Some(h) = new_handle {
                                        if h.group_intersection != old_intersection {
                                            tracing::info!(
                                                peer = %peer.node_id,
                                                old = ?old_intersection,
                                                new = ?h.group_intersection,
                                                "group intersection updated"
                                            );
                                        }
                                    }
                                    // Dynamic relay: learn groups from non-relay peers
                                    if let Some(learned) = relay_learned {
                                        if !peer.is_relay {
                                            let mut set = learned.write().await;
                                            for g in &resp.groups {
                                                set.insert(g.clone());
                                            }
                                        }
                                    }
                                }
                                Ok(Err(e)) => {
                                    tracing::debug!(
                                        peer = %peer.node_id,
                                        "group exchange failed: {e}"
                                    );
                                }
                                Err(_) => {
                                    tracing::debug!(
                                        peer = %peer.node_id,
                                        "group exchange: response channel dropped"
                                    );
                                }
                            }
                        });
                    }
                }

                // Periodic peer discovery via gossip
                if tick_count.is_multiple_of(peer_discovery_every) {
                    let pool2 = pool.clone();
                    let gov2 = governor.clone();
                    let cmd_tx2 = cmd_tx.clone();
                    let local_id = our_peer_id;
                    tokio::spawn(async move {
                        discover_peers(&pool2, &gov2, &cmd_tx2, local_id).await;
                    });
                }
            }

            // Handle swarm events
            event = event_rx.recv() => {
                match event {
                    Ok(SwarmEvent2::PeerConnected { peer_id, addrs }) => {
                        tracing::info!(
                            %peer_id,
                            addr = ?addrs.first(),
                            "gov: peer connected"
                        );

                        // Check if this connection resulted from dialling a
                        // bootnode placeholder -- if so, replace the placeholder
                        // PeerId with the real one.
                        let placeholder = addrs
                            .iter()
                            .find_map(|a| pending_dials.remove(a));

                        let mut gov = governor.lock().await;
                        if let Some(old_id) = placeholder {
                            if old_id != peer_id {
                                gov.replace_node_id(&old_id, peer_id, vec![]);
                                tracing::info!(
                                    %peer_id,
                                    old = %old_id,
                                    "replaced bootnode placeholder"
                                );
                            }
                            gov.mark_connected(&peer_id);
                        } else if gov.peer_info(&peer_id).is_some() {
                            gov.mark_connected(&peer_id);
                        } else {
                            gov.add_peer(peer_id, addrs.clone(), vec![]);
                            gov.mark_connected(&peer_id);
                        }
                        // Check if governor knows this peer is a relay (e.g. seeded bootnode)
                        let is_relay = gov.peer_info(&peer_id)
                            .map(|p| p.is_relay)
                            .unwrap_or(false);
                        drop(gov);

                        // Insert into pool as Warm
                        pool.insert(
                            peer_id,
                            addrs,
                            vec![], // groups populated on group exchange
                            cordelia_governor::PeerState::Warm,
                            1,     // protocol version
                            is_relay,
                        ).await;

                        // Immediate group exchange after connect
                        let cmd_tx = cmd_tx.clone();
                        let pool = pool.clone();
                        let governor = governor.clone();
                        let groups = shared_groups.read().await.clone();
                        let relay_learned = relay_learned_groups.clone();
                        let peer_is_relay = is_relay;
                        tokio::spawn(async move {
                            let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
                            if let Err(e) = cmd_tx.send(SwarmCommand::SendGroupExchange {
                                peer: peer_id,
                                request: cordelia_protocol::messages::GroupExchange {
                                    groups: groups.clone(),
                                },
                                response_tx: resp_tx,
                            }).await {
                                tracing::warn!(%peer_id, "gov: failed to send initial group exchange: {e}");
                                return;
                            }

                            match resp_rx.await {
                                Ok(Ok(resp)) => {
                                    tracing::debug!(
                                        %peer_id,
                                        their_groups = resp.groups.len(),
                                        "gov: initial group exchange complete"
                                    );
                                    pool.update_peer_groups(&peer_id, resp.groups.clone()).await;
                                    let addrs = pool.get(&peer_id).await
                                        .map(|h| h.addrs.clone())
                                        .unwrap_or_default();
                                    governor.lock().await.add_peer(peer_id, addrs, resp.groups.clone());
                                    // Dynamic relay: learn groups from non-relay peers
                                    if let Some(learned) = relay_learned {
                                        if !peer_is_relay {
                                            let mut set = learned.write().await;
                                            for g in &resp.groups {
                                                set.insert(g.clone());
                                            }
                                        }
                                    }
                                }
                                Ok(Err(e)) => {
                                    tracing::debug!(%peer_id, "gov: initial group exchange failed: {e}");
                                }
                                Err(_) => {
                                    tracing::debug!(%peer_id, "gov: initial group exchange: channel dropped");
                                }
                            }
                        });
                    }
                    Ok(SwarmEvent2::PeerDisconnected { peer_id }) => {
                        tracing::info!(%peer_id, "gov: peer disconnected");
                        pool.remove(&peer_id).await;
                        governor.lock().await.mark_disconnected(&peer_id);
                    }
                    Ok(SwarmEvent2::PingRtt { peer_id, rtt_ms }) => {
                        tracing::trace!(%peer_id, rtt_ms, "gov: ping rtt");
                        governor.lock().await.record_activity(&peer_id, Some(rtt_ms));
                        pool.update_rtt(&peer_id, rtt_ms).await;
                    }
                    Ok(SwarmEvent2::IdentifyReceived {
                        peer_id,
                        listen_addrs,
                        ..
                    }) => {
                        let mut gov = governor.lock().await;

                        // Check if this peer's listen addrs match a seeded
                        // bootnode placeholder. Only match Cold peers (placeholders
                        // are never marked connected) to avoid clobbering real peers
                        // behind the same NAT.
                        let placeholder = gov
                            .all_peers()
                            .filter(|p| {
                                p.node_id != peer_id
                                    && matches!(
                                        p.state,
                                        cordelia_governor::PeerState::Cold
                                    )
                            })
                            .find(|p| {
                                p.addrs.iter().any(|a| listen_addrs.contains(a))
                            })
                            .map(|p| p.node_id);

                        if let Some(old_id) = placeholder {
                            gov.replace_node_id(&old_id, peer_id, vec![]);
                            tracing::info!(
                                %peer_id,
                                old = %old_id,
                                "replaced bootnode placeholder via identify"
                            );
                        }

                        // Update peer addresses from identify.
                        // Filter: always remove loopback. If the peer announces
                        // both public and private addresses, keep only public
                        // (the private ones are container-internal). If only
                        // private, keep them (LAN peer).
                        let filtered_addrs = filter_identify_addrs(listen_addrs);

                        if !filtered_addrs.is_empty() {
                            if let Some(handle) = pool.get(&peer_id).await {
                                if handle.addrs != filtered_addrs {
                                    tracing::info!(
                                        %peer_id,
                                        old = ?handle.addrs,
                                        new = ?filtered_addrs,
                                        "gov: peer addresses updated via identify"
                                    );
                                    let groups = handle.groups.clone();
                                    gov.add_peer(
                                        peer_id,
                                        filtered_addrs.clone(),
                                        groups,
                                    );
                                    pool.update_addrs(&peer_id, filtered_addrs).await;
                                }
                            }
                        }
                    }
                    Ok(SwarmEvent2::DialFailure { peer_id }) => {
                        if let Some(peer_id) = peer_id {
                            tracing::debug!(%peer_id, "gov: dial failure, marking failed");
                            governor.lock().await.mark_dial_failed(&peer_id);
                        }
                    }
                    Ok(SwarmEvent2::ExternalAddrConfirmed { addr }) => {
                        tracing::info!(%addr, "external address confirmed");
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("governor event receiver lagged by {n}");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }

            _ = shutdown.recv() => {
                let (warm, hot) = pool.peer_count_by_state().await;
                tracing::info!(warm, hot, ticks = tick_count, "gov: shutting down");
                return;
            }
        }
    }
}

/// Ask a random connected relay peer for its known peers, then register
/// any genuinely new ones in the governor. Skips our own PeerId and
/// peers already in a connected state (Warm/Hot).
/// Maximum number of relays to query per discovery round.
const GOSSIP_FAN_OUT: usize = 3;

async fn discover_peers(
    pool: &PeerPool,
    governor: &Arc<Mutex<Governor>>,
    cmd_tx: &mpsc::Sender<SwarmCommand>,
    our_peer_id: PeerId,
) {
    let mut relays = pool.relay_peers().await;
    if relays.is_empty() {
        return;
    }

    // Shuffle and pick up to GOSSIP_FAN_OUT relays for broader discovery
    use rand::seq::SliceRandom;
    relays.shuffle(&mut rand::thread_rng());
    let targets: Vec<_> = relays.into_iter().take(GOSSIP_FAN_OUT).collect();

    // Query all target relays concurrently
    let mut futs = Vec::with_capacity(targets.len());
    for relay in &targets {
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        tracing::debug!(relay = %relay.node_id, "gov: requesting peer share");
        if let Err(e) = cmd_tx
            .send(SwarmCommand::SendPeerShareRequest {
                peer: relay.node_id,
                request: cordelia_protocol::messages::PeerShareRequest { max_peers: 20 },
                response_tx: resp_tx,
            })
            .await
        {
            tracing::warn!(relay = %relay.node_id, "gov: peer discovery send failed: {e}");
            continue;
        }
        futs.push((relay.node_id, resp_rx));
    }

    // Collect all responses
    let mut all_peers = Vec::new();
    for (relay_id, resp_rx) in futs {
        match resp_rx.await {
            Ok(Ok(resp)) => all_peers.extend(resp.peers),
            Ok(Err(e)) => {
                tracing::debug!(relay = %relay_id, "peer discovery failed: {e}");
            }
            Err(_) => {
                tracing::debug!(relay = %relay_id, "peer discovery: response channel dropped");
            }
        }
    }

    if all_peers.is_empty() {
        return;
    }

    let mut gov = governor.lock().await;
    let mut added = 0u32;
    for pa in &all_peers {
        let peer_id: PeerId = match pa.peer_id.parse() {
            Ok(id) => id,
            Err(_) => continue,
        };

        // Never add ourselves
        if peer_id == our_peer_id {
            continue;
        }

        // Skip peers already in a connected state (Warm/Hot) -- only add
        // genuinely new or Cold peers to avoid re-creating cold entries
        // for peers we're already talking to.
        if let Some(existing) = gov.peer_info(&peer_id) {
            if existing.state.is_active() {
                continue;
            }
        }

        let addrs: Vec<Multiaddr> = pa.addrs.iter().filter_map(|a| a.parse().ok()).collect();
        if addrs.is_empty() {
            continue;
        }
        gov.add_peer(peer_id, addrs, pa.groups.clone());
        if pa.role == "relay" {
            gov.set_peer_relay(&peer_id, true);
        }
        added += 1;
    }
    drop(gov);

    if added > 0 {
        tracing::info!(
            relays_queried = targets.len(),
            discovered = added,
            "peer discovery: registered new peers via gossip"
        );
    }
}

/// Parse a bootnode address string into a Multiaddr.
/// Supports Multiaddr format (/ip4/.../tcp/...), IP:port, and hostname:port
/// (resolved via async DNS).
async fn parse_bootnode_multiaddr(boot: &BootnodeEntry) -> Option<Multiaddr> {
    // Try Multiaddr first
    if let Ok(addr) = boot.addr.parse::<Multiaddr>() {
        return Some(addr);
    }

    // Try IP:port (no DNS needed)
    if let Ok(socket_addr) = boot.addr.parse::<std::net::SocketAddr>() {
        return format!("/ip4/{}/tcp/{}", socket_addr.ip(), socket_addr.port())
            .parse()
            .ok();
    }

    // Async DNS resolution for hostname:port
    match tokio::net::lookup_host(&boot.addr).await {
        Ok(mut addrs) => {
            let resolved = addrs.next()?;
            format!("/ip4/{}/tcp/{}", resolved.ip(), resolved.port())
                .parse()
                .ok()
        }
        Err(e) => {
            tracing::debug!(addr = &boot.addr, "DNS lookup failed: {e}");
            None
        }
    }
}

/// Seed a resolved bootnode into the governor as a cold relay peer.
/// Uses a deterministic PeerId derived from the address (replaced on handshake).
fn seed_bootnode(gov: &mut Governor, bootnode_addr: &str, addr: Multiaddr) {
    // Generate a deterministic PeerId from the address hash.
    // This gets replaced with the real PeerId on first connect (via identify).
    let hash = cordelia_crypto::sha256_hex(bootnode_addr.as_bytes());
    let hash_bytes = hex::decode(&hash).unwrap_or_default();
    let mut seed = [0u8; 32];
    let len = seed.len().min(hash_bytes.len());
    seed[..len].copy_from_slice(&hash_bytes[..len]);

    let keypair = libp2p::identity::Keypair::ed25519_from_bytes(seed).expect("valid ed25519 seed");
    let placeholder_id = PeerId::from(keypair.public());

    gov.add_peer(placeholder_id, vec![addr.clone()], vec![]);
    gov.set_peer_relay(&placeholder_id, true);
    tracing::info!(bootnode = bootnode_addr, addr = %addr, "seeded bootnode (relay)");
}

/// Filter identify listen_addrs for storage in governor/pool.
///
/// Rules:
///   1. Always remove loopback (127.x.x.x).
///   2. If the peer announces both public and private (RFC1918) addresses,
///      keep only public -- the private ones are container-internal or
///      behind NAT and unreachable from outside.
///   3. If the peer announces only private addresses, keep them all --
///      it's a LAN peer and those addresses are how we reach it.
fn filter_identify_addrs(addrs: Vec<Multiaddr>) -> Vec<Multiaddr> {
    // Remove loopback first
    let non_loopback: Vec<Multiaddr> = addrs.into_iter().filter(|a| !is_loopback_addr(a)).collect();

    // Check if any address is public (non-RFC1918)
    let has_public = non_loopback.iter().any(is_public_addr);

    if has_public {
        // Keep only public addresses (discard container-internal RFC1918)
        non_loopback.into_iter().filter(is_public_addr).collect()
    } else {
        // LAN-only peer: keep all non-loopback addresses
        non_loopback
    }
}

fn is_loopback_addr(addr: &Multiaddr) -> bool {
    addr.iter()
        .any(|proto| matches!(proto, libp2p::multiaddr::Protocol::Ip4(ip) if ip.is_loopback()))
}

/// True if the address contains no RFC1918 IPv4 components.
fn is_public_addr(addr: &Multiaddr) -> bool {
    for proto in addr.iter() {
        if let libp2p::multiaddr::Protocol::Ip4(ip) = proto {
            if ip.is_private() {
                return false;
            }
        }
    }
    true
}
