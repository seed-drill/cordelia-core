//! Swarm task -- owns the libp2p Swarm, dispatches events via channels.
//!
//! All network I/O flows through this task. Governor and replication tasks
//! communicate via SwarmCommand/SwarmEvent channels.

use cordelia_crypto::identity::NodeIdentity;
use cordelia_protocol::messages::*;
use cordelia_replication::{ReceiveOutcome, ReplicationEngine};
use cordelia_storage::Storage;
use libp2p::futures::StreamExt;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::SwarmEvent;
use libp2p::{identity, Multiaddr, PeerId, StreamProtocol, Swarm};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot, RwLock};

// ============================================================================
// Group descriptor helpers (R4-030)
// ============================================================================

/// Build GroupDescriptors from local storage for all our groups.
/// Lazy-signs owned groups that don't have a signature yet.
fn build_descriptors(
    storage: &dyn Storage,
    node_identity: &NodeIdentity,
    our_entity_id: &str,
) -> Option<Vec<GroupDescriptor>> {
    match storage.list_groups() {
        Ok(groups) if !groups.is_empty() => {
            let pubkey_hex = hex::encode(node_identity.public_key());
            let descs: Vec<GroupDescriptor> = groups
                .into_iter()
                .map(|g| {
                    let checksum = GroupDescriptor::compute_checksum(&g.id, &g.culture);
                    let mut desc = GroupDescriptor {
                        id: g.id.clone(),
                        culture: g.culture,
                        updated_at: g.updated_at,
                        checksum,
                        owner_id: g.owner_id.clone(),
                        owner_pubkey: g.owner_pubkey.clone(),
                        signature: g.signature.clone(),
                    };

                    // Lazy-sign: if we own this group and it's unsigned, sign it now
                    let we_own_it = g.owner_id.as_deref() == Some(our_entity_id)
                        || (g.owner_id.is_none() && is_owner(storage, &g.id, our_entity_id));

                    if we_own_it && g.signature.is_none() {
                        let sig = hex::encode(node_identity.sign(&desc.signing_payload()));
                        desc.owner_id = Some(our_entity_id.to_string());
                        desc.owner_pubkey = Some(pubkey_hex.clone());
                        desc.signature = Some(sig.clone());

                        // Persist so we don't re-sign every exchange
                        if let Err(e) =
                            storage.write_group_signature(&g.id, our_entity_id, &pubkey_hex, &sig)
                        {
                            tracing::warn!(
                                group_id = %g.id,
                                "failed to persist group signature: {e}"
                            );
                        } else {
                            tracing::info!(
                                group_id = %g.id,
                                "net: lazy-signed group descriptor"
                            );
                        }
                    }

                    desc
                })
                .collect();
            Some(descs)
        }
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("failed to list groups for descriptors: {e}");
            None
        }
    }
}

/// Check if entity is owner of group via group_members table.
fn is_owner(storage: &dyn Storage, group_id: &str, entity_id: &str) -> bool {
    storage
        .get_membership(group_id, entity_id)
        .ok()
        .flatten()
        .is_some_and(|m| m.role == "owner")
}

