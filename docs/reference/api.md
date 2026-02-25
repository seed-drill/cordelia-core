# API Reference -- cordelia-node HTTP Endpoints

All endpoints are POST requests on port 9473 (default). Every request requires a `Bearer` token in the `Authorization` header.

**Source of truth**: `crates/cordelia-api/src/lib.rs`

---

## Authentication

All endpoints check the `Authorization` header:

```
Authorization: Bearer <token>
```

The token is configured in the node's config or via `BEARER_TOKEN` environment variable. Returns `401 Unauthorized` if missing or invalid.

---

## L1 -- Hot Context

Entity identity blobs (~50KB). Stored as opaque BLOBs, encrypted at the proxy layer.

### POST /api/v1/l1/read

Read an entity's L1 hot context.

**Request:**
```json
{ "user_id": "russell" }
```

**Response (200):** The raw JSON blob (if valid JSON) or base64-encoded data:
```json
{ "data": "<base64-encoded blob>" }
```

**Errors:** `404` if not found.

### POST /api/v1/l1/write

Write (upsert) an entity's L1 hot context.

**Request:**
```json
{
  "user_id": "russell",
  "data": { "name": "Russell", "roles": ["founder"] }
}
```

**Response (200):**
```json
{ "ok": true }
```

**Notes:** The `data` field accepts any JSON value. It is serialised to bytes and stored as-is.

### POST /api/v1/l1/delete

Delete an entity's L1 hot context.

**Request:**
```json
{ "user_id": "russell" }
```

**Response (200):**
```json
{ "ok": true }
```

**Errors:** `404` if not found.

### POST /api/v1/l1/list

List all entity IDs with L1 data.

**Request:**
```json
{}
```

**Response (200):**
```json
{ "users": ["russell", "martin", "bill"] }
```

---

## L2 -- Memory Items

Persistent memory items (learning, entity, session). Encrypted at the proxy layer. Items with a `group_id` participate in P2P replication.

### POST /api/v1/l2/read

Read an item by ID.

**Request:**
```json
{ "item_id": "mem-abc123" }
```

**Response (200):**
```json
{
  "data": { ... },
  "type": "learning",
  "meta": {
    "owner_id": "russell",
    "visibility": "group",
    "group_id": "team-alpha",
    "author_id": "russell",
    "key_version": 1,
    "checksum": "a1b2c3...",
    "parent_id": null,
    "is_copy": false
  }
}
```

**Errors:** `404` if not found.

### POST /api/v1/l2/write

Write (upsert) a memory item. Triggers replication for items with a `group_id`.

**Request:**
```json
{
  "item_id": "mem-abc123",
  "type": "learning",
  "data": { "content": "..." },
  "meta": {
    "owner_id": "russell",
    "visibility": "group",
    "group_id": "team-alpha",
    "author_id": "russell",
    "key_version": 1,
    "parent_id": null,
    "is_copy": false
  }
}
```

All `meta` fields are optional. Defaults:
- `visibility`: `"private"`
- `author_id`: node's `entity_id` if not provided
- `key_version`: `1`
- `is_copy`: `false`

**Response (200):**
```json
{ "ok": true }
```

**Errors:**
- `400` if data cannot be serialised
- `413 Payload Too Large` if serialised data exceeds 16 KB (`MAX_ITEM_BYTES`)

**Side effects:** Emits a `WriteNotification` to the replication task. For chatty groups, the item is eagerly pushed to all active group peers and relay peers.

### POST /api/v1/l2/delete

Delete a memory item. Triggers tombstone replication for grouped items.

**Request:**
```json
{ "item_id": "mem-abc123" }
```

**Response (200):**
```json
{ "ok": true }
```

**Errors:** `404` if not found.

**Side effects:** If the deleted item had a `group_id`, a tombstone (`item_type: "__tombstone__"`) is broadcast to peers, causing them to delete their local copy.

### POST /api/v1/l2/search

Full-text search across L2 items (FTS5 BM25).

**Request:**
```json
{
  "query": "founder seed drill",
  "limit": 20,
  "group_id": "team-alpha"
}
```

- `limit`: max results, default `20`
- `group_id`: optional filter, only return items belonging to this group

**Response (200):**
```json
{ "results": ["mem-abc123", "mem-def456"] }
```

Returns item IDs only. Use `l2/read` to fetch full items.

**Notes:** When `group_id` is set, the query fetches `limit * 4` results from FTS and post-filters by group membership to ensure enough results after filtering.

---

## Groups

Group descriptors and membership. Descriptors propagate via P2P GroupExchange. Membership is local-only (R4-030).

### POST /api/v1/groups/create

Create a new group. Adds to local storage and `shared_groups` (enables replication).

