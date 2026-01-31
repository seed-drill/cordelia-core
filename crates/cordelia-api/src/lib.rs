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
use std::sync::Arc;

/// Notification sent when a local L2 write occurs (for replication dispatch).
#[derive(Debug, Clone)]
pub struct WriteNotification {
    pub item_id: String,
    pub item_type: String,
    pub group_id: Option<String>,
    pub data: Vec<u8>,
    pub key_version: u32,
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
}

/// Build the axum router.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/l1/read", post(l1_read))
        .route("/api/v1/l1/write", post(l1_write))
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
        .route("/api/v1/status", post(status))
        .route("/api/v1/peers", post(peers))
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

fn default_culture() -> String {
    r#"{"broadcast_eagerness":"moderate"}"#.into()
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

    let data = serde_json::to_vec(&req.data).unwrap_or_default();
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
    };

    match state.storage.write_l2_item(&write) {
        Ok(()) => {
            // Notify replication task of the write
            if let Some(tx) = &state.write_notify {
                let _ = tx.send(WriteNotification {
                    item_id: write.id,
                    item_type: write.item_type,
                    group_id: write.group_id,
                    data: write.data,
                    key_version: write.key_version as u32,
                });
            }
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
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

    match state.storage.delete_l2_item(&req.item_id) {
        Ok(true) => {
            // TODO: trigger tombstone replication
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
            // Push new group into shared dynamic groups
            if let Some(shared) = &state.shared_groups {
                let mut groups = shared.write().await;
                if !groups.contains(&req.group_id) {
                    groups.push(req.group_id.clone());
                }
            }
            Json(serde_json::json!({
                "ok": true,
                "group_id": req.group_id,
            }))
            .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn groups_list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }

    match state.storage.list_groups() {
        Ok(groups) => Json(serde_json::json!({ "groups": groups })).into_response(),
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

    match state.storage.delete_group(&req.group_id) {
        Ok(true) => {
            // Remove from shared dynamic groups
            if let Some(shared) = &state.shared_groups {
                let mut groups = shared.write().await;
                groups.retain(|g| g != &req.group_id);
            }

            // Log access
            let _ = state.storage.log_access(&cordelia_storage::AccessLogEntry {
                entity_id: state.entity_id.clone(),
                action: "delete_group".into(),
                resource_type: "group".into(),
                resource_id: Some(req.group_id.clone()),
                group_id: Some(req.group_id),
                detail: None,
            });

            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, "group not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
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

// Need base64 for l1_read fallback
use base64::Engine;