/// Merge incoming descriptors into local storage (LWW by updated_at).
/// Verifies checksum and signature before accepting. Returns count of upserted groups.
fn merge_descriptors(storage: &dyn Storage, descriptors: &[GroupDescriptor]) -> usize {
    let mut upserted = 0;
    for desc in descriptors {
        // Verify checksum integrity
        if !desc.verify_checksum() {
            tracing::warn!(
                group_id = %desc.id,
                "net: rejecting group descriptor with bad checksum"
            );
            continue;
        }

        // Culture size limit (4KB as per R4-030)
        if desc.culture.len() > 4096 {
            tracing::warn!(
                group_id = %desc.id,
                culture_len = desc.culture.len(),
                "net: rejecting group descriptor with oversized culture"
            );
            continue;
        }

        // Verify signature if present
        if let (Some(ref pubkey_hex), Some(ref sig_hex)) = (&desc.owner_pubkey, &desc.signature) {
            if !verify_descriptor_signature(desc, pubkey_hex, sig_hex) {
                tracing::warn!(
                    group_id = %desc.id,
                    "net: rejecting group descriptor with invalid signature"
                );
                continue;
            }
        }

        // If we already know this group and it has a signature, reject unsigned updates
        if let Ok(Some(ref local)) = storage.read_group(&desc.id) {
            if local.signature.is_some() && desc.signature.is_none() {
                tracing::warn!(
                    group_id = %desc.id,
                    "net: rejecting unsigned descriptor for signed group"
                );
                continue;
            }

            // If both signed, ensure same owner (prevent owner hijack)
            if let (Some(ref local_owner), Some(ref incoming_owner)) =
                (&local.owner_pubkey, &desc.owner_pubkey)
            {
                if local_owner != incoming_owner {
                    tracing::warn!(
                        group_id = %desc.id,
                        "net: rejecting descriptor with different owner pubkey"
                    );
                    continue;
                }
            }
        }

        // LWW: only upsert if incoming is newer than local
        // Also capture local name (name is not on wire -- portal distributes it out-of-band)
        let local_name;
        match storage.read_group(&desc.id) {
            Ok(Some(local)) if local.updated_at >= desc.updated_at => {
                tracing::trace!(
                    group_id = %desc.id,
                    "net: skipping group descriptor (local is same or newer)"
                );
                continue;
            }
            Ok(Some(local)) => {
                local_name = local.name.clone();
                // Check for eagerness increase (soft downgrade policy)
                if eagerness_increased(&local.culture, &desc.culture) {
                    tracing::info!(
                        group_id = %desc.id,
                        "net: group culture eagerness increased via replication (allowed, soft policy)"
                    );
                }
            }
            _ => {
                // No local copy -- use group ID as placeholder name
                // (portal will provide display name during enrollment)
                local_name = desc.id.clone();
            }
        }

        match storage.write_group(&desc.id, &local_name, &desc.culture, "{}") {
            Ok(_) => {
                // Persist signature fields if present
                if let (Some(ref oid), Some(ref pk), Some(ref sig)) =
                    (&desc.owner_id, &desc.owner_pubkey, &desc.signature)
                {
                    let _ = storage.write_group_signature(&desc.id, oid, pk, sig);
                }
                tracing::debug!(
                    group_id = %desc.id,
                    signed = desc.signature.is_some(),
                    "net: merged group descriptor from peer"
                );
                upserted += 1;
            }
            Err(e) => {
                tracing::warn!(
                    group_id = %desc.id,
                    "net: failed to write group descriptor: {e}"
                );
            }
        }
    }
    upserted
}

/// Verify an Ed25519 signature on a group descriptor.
fn verify_descriptor_signature(desc: &GroupDescriptor, pubkey_hex: &str, sig_hex: &str) -> bool {
    let Ok(pubkey_bytes) = hex::decode(pubkey_hex) else {
        return false;
    };
    let Ok(sig_bytes) = hex::decode(sig_hex) else {
        return false;
    };
    let payload = desc.signing_payload();
    let public_key =
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &pubkey_bytes);
    public_key.verify(&payload, &sig_bytes).is_ok()
}

/// Check if broadcast_eagerness increased (informational only, soft policy).
/// "moderate" is treated as chatty (deprecated, maps to eager).
fn eagerness_increased(old_culture: &str, new_culture: &str) -> bool {
    fn parse_eagerness(culture: &str) -> u8 {
        if culture.contains("\"eager\"") || culture.contains("\"chatty\"") || culture.contains("\"moderate\"") {
            2
        } else {
            0
        } // passive/taciturn or unknown
    }
    parse_eagerness(new_culture) > parse_eagerness(old_culture)
}

// ============================================================================
// Behaviour definition
// ============================================================================

#[derive(libp2p::swarm::NetworkBehaviour)]
pub struct CordeliaBehaviour {
    pub ping: libp2p::ping::Behaviour,
    pub identify: libp2p::identify::Behaviour,
    pub peer_share: request_response::json::Behaviour<PeerShareRequest, PeerShareResponse>,
    pub memory_sync: request_response::json::Behaviour<SyncRequest, SyncResponse>,
    pub memory_fetch: request_response::json::Behaviour<FetchRequest, FetchResponse>,
    pub memory_push: request_response::json::Behaviour<MemoryPushRequest, PushAck>,
    pub group_exchange: request_response::json::Behaviour<GroupExchange, GroupExchangeResponse>,
}

// ============================================================================
// Commands (inbound to swarm task)
// ============================================================================

