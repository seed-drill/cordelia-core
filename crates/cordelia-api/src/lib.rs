//! Cordelia API -- local node HTTP/Unix socket API.
//!
//! Unix socket (~/.cordelia/node.sock) or HTTP (127.0.0.1:9473).
//! Bearer token auth from ~/.cordelia/node-token.
//! Routes from spec section 6.

use axum::{
    extract::{Json, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Router,
};
use cordelia_storage::{L2ItemWrite, Storage};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Notification sent when a local L2 write occurs (for replication dispatch).
#[derive(Debug, Clone)]
pub struct WriteNotification {
    pub item_id: String,
    pub item_type: String,
    pub group_id: Option<String>,
    pub data: Vec<u8>,
    pub key_version: u32,
    pub parent_id: Option<String>,
    pub is_copy: bool,
}

/// Replication diagnostics counters -- shared between replication task and API.
pub struct ReplicationStats {
    /// Items pushed to peers (batched sends, counted per item).
    pub items_pushed: AtomicU64,
    /// Items received via anti-entropy sync.
    pub items_synced: AtomicU64,
    /// Items rejected on receive (integrity, membership, size).
    pub items_rejected: AtomicU64,
    /// Items received as duplicates (already stored).
    pub items_duplicate: AtomicU64,
    /// Push retries that exhausted all attempts.
    pub push_retries_exhausted: AtomicU64,
    /// Anti-entropy sync rounds completed.
    pub sync_rounds: AtomicU64,
    /// Anti-entropy sync rounds that found missing items.
    pub sync_rounds_with_diff: AtomicU64,
    /// Anti-entropy sync rounds that failed (no peer, error).
    pub sync_errors: AtomicU64,
    /// Items buffered in write coalescing buffer (set, not incremented).
    pub write_buffer_depth: AtomicU64,
    /// Pending push retries in queue (set, not incremented).
    pub pending_push_count: AtomicU64,
}

impl ReplicationStats {
    pub fn new() -> Self {
        Self {
            items_pushed: AtomicU64::new(0),
            items_synced: AtomicU64::new(0),
            items_rejected: AtomicU64::new(0),
            items_duplicate: AtomicU64::new(0),
            push_retries_exhausted: AtomicU64::new(0),
            sync_rounds: AtomicU64::new(0),
            sync_rounds_with_diff: AtomicU64::new(0),
            sync_errors: AtomicU64::new(0),
            write_buffer_depth: AtomicU64::new(0),
            pending_push_count: AtomicU64::new(0),
        }
    }
}

impl Default for ReplicationStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Callback to get peer counts (warm, hot) from the node's peer pool.
pub type PeerCountFn = Box<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = (usize, usize)> + Send>>
        + Send
        + Sync,
>;

/// Peer details for API response.
#[derive(Debug, Clone, Serialize)]
pub struct PeerDetail {
    pub node_id: String,
    pub addrs: Vec<String>,
    pub state: String,
    pub rtt_ms: Option<f64>,
    pub items_delivered: u64,
    pub groups: Vec<String>,
    pub group_intersection: Vec<String>,
    pub is_relay: bool,
    pub protocol_version: u16,
}

/// Callback to get peer list from the node's peer pool.
pub type PeerListFn = Box<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<PeerDetail>> + Send>>
        + Send
        + Sync,
>;

/// Shared state for all API handlers.
pub struct AppState {
    pub storage: Box<dyn Storage>,
    pub node_id: String,
    pub entity_id: String,
    pub bearer_token: String,
    pub start_time: std::time::Instant,
    pub write_notify: Option<tokio::sync::broadcast::Sender<WriteNotification>>,
    pub shared_groups: Option<std::sync::Arc<tokio::sync::RwLock<Vec<String>>>>,
    pub peer_count_fn: Option<PeerCountFn>,
    pub peer_list_fn: Option<PeerListFn>,
    pub replication_stats: Option<Arc<ReplicationStats>>,
    /// Signal to trigger immediate anti-entropy sync for a newly added group.
    pub bootstrap_sync: Option<tokio::sync::mpsc::Sender<String>>,
}

/// Build the axum router.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/l1/read", post(l1_read))
        .route("/api/v1/l1/write", post(l1_write))
        .route("/api/v1/l1/delete", post(l1_delete))
        .route("/api/v1/l1/list", post(l1_list))
        .route("/api/v1/l2/read", post(l2_read))
        .route("/api/v1/l2/write", post(l2_write))
        .route("/api/v1/l2/delete", post(l2_delete))
        .route("/api/v1/l2/search", post(l2_search))
        .route("/api/v1/groups/create", post(groups_create))
        .route("/api/v1/groups/list", post(groups_list))
        .route("/api/v1/groups/read", post(groups_read))
        .route("/api/v1/groups/items", post(groups_items))
        .route("/api/v1/groups/delete", post(groups_delete))
        .route("/api/v1/groups/add_member", post(groups_add_member))
        .route("/api/v1/groups/remove_member", post(groups_remove_member))
        .route("/api/v1/groups/update_posture", post(groups_update_posture))
        .route("/api/v1/devices/register", post(devices_register))
        .route("/api/v1/devices/list", post(devices_list))
        .route("/api/v1/devices/revoke", post(devices_revoke))
        .route("/api/v1/status", post(status))
        .route("/api/v1/peers", post(peers))
        .route("/api/v1/diagnostics", post(diagnostics))
        .with_state(state)
}

