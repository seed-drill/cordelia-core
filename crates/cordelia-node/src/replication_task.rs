//! Replication background task -- dispatch local writes and run anti-entropy sync.
//!
//! Two loops:
//!   1. Local write notifications → dispatch to hot peers via pool
//!   2. Anti-entropy timer → per-group sync with random hot peer

use std::collections::HashMap;
use std::sync::Arc;

use cordelia_api::WriteNotification;
use cordelia_protocol::era::CURRENT_ERA;
use cordelia_protocol::messages::FetchedItem;
use cordelia_replication::{GroupCulture, ReplicationEngine};
use cordelia_storage::Storage;
use tokio::sync::broadcast;
use tokio::time::Instant;

use crate::mini_protocols;
use crate::peer_pool::PeerPool;

/// A pending push awaiting retry delivery.
struct PendingPush {
    item: FetchedItem,
    group_id: String,
    attempt: usize,
    next_at: Instant,
}

/// Run the replication loop until shutdown.
pub async fn run_replication_loop(
    engine: ReplicationEngine,
    pool: PeerPool,
    storage: Arc<dyn Storage>,
    shared_groups: Arc<tokio::sync::RwLock<Vec<String>>>,
    mut write_rx: broadcast::Receiver<WriteNotification>,
    mut shutdown: broadcast::Receiver<()>,
) {
    // Anti-entropy sync interval (from node config via engine)
    let sync_interval = std::time::Duration::from_secs(engine.config().sync_interval_moderate_secs);
    let mut sync_timer = tokio::time::interval(sync_interval);
    sync_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Skip the first immediate tick
    sync_timer.tick().await;

    // Push retry queue: item_id → pending push with backoff
    let mut pending_pushes: HashMap<String, PendingPush> = HashMap::new();
    let mut retry_tick = tokio::time::interval(std::time::Duration::from_secs(1));
    retry_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    retry_tick.tick().await;

    loop {
        tokio::select! {
            // Local write notification → dispatch to peers + enqueue retry
            write_result = write_rx.recv() => {
                match write_result {
                    Ok(notif) => {
                        if let Some(group_id) = &notif.group_id {
                            // Look up group culture from storage
                            let culture = load_group_culture(&storage, group_id)
                                .unwrap_or_default();

                            tracing::debug!(
                                item_id = &notif.item_id,
                                group = group_id.as_str(),
                                eagerness = culture.broadcast_eagerness.as_str(),
                                "replication: processing local write"
                            );

                            let action = engine.on_local_write(
                                group_id,
                                &culture,
                                &notif.item_id,
                                &notif.item_type,
                                &notif.data,
                                notif.key_version,
                            );

                            tracing::debug!(
                                action = ?action,
                                "replication: outbound action"
                            );

                            if let Some(pending) = dispatch_outbound(action, &pool, &storage).await {
                                tracing::debug!(
                                    item_id = pending.item.item_id,
                                    group = pending.group_id.as_str(),
                                    retries = CURRENT_ERA.push_retry_count,
                                    "enqueued push retry",
                                );
                                pending_pushes.insert(pending.item.item_id.clone(), pending);
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("replication lagged, missed {n} notifications");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("write notification channel closed");
                        return;
                    }
                }
            }

            // Push retry tick -- re-push pending items with backoff
            _ = retry_tick.tick() => {
                if pending_pushes.is_empty() {
                    continue;
                }
                let now = Instant::now();
                let mut retired = Vec::new();
                for (item_id, pending) in pending_pushes.iter_mut() {
                    if now < pending.next_at {
                        continue;
                    }
                    let peers = pool.active_peers_for_group(&pending.group_id).await;
                    if !peers.is_empty() {
                        tracing::debug!(
                            item_id = item_id.as_str(),
                            attempt = pending.attempt + 1,
                            peer_count = peers.len(),
                            "push retry"
                        );
                        for peer in peers {
                            let item = pending.item.clone();
                            tokio::spawn(async move {
                                if let Err(e) = send_push(&peer.connection, vec![item]).await {
                                    tracing::debug!(
                                        peer = hex::encode(peer.node_id),
                                        "retry push failed: {e}"
                                    );
                                }
                            });
                        }
                    }
                    pending.attempt += 1;
                    if pending.attempt >= CURRENT_ERA.push_retry_count as usize {
                        retired.push(item_id.clone());
                    } else {
                        pending.next_at = now + std::time::Duration::from_secs(
                            CURRENT_ERA.push_retry_backoffs[pending.attempt],
                        );
                    }
                }
                for id in retired {
                    tracing::debug!(item_id = id, "push retry exhausted, relying on anti-entropy");
                    pending_pushes.remove(&id);
                }
            }

            // Anti-entropy sync timer
            _ = sync_timer.tick() => {
                let current_groups = shared_groups.read().await.clone();
                for group_id in &current_groups {
                    if let Err(e) = run_anti_entropy(
                        &engine,
                        &pool,
                        &storage,
                        group_id,
                        &current_groups,
                    ).await {
                        tracing::debug!(group = group_id, "anti-entropy sync error: {e}");
                    }
                }
            }

            _ = shutdown.recv() => {
                tracing::info!("replication shutting down");
                return;
            }
        }
    }
}

/// Dispatch an outbound action to peers.
///
/// Pushes to ALL active peers (hot + warm) for the group -- critical for small
/// meshes where hot peers may not exist yet. Returns a `PendingPush` for the
/// retry queue so the item is re-pushed with backoff until anti-entropy confirms.
async fn dispatch_outbound(
    action: cordelia_replication::engine::OutboundAction,
    pool: &PeerPool,
    storage: &Arc<dyn Storage>,
) -> Option<PendingPush> {
    use cordelia_replication::engine::OutboundAction;

    match action {
        OutboundAction::BroadcastItem { group_id, item } => {
            let peers = pool.active_peers_for_group(&group_id).await;
            tracing::debug!(
                item_id = item.item_id,
                peer_count = peers.len(),
                "dispatch: broadcast item to active peers"
            );
            for peer in &peers {
                let item = item.clone();
                let conn = peer.connection.clone();
                let nid = peer.node_id;
                tokio::spawn(async move {
                    if let Err(e) = send_push(&conn, vec![item]).await {
                        tracing::debug!(peer = hex::encode(nid), "broadcast item failed: {e}");
                    }
                });
            }
            Some(PendingPush {
                item,
                group_id,
                attempt: 0,
                next_at: Instant::now()
                    + std::time::Duration::from_secs(CURRENT_ERA.push_retry_backoffs[0]),
            })
        }
        OutboundAction::BroadcastHeader { group_id, header } => {
            // For notify-and-fetch (moderate culture), read the full item from
            // storage and push it. The pure header-only notify flow is not yet
            // implemented on the receive side.
            let full_item = storage.read_l2_item(&header.item_id).ok().flatten();
            let peers = pool.active_peers_for_group(&group_id).await;
            if let Some(row) = full_item {
                let item = FetchedItem {
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
                };
                tracing::debug!(
                    item_id = item.item_id,
                    peer_count = peers.len(),
                    "dispatch: broadcast header->push to active peers"
                );
                for peer in &peers {
                    let item = item.clone();
                    let conn = peer.connection.clone();
                    let nid = peer.node_id;
                    tokio::spawn(async move {
                        if let Err(e) = send_push(&conn, vec![item]).await {
                            tracing::debug!(
                                peer = hex::encode(nid),
                                "broadcast header->push failed: {e}"
                            );
                        }
                    });
                }
                Some(PendingPush {
                    item,
                    group_id,
                    attempt: 0,
                    next_at: Instant::now()
                        + std::time::Duration::from_secs(CURRENT_ERA.push_retry_backoffs[0]),
                })
            } else {
                None
            }
        }
        OutboundAction::None => None,
    }
}

/// Send a FetchResponse via MEMORY_PUSH (0x06) for unsolicited item delivery.
async fn send_push(
    conn: &quinn::Connection,
    items: Vec<FetchedItem>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (mut send, mut recv) =
        crate::quic_transport::open_protocol_stream(conn, crate::quic_transport::PROTO_MEMORY_PUSH)
            .await?;

    let msg = cordelia_protocol::messages::Message::FetchResponse(
        cordelia_protocol::messages::FetchResponse { items },
    );
    let mut codec = cordelia_protocol::MessageCodec;
    let mut buf = bytes::BytesMut::new();
    tokio_util::codec::Encoder::encode(&mut codec, msg, &mut buf)?;
    send.write_all(&buf).await?;
    send.finish()?;

    // Read ack (best-effort, don't fail on timeout)
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut len_buf = [0u8; 4];
        if recv.read_exact(&mut len_buf).await.is_ok() {
            let len = u32::from_be_bytes(len_buf) as usize;
            if len <= 16 * 1024 * 1024 {
                let mut buf = vec![0u8; len];
                let _ = recv.read_exact(&mut buf).await;
            }
        }
    })
    .await;

    Ok(())
}