pub enum SwarmCommand {
    Dial(Multiaddr),
    Disconnect(PeerId),
    SendPeerShareRequest {
        peer: PeerId,
        request: PeerShareRequest,
        response_tx: oneshot::Sender<Result<PeerShareResponse, String>>,
    },
    SendSyncRequest {
        peer: PeerId,
        request: SyncRequest,
        response_tx: oneshot::Sender<Result<SyncResponse, String>>,
    },
    SendFetchRequest {
        peer: PeerId,
        request: FetchRequest,
        response_tx: oneshot::Sender<Result<FetchResponse, String>>,
    },
    SendMemoryPush {
        peer: PeerId,
        request: MemoryPushRequest,
    },
    SendGroupExchange {
        peer: PeerId,
        request: GroupExchange,
        response_tx: oneshot::Sender<Result<GroupExchangeResponse, String>>,
    },
}

// ============================================================================
// Events (outbound from swarm task)
// ============================================================================

#[derive(Debug, Clone)]
pub enum SwarmEvent2 {
    PeerConnected {
        peer_id: PeerId,
        addrs: Vec<Multiaddr>,
    },
    PeerDisconnected {
        peer_id: PeerId,
    },
    PingRtt {
        peer_id: PeerId,
        rtt_ms: f64,
    },
    IdentifyReceived {
        peer_id: PeerId,
        listen_addrs: Vec<Multiaddr>,
        #[allow(dead_code)]
        observed_addr: Multiaddr,
        #[allow(dead_code)]
        agent_version: String,
    },
    ExternalAddrConfirmed {
        addr: Multiaddr,
    },
    DialFailure {
        peer_id: Option<PeerId>,
    },
}

// ============================================================================
// Build swarm
// ============================================================================

pub fn build_swarm(
    keypair: identity::Keypair,
    listen_addr: Multiaddr,
) -> Result<Swarm<CordeliaBehaviour>, Box<dyn std::error::Error + Send + Sync>> {
    let peer_id = PeerId::from(keypair.public());

    let behaviour = CordeliaBehaviour {
        ping: libp2p::ping::Behaviour::new(
            libp2p::ping::Config::new().with_interval(Duration::from_secs(15)),
        ),
        identify: libp2p::identify::Behaviour::new(libp2p::identify::Config::new(
            "/cordelia/id/1".into(),
            keypair.public(),
        )),
        peer_share: request_response::json::Behaviour::new(
            [(
                StreamProtocol::new("/cordelia/peer-share/1"),
                ProtocolSupport::Full,
            )],
            request_response::Config::default(),
        ),
        memory_sync: request_response::json::Behaviour::new(
            [(
                StreamProtocol::new("/cordelia/memory-sync/1"),
                ProtocolSupport::Full,
            )],
            request_response::Config::default(),
        ),
        memory_fetch: request_response::json::Behaviour::new(
            [(
                StreamProtocol::new("/cordelia/memory-fetch/1"),
                ProtocolSupport::Full,
            )],
            request_response::Config::default(),
        ),
        memory_push: request_response::json::Behaviour::new(
            [(
                StreamProtocol::new("/cordelia/memory-push/1"),
                ProtocolSupport::Full,
            )],
            request_response::Config::default(),
        ),
        group_exchange: request_response::json::Behaviour::new(
            [(
                StreamProtocol::new("/cordelia/group-exchange/1"),
                ProtocolSupport::Full,
            )],
            request_response::Config::default(),
        ),
    };

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            libp2p::tcp::Config::default(),
            libp2p::noise::Config::new,
            libp2p::yamux::Config::default,
        )?
        .with_dns()?
        .with_behaviour(|_| behaviour)?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(120)))
        .build();

    swarm.listen_on(listen_addr)?;

    tracing::info!(%peer_id, "swarm built");
    Ok(swarm)
}

// ============================================================================
// Run loop
// ============================================================================

