//! Governor background task -- ticks every 10s, manages peer lifecycle.
//!
//! On each tick:
//!   1. governor.tick() -> GovernorActions
//!   2. Connect actions: dial peer, handshake, register in pool
//!   3. Disconnect/demote: close connection, update pool
//!   4. On startup: seed bootnodes as cold, attempt initial connections

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use cordelia_governor::Governor;
use cordelia_storage::Storage;
use tokio::sync::{broadcast, Mutex};

use crate::config::BootnodeEntry;
use crate::peer_pool::PeerPool;
use crate::quic_transport::QuicTransport;

#[allow(clippy::too_many_arguments)]
/// Run the governor loop until shutdown.
pub async fn run_governor_loop(
    governor: Arc<Mutex<Governor>>,
    pool: PeerPool,
    transport: Arc<QuicTransport>,
    storage: Arc<dyn Storage>,
    bootnodes: Vec<BootnodeEntry>,
    our_node_id: [u8; 32],
    our_groups: Vec<String>,
    our_role: String,
    mut shutdown: broadcast::Receiver<()>,
    shutdown_tx: broadcast::Sender<()>,
) {
    if bootnodes.is_empty() {
        tracing::warn!("no bootnodes configured -- node will only accept inbound connections");
    }

    // Seed bootnodes as cold peers (resolve DNS hostnames to IPs)
    let mut unresolved_bootnodes: Vec<BootnodeEntry> = Vec::new();
    {
        let mut gov = governor.lock().await;
        for boot in &bootnodes {
            match resolve_bootnode(boot) {
                Some(addr) => {
                    seed_bootnode(&mut gov, &boot.addr, addr);
                }
                None => {
                    tracing::warn!(bootnode = &boot.addr, "failed to resolve bootnode address");
                    unresolved_bootnodes.push(boot.clone());
                }
            }
        }
    }

    let tick_interval = cordelia_governor::TICK_INTERVAL;
    let mut tick_count: u64 = 0;
    // Exchange groups every 6 ticks (60s at 10s tick interval)
    const GROUP_EXCHANGE_EVERY: u64 = 6;
    // Retry unresolved bootnodes every 30 ticks (5 minutes)
    const BOOTNODE_RETRY_EVERY: u64 = 30;

    loop {
        tokio::select! {
            _ = tokio::time::sleep(tick_interval) => {}
            _ = shutdown.recv() => {
                tracing::info!("governor shutting down");
                return;
            }
        }

        let actions = {
            let mut gov = governor.lock().await;
            gov.tick()
        };

        // Apply state transitions to pool
        pool.apply_governor_actions(&actions).await;

        // Log transitions
        for (node_id, from, to) in &actions.transitions {
            tracing::debug!(
                peer = hex::encode(node_id),
                from,
                to,
                "peer state transition"
            );
        }

        // Connect to promoted peers
        for node_id in &actions.connect {
            let node_id = *node_id;
            let shutdown_tx = shutdown_tx.clone();
            let addr = {
                let gov = governor.lock().await;
                gov.peer_info(&node_id)
                    .and_then(|p| p.addrs.first().copied())
            };

            if let Some(addr) = addr {
                let transport = transport.clone();
                let pool = pool.clone();
                let governor = governor.clone();
                let storage = storage.clone();
                let our_groups = our_groups.clone();
                let our_role = our_role.clone();

                tokio::spawn(async move {
                    match transport.dial(addr).await {
                        Ok(conn) => {
                            match crate::quic_transport::outbound_handshake(
                                &conn,
                                &pool,
                                our_node_id,
                                &our_groups,
                            )
                            .await
                            {
                                Ok(peer_id) => {
                                    // Immediate group exchange after handshake (R3-024 fix)
                                    if let Ok(fresh_groups) =
                                        crate::mini_protocols::request_group_exchange(
                                            &conn,
                                            &our_groups,
                                        )
                                        .await
                                    {
                                        pool.update_peer_groups(&peer_id, fresh_groups).await;
                                    }

                                    // Get the peer's advertised groups from the pool
                                    let peer_groups = pool
                                        .get(&peer_id)
                                        .await
                                        .map(|h| h.groups.clone())
                                        .unwrap_or_default();

                                    let mut gov = governor.lock().await;
                                    // Replace fake bootnode ID with real handshake ID
                                    // relay flag is preserved by replace_node_id
                                    let was_relay = gov
                                        .peer_info(&node_id)
                                        .map(|p| p.is_relay)
                                        .unwrap_or(false);
                                    if node_id != peer_id {
                                        gov.replace_node_id(&node_id, peer_id, peer_groups.clone());
                                    } else {
                                        // Update groups even if ID matches
                                        gov.add_peer(peer_id, vec![addr], peer_groups);
                                        // Preserve relay flag for bootnodes
                                        if was_relay {
                                            gov.set_peer_relay(&peer_id, true);
                                        }
                                    }
                                    gov.mark_connected(&peer_id);
                                    drop(gov);

                                    // Mark as relay in pool too
                                    if was_relay {
                                        pool.set_relay(&peer_id, true).await;
                                    }

                                    tracing::info!(
                                        peer = hex::encode(peer_id),
                                        addr = %addr,
                                        "connected to peer"
                                    );

                                    // Spawn keepalive loop to keep governor activity fresh
                                    let ka_conn = conn.clone();
                                    let ka_gov = governor.clone();
                                    let ka_shutdown = shutdown_tx.subscribe();
                                    tokio::spawn(async move {
                                        crate::mini_protocols::run_keepalive_loop(
                                            &ka_conn,
                                            &peer_id,
                                            &ka_gov,
                                            ka_shutdown,
                                        )
                                        .await;
                                    });

                                    // Spawn connection handler for the dialled peer
                                    let pool2 = pool.clone();
                                    let storage2 = storage.clone();
                                    let groups2 = our_groups.clone();
                                    let role2 = our_role.clone();
                                    let gov2 = governor.clone();
                                    tokio::spawn(async move {
                                        crate::quic_transport::run_connection(
                                            conn,
                                            peer_id,
                                            pool2,
                                            storage2,
                                            our_node_id,
                                            groups2,
                                            role2,
                                            Some(gov2),
                                            false,
                                        )
                                        .await;
                                    });
                                }
                                Err(e) => {
                                    tracing::warn!(addr = %addr, "handshake failed: {e}");
                                }
                            }
                        }
                        Err(e) => {
                            // Track dial failure for exponential backoff
                            let mut gov = governor.lock().await;
                            gov.mark_dial_failed(&node_id);
                            drop(gov);
                            tracing::warn!(addr = %addr, "dial failed: {e}");
                        }
                    }
                });
            }
        }

        let (warm, hot) = pool.peer_count_by_state().await;
        tracing::debug!(warm, hot, tick = tick_count, "governor tick complete");

        // Retry unresolved bootnodes periodically
        tick_count += 1;
        if !unresolved_bootnodes.is_empty() && tick_count.is_multiple_of(BOOTNODE_RETRY_EVERY) {
            let mut gov = governor.lock().await;
            unresolved_bootnodes.retain(|boot| {
                match resolve_bootnode(boot) {
                    Some(addr) => {
                        seed_bootnode(&mut gov, &boot.addr, addr);
                        tracing::info!(bootnode = &boot.addr, resolved = %addr, "resolved previously unresolved bootnode");
                        false // remove from unresolved list
                    }
                    None => {
                        tracing::debug!(bootnode = &boot.addr, "bootnode still unresolvable");
                        true // keep in unresolved list
                    }
                }
            });
        }

        // Periodic group exchange with hot peers
        if tick_count.is_multiple_of(GROUP_EXCHANGE_EVERY) {
            let hot_peers = pool.active_peers().await;
            let groups = our_groups.clone();
            for peer in hot_peers {
                let groups = groups.clone();
                let pool = pool.clone();
                tokio::spawn(async move {
                    match crate::mini_protocols::request_group_exchange(&peer.connection, &groups)
                        .await
                    {
                        Ok(peer_groups) => {
                            let old_intersection = peer.group_intersection.clone();
                            pool.update_peer_groups(&peer.node_id, peer_groups).await;
                            let new_handle = pool.get(&peer.node_id).await;
                            if let Some(h) = new_handle {
                                if h.group_intersection != old_intersection {
                                    tracing::info!(
                                        peer = hex::encode(peer.node_id),
                                        old = ?old_intersection,
                                        new = ?h.group_intersection,
                                        "group intersection updated"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            tracing::debug!(
                                peer = hex::encode(peer.node_id),
                                "group exchange failed: {e}"
                            );
                        }
                    }
                });
            }
        }
    }
}

/// Try to resolve a bootnode address to a SocketAddr.
fn resolve_bootnode(boot: &BootnodeEntry) -> Option<SocketAddr> {
    boot.addr.parse::<SocketAddr>().ok().or_else(|| {
        boot.addr
            .to_socket_addrs()
            .ok()
            .and_then(|mut addrs| addrs.next())
    })
}

/// Seed a resolved bootnode into the governor as a cold relay peer.
fn seed_bootnode(gov: &mut Governor, bootnode_addr: &str, addr: SocketAddr) {
    let mut id = [0u8; 32];
    let addr_bytes = addr.to_string();
    let hash = cordelia_crypto::sha256_hex(addr_bytes.as_bytes());
    let hash_bytes = hex::decode(&hash).unwrap_or_default();
    let len = id.len().min(hash_bytes.len());
    id[..len].copy_from_slice(&hash_bytes[..len]);
    gov.add_peer(id, vec![addr], vec![]);
    gov.set_peer_relay(&id, true);
    tracing::info!(bootnode = bootnode_addr, resolved = %addr, "seeded bootnode (relay)");
}