// ============================================================================
// Auth middleware (inline check)
// ============================================================================

fn check_auth(state: &AppState, headers: &HeaderMap) -> Result<(), (StatusCode, &'static str)> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let expected = format!("Bearer {}", state.bearer_token);
    if auth != expected {
        return Err((StatusCode::UNAUTHORIZED, "invalid bearer token"));
    }
    Ok(())
}

// ============================================================================
// Request/Response types
// ============================================================================

#[derive(Deserialize)]
pub struct L1ReadRequest {
    pub user_id: String,
}

#[derive(Deserialize)]
pub struct L1WriteRequest {
    pub user_id: String,
    pub data: serde_json::Value,
}

#[derive(Deserialize)]
pub struct L1DeleteRequest {
    pub user_id: String,
}

#[derive(Deserialize)]
pub struct L1ListRequest {}

#[derive(Deserialize)]
pub struct L2ReadRequest {
    pub item_id: String,
}

#[derive(Serialize)]
pub struct L2ReadResponse {
    pub data: serde_json::Value,
    #[serde(rename = "type")]
    pub item_type: String,
    pub meta: L2MetaResponse,
}

#[derive(Serialize)]
pub struct L2MetaResponse {
    pub owner_id: Option<String>,
    pub visibility: String,
    pub group_id: Option<String>,
    pub author_id: Option<String>,
    pub key_version: i32,
    pub checksum: Option<String>,
    pub parent_id: Option<String>,
    pub is_copy: bool,
}

#[derive(Deserialize)]
pub struct L2WriteRequest {
    pub item_id: String,
    #[serde(rename = "type")]
    pub item_type: String,
    pub data: serde_json::Value,
    #[serde(default)]
    pub meta: Option<L2WriteMeta>,
}

#[derive(Deserialize, Default)]
pub struct L2WriteMeta {
    pub owner_id: Option<String>,
    pub visibility: Option<String>,
    pub group_id: Option<String>,
    pub author_id: Option<String>,
    pub key_version: Option<i32>,
    pub parent_id: Option<String>,
    pub is_copy: Option<bool>,
}

#[derive(Deserialize)]
pub struct L2DeleteRequest {
    pub item_id: String,
}

#[derive(Deserialize)]
pub struct L2SearchRequest {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Optional group_id filter: only return items belonging to this group.
    pub group_id: Option<String>,
}

fn default_limit() -> u32 {
    20
}

#[derive(Deserialize)]
pub struct GroupReadRequest {
    pub group_id: String,
}