**Request:**
```json
{
  "group_id": "team-alpha",
  "name": "Team Alpha",
  "culture": "{\"broadcast_eagerness\":\"chatty\"}",
  "security_policy": "{}"
}
```

- `culture`: JSON string, default `{"broadcast_eagerness":"chatty"}`
- `security_policy`: JSON string, default `{}`

**Response (200):**
```json
{ "ok": true, "group_id": "team-alpha" }
```

**Side effects:** Group added to `shared_groups`. Descriptor will be included in next GroupExchange cycle (~60s) and propagate to peers.

### POST /api/v1/groups/list

List all groups known to this node (includes groups received via GroupExchange).

**Request:**
```json
{}
```

**Response (200):**
```json
{
  "groups": [
    {
      "id": "team-alpha",
      "name": "Team Alpha",
      "culture": "{\"broadcast_eagerness\":\"chatty\"}",
      "security_policy": "{}",
      "created_at": "2026-02-25T12:00:00",
      "updated_at": "2026-02-25T12:00:00",
      "owner_id": "russell",
      "owner_pubkey": "a1b2c3...",
      "signature": "d4e5f6..."
    }
  ]
}
```

### POST /api/v1/groups/read

Read a group descriptor and its members.

**Request:**
```json
{ "group_id": "team-alpha" }
```

**Response (200):**
```json
{
  "group": {
    "id": "team-alpha",
    "name": "Team Alpha",
    "culture": "{\"broadcast_eagerness\":\"chatty\"}",
    "security_policy": "{}",
    "created_at": "2026-02-25T12:00:00",
    "updated_at": "2026-02-25T12:00:00",
    "owner_id": null,
    "owner_pubkey": null,
    "signature": null
  },
  "members": [
    {
      "group_id": "team-alpha",
      "entity_id": "russell",
      "role": "owner",
      "posture": "active",
      "joined_at": "2026-02-25T12:00:00"
    }
  ]
}
```

**Errors:** `404` if group not found.

**Notes:** Members are local-only. Other nodes may have different member lists for the same group.

### POST /api/v1/groups/items

List item headers for a group (used by the sync protocol).

**Request:**
```json
{
  "group_id": "team-alpha",
  "since": "2026-02-25T12:00:00",
  "limit": 100
}
```

- `since`: optional ISO 8601 timestamp for incremental sync (items updated after this time)
- `limit`: max results, default `20`

**Response (200):**
```json
{
  "items": [
    {
      "item_id": "mem-abc123",
      "item_type": "learning",
      "checksum": "a1b2c3...",
      "updated_at": "2026-02-25T12:30:00",
      "author_id": "russell",
      "is_deletion": false
    }
  ]
}
```

### POST /api/v1/groups/delete

Tombstone a group. Writes a deletion marker (`culture = "__deleted__"`) that propagates to peers via GroupExchange. Members are soft-removed (`posture = "removed"`). The group row is retained as a tombstone until GC purges it after the retention window (default 7 days).

**Request:**
```json
{ "group_id": "team-alpha" }
```

**Response (200):**
```json
{ "ok": true }
```

**Errors:** `404` if group not found.

**Side effects:** Group removed from `shared_groups` (stops replication). Tombstone descriptor propagates via GroupExchange -- receiving peers auto-remove members and stop replicating. L2 items with this group_id are NOT deleted. Daily GC purges tombstoned groups past retention.

### POST /api/v1/groups/add_member

Add a member to a group on this node.

**Request:**
```json
{
  "group_id": "team-alpha",
  "entity_id": "alice",
  "role": "member"
}
```

- `role`: `owner`, `admin`, `member` (default), or `viewer`

**Response (200):**
```json
{ "ok": true }
```

**Errors:**
- `400` if role is invalid

**Notes:** Upsert pattern -- calling again with a different role updates the existing membership. Local-only: does not propagate to other nodes. If the entity has no L1 entry, a minimal stub (`{}`) is auto-created to satisfy the FK constraint.

### POST /api/v1/groups/remove_member

Soft-remove a member from a group on this node. Sets `posture = "removed"` (CoW -- no hard delete). The member row is retained but filtered from `list_members` and `get_membership` responses.

**Request:**
```json
{
  "group_id": "team-alpha",
  "entity_id": "alice"
}
```

**Response (200):**
```json
{ "ok": true }
```

**Errors:** `404` if member not found (or already removed).

**Notes:** Local-only -- does not propagate to other nodes. Portal must call on all nodes independently. No key rotation or item cleanup (see [member removal design](../design/member-removal.md)).

### POST /api/v1/groups/update_posture

Change a member's broadcast posture.

**Request:**
```json
{
  "group_id": "team-alpha",
  "entity_id": "alice",
  "posture": "silent"
}
```

- `posture`: `active`, `silent`, or `emcon`

