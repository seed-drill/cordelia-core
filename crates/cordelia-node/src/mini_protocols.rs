//! Mini-protocol handlers for QUIC bidirectional streams.
//!
//! Five protocols:
//!   1. Handshake -- propose/accept, compute group intersection, register peer
//!   2. Keep-alive -- ping/pong, RTT measurement
//!   3. Peer-sharing -- exchange known peer addresses
//!   4. Memory-sync -- exchange item headers for anti-entropy
//!   5. Memory-fetch -- fetch encrypted blobs by item ID

use bytes::BytesMut;
use cordelia_governor::PeerState;
use cordelia_protocol::messages::*;
use cordelia_protocol::{
    MessageCodec, NodeId, ProtocolError, KEEPALIVE_INTERVAL_SECS, PROTOCOL_MAGIC, VERSION_MAX,
    VERSION_MIN,
};
use cordelia_storage::Storage;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_util::codec::Encoder;

use crate::external_addr::ExternalAddr;
use crate::peer_pool::PeerPool;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

// ============================================================================
// Codec helpers -- read/write a single Message on a QUIC stream
// ============================================================================

async fn read_message(recv: &mut quinn::RecvStream) -> Result<Message, BoxError> {
    // Read 4-byte length prefix
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len > cordelia_protocol::MAX_MESSAGE_BYTES {
        return Err(Box::new(ProtocolError::MessageTooLarge {
            size: len,
            max: cordelia_protocol::MAX_MESSAGE_BYTES,
        }));
    }

    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf).await?;

    let msg: Message = serde_json::from_slice(&buf)?;
    Ok(msg)
}