#[derive(Deserialize)]
pub struct GroupItemsRequest {
    pub group_id: String,
    pub since: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

#[derive(Deserialize)]
pub struct GroupCreateRequest {
    pub group_id: String,
    pub name: String,
    #[serde(default = "default_culture")]
    pub culture: String,
    #[serde(default = "default_security_policy")]
    pub security_policy: String,
}

#[derive(Deserialize)]
pub struct GroupDeleteRequest {
    pub group_id: String,
}

#[derive(Deserialize)]
pub struct GroupAddMemberRequest {
    pub group_id: String,
    pub entity_id: String,
    #[serde(default = "default_member_role")]
    pub role: String,
}

fn default_member_role() -> String {
    "member".into()
}

#[derive(Deserialize)]
pub struct GroupRemoveMemberRequest {
    pub group_id: String,
    pub entity_id: String,
}

#[derive(Deserialize)]
pub struct GroupUpdatePostureRequest {
    pub group_id: String,
    pub entity_id: String,
    pub posture: String,
}

// ============================================================================
// Device types
// ============================================================================

#[derive(Deserialize)]
pub struct DeviceRegisterRequest {
    pub device_id: String,
    pub entity_id: String,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default = "default_device_type")]
    pub device_type: String,
    pub auth_token_hash: String,
}

fn default_device_type() -> String {
    "node".into()
}

#[derive(Deserialize)]
pub struct DeviceListRequest {
    pub entity_id: String,
}

#[derive(Deserialize)]
pub struct DeviceRevokeRequest {
    pub entity_id: String,
    pub device_id: String,
}

fn default_culture() -> String {
    r#"{"broadcast_eagerness":"chatty"}"#.into()
}

fn default_security_policy() -> String {
    "{}".into()
}

#[derive(Serialize)]
pub struct StatusResponse {
    pub node_id: String,
    pub entity_id: String,
    pub uptime_secs: u64,
    pub peers_warm: usize,
    pub peers_hot: usize,
    pub groups: Vec<String>,
}

#[derive(Serialize)]
pub struct PeersResponse {
    pub warm: usize,
    pub hot: usize,
    pub total: usize,
    pub peers: Vec<PeerDetail>,
}

// ============================================================================
// Handlers
// ============================================================================