#[allow(dead_code)]
/// Send a SyncResponse with a single header (for notify-and-fetch).
/// Uses MEMORY_PUSH (0x06) to distinguish from request-response sync streams.
async fn send_sync_notification(
    conn: &quinn::Connection,
    _group_id: &str,
    header: &cordelia_protocol::messages::ItemHeader,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // For notify-and-fetch, we push a FetchResponse with a minimal item
    // so the receiver can store it directly. The header-only notification
    // pattern is handled via anti-entropy sync instead.
    // This keeps the push path simple: always full items.
    let (mut send, _recv) =
        crate::quic_transport::open_protocol_stream(conn, crate::quic_transport::PROTO_MEMORY_SYNC)
            .await?;

    let msg = cordelia_protocol::messages::Message::SyncResponse(
        cordelia_protocol::messages::SyncResponse {
            items: vec![header.clone()],
            has_more: false,
        },
    );
    let mut codec = cordelia_protocol::MessageCodec;
    let mut buf = bytes::BytesMut::new();
    tokio_util::codec::Encoder::encode(&mut codec, msg, &mut buf)?;
    send.write_all(&buf).await?;
    send.finish()?;
    Ok(())
}

/// Run anti-entropy sync for a single group.
async fn run_anti_entropy(
    engine: &ReplicationEngine,
    pool: &PeerPool,
    storage: &Arc<dyn Storage>,
    group_id: &str,
    our_groups: &[String],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let peer = match pool.random_hot_peer_for_group(group_id).await {
        Some(p) => p,
        None => return Ok(()), // no hot peers for this group
    };

    // Send sync request
    let remote_headers =
        mini_protocols::request_sync(&peer.connection, group_id, None, engine.max_batch_size())
            .await?;

    // Get local headers
    let local_headers = storage
        .list_group_items(group_id, None, engine.max_batch_size())
        .map_err(|e| format!("storage error: {e}"))?;

    // Convert storage headers to protocol headers for diff
    let local_proto: Vec<cordelia_protocol::messages::ItemHeader> = local_headers
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

    let needed_ids = cordelia_replication::diff_headers(&local_proto, &remote_headers.items);

    if needed_ids.is_empty() {
        return Ok(());
    }

    tracing::info!(
        group = group_id,
        count = needed_ids.len(),
        peer = hex::encode(peer.node_id),
        "fetching missing items"
    );

    // Fetch in batches
    for chunk in needed_ids.chunks(engine.max_batch_size() as usize) {
        let items = mini_protocols::fetch_items(&peer.connection, chunk.to_vec()).await?;

        for item in &items {
            let outcome = engine.on_receive(storage.as_ref(), item, our_groups);
            tracing::debug!(
                item_id = &item.item_id,
                outcome = ?outcome,
                "received replicated item"
            );
        }
    }

    Ok(())
}

/// Load group culture from storage, returning default if not found or unparseable.
fn load_group_culture(storage: &Arc<dyn Storage>, group_id: &str) -> Option<GroupCulture> {
    let group = storage.read_group(group_id).ok()??;
    // Try JSON parse first, fall back to treating raw string as eagerness level
    serde_json::from_str(&group.culture).ok().or_else(|| {
        Some(GroupCulture {
            broadcast_eagerness: group.culture.clone(),
            ..Default::default()
        })
    })
}