/// Run the swarm event loop. Handles commands from governor/replication and
/// dispatches inbound requests to storage.
///
/// `relay_accepted_groups`: for relay nodes, the set of groups this relay accepts.
///   For transparent relays this is None (accept all). For dynamic/explicit, it's the
///   computed acceptance set. Non-relay nodes pass None.
/// `relay_blocked_groups`: deny-list applied on top of any posture.
#[allow(clippy::too_many_arguments)]
pub async fn run_swarm_loop(
    mut swarm: Swarm<CordeliaBehaviour>,
    mut cmd_rx: mpsc::Receiver<SwarmCommand>,
    event_tx: broadcast::Sender<SwarmEvent2>,
    storage: Arc<dyn Storage>,
    shared_groups: Arc<RwLock<Vec<String>>>,
    pool: crate::peer_pool::PeerPool,
    our_role: crate::config::NodeRole,
    relay_posture: Option<crate::config::RelayPosture>,
    relay_accepted_groups: Option<Arc<RwLock<HashSet<String>>>>,
    relay_blocked_groups: Arc<HashSet<String>>,
    node_identity: Arc<NodeIdentity>,
    our_entity_id: String,
    mut shutdown: broadcast::Receiver<()>,
) {
    // Track pending outbound request-response channels
    type ReqId = request_response::OutboundRequestId;
    let mut pending_peer_share: HashMap<ReqId, oneshot::Sender<Result<PeerShareResponse, String>>> =
        HashMap::new();
    let mut pending_sync: HashMap<ReqId, oneshot::Sender<Result<SyncResponse, String>>> =
        HashMap::new();
    let mut pending_fetch: HashMap<ReqId, oneshot::Sender<Result<FetchResponse, String>>> =
        HashMap::new();
    let mut pending_group_exchange: HashMap<
        ReqId,
        oneshot::Sender<Result<GroupExchangeResponse, String>>,
    > = HashMap::new();

    loop {
        tokio::select! {
            // Process commands from governor/replication tasks
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    SwarmCommand::Dial(addr) => {
                        tracing::debug!(%addr, "net: dialling");
                        if let Err(e) = swarm.dial(addr.clone()) {
                            tracing::warn!(%addr, "net: dial failed: {e}");
                        }
                    }
                    SwarmCommand::Disconnect(peer_id) => {
                        tracing::debug!(%peer_id, "net: disconnecting peer");
                        if let Err(e) = swarm.disconnect_peer_id(peer_id) {
                            tracing::warn!(%peer_id, "net: disconnect failed: {e:?}");
                        }
                    }
                    SwarmCommand::SendPeerShareRequest { peer, request, response_tx } => {
                        tracing::debug!(%peer, max_peers = request.max_peers, "net: sending peer share request");
                        let req_id = swarm.behaviour_mut().peer_share.send_request(&peer, request);
                        pending_peer_share.insert(req_id, response_tx);
                    }
                    SwarmCommand::SendSyncRequest { peer, request, response_tx } => {
                        tracing::debug!(%peer, group = request.group_id, "net: sending sync request");
                        let req_id = swarm.behaviour_mut().memory_sync.send_request(&peer, request);
                        pending_sync.insert(req_id, response_tx);
                    }
                    SwarmCommand::SendFetchRequest { peer, request, response_tx } => {
                        tracing::debug!(%peer, items = request.item_ids.len(), "net: sending fetch request");
                        let req_id = swarm.behaviour_mut().memory_fetch.send_request(&peer, request);
                        pending_fetch.insert(req_id, response_tx);
                    }
                    SwarmCommand::SendMemoryPush { peer, request } => {
                        tracing::debug!(%peer, items = request.items.len(), "net: sending push");
                        swarm.behaviour_mut().memory_push.send_request(&peer, request);
                    }
                    SwarmCommand::SendGroupExchange { peer, mut request, response_tx } => {
                        // Enrich with descriptors from local storage (R4-030)
                        if request.descriptors.is_none() {
                            request.descriptors = build_descriptors(storage.as_ref(), &node_identity, &our_entity_id);
                        }
                        tracing::debug!(%peer, our_groups = request.groups.len(), descriptors = request.descriptors.as_ref().map_or(0, |d| d.len()), "net: sending group exchange");
                        let req_id = swarm.behaviour_mut().group_exchange.send_request(&peer, request);
                        pending_group_exchange.insert(req_id, response_tx);
                    }
                }
            }

            // Process swarm events
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        tracing::info!(%address, "listening");
                    }
                    SwarmEvent::ConnectionEstablished {
                        peer_id,
                        endpoint,
                        num_established,
                        ..
                    } => {
                        let addr = endpoint.get_remote_address().clone();
                        let direction = if endpoint.is_dialer() { "outbound" } else { "inbound" };
                        let conns = num_established.get();
                        tracing::info!(
                            %peer_id,
                            %addr,
                            direction,
                            connections = conns,
                            "net: connection established"
                        );
                        // Only emit connect for the first connection to this peer
                        if conns == 1 {
                            if let Err(e) = event_tx.send(SwarmEvent2::PeerConnected {
                                peer_id,
                                addrs: vec![addr],
                            }) {
                                tracing::warn!(%peer_id, "net: failed to send PeerConnected event: {e}");
                            }
                        }
                    }
                    SwarmEvent::ConnectionClosed {
                        peer_id,
                        num_established,
                        cause,
                        ..
                    } => {
                        tracing::info!(
                            %peer_id,
                            remaining = num_established,
                            cause = cause.as_ref().map(|c| format!("{c}")).as_deref().unwrap_or("clean"),
                            "net: connection closed"
                        );
                        // Only emit disconnect when last connection closes
                        if num_established == 0 {
                            if let Err(e) = event_tx.send(SwarmEvent2::PeerDisconnected { peer_id }) {
                                tracing::warn!(%peer_id, "net: failed to send PeerDisconnected event: {e}");
                            }
                        }
                    }
                    SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                        tracing::warn!(
                            peer = ?peer_id,
                            error = %error,
                            "net: outgoing connection failed"
                        );
                        let _ = event_tx.send(SwarmEvent2::DialFailure { peer_id });
                    }
                    SwarmEvent::ExternalAddrConfirmed { address } => {
                        tracing::info!(%address, "net: external address confirmed");
                        let _ = event_tx.send(SwarmEvent2::ExternalAddrConfirmed { addr: address });
                    }
                    SwarmEvent::Behaviour(CordeliaBehaviourEvent::PeerShare(
                        request_response::Event::Message {
                            message: request_response::Message::Request { channel, request, .. },
                            ..
                        },
                    )) => {
                        // Handle peer share request async (needs pool access)
                        let max = request.max_peers as usize;
                        let relay_peers = pool.relay_peers().await;
                        let peers: Vec<PeerAddress> = relay_peers
                            .iter()
                            .take(max)
                            .map(|h| PeerAddress {
                                peer_id: h.node_id.to_string(),
                                addrs: h.addrs.iter().map(|a| a.to_string()).collect(),
                                last_seen: 0,
                                groups: h.groups.clone(),
                                role: if h.is_relay { "relay".into() } else { our_role.as_str().into() },
                            })
                            .collect();
                        tracing::debug!(
                            requested = max,
                            shared = peers.len(),
                            "net: served peer share request"
                        );
                        let resp = PeerShareResponse { peers };
                        let _ = swarm.behaviour_mut().peer_share.send_response(channel, resp);
                    }
                    // Memory push inbound: handled here for async pool access (relay re-push)
                    SwarmEvent::Behaviour(CordeliaBehaviourEvent::MemoryPush(
                        request_response::Event::Message {
                            message: request_response::Message::Request { request, channel, .. },
                            peer,
                            ..
                        },
                    )) => {
                        let groups_snap = shared_groups.read().await.clone();

                        // Snapshot relay acceptance for this push
                        let accepted_snap: Option<HashSet<String>> = match relay_posture {
                            Some(crate::config::RelayPosture::Dynamic)
                            | Some(crate::config::RelayPosture::Explicit) => {
                                relay_accepted_groups.as_ref().map(|a| {
                                    // We're in async context, use try_read to avoid deadlock.
                                    // If lock is held, fall back to empty (conservative).
                                    a.try_read().map(|g| g.clone()).unwrap_or_default()
                                })
                            }
                            _ => None,
                        };
                        let blocked_snap = relay_blocked_groups.clone();
                        let is_transparent = relay_posture == Some(crate::config::RelayPosture::Transparent);

                        let relay_check = if relay_posture.is_some() {
                            let check = move |gid: &str| -> bool {
                                if blocked_snap.contains(gid) {
                                    return false;
                                }
                                if is_transparent {
                                    return true;
                                }
                                accepted_snap.as_ref().is_some_and(|s| s.contains(gid))
                            };
                            Some(check)
                        } else {
                            None
                        };
                        let relay_accepts_ref: Option<&dyn Fn(&str) -> bool> =
                            relay_check.as_ref().map(|f| f as &dyn Fn(&str) -> bool);

                        let item_count = request.items.len();
                        let ack =
                            handle_push_request(&storage, &request, &groups_snap, relay_accepts_ref);

                        if ack.rejected > 0 {
                            tracing::warn!(
                                items = item_count,
                                stored = ack.stored,
                                rejected = ack.rejected,
                                sender = %peer,
                                "net: push received (with rejections)"
                            );
                        } else if ack.stored > 0 {
                            tracing::info!(
                                items = item_count,
                                stored = ack.stored,
                                sender = %peer,
                                "net: push received"
                            );
                        }
                        let _ = swarm
                            .behaviour_mut()
                            .memory_push
                            .send_response(channel, ack.clone());

                        // Relay re-push: forward to all connected peers (excluding sender).
                        // Loop prevention: duplicate items -> stored == 0 -> no re-push.
                        if relay_posture.is_some() && ack.stored > 0 {
                            let all_peers = pool.active_peers().await;
                            let forward_peers: Vec<_> = all_peers
                                .into_iter()
                                .filter(|h| h.node_id != peer)
                                .collect();
                            if !forward_peers.is_empty() {
                                tracing::debug!(
                                    forward_to = forward_peers.len(),
                                    stored = ack.stored,
                                    "relay: re-pushing to connected peers"
                                );
                                for relay_peer in &forward_peers {
                                    swarm.behaviour_mut().memory_push.send_request(
                                        &relay_peer.node_id,
                                        MemoryPushRequest {
                                            items: request.items.clone(),
                                        },
                                    );
                                }
                            }
                        }
                    }
                    SwarmEvent::Behaviour(ev) => {
                        let mut groups_snap = shared_groups.read().await.clone();
                        // Relay nodes: include learned groups so exchange responses
                        // advertise all groups the relay handles (not just formal members).
                        if let Some(ref accepted) = relay_accepted_groups {
                            if let Ok(set) = accepted.try_read() {
                                for g in set.iter() {
                                    if !groups_snap.contains(g) {
                                        groups_snap.push(g.clone());
                                    }
                                }
                            }
                        }
                        handle_behaviour_event(
                            ev,
                            &mut swarm,
                            &event_tx,
                            &storage,
                            &groups_snap,
                            &mut pending_peer_share,
                            &mut pending_sync,
                            &mut pending_fetch,
                            &mut pending_group_exchange,
                            &node_identity,
                            &our_entity_id,
                        );
                    }
                    _ => {}
                }
            }

            _ = shutdown.recv() => {
                tracing::info!(
                    pending_sync = pending_sync.len(),
                    pending_fetch = pending_fetch.len(),
                    pending_peer_share = pending_peer_share.len(),
                    pending_group_exchange = pending_group_exchange.len(),
                    "net: swarm shutting down"
                );
                break;
            }
        }
    }
}