async fn l1_read(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<L1ReadRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    match state.storage.read_l1(&req.user_id) {
        Ok(Some(data)) => {
            // Return raw blob as JSON (it's already encrypted JSON)
            match serde_json::from_slice::<serde_json::Value>(&data) {
                Ok(val) => Json(val).into_response(),
                Err(_) => {
                    // Return as base64 if not valid JSON
                    let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
                    Json(serde_json::json!({ "data": encoded })).into_response()
                }
            }
        }
        Ok(None) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn l1_write(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<L1WriteRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    let data = serde_json::to_vec(&req.data).unwrap_or_default();
    match state.storage.write_l1(&req.user_id, &data) {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn l1_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<L1DeleteRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    match state.storage.delete_l1(&req.user_id) {
        Ok(true) => Json(serde_json::json!({ "ok": true })).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn l1_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(_req): Json<L1ListRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    match state.storage.list_l1_users() {
        Ok(users) => Json(serde_json::json!({ "users": users })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn l2_read(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<L2ReadRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    match state.storage.read_l2_item(&req.item_id) {
        Ok(Some(row)) => {
            let data_val = serde_json::from_slice::<serde_json::Value>(&row.data)
                .unwrap_or(serde_json::Value::Null);

            Json(L2ReadResponse {
                data: data_val,
                item_type: row.item_type,
                meta: L2MetaResponse {
                    owner_id: row.owner_id,
                    visibility: row.visibility,
                    group_id: row.group_id,
                    author_id: row.author_id,
                    key_version: row.key_version,
                    checksum: row.checksum,
                    parent_id: row.parent_id,
                    is_copy: row.is_copy,
                },
            })
            .into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn l2_write(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<L2WriteRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    let data = match serde_json::to_vec(&req.data) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(
                item_id = req.item_id,
                error = %e,
                "mem: l2 write failed to serialise data"
            );
            return (
                StatusCode::BAD_REQUEST,
                format!("failed to serialise item data: {e}"),
            )
                .into_response();
        }
    };

    // Enforce item size limit (backpressure: reject before storage/replication)
    if data.len() > cordelia_protocol::MAX_ITEM_BYTES {
        tracing::warn!(
            item_id = req.item_id,
            bytes = data.len(),
            limit = cordelia_protocol::MAX_ITEM_BYTES,
            "mem: l2 write rejected (too large)"
        );
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Conditions Not Met: item is {} bytes but era limit is {} bytes -- memories should be dense, not large",
                data.len(),
                cordelia_protocol::MAX_ITEM_BYTES
            ),
        )
            .into_response();
    }

    let meta = req.meta.unwrap_or_default();

    let write = L2ItemWrite {
        id: req.item_id,
        item_type: req.item_type,
        data,
        owner_id: meta.owner_id,
        visibility: meta.visibility.unwrap_or_else(|| "private".into()),
        group_id: meta.group_id,
        author_id: meta.author_id.or_else(|| Some(state.entity_id.clone())),
        key_version: meta.key_version.unwrap_or(1),
        parent_id: meta.parent_id,
        is_copy: meta.is_copy.unwrap_or(false),
        updated_at: None, // local write: use datetime('now')
    };

    match state.storage.write_l2_item(&write) {
        Ok(()) => {
            tracing::info!(
                item_id = write.id,
                item_type = write.item_type,
                group = write.group_id.as_deref().unwrap_or("(private)"),
                bytes = write.data.len(),
                is_copy = write.is_copy,
                "mem: l2 item written"
            );
            // Notify replication task of the write
            if let Some(tx) = &state.write_notify {
                let _ = tx.send(WriteNotification {
                    item_id: write.id,
                    item_type: write.item_type,
                    group_id: write.group_id,
                    data: write.data,
                    key_version: write.key_version as u32,
                    parent_id: write.parent_id,
                    is_copy: write.is_copy,
                });
            }
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "mem: l2 write failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn l2_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<L2DeleteRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    // Read metadata before delete to get group_id for tombstone replication
    let meta = state.storage.read_l2_item_meta(&req.item_id).ok().flatten();

    match state.storage.delete_l2_item(&req.item_id) {
        Ok(true) => {
            tracing::info!(item_id = req.item_id, "mem: l2 item deleted");
            // Tombstone replication: notify peers to delete this item
            if let Some(ref meta) = meta {
                if let (Some(tx), Some(group_id)) = (&state.write_notify, &meta.group_id) {
                    let _ = tx.send(WriteNotification {
                        item_id: req.item_id.clone(),
                        item_type: "__tombstone__".into(),
                        group_id: Some(group_id.clone()),
                        data: Vec::new(),
                        key_version: meta.key_version as u32,
                        parent_id: None,
                        is_copy: false,
                    });
                    tracing::info!(
                        item_id = req.item_id,
                        group = group_id.as_str(),
                        "mem: tombstone replication triggered"
                    );
                }
            }
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn l2_search(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<L2SearchRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    // Fetch more results if group_id filter is active (post-filter needs headroom)
    let fetch_limit = if req.group_id.is_some() {
        req.limit * 4
    } else {
        req.limit
    };

    match state.storage.fts_search(&req.query, fetch_limit) {
        Ok(ids) => {
            let filtered = if let Some(ref group_id) = req.group_id {
                // Post-filter: only return items belonging to the requested group
                ids.into_iter()
                    .filter(|id| {
                        state
                            .storage
                            .read_l2_item_meta(id)
                            .ok()
                            .flatten()
                            .map(|m| m.group_id.as_deref() == Some(group_id))
                            .unwrap_or(false)
                    })
                    .take(req.limit as usize)
                    .collect::<Vec<_>>()
            } else {
                ids
            };
            Json(serde_json::json!({ "results": filtered })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn groups_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<GroupCreateRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    match state
        .storage
        .write_group(&req.group_id, &req.name, &req.culture, &req.security_policy)
    {
        Ok(()) => {
            tracing::info!(
                group_id = req.group_id,
                name = req.name,
                culture = req.culture,
                "mem: group created"
            );
            // Push new group into shared dynamic groups
            if let Some(shared) = &state.shared_groups {
                let mut groups = shared.write().await;
                if !groups.contains(&req.group_id) {
                    groups.push(req.group_id.clone());
                    tracing::info!(
                        group_id = req.group_id,
                        total_groups = groups.len(),
                        "mem: group added to shared_groups"
                    );
                }
            }
            // Trigger immediate anti-entropy sync for the new group
            if let Some(tx) = &state.bootstrap_sync {
                let _ = tx.try_send(req.group_id.clone());
            }
            Json(serde_json::json!({
                "ok": true,
                "group_id": req.group_id,
            }))
            .into_response()
        }
        Err(e) => {
            tracing::warn!(group_id = req.group_id, error = %e, "mem: group create failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn groups_list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    match state.storage.list_groups() {
        Ok(groups) => {
            let live: Vec<_> = groups
                .into_iter()
                .filter(|g| g.culture != cordelia_protocol::messages::GROUP_TOMBSTONE_CULTURE)
                .collect();
            Json(serde_json::json!({ "groups": live })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn groups_read(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<GroupReadRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    match state.storage.read_group(&req.group_id) {
        Ok(Some(group))
            if group.culture == cordelia_protocol::messages::GROUP_TOMBSTONE_CULTURE =>
        {
            (StatusCode::NOT_FOUND, "group not found").into_response()
        }
        Ok(Some(group)) => {
            let members = state
                .storage
                .list_members(&req.group_id)
                .unwrap_or_default();
            Json(serde_json::json!({
                "group": group,
                "members": members,
            }))
            .into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "group not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn groups_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<GroupItemsRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    match state
        .storage
        .list_group_items(&req.group_id, req.since.as_deref(), req.limit)
    {
        Ok(items) => Json(serde_json::json!({ "items": items })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn groups_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<GroupDeleteRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    // Check group exists
    match state.storage.read_group(&req.group_id) {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, "group not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }

    // Write tombstone descriptor (culture = __deleted__) instead of deleting.
    // This propagates via GroupExchange to peers using LWW semantics.
    let tombstone_culture = cordelia_protocol::messages::GROUP_TOMBSTONE_CULTURE;
    if let Err(e) = state
        .storage
        .write_group(&req.group_id, &req.group_id, tombstone_culture, "{}")
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    // Soft-remove members (CoW: posture = 'removed', no hard delete)
    if let Ok(members) = state.storage.list_members(&req.group_id) {
        for m in members {
            let _ = state
                .storage
                .update_member_posture(&req.group_id, &m.entity_id, "removed");
        }
    }

    tracing::info!(
        group_id = req.group_id,
        "mem: group tombstoned for deletion"
    );

    // Remove from shared dynamic groups (stops item replication)
    if let Some(shared) = &state.shared_groups {
        let mut groups = shared.write().await;
        groups.retain(|g| g != &req.group_id);
        tracing::info!(
            group_id = req.group_id,
            remaining_groups = groups.len(),
            "mem: group removed from shared_groups"
        );
    }

    // Log access
    let _ = state.storage.log_access(&cordelia_storage::AccessLogEntry {
        entity_id: state.entity_id.clone(),
        action: "delete_group".into(),
        resource_type: "group".into(),
        resource_id: Some(req.group_id.clone()),
        group_id: Some(req.group_id),
        detail: Some("tombstone".into()),
    });

    Json(serde_json::json!({ "ok": true })).into_response()
}

async fn groups_add_member(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<GroupAddMemberRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    // Validate role
    match req.role.as_str() {
        "owner" | "admin" | "member" | "viewer" => {}
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "invalid role: must be owner, admin, member, or viewer",
            )
                .into_response()
        }
    }

    match state
        .storage
        .add_member(&req.group_id, &req.entity_id, &req.role)
    {
        Ok(()) => {
            tracing::info!(
                group_id = req.group_id,
                entity_id = req.entity_id,
                role = req.role,
                "mem: member added to group"
            );
            let _ = state.storage.log_access(&cordelia_storage::AccessLogEntry {
                entity_id: state.entity_id.clone(),
                action: "add_member".into(),
                resource_type: "group".into(),
                resource_id: Some(req.group_id.clone()),
                group_id: Some(req.group_id),
                detail: Some(format!("entity={} role={}", req.entity_id, req.role)),
            });

            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn groups_remove_member(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<GroupRemoveMemberRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    match state.storage.remove_member(&req.group_id, &req.entity_id) {
        Ok(true) => {
            tracing::info!(
                group_id = req.group_id,
                entity_id = req.entity_id,
                "mem: member removed from group"
            );
            let _ = state.storage.log_access(&cordelia_storage::AccessLogEntry {
                entity_id: state.entity_id.clone(),
                action: "remove_member".into(),
                resource_type: "group".into(),
                resource_id: Some(req.group_id.clone()),
                group_id: Some(req.group_id),
                detail: Some(format!("entity={}", req.entity_id)),
            });

            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "member not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn groups_update_posture(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<GroupUpdatePostureRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    // Validate posture
    match req.posture.as_str() {
        "active" | "silent" | "emcon" => {}
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "invalid posture: must be active, silent, or emcon",
            )
                .into_response()
        }
    }

    match state
        .storage
        .update_member_posture(&req.group_id, &req.entity_id, &req.posture)
    {
        Ok(true) => {
            tracing::info!(
                group_id = req.group_id,
                entity_id = req.entity_id,
                posture = req.posture,
                "mem: member posture updated"
            );
            let _ = state.storage.log_access(&cordelia_storage::AccessLogEntry {
                entity_id: state.entity_id.clone(),
                action: "update_posture".into(),
                resource_type: "group".into(),
                resource_id: Some(req.group_id.clone()),
                group_id: Some(req.group_id),
                detail: Some(format!("entity={} posture={}", req.entity_id, req.posture)),
            });

            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "member not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ============================================================================
// Device handlers
// ============================================================================

async fn devices_register(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<DeviceRegisterRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    // Validate device_type
    match req.device_type.as_str() {
        "node" | "browser" | "mobile" => {}
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "invalid device_type: must be node, browser, or mobile",
            )
                .into_response()
        }
    }

    let device = cordelia_storage::DeviceRow {
        device_id: req.device_id.clone(),
        entity_id: req.entity_id.clone(),
        device_name: req.device_name.clone(),
        device_type: req.device_type,
        auth_token_hash: req.auth_token_hash,
        created_at: String::new(), // Set by DB default
        last_seen_at: None,
        revoked_at: None,
    };

    match state.storage.register_device(&device) {
        Ok(()) => {
            tracing::info!(
                device_id = req.device_id,
                entity_id = req.entity_id,
                "device: registered"
            );
            let _ = state.storage.log_access(&cordelia_storage::AccessLogEntry {
                entity_id: req.entity_id,
                action: "register_device".into(),
                resource_type: "device".into(),
                resource_id: Some(req.device_id),
                group_id: None,
                detail: req.device_name,
            });
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn devices_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<DeviceListRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    match state.storage.list_devices(&req.entity_id) {
        Ok(devices) => {
            // Redact auth_token_hash from response
            let redacted: Vec<serde_json::Value> = devices
                .iter()
                .map(|d| {
                    serde_json::json!({
                        "device_id": d.device_id,
                        "entity_id": d.entity_id,
                        "device_name": d.device_name,
                        "device_type": d.device_type,
                        "created_at": d.created_at,
                        "last_seen_at": d.last_seen_at,
                        "revoked_at": d.revoked_at,
                    })
                })
                .collect();
            Json(serde_json::json!({ "devices": redacted })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn devices_revoke(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<DeviceRevokeRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    match state.storage.revoke_device(&req.entity_id, &req.device_id) {
        Ok(true) => {
            tracing::info!(
                device_id = req.device_id,
                entity_id = req.entity_id,
                "device: revoked"
            );
            let _ = state.storage.log_access(&cordelia_storage::AccessLogEntry {
                entity_id: req.entity_id,
                action: "revoke_device".into(),
                resource_type: "device".into(),
                resource_id: Some(req.device_id),
                group_id: None,
                detail: None,
            });
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "device not found or already revoked").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    let (warm, hot) = if let Some(f) = &state.peer_count_fn {
        f().await
    } else {
        (0, 0)
    };

    let groups = if let Some(sg) = &state.shared_groups {
        sg.read().await.clone()
    } else {
        vec![]
    };

    Json(StatusResponse {
        node_id: state.node_id.clone(),
        entity_id: state.entity_id.clone(),
        uptime_secs: state.start_time.elapsed().as_secs(),
        peers_warm: warm,
        peers_hot: hot,
        groups,
    })
    .into_response()
}

async fn peers(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    let (warm, hot) = if let Some(f) = &state.peer_count_fn {
        f().await
    } else {
        (0, 0)
    };

    let peers = if let Some(f) = &state.peer_list_fn {
        f().await
    } else {
        vec![]
    };

    Json(PeersResponse {
        warm,
        hot,
        total: warm + hot,
        peers,
    })
    .into_response()
}

async fn diagnostics(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    let (warm, hot) = if let Some(f) = &state.peer_count_fn {
        f().await
    } else {
        (0, 0)
    };

    let groups = if let Some(sg) = &state.shared_groups {
        sg.read().await.clone()
    } else {
        vec![]
    };

    let repl = if let Some(stats) = &state.replication_stats {
        serde_json::json!({
            "items_pushed": stats.items_pushed.load(Ordering::Relaxed),
            "items_synced": stats.items_synced.load(Ordering::Relaxed),
            "items_rejected": stats.items_rejected.load(Ordering::Relaxed),
            "items_duplicate": stats.items_duplicate.load(Ordering::Relaxed),
            "push_retries_exhausted": stats.push_retries_exhausted.load(Ordering::Relaxed),
            "sync_rounds": stats.sync_rounds.load(Ordering::Relaxed),
            "sync_rounds_with_diff": stats.sync_rounds_with_diff.load(Ordering::Relaxed),
            "sync_errors": stats.sync_errors.load(Ordering::Relaxed),
            "write_buffer_depth": stats.write_buffer_depth.load(Ordering::Relaxed),
            "pending_push_count": stats.pending_push_count.load(Ordering::Relaxed),
        })
    } else {
        serde_json::json!("not available")
    };

    let mempool = match state.storage.storage_stats() {
        Ok(stats) => {
            let group_details: Vec<serde_json::Value> = stats
                .groups
                .iter()
                .map(|g| {
                    serde_json::json!({
                        "group_id": g.group_id,
                        "items": g.item_count,
                        "data_bytes": g.data_bytes,
                        "members": g.member_count,
                    })
                })
                .collect();
            serde_json::json!({
                "l2_items": stats.l2_item_count,
                "l2_data_bytes": stats.l2_data_bytes,
                "l2_data_human": format_bytes(stats.l2_data_bytes),
                "groups": stats.group_count,
                "group_details": group_details,
            })
        }
        Err(e) => serde_json::json!({ "error": e.to_string() }),
    };

    Json(serde_json::json!({
        "node_id": state.node_id,
        "entity_id": state.entity_id,
        "uptime_secs": state.start_time.elapsed().as_secs(),
        "peers": {
            "warm": warm,
            "hot": hot,
        },
        "groups": groups,
        "replication": repl,
        "mempool": mempool,
    }))
    .into_response()
}

// Need base64 for l1_read fallback
use base64::Engine;

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}
