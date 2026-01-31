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

/// Run the governor loop until shutdown.
pub async fn run_governor_loop(
    governor: Arc<Mutex<Governor>>,
    pool: PeerPool,
    transport: Arc<QuicTransport>,
    storage: Arc<dyn Storage>,
    bootnodes: Vec<BootnodeEntry>,
    our_node_id: [u8; 32],
    our_groups: Vec<String>,
    mut shutdown: broadcast::Receiver<()>,
    shutdown_tx: broadcast::Sender<()>,
) {
    // Seed bootnodes as cold peers (resolve DNS hostnames to IPs)
    {
        let mut gov = governor.lock().await;
        for boot in &bootnodes {
            // Try direct SocketAddr parse first, then DNS resolution
            let addr = boot.addr.parse::<SocketAddr>().ok().or_else(|| {
                boot.addr
                    .to_socket_addrs()
                    .ok()
                    .and_then(|mut addrs| addrs.next())
            });

            if let Some(addr) = addr {
                let mut id = [0u8; 32];
                let addr_bytes = addr.to_string();
                let hash = cordelia_crypto::sha256_hex(addr_bytes.as_bytes());
                let hash_bytes = hex::decode(&hash).unwrap_or_default();
                let len = id.len().min(hash_bytes.len());
                id[..len].copy_from_slice(&hash_bytes[..len]);
                gov.add_peer(id, vec![addr], vec![]);
                tracing::info!(bootnode = &boot.addr, resolved = %addr, "seeded bootnode");
            } else {
                tracing::warn!(bootnode = &boot.addr, "failed to resolve bootnode address");
            }
        }
    }

    let tick_interval = cordelia_governor::TICK_INTERVAL;

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
                                    // Get the peer's advertised groups from the pool
                                    let peer_groups = pool
                                        .get(&peer_id)
                                        .await
                                        .map(|h| h.groups.clone())
                                        .unwrap_or_default();

                                    let mut gov = governor.lock().await;
                                    // Replace fake bootnode ID with real handshake ID
                                    if node_id != peer_id {
                                        gov.replace_node_id(&node_id, peer_id, peer_groups.clone());
                                    } else {
                                        // Update groups even if ID matches
                                        gov.add_peer(peer_id, vec![addr], peer_groups);
                                    }
                                    gov.mark_connected(&peer_id);
                                    drop(gov);

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
                                            &ka_conn, &peer_id, &ka_gov, ka_shutdown,
                                        )
                                        .await;
                                    });

                                    // Spawn connection handler for the dialled peer
                                    let pool2 = pool.clone();
                                    let storage2 = storage.clone();
                                    let groups2 = our_groups.clone();
                                    let gov2 = governor.clone();
                                    tokio::spawn(async move {
                                        crate::quic_transport::run_connection(
                                            conn, peer_id, pool2, storage2, our_node_id, groups2, Some(gov2), false,
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
                            tracing::warn!(addr = %addr, "dial failed: {e}");
                        }
                    }
                });
            }
        }

        let (warm, hot) = pool.peer_count_by_state().await;
        tracing::debug!(warm, hot, "governor tick complete");
    }
}