async fn write_message(send: &mut quinn::SendStream, msg: &Message) -> Result<(), BoxError> {
    let mut codec = MessageCodec;
    let mut buf = BytesMut::new();
    codec.encode(msg.clone(), &mut buf)?;
    send.write_all(&buf).await?;
    Ok(())
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

// ============================================================================
// 1. Handshake
// ============================================================================

/// Handle inbound handshake (we are the acceptor).
pub async fn handle_inbound_handshake(
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    conn: &quinn::Connection,
    pool: &PeerPool,
    our_node_id: NodeId,
    our_groups: &[String],
    _external_addr: &Arc<tokio::sync::RwLock<ExternalAddr>>,
) -> Result<NodeId, BoxError> {
    // Read protocol byte
    let mut proto = [0u8; 1];
    recv.read_exact(&mut proto).await?;
    if proto[0] != crate::quic_transport::PROTO_HANDSHAKE {
        return Err(format!(
            "expected handshake protocol byte 0x01, got {:#04x}",
            proto[0]
        )
        .into());
    }

    // Read propose
    let msg = read_message(&mut recv).await?;
    let propose = match msg {
        Message::HandshakePropose(p) => p,
        other => return Err(format!("expected HandshakePropose, got {other:?}").into()),
    };

    // Validate magic
    if propose.magic != PROTOCOL_MAGIC {
        let reject = Message::HandshakeAccept(HandshakeAccept {
            version: 0,
            node_id: our_node_id,
            timestamp: now_ts(),
            groups: vec![],
            reject_reason: Some(format!(
                "invalid magic: expected {:#010x}, got {:#010x}",
                PROTOCOL_MAGIC, propose.magic
            )),
            observed_addr: None,
            era: cordelia_protocol::ERA_0.id,
        });
        write_message(&mut send, &reject).await?;
        return Err("invalid magic".into());
    }

    // Version negotiation
    let version = negotiate_version(propose.version_min, propose.version_max);
    if version == 0 {
        let reject = Message::HandshakeAccept(HandshakeAccept {
            version: 0,
            node_id: our_node_id,
            timestamp: now_ts(),
            groups: vec![],
            reject_reason: Some("no compatible version".into()),
            observed_addr: None,
            era: cordelia_protocol::ERA_0.id,
        });
        write_message(&mut send, &reject).await?;
        return Err("version mismatch".into());
    }

    // Accept -- tell the peer what address we see them as (NAT hairpin avoidance)
    let accept = Message::HandshakeAccept(HandshakeAccept {
        version,
        node_id: our_node_id,
        timestamp: now_ts(),
        groups: our_groups.to_vec(),
        reject_reason: None,
        observed_addr: Some(conn.remote_address()),
        era: cordelia_protocol::ERA_0.id,
    });
    write_message(&mut send, &accept).await?;

    // Register peer in pool (relay status unknown at handshake, default false)
    pool.insert(
        propose.node_id,
        conn.clone(),
        propose.groups,
        PeerState::Warm,
        version,
        false,
    )
    .await;

    tracing::info!(
        peer = hex::encode(propose.node_id),
        remote = %conn.remote_address(),
        protocol_version = version,
        "inbound handshake complete"
    );

    Ok(propose.node_id)
}

/// Handle outbound handshake (we are the proposer).
pub async fn handle_outbound_handshake(
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    conn: &quinn::Connection,
    pool: &PeerPool,
    our_node_id: NodeId,
    our_groups: &[String],
    external_addr: &Arc<tokio::sync::RwLock<ExternalAddr>>,
) -> Result<NodeId, BoxError> {
    // Write protocol byte
    send.write_all(&[crate::quic_transport::PROTO_HANDSHAKE])
        .await?;

    // Send propose
    let propose = Message::HandshakePropose(HandshakePropose {
        magic: PROTOCOL_MAGIC,
        version_min: VERSION_MIN,
        version_max: VERSION_MAX,
        node_id: our_node_id,
        timestamp: now_ts(),
        groups: our_groups.to_vec(),
        era: cordelia_protocol::ERA_0.id,
    });
    write_message(&mut send, &propose).await?;

    // Read accept
    let msg = read_message(&mut recv).await?;
    let accept = match msg {
        Message::HandshakeAccept(a) => a,
        other => return Err(format!("expected HandshakeAccept, got {other:?}").into()),
    };

    if accept.version == 0 {
        return Err(format!(
            "handshake rejected: {}",
            accept.reject_reason.unwrap_or_default()
        )
        .into());
    }

    // Feed observed address for NAT hairpin avoidance
    if let Some(observed) = accept.observed_addr {
        external_addr.write().await.observe(observed.ip());
    }

    // Register peer in pool (relay status unknown at handshake, default false)
    pool.insert(
        accept.node_id,
        conn.clone(),
        accept.groups,
        PeerState::Warm,
        accept.version,
        false,
    )
    .await;

    tracing::info!(
        peer = hex::encode(accept.node_id),
        remote = %conn.remote_address(),
        protocol_version = accept.version,
        "outbound handshake complete"
    );

    Ok(accept.node_id)
}

fn negotiate_version(peer_min: u16, peer_max: u16) -> u16 {
    let common_min = peer_min.max(VERSION_MIN);
    let common_max = peer_max.min(VERSION_MAX);
    if common_min <= common_max {
        common_max
    } else {
        0 // no compatible version
    }
}

// ============================================================================
// 2. Keep-alive
// ============================================================================

/// Handle inbound keep-alive (respond to pings with pongs).
pub async fn handle_keepalive(
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    conn: &quinn::Connection,
) -> Result<(), BoxError> {
    loop {
        let msg = match read_message(&mut recv).await {
            Ok(m) => m,
            Err(_) => return Ok(()), // stream closed
        };

        match msg {
            Message::Ping(ping) => {
                let pong = Message::Pong(Pong {
                    seq: ping.seq,
                    sent_at_ns: ping.sent_at_ns,
                    recv_at_ns: now_ns(),
                    observed_addr: Some(conn.remote_address()),
                });
                write_message(&mut send, &pong).await?;
            }
            _ => {
                tracing::debug!("unexpected message in keepalive stream");
            }
        }
    }
}

/// Run outbound keep-alive loop (send pings, read pongs, record RTT).
/// Returns when the connection drops or the shutdown signal fires.
pub async fn run_keepalive_loop(
    conn: &quinn::Connection,
    node_id: &NodeId,
    governor: &Arc<tokio::sync::Mutex<cordelia_governor::Governor>>,
    external_addr: &Arc<tokio::sync::RwLock<ExternalAddr>>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) {
    let mut seq: u64 = 0;
    let mut missed = 0u32;

    loop {
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(KEEPALIVE_INTERVAL_SECS)) => {}
            _ = shutdown.recv() => return,
        }

        let (mut send, mut recv) = match conn.open_bi().await {
            Ok(s) => s,
            Err(_) => return,
        };

        if send
            .write_all(&[crate::quic_transport::PROTO_KEEPALIVE])
            .await
            .is_err()
        {
            return;
        }

        seq += 1;
        let sent_at = now_ns();
        let ping = Message::Ping(Ping {
            seq,
            sent_at_ns: sent_at,
        });

        if write_message(&mut send, &ping).await.is_err() {
            missed += 1;
            if missed >= cordelia_protocol::KEEPALIVE_MISS_LIMIT {
                tracing::warn!(
                    peer = hex::encode(node_id),
                    "peer dead: {missed} missed pings"
                );
                return;
            }
            continue;
        }

        // Wait for pong with timeout
        let pong_result = tokio::time::timeout(
            std::time::Duration::from_secs(cordelia_protocol::PONG_TIMEOUT_SECS),
            read_message(&mut recv),
        )
        .await;

        match pong_result {
            Ok(Ok(Message::Pong(pong))) => {
                missed = 0;
                let rtt_ns = now_ns().saturating_sub(pong.sent_at_ns);
                let rtt_ms = rtt_ns as f64 / 1_000_000.0;
                governor.lock().await.record_activity(node_id, Some(rtt_ms));
                // Feed observed address for NAT hairpin avoidance
                if let Some(observed) = pong.observed_addr {
                    external_addr.write().await.observe(observed.ip());
                }
            }
            _ => {
                missed += 1;
                if missed >= cordelia_protocol::KEEPALIVE_MISS_LIMIT {
                    tracing::warn!(
                        peer = hex::encode(node_id),
                        "peer dead: {missed} missed pings"
                    );
                    return;
                }
            }
        }
    }
}

