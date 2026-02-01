//! Replication background task -- dispatch local writes and run anti-entropy sync.
//!
//! Two loops:
//!   1. Local write notifications -> dispatch to hot peers via SwarmCommand
//!   2. Anti-entropy timer -> per-group sync with random hot peer

use std::collections::HashMap;
use std::sync::Arc;

use cordelia_api::WriteNotification;
use cordelia_protocol::era::CURRENT_ERA;
use cordelia_protocol::messages::{FetchRequest, FetchedItem, MemoryPushRequest, SyncRequest};
use cordelia_replication::{GroupCulture, ReplicationEngine};
use cordelia_storage::Storage;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;

use crate::peer_pool::PeerPool;
use crate::swarm_task::SwarmCommand;

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
    cmd_tx: mpsc::Sender<SwarmCommand>,
    mut write_rx: broadcast::Receiver<WriteNotification>,
    mut shutdown: broadcast::Receiver<()>,
) {
    let sync_interval = std::time::Duration::from_secs(engine.config().sync_interval_moderate_secs);
    let mut sync_timer = tokio::time::interval(sync_interval);
    sync_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    sync_timer.tick().await;

    let mut pending_pushes: HashMap<String, PendingPush> = HashMap::new();
    let mut retry_tick = tokio::time::interval(std::time::Duration::from_secs(1));
    retry_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    retry_tick.tick().await;

    loop {
        tokio::select! {
            // Local write notification -> dispatch to peers + enqueue retry
            write_result = write_rx.recv() => {
                match write_result {
                    Ok(notif) => {
                        if let Some(group_id) = &notif.group_id {
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

                            if let Some(pending) = dispatch_outbound(
                                action, &pool, &storage, &cmd_tx,
                            ).await {
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

            // Push retry tick
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
                            let _ = cmd_tx.send(SwarmCommand::SendMemoryPush {
                                peer: peer.node_id,
                                request: MemoryPushRequest {
                                    items: vec![pending.item.clone()],
                                },
                            }).await;
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
                        &cmd_tx,
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

/// Dispatch an outbound action to peers via SwarmCommand.
async fn dispatch_outbound(
    action: cordelia_replication::engine::OutboundAction,
    pool: &PeerPool,
    storage: &Arc<dyn Storage>,
    cmd_tx: &mpsc::Sender<SwarmCommand>,
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
                let _ = cmd_tx
                    .send(SwarmCommand::SendMemoryPush {
                        peer: peer.node_id,
                        request: MemoryPushRequest {
                            items: vec![item.clone()],
                        },
                    })
                    .await;
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
                    let _ = cmd_tx
                        .send(SwarmCommand::SendMemoryPush {
                            peer: peer.node_id,
                            request: MemoryPushRequest {
                                items: vec![item.clone()],
                            },
                        })
                        .await;
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

/// Run anti-entropy sync for a single group via SwarmCommand.
async fn run_anti_entropy(
    engine: &ReplicationEngine,
    pool: &PeerPool,
    storage: &Arc<dyn Storage>,
    group_id: &str,
    our_groups: &[String],
    cmd_tx: &mpsc::Sender<SwarmCommand>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let peer = match pool.random_hot_peer_for_group(group_id).await {
        Some(p) => p,
        None => return Ok(()),
    };

    // Send sync request via swarm
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    cmd_tx
        .send(SwarmCommand::SendSyncRequest {
            peer: peer.node_id,
            request: SyncRequest {
                group_id: group_id.to_string(),
                since: None,
                limit: engine.max_batch_size(),
            },
            response_tx: resp_tx,
        })
        .await
        .map_err(|e| format!("send sync request failed: {e}"))?;

    let remote_headers = resp_rx
        .await
        .map_err(|_| "sync response channel closed")?
        .map_err(|e| format!("sync request failed: {e}"))?;

    // Get local headers
    let local_headers = storage
        .list_group_items(group_id, None, engine.max_batch_size())
        .map_err(|e| format!("storage error: {e}"))?;

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
        peer = %peer.node_id,
        "fetching missing items"
    );

    // Fetch in batches
    for chunk in needed_ids.chunks(engine.max_batch_size() as usize) {
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        cmd_tx
            .send(SwarmCommand::SendFetchRequest {
                peer: peer.node_id,
                request: FetchRequest {
                    item_ids: chunk.to_vec(),
                },
                response_tx: resp_tx,
            })
            .await
            .map_err(|e| format!("send fetch request failed: {e}"))?;

        let fetch_resp = resp_rx
            .await
            .map_err(|_| "fetch response channel closed")?
            .map_err(|e| format!("fetch request failed: {e}"))?;

        for item in &fetch_resp.items {
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

/// Load group culture from storage.
fn load_group_culture(storage: &Arc<dyn Storage>, group_id: &str) -> Option<GroupCulture> {
    let group = storage.read_group(group_id).ok()??;
    serde_json::from_str(&group.culture).ok().or_else(|| {
        Some(GroupCulture {
            broadcast_eagerness: group.culture.clone(),
            ..Default::default()
        })
    })
}
