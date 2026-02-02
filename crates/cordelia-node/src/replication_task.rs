//! Replication background task -- dispatch local writes and run anti-entropy sync.
//!
//! Three loops:
//!   1. Local write notifications -> buffer items for batched push
//!   2. Flush timer (100ms) -> batch dispatch buffered items to hot peers
//!   3. Anti-entropy timer -> per-group sync with random hot peer (per-culture interval)

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use cordelia_api::{ReplicationStats, WriteNotification};
use cordelia_protocol::era::CURRENT_ERA;
use cordelia_protocol::messages::{FetchRequest, FetchedItem, MemoryPushRequest, SyncRequest};
use cordelia_replication::{GroupCulture, ReceiveOutcome, ReplicationEngine};
use cordelia_storage::Storage;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;

use crate::peer_pool::PeerPool;
use crate::swarm_task::SwarmCommand;

/// A pending push awaiting retry delivery.
struct PendingPush {
    items: Vec<FetchedItem>,
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
    stats: Arc<ReplicationStats>,
) {
    // Base tick for per-culture sync scheduling (fastest culture interval = 60s for chatty)
    let mut sync_base_tick = tokio::time::interval(std::time::Duration::from_secs(
        cordelia_protocol::EAGER_PUSH_INTERVAL_SECS,
    ));
    sync_base_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    sync_base_tick.tick().await;

    let mut pending_pushes: Vec<PendingPush> = Vec::new();
    // Per-group sync scheduling: next deadline based on culture interval
    let mut next_sync_at: HashMap<String, Instant> = HashMap::new();
    // Incremental anti-entropy: track last sync timestamp and cycle count per group
    let mut last_sync_at: HashMap<String, String> = HashMap::new();
    let mut sync_cycle_count: HashMap<String, u32> = HashMap::new();
    /// Full sync every N cycles to catch edge cases (deletes, clock skew).
    const FULL_SYNC_EVERY: u32 = 10;

    let mut retry_tick = tokio::time::interval(std::time::Duration::from_secs(1));
    retry_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    retry_tick.tick().await;

    // Write coalescing buffer: group_id -> buffered items for batched push
    let mut write_buffer: HashMap<String, Vec<FetchedItem>> = HashMap::new();
    let mut flush_tick = tokio::time::interval(std::time::Duration::from_millis(100));
    flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    flush_tick.tick().await;

    tracing::info!("replication loop started");

    loop {
        // Update gauge-style stats each iteration
        let buf_depth: u64 = write_buffer.values().map(|v| v.len() as u64).sum();
        stats
            .write_buffer_depth
            .store(buf_depth, Ordering::Relaxed);
        stats
            .pending_push_count
            .store(pending_pushes.len() as u64, Ordering::Relaxed);

        tokio::select! {
            // Local write notification -> buffer for batched dispatch
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
                                is_copy = notif.is_copy,
                                parent_id = notif.parent_id.as_deref().unwrap_or("-"),
                                "repl: local write received"
                            );

                            let action = engine.on_local_write(
                                group_id,
                                &culture,
                                &notif.item_id,
                                &notif.item_type,
                                &notif.data,
                                notif.key_version,
                                notif.parent_id.clone(),
                                notif.is_copy,
                            );

                            match action {
                                cordelia_replication::engine::OutboundAction::BroadcastItem { group_id, item } => {
                                    tracing::debug!(
                                        item_id = item.item_id,
                                        group = group_id.as_str(),
                                        "repl: buffered for eager push"
                                    );
                                    write_buffer.entry(group_id).or_default().push(item);
                                }
                                cordelia_replication::engine::OutboundAction::BroadcastHeader { group_id, header } => {
                                    tracing::debug!(
                                        item_id = header.item_id,
                                        group = group_id.as_str(),
                                        "repl: moderate culture, anti-entropy only"
                                    );
                                }
                                cordelia_replication::engine::OutboundAction::None => {
                                    tracing::debug!(
                                        item_id = &notif.item_id,
                                        "repl: no outbound action (passive/suppressed)"
                                    );
                                }
                            }
                        } else {
                            tracing::debug!(
                                item_id = &notif.item_id,
                                "repl: write has no group_id, skipping"
                            );
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(missed = n, "repl: write notification channel lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("repl: write notification channel closed, shutting down");
                        return;
                    }
                }
            }

            // Flush coalesced writes as batched pushes (100ms coalescing window)
            _ = flush_tick.tick() => {
                for (group_id, items) in write_buffer.drain() {
                    if items.is_empty() {
                        continue;
                    }
                    let item_count = items.len();
                    let peers = pool.active_peers_for_group(&group_id).await;

                    if peers.is_empty() {
                        tracing::info!(
                            group = group_id.as_str(),
                            item_count,
                            "repl: no peers for group, enqueuing push retry"
                        );
                        pending_pushes.push(PendingPush {
                            items,
                            group_id,
                            attempt: 0,
                            next_at: Instant::now()
                                + std::time::Duration::from_secs(CURRENT_ERA.push_retry_backoffs[0]),
                        });
                        continue;
                    }

                    // Split into batches respecting MAX_MESSAGE_BYTES
                    let batches = split_into_batches(&items);
                    let peer_count = peers.len();

                    tracing::info!(
                        group = group_id.as_str(),
                        items = item_count,
                        batches = batches.len(),
                        peers = peer_count,
                        "repl: push flush"
                    );

                    for batch in &batches {
                        for peer in &peers {
                            if let Err(e) = cmd_tx
                                .send(SwarmCommand::SendMemoryPush {
                                    peer: peer.node_id,
                                    request: MemoryPushRequest {
                                        items: batch.clone(),
                                    },
                                })
                                .await
                            {
                                tracing::warn!(
                                    peer = %peer.node_id,
                                    group = group_id.as_str(),
                                    "repl: failed to send push command: {e}"
                                );
                            }
                        }
                    }

                    stats
                        .items_pushed
                        .fetch_add(item_count as u64, Ordering::Relaxed);

                    // Enqueue for retry
                    pending_pushes.push(PendingPush {
                        items,
                        group_id,
                        attempt: 0,
                        next_at: Instant::now()
                            + std::time::Duration::from_secs(CURRENT_ERA.push_retry_backoffs[0]),
                    });
                }
            }

            // Push retry tick
            _ = retry_tick.tick() => {
                if pending_pushes.is_empty() {
                    continue;
                }
                let now = Instant::now();
                let mut keep = Vec::new();
                for mut pending in pending_pushes.drain(..) {
                    if now < pending.next_at {
                        keep.push(pending);
                        continue;
                    }
                    let peers = pool.active_peers_for_group(&pending.group_id).await;
                    if !peers.is_empty() {
                        tracing::info!(
                            group = pending.group_id.as_str(),
                            items = pending.items.len(),
                            attempt = pending.attempt + 1,
                            peers = peers.len(),
                            "repl: push retry"
                        );
                        let batches = split_into_batches(&pending.items);
                        for batch in &batches {
                            for peer in &peers {
                                if let Err(e) = cmd_tx.send(SwarmCommand::SendMemoryPush {
                                    peer: peer.node_id,
                                    request: MemoryPushRequest {
                                        items: batch.clone(),
                                    },
                                }).await {
                                    tracing::warn!(
                                        peer = %peer.node_id,
                                        "repl: retry push send failed: {e}"
                                    );
                                }
                            }
                        }
                    } else {
                        tracing::debug!(
                            group = pending.group_id.as_str(),
                            attempt = pending.attempt + 1,
                            "repl: retry skipped, no peers"
                        );
                    }
                    pending.attempt += 1;
                    if pending.attempt >= CURRENT_ERA.push_retry_count as usize {
                        tracing::warn!(
                            group = pending.group_id,
                            items = pending.items.len(),
                            attempts = CURRENT_ERA.push_retry_count,
                            "repl: push retries exhausted, relying on anti-entropy"
                        );
                        stats
                            .push_retries_exhausted
                            .fetch_add(1, Ordering::Relaxed);
                    } else {
                        pending.next_at = now + std::time::Duration::from_secs(
                            CURRENT_ERA.push_retry_backoffs[pending.attempt],
                        );
                        keep.push(pending);
                    }
                }
                pending_pushes = keep;
            }

            // Per-culture anti-entropy sync (base tick fires at fastest interval)
            _ = sync_base_tick.tick() => {
                let now = Instant::now();
                let current_groups = shared_groups.read().await.clone();
                for group_id in &current_groups {
                    // Check if this group is due for sync
                    let deadline = next_sync_at.get(group_id).copied();
                    if deadline.map_or(false, |d| now < d) {
                        continue; // not yet due
                    }

                    // Compute culture-specific interval for next deadline
                    let culture = load_group_culture(&storage, group_id)
                        .unwrap_or_default();
                    let interval_secs = engine.sync_interval(&culture);
                    next_sync_at.insert(
                        group_id.clone(),
                        now + std::time::Duration::from_secs(interval_secs),
                    );

                    let cycle = sync_cycle_count.entry(group_id.clone()).or_insert(0);
                    *cycle += 1;
                    let is_full_sync = *cycle % FULL_SYNC_EVERY == 0;
                    let since = if is_full_sync {
                        tracing::info!(
                            group = group_id,
                            cycle = *cycle,
                            "repl: full anti-entropy sync (periodic)"
                        );
                        None
                    } else {
                        last_sync_at.get(group_id).cloned()
                    };

                    match run_anti_entropy(
                        &engine,
                        &pool,
                        &storage,
                        group_id,
                        &current_groups,
                        &cmd_tx,
                        since.as_deref(),
                        &stats,
                    ).await {
                        Ok(latest_ts) => {
                            stats.sync_rounds.fetch_add(1, Ordering::Relaxed);
                            if let Some(ts) = latest_ts {
                                last_sync_at.insert(group_id.clone(), ts);
                            }
                        }
                        Err(e) => {
                            stats.sync_errors.fetch_add(1, Ordering::Relaxed);
                            tracing::warn!(
                                group = group_id,
                                error = %e,
                                "repl: anti-entropy sync failed"
                            );
                        }
                    }
                }
            }

            _ = shutdown.recv() => {
                tracing::info!(
                    pushed = stats.items_pushed.load(Ordering::Relaxed),
                    synced = stats.items_synced.load(Ordering::Relaxed),
                    rejected = stats.items_rejected.load(Ordering::Relaxed),
                    sync_rounds = stats.sync_rounds.load(Ordering::Relaxed),
                    "repl: shutting down (final stats)"
                );
                return;
            }
        }
    }
}