// ============================================================================
// 3. Peer-sharing
// ============================================================================

/// Handle inbound peer-share request.
/// `our_role` determines which peers we gossip: relays share all active peers,
/// personal/keeper nodes only share relay peers (never leak personal nodes).
pub async fn handle_peer_share(
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    pool: &PeerPool,
    our_role: &str,
) -> Result<(), BoxError> {
    let msg = read_message(&mut recv).await?;

    match msg {
        Message::PeerShareRequest(req) => {
            // Relays share all active peers; personal/keeper only share relays
            let shareable = if our_role == "relay" {
                pool.active_peers().await
            } else {
                pool.relay_peers().await
            };
            let peers: Vec<PeerAddress> = shareable
                .iter()
                .take(req.max_peers as usize)
                .map(|h| PeerAddress {
                    node_id: h.node_id,
                    addrs: vec![h.connection.remote_address()],
                    last_seen: now_ts(),
                    groups: h.groups.clone(),
                    role: if h.is_relay {
                        "relay".into()
                    } else {
                        String::new()
                    },
                })
                .collect();

            let resp = Message::PeerShareResponse(PeerShareResponse { peers });
            write_message(&mut send, &resp).await?;
        }
        _ => {
            tracing::debug!("unexpected message in peer-share stream");
        }
    }

    Ok(())
}

/// Request peers from a remote node.
pub async fn request_peers(
    conn: &quinn::Connection,
    max_peers: u16,
) -> Result<Vec<PeerAddress>, BoxError> {
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(&[crate::quic_transport::PROTO_PEER_SHARE])
        .await?;

    let req = Message::PeerShareRequest(PeerShareRequest { max_peers });
    write_message(&mut send, &req).await?;

    let msg = read_message(&mut recv).await?;
    match msg {
        Message::PeerShareResponse(resp) => Ok(resp.peers),
        other => Err(format!("expected PeerShareResponse, got {other:?}").into()),
    }
}

// ============================================================================
// 4. Memory-sync
// ============================================================================

/// Handle inbound memory-sync request.
pub async fn handle_memory_sync(
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    storage: &Arc<dyn Storage>,
) -> Result<(), BoxError> {
    let msg = read_message(&mut recv).await?;

    match msg {
        Message::SyncRequest(req) => {
            let items = storage
                .list_group_items(&req.group_id, req.since.as_deref(), req.limit)
                .map_err(|e| format!("storage error: {e}"))?;

            let has_more = items.len() == req.limit as usize;

            // Convert storage ItemHeader to protocol ItemHeader
            let proto_items: Vec<cordelia_protocol::messages::ItemHeader> = items
                .into_iter()
                .map(|h| cordelia_protocol::messages::ItemHeader {
                    item_id: h.item_id,
                    item_type: h.item_type,
                    checksum: h.checksum,
                    updated_at: h.updated_at,
                    author_id: h.author_id,
                    is_deletion: h.is_deletion,
                })
                .collect();

            let resp = Message::SyncResponse(SyncResponse {
                items: proto_items,
                has_more,
            });
            write_message(&mut send, &resp).await?;
        }
        _ => {
            tracing::debug!("unexpected message in memory-sync stream");
        }
    }

    Ok(())
}

/// Send a sync request to a peer and get back headers.
pub async fn request_sync(
    conn: &quinn::Connection,
    group_id: &str,
    since: Option<String>,
    limit: u32,
) -> Result<SyncResponse, BoxError> {
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(&[crate::quic_transport::PROTO_MEMORY_SYNC])
        .await?;

    let req = Message::SyncRequest(SyncRequest {
        group_id: group_id.to_string(),
        since,
        limit,
    });
    write_message(&mut send, &req).await?;

    let msg = read_message(&mut recv).await?;
    match msg {
        Message::SyncResponse(resp) => Ok(resp),
        other => Err(format!("expected SyncResponse, got {other:?}").into()),
    }
}

// ============================================================================
// 5. Memory-fetch
// ============================================================================

/// Handle inbound memory-fetch request.
pub async fn handle_memory_fetch(
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    storage: &Arc<dyn Storage>,
) -> Result<(), BoxError> {
    let msg = read_message(&mut recv).await?;

    match msg {
        Message::FetchRequest(req) => {
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

            let resp = Message::FetchResponse(FetchResponse { items });
            write_message(&mut send, &resp).await?;
        }
        _ => {
            tracing::debug!("unexpected message in memory-fetch stream");
        }
    }

    Ok(())
}