**Response (200):**
```json
{ "ok": true }
```

**Errors:**
- `400` if posture is invalid
- `404` if member not found

---

## Devices

Device registration for portal enrollment (RFC 8628).

### POST /api/v1/devices/register

Register a new device.

**Request:**
```json
{
  "device_id": "dev-abc123",
  "entity_id": "russell",
  "device_name": "MacBook Pro",
  "device_type": "node",
  "auth_token_hash": "sha256..."
}
```

- `device_type`: `node` (default), `browser`, or `mobile`
- `auth_token_hash`: SHA-256 of the bearer token (raw token never stored)

**Response (200):**
```json
{ "ok": true }
```

**Errors:** `400` if device_type is invalid.

### POST /api/v1/devices/list

List devices for an entity.

**Request:**
```json
{ "entity_id": "russell" }
```

**Response (200):**
```json
{
  "devices": [
    {
      "device_id": "dev-abc123",
      "entity_id": "russell",
      "device_name": "MacBook Pro",
      "device_type": "node",
      "created_at": "2026-02-25T12:00:00",
      "last_seen_at": null,
      "revoked_at": null
    }
  ]
}
```

**Notes:** `auth_token_hash` is redacted from the response.

### POST /api/v1/devices/revoke

Revoke a device (soft delete).

**Request:**
```json
{
  "entity_id": "russell",
  "device_id": "dev-abc123"
}
```

**Response (200):**
```json
{ "ok": true }
```

**Errors:** `404` if device not found or already revoked.

---

## Diagnostics

Node health, peer state, and replication metrics.

### POST /api/v1/status

Lightweight status check.

**Request:**
```json
{}
```

**Response (200):**
```json
{
  "node_id": "12D3KooWxyz...",
  "entity_id": "russell",
  "uptime_secs": 3600,
  "peers_warm": 5,
  "peers_hot": 2,
  "groups": ["team-alpha", "shared-xorg"]
}
```

### POST /api/v1/peers

Detailed peer list.

**Request:**
```json
{}
```

**Response (200):**
```json
{
  "warm": 5,
  "hot": 2,
  "total": 7,
  "peers": [
    {
      "node_id": "12D3KooWabc...",
      "addrs": ["/ip4/192.168.1.100/udp/9474/quic-v1"],
      "state": "hot",
      "rtt_ms": 45.5,
      "items_delivered": 123,
      "groups": ["team-alpha"],
      "group_intersection": ["team-alpha"],
      "is_relay": false,
      "protocol_version": 1
    }
  ]
}
```

### POST /api/v1/diagnostics

Full diagnostics including replication stats and storage metrics.

**Request:**
```json
{}
```

**Response (200):**
```json
{
  "node_id": "12D3KooWxyz...",
  "entity_id": "russell",
  "uptime_secs": 3600,
  "peers": { "warm": 5, "hot": 2 },
  "groups": ["team-alpha", "shared-xorg"],
  "replication": {
    "items_pushed": 500,
    "items_synced": 300,
    "items_rejected": 5,
    "items_duplicate": 10,
    "push_retries_exhausted": 2,
    "sync_rounds": 50,
    "sync_rounds_with_diff": 15,
    "sync_errors": 1,
    "write_buffer_depth": 3,
    "pending_push_count": 2
  },
  "mempool": {
    "l2_items": 1500,
    "l2_data_bytes": 52428800,
    "l2_data_human": "50.0 MB",
    "groups": 10,
    "group_details": [
      {
        "group_id": "team-alpha",
        "items": 150,
        "data_bytes": 5242880,
        "members": 5
      }
    ]
  }
}
```

**Replication counters:**

| Field | Description |
|-------|-------------|
| `items_pushed` | Items pushed to peers (counted per item) |
| `items_synced` | Items received via anti-entropy sync |
| `items_rejected` | Items rejected on receive (integrity, membership, size) |
| `items_duplicate` | Items received that were already stored |
| `push_retries_exhausted` | Push retries that failed all attempts |
| `sync_rounds` | Anti-entropy sync rounds completed |
| `sync_rounds_with_diff` | Sync rounds that found missing items |
| `sync_errors` | Sync rounds that failed (no peer, error) |
| `write_buffer_depth` | Items in write coalescing buffer (gauge) |
| `pending_push_count` | Pending push retries in queue (gauge) |

---

## Error Responses

All error responses return an appropriate HTTP status code with a plain text body:

| Status | Meaning |
|--------|---------|
| `400` | Bad request (invalid role, posture, device_type, or data) |
| `401` | Unauthorized (missing or invalid bearer token) |
| `404` | Not found (item, group, member, or device) |
| `413` | Payload too large (L2 item exceeds 16 KB) |
| `500` | Internal server error (storage failure) |