/// Split items into batches respecting MAX_MESSAGE_BYTES (512KB).
fn split_into_batches(items: &[FetchedItem]) -> Vec<Vec<FetchedItem>> {
    let mut batches = Vec::new();
    let mut current_batch = Vec::new();
    let mut current_size: usize = 0;

    for item in items {
        let item_size = item.encrypted_blob.len() + item.item_id.len() + 256; // overhead estimate
        if !current_batch.is_empty()
            && current_size + item_size > cordelia_protocol::MAX_MESSAGE_BYTES
        {
            batches.push(std::mem::take(&mut current_batch));
            current_size = 0;
        }
        current_size += item_size;
        current_batch.push(item.clone());
    }
    if !current_batch.is_empty() {
        batches.push(current_batch);
    }
    batches
}

/// Run anti-entropy sync for a single group via SwarmCommand.
/// Returns the latest `updated_at` from remote headers on success (for incremental sync).
async fn run_anti_entropy(
    engine: &ReplicationEngine,
    pool: &PeerPool,
    storage: &Arc<dyn Storage>,
    group_id: &str,
    our_groups: &[String],
    cmd_tx: &mpsc::Sender<SwarmCommand>,
    since: Option<&str>,
    stats: &Arc<ReplicationStats>,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let peer = match pool.random_hot_peer_for_group(group_id).await {
        Some(p) => p,
        None => {
            tracing::debug!(group = group_id, "repl: no peers available");
            return Ok(None);
        }
    };

    // Send sync request via swarm
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    cmd_tx
        .send(SwarmCommand::SendSyncRequest {
            peer: peer.node_id,
            request: SyncRequest {
                group_id: group_id.to_string(),
                since: since.map(|s| s.to_string()),
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

    // Track latest remote timestamp for incremental sync
    let latest_ts = remote_headers
        .items
        .iter()
        .map(|h| h.updated_at.as_str())
        .max()
        .map(|s| s.to_string());

    // Get local headers (use same since filter for consistent comparison)
    let local_headers = storage
        .list_group_items(group_id, since, engine.max_batch_size())
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
        tracing::debug!(
            group = group_id,
            peer = %peer.node_id,
            remote_headers = remote_headers.items.len(),
            local_headers = local_proto.len(),
            incremental = since.is_some(),
            "repl: in sync"
        );
        return Ok(latest_ts);
    }

    stats.sync_rounds_with_diff.fetch_add(1, Ordering::Relaxed);

    tracing::info!(
        group = group_id,
        needed = needed_ids.len(),
        remote_headers = remote_headers.items.len(),
        local_headers = local_proto.len(),
        peer = %peer.node_id,
        incremental = since.is_some(),
        "repl: fetching missing items"
    );

    // Fetch in batches
    let mut stored = 0u64;
    let mut rejected = 0u64;
    let mut duplicate = 0u64;

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
            match &outcome {
                ReceiveOutcome::Stored => {
                    stored += 1;
                    tracing::debug!(
                        item_id = &item.item_id,
                        group = group_id,
                        "repl: stored"
                    );
                }
                ReceiveOutcome::Duplicate => {
                    duplicate += 1;
                }
                ReceiveOutcome::Rejected(reason) => {
                    rejected += 1;
                    tracing::warn!(
                        item_id = &item.item_id,
                        group = group_id,
                        reason,
                        "repl: rejected"
                    );
                }
            }
        }
    }

    stats.items_synced.fetch_add(stored, Ordering::Relaxed);
    stats.items_rejected.fetch_add(rejected, Ordering::Relaxed);
    stats.items_duplicate.fetch_add(duplicate, Ordering::Relaxed);

    tracing::info!(
        group = group_id,
        peer = %peer.node_id,
        stored,
        rejected,
        duplicate,
        "repl: round complete"
    );

    Ok(latest_ts)
}

/// Load group culture from storage.
fn load_group_culture(storage: &Arc<dyn Storage>, group_id: &str) -> Option<GroupCulture> {
    let group = match storage.read_group(group_id) {
        Ok(Some(g)) => g,
        Ok(None) => {
            tracing::warn!(group = group_id, "repl: group not found in storage, defaulting to moderate");
            return None;
        }
        Err(e) => {
            tracing::warn!(group = group_id, error = %e, "repl: failed to read group, defaulting to moderate");
            return None;
        }
    };
    match serde_json::from_str::<GroupCulture>(&group.culture) {
        Ok(culture) => Some(culture),
        Err(_) => {
            tracing::debug!(
                group = group_id,
                raw_culture = group.culture,
                "repl: culture JSON parse failed, treating as bare eagerness string"
            );
            Some(GroupCulture {
                broadcast_eagerness: group.culture.clone(),
                ..Default::default()
            })
        }
    }
}