// ============================================================================
// Behaviour event handler
// ============================================================================

#[allow(clippy::too_many_arguments)]
fn handle_behaviour_event(
    event: CordeliaBehaviourEvent,
    swarm: &mut Swarm<CordeliaBehaviour>,
    event_tx: &broadcast::Sender<SwarmEvent2>,
    storage: &Arc<dyn Storage>,
    our_groups: &[String],
    pending_peer_share: &mut HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<Result<PeerShareResponse, String>>,
    >,
    pending_sync: &mut HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<Result<SyncResponse, String>>,
    >,
    pending_fetch: &mut HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<Result<FetchResponse, String>>,
    >,
    pending_group_exchange: &mut HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<Result<GroupExchangeResponse, String>>,
    >,
    node_identity: &Arc<NodeIdentity>,
    our_entity_id: &str,
) {
    match event {
        // -- Ping --
        CordeliaBehaviourEvent::Ping(libp2p::ping::Event {
            peer,
            result: Ok(rtt),
            ..
        }) => {
            let _ = event_tx.send(SwarmEvent2::PingRtt {
                peer_id: peer,
                rtt_ms: rtt.as_secs_f64() * 1000.0,
            });
        }

        // -- Identify --
        CordeliaBehaviourEvent::Identify(libp2p::identify::Event::Received {
            peer_id,
            info,
            ..
        }) => {
            tracing::debug!(
                %peer_id,
                agent = info.agent_version,
                listen_addrs = info.listen_addrs.len(),
                observed = %info.observed_addr,
                "net: identify received"
            );
            let _ = event_tx.send(SwarmEvent2::IdentifyReceived {
                peer_id,
                listen_addrs: info.listen_addrs,
                observed_addr: info.observed_addr,
                agent_version: info.agent_version,
            });
        }

        // -- Peer Share --
        // Inbound requests handled in run_swarm_loop (needs async pool access)
        CordeliaBehaviourEvent::PeerShare(request_response::Event::Message {
            message:
                request_response::Message::Response {
                    request_id,
                    response,
                },
            ..
        }) => {
            if let Some(tx) = pending_peer_share.remove(&request_id) {
                let _ = tx.send(Ok(response));
            }
        }
        CordeliaBehaviourEvent::PeerShare(request_response::Event::OutboundFailure {
            request_id,
            error,
            ..
        }) => {
            tracing::warn!(error = %error, "net: peer share request failed");
            if let Some(tx) = pending_peer_share.remove(&request_id) {
                let _ = tx.send(Err(error.to_string()));
            }
        }

        // -- Memory Sync --
        CordeliaBehaviourEvent::MemorySync(request_response::Event::Message {
            message:
                request_response::Message::Request {
                    request, channel, ..
                },
            ..
        }) => {
            let resp = handle_sync_request(storage, &request);
            tracing::debug!(
                group = request.group_id,
                since = request.since.as_deref().unwrap_or("(full)"),
                headers_returned = resp.items.len(),
                has_more = resp.has_more,
                "net: served sync request"
            );
            let _ = swarm
                .behaviour_mut()
                .memory_sync
                .send_response(channel, resp);
        }
        CordeliaBehaviourEvent::MemorySync(request_response::Event::Message {
            message:
                request_response::Message::Response {
                    request_id,
                    response,
                },
            ..
        }) => {
            if let Some(tx) = pending_sync.remove(&request_id) {
                let _ = tx.send(Ok(response));
            }
        }
        CordeliaBehaviourEvent::MemorySync(request_response::Event::OutboundFailure {
            request_id,
            error,
            ..
        }) => {
            tracing::warn!(error = %error, "net: sync request failed");
            if let Some(tx) = pending_sync.remove(&request_id) {
                let _ = tx.send(Err(error.to_string()));
            }
        }

        // -- Memory Fetch --
        CordeliaBehaviourEvent::MemoryFetch(request_response::Event::Message {
            message:
                request_response::Message::Request {
                    request, channel, ..
                },
            ..
        }) => {
            let requested = request.item_ids.len();
            let resp = handle_fetch_request(storage, &request);
            tracing::debug!(
                requested,
                returned = resp.items.len(),
                "net: served fetch request"
            );
            let _ = swarm
                .behaviour_mut()
                .memory_fetch
                .send_response(channel, resp);
        }
        CordeliaBehaviourEvent::MemoryFetch(request_response::Event::Message {
            message:
                request_response::Message::Response {
                    request_id,
                    response,
                },
            ..
        }) => {
            if let Some(tx) = pending_fetch.remove(&request_id) {
                let _ = tx.send(Ok(response));
            }
        }
        CordeliaBehaviourEvent::MemoryFetch(request_response::Event::OutboundFailure {
            request_id,
            error,
            ..
        }) => {
            tracing::warn!(error = %error, "net: fetch request failed");
            if let Some(tx) = pending_fetch.remove(&request_id) {
                let _ = tx.send(Err(error.to_string()));
            }
        }

        // -- Memory Push --
        // Handled in run_swarm_loop (needs async pool access for relay re-push)
        CordeliaBehaviourEvent::MemoryPush(request_response::Event::Message {
            message: request_response::Message::Request { .. },
            ..
        }) => {
            // This arm should not fire -- push requests are handled in the
            // run_swarm_loop main select via the moved-up pattern. If we get
            // here, it means the pattern didn't match (shouldn't happen).
            tracing::warn!("net: unexpected push request in behaviour handler");
        }
        CordeliaBehaviourEvent::MemoryPush(request_response::Event::Message {
            message: request_response::Message::Response { response, .. },
            ..
        }) => {
            tracing::debug!(
                stored = response.stored,
                rejected = response.rejected,
                "net: push ack received"
            );
        }

        // -- Group Exchange --
        CordeliaBehaviourEvent::GroupExchange(request_response::Event::Message {
            message:
                request_response::Message::Request {
                    channel, request, ..
                },
            peer,
            ..
        }) => {
            tracing::debug!(
                %peer,
                their_groups = request.groups.len(),
                their_descriptors = request.descriptors.as_ref().map_or(0, |d| d.len()),
                our_groups = our_groups.len(),
                "net: served group exchange request"
            );

            // Merge incoming descriptors from peer (R4-030)
            if let Some(ref descs) = request.descriptors {
                let merged = merge_descriptors(storage.as_ref(), descs);
                if merged > 0 {
                    tracing::info!(
                        %peer,
                        merged,
                        "net: merged group descriptors from peer request"
                    );
                }
            }

            let resp = GroupExchangeResponse {
                groups: our_groups.to_vec(),
                descriptors: build_descriptors(storage.as_ref(), node_identity, our_entity_id),
            };
            let _ = swarm
                .behaviour_mut()
                .group_exchange
                .send_response(channel, resp);
        }
        CordeliaBehaviourEvent::GroupExchange(request_response::Event::Message {
            message:
                request_response::Message::Response {
                    request_id,
                    response,
                },
            ..
        }) => {
            // Merge incoming descriptors from peer response (R4-030)
            if let Some(ref descs) = response.descriptors {
                let merged = merge_descriptors(storage.as_ref(), descs);
                if merged > 0 {
                    tracing::info!(merged, "net: merged group descriptors from peer response");
                }
            }

            if let Some(tx) = pending_group_exchange.remove(&request_id) {
                let _ = tx.send(Ok(response));
            }
        }
        CordeliaBehaviourEvent::GroupExchange(request_response::Event::OutboundFailure {
            request_id,
            error,
            ..
        }) => {
            tracing::warn!(error = %error, "net: group exchange request failed");
            if let Some(tx) = pending_group_exchange.remove(&request_id) {
                let _ = tx.send(Err(error.to_string()));
            }
        }

        // Memory push outbound failure (fire-and-forget, no pending channel)
        CordeliaBehaviourEvent::MemoryPush(request_response::Event::OutboundFailure {
            error,
            ..
        }) => {
            tracing::warn!(error = %error, "net: push outbound failure");
        }

        // Log inbound failures (peer failed to respond to our request)
        CordeliaBehaviourEvent::MemorySync(request_response::Event::InboundFailure {
            error,
            ..
        })
        | CordeliaBehaviourEvent::MemoryFetch(request_response::Event::InboundFailure {
            error,
            ..
        })
        | CordeliaBehaviourEvent::MemoryPush(request_response::Event::InboundFailure {
            error,
            ..
        }) => {
            tracing::debug!(error = %error, "net: inbound request failure");
        }

        // Catch-all for remaining events (ping failures, identify push, etc.)
        _ => {}
    }
}

