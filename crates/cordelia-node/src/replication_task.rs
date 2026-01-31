//! Replication background task -- dispatch local writes and run anti-entropy sync.
//!
//! Two loops:
//!   1. Local write notifications → dispatch to hot peers via pool
//!   2. Anti-entropy timer → per-group sync with random hot peer

use std::sync::Arc;

use cordelia_api::WriteNotification;
use cordelia_protocol::messages::FetchedItem;
use cordelia_replication::{GroupCulture, ReplicationConfig, ReplicationEngine};
use cordelia_storage::Storage;
use tokio::sync::broadcast;

use crate::mini_protocols;
use crate::peer_pool::PeerPool;

/// Run the replication loop until shutdown.
pub async fn run_replication_loop(
    engine: ReplicationEngine,
    pool: PeerPool,
    storage: Arc<dyn Storage>,
    shared_groups: Arc<tokio::sync::RwLock<Vec<String>>>,
    mut write_rx: broadcast::Receiver<WriteNotification>,
    mut shutdown: broadcast::Receiver<()>,
) {
    let config = ReplicationConfig::default();

    // Anti-entropy sync interval (use moderate default)
    let sync_interval = std::time::Duration::from_secs(config.sync_interval_moderate_secs);
    let mut sync_timer = tokio::time::interval(sync_interval);
    sync_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Skip the first immediate tick
    sync_timer.tick().await;

    loop {
        tokio::select! {
            // Local write notification → dispatch to peers
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

                            dispatch_outbound(action, &pool, &storage, group_id).await;
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
async fn dispatch_outbound(
    action: cordelia_replication::engine::OutboundAction,
    pool: &PeerPool,
    storage: &Arc<dyn Storage>,
    _group_id: &str,
) {
    use cordelia_replication::engine::OutboundAction;

    match action {
        OutboundAction::BroadcastItem { group_id, item } => {
            let peers = pool.hot_peers_for_group(&group_id).await;
            for peer in peers {
                let item = item.clone();
                tokio::spawn(async move {
                    if let Err(e) = send_push(&peer.connection, vec![item]).await {
                        tracing::debug!(
                            peer = hex::encode(peer.node_id),
                            "broadcast item failed: {e}"
                        );
                    }
                });
            }
        }
        OutboundAction::BroadcastHeader { group_id, header } => {
            // For notify-and-fetch (moderate culture), read the full item from
            // storage and push it. The pure header-only notify flow is not yet
            // implemented on the receive side.
            let full_item = storage.read_l2_item(&header.item_id).ok().flatten();
            let peers = pool.hot_peers_for_group(&group_id).await;
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
                for peer in peers {
                    let item = item.clone();
                    tokio::spawn(async move {
                        if let Err(e) = send_push(&peer.connection, vec![item]).await {
                            tracing::debug!(
                                peer = hex::encode(peer.node_id),
                                "broadcast header->push failed: {e}"
                            );
                        }
                    });
                }
            }
        }
        OutboundAction::None => {}
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