/// Fetch items from a peer by ID.
pub async fn fetch_items(
    conn: &quinn::Connection,
    item_ids: Vec<String>,
) -> Result<Vec<FetchedItem>, BoxError> {
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(&[crate::quic_transport::PROTO_MEMORY_FETCH])
        .await?;

    let req = Message::FetchRequest(FetchRequest { item_ids });
    write_message(&mut send, &req).await?;

    let msg = read_message(&mut recv).await?;
    match msg {
        Message::FetchResponse(resp) => Ok(resp.items),
        other => Err(format!("expected FetchResponse, got {other:?}").into()),
    }
}

// ============================================================================
// 6. Memory-push (unsolicited item delivery)
// ============================================================================

/// Handle inbound memory-push: receive pushed items, store via engine, ack.
pub async fn handle_memory_push(
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    storage: &Arc<dyn Storage>,
    our_groups: &[String],
) -> Result<(), BoxError> {
    let msg = read_message(&mut recv).await?;

    match msg {
        Message::FetchResponse(resp) => {
            let engine = cordelia_replication::ReplicationEngine::new(
                cordelia_replication::ReplicationConfig::default(),
                String::new(), // entity_id not needed for receive
            );

            let mut stored = 0u32;
            let mut rejected = 0u32;

            for item in &resp.items {
                match engine.on_receive(storage.as_ref(), item, our_groups) {
                    cordelia_replication::ReceiveOutcome::Stored => {
                        stored += 1;
                        tracing::debug!(
                            item_id = &item.item_id,
                            group = &item.group_id,
                            "push: stored replicated item"
                        );
                    }
                    cordelia_replication::ReceiveOutcome::Duplicate => {
                        tracing::debug!(item_id = &item.item_id, "push: duplicate, skipped");
                    }
                    cordelia_replication::ReceiveOutcome::Rejected(reason) => {
                        rejected += 1;
                        tracing::warn!(item_id = &item.item_id, reason, "push: rejected");
                    }
                }
            }

            // Ack with counts
            let ack = Message::PushAck(PushAck { stored, rejected });
            write_message(&mut send, &ack).await?;
        }
        _ => {
            tracing::debug!("unexpected message in memory-push stream");
        }
    }

    Ok(())
}

// ============================================================================
// 7. Group exchange -- refresh group intersection post-handshake
// ============================================================================

/// Handle inbound group exchange: receive peer's groups, send ours, update pool.
pub async fn handle_group_exchange(
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    pool: &PeerPool,
    our_groups: &[String],
    peer_node_id: &NodeId,
) -> Result<(), BoxError> {
    let msg = read_message(&mut recv).await?;

    match msg {
        Message::GroupExchange(exchange) => {
            // Send our groups back
            let resp = Message::GroupExchangeResponse(GroupExchangeResponse {
                groups: our_groups.to_vec(),
            });
            write_message(&mut send, &resp).await?;

            // Update peer's groups and recompute intersection
            let old_handle = pool.get(peer_node_id).await;
            pool.update_peer_groups(peer_node_id, exchange.groups.clone())
                .await;
            let new_handle = pool.get(peer_node_id).await;

            if let (Some(old), Some(new)) = (old_handle, new_handle) {
                if old.group_intersection != new.group_intersection {
                    tracing::info!(
                        peer = hex::encode(peer_node_id),
                        old = ?old.group_intersection,
                        new = ?new.group_intersection,
                        "group intersection updated (inbound exchange)"
                    );
                }
            }
        }
        _ => {
            tracing::debug!("unexpected message in group-exchange stream");
        }
    }

    Ok(())
}

/// Request group exchange from a peer: send our groups, receive theirs.
/// Returns the peer's current groups.
pub async fn request_group_exchange(
    conn: &quinn::Connection,
    our_groups: &[String],
) -> Result<Vec<String>, BoxError> {
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(&[crate::quic_transport::PROTO_GROUP_EXCHANGE])
        .await?;

    let req = Message::GroupExchange(GroupExchange {
        groups: our_groups.to_vec(),
    });
    write_message(&mut send, &req).await?;

    let msg = read_message(&mut recv).await?;
    match msg {
        Message::GroupExchangeResponse(resp) => Ok(resp.groups),
        other => Err(format!("expected GroupExchangeResponse, got {other:?}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_negotiate_version() {
        assert_eq!(negotiate_version(1, 1), 1);
        assert_eq!(negotiate_version(1, 2), 1);
        assert_eq!(negotiate_version(2, 3), 0);
        assert_eq!(negotiate_version(0, 1), 1);
    }
}