// ============================================================================
// Inbound request handlers
// ============================================================================

fn handle_sync_request(storage: &Arc<dyn Storage>, req: &SyncRequest) -> SyncResponse {
    let items = storage
        .list_group_items(&req.group_id, req.since.as_deref(), req.limit)
        .unwrap_or_default();

    let has_more = items.len() == req.limit as usize;

    let proto_items: Vec<ItemHeader> = items
        .into_iter()
        .map(|h| ItemHeader {
            item_id: h.item_id,
            item_type: h.item_type,
            checksum: h.checksum,
            updated_at: h.updated_at,
            author_id: h.author_id,
            is_deletion: h.is_deletion,
        })
        .collect();

    SyncResponse {
        items: proto_items,
        has_more,
    }
}

fn handle_fetch_request(storage: &Arc<dyn Storage>, req: &FetchRequest) -> FetchResponse {
    let mut items = Vec::new();

    for id in &req.item_ids {
        if let Ok(Some(row)) = storage.read_l2_item(id) {
            items.push(FetchedItem {
                item_id: row.id,
                item_type: row.item_type,
                encrypted_blob: row.data,
                checksum: row.checksum.unwrap_or_default(),
                author_id: row.author_id.unwrap_or_default(),
                group_id: row.group_id.unwrap_or_default(),
                key_version: row.key_version as u32,
                parent_id: row.parent_id,
                is_copy: row.is_copy,
                updated_at: row.updated_at,
            });
        }
    }

    FetchResponse { items }
}

fn handle_push_request(
    storage: &Arc<dyn Storage>,
    req: &MemoryPushRequest,
    our_groups: &[String],
    relay_accepts: Option<&dyn Fn(&str) -> bool>,
) -> PushAck {
    let engine = ReplicationEngine::new(
        cordelia_replication::ReplicationConfig::default(),
        String::new(),
    );

    let mut stored = 0u32;
    let mut rejected = 0u32;

    for item in &req.items {
        match engine.on_receive(storage.as_ref(), item, our_groups, relay_accepts) {
            ReceiveOutcome::Stored => {
                stored += 1;
                tracing::debug!(
                    item_id = &item.item_id,
                    group = &item.group_id,
                    "push: stored replicated item"
                );
            }
            ReceiveOutcome::Duplicate => {
                tracing::debug!(item_id = &item.item_id, "push: duplicate, skipped");
            }
            ReceiveOutcome::Rejected(reason) => {
                rejected += 1;
                tracing::warn!(item_id = &item.item_id, reason, "push: rejected");
            }
        }
    }

    PushAck { stored, rejected }
}
