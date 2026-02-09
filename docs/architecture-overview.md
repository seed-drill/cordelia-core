# Cordelia Architecture Overview

See [architecture-diagram.drawio](architecture-diagram.drawio) for the visual diagram
(open in [diagrams.net](https://app.diagrams.net)).

## Component Summary

```
Entity (Human + LLM)
  |
  | stdio (MCP / JSON-RPC)
  v
@cordelia/proxy (TypeScript)         cordelia-portal
  - 25 MCP tools                       - Auth, enrollment, vault
  - HTTP REST sidecar (:3847)  <----   - Device management
  - Novelty engine                     - Groups UI
  - Embedding (Ollama)
  - Encryption boundary (AES-256-GCM)
  |
  | SQLite (WAL, schema v4)
  |
  v
cordelia-node (Rust)
  - cordelia-api (Axum, 11 endpoints)
  - Governor (cold/warm/hot/banned)
  - Replication (culture-governed, anti-entropy)
  - QUIC transport (quinn, UDP 9474)
```

## Key Principles

- **Proxy is the universal API gateway.** Portal, Claude Code, and any MCP-capable
  client all talk to the proxy. The node is dumb transport (TCP/IP analogy).
- **Encryption boundary sits in the proxy.** The node never sees plaintext.
- **Portal never talks to the node directly.** All memory and group operations
  go through the proxy HTTP sidecar.
- **Entity sovereignty.** Each entity holds its own keys and controls its own data.

## Data Flow

1. **Claude Code** connects to proxy via stdio (MCP JSON-RPC)
2. **Portal** connects to proxy via HTTP REST (localhost:3847)
3. **Proxy** delegates L1/L2/group storage to node via HTTP (`CORDELIA_STORAGE=node`)
4. **Proxy** keeps FTS, embedding cache, and audit log in local SQLite
5. **Node** handles persistent storage, QUIC transport, governor, and replication

## Schema (SQLite v4)

| Table | Purpose |
|-------|---------|
| `l1_hot` | Entity identity (~50KB) |
| `l2_items` | All memories (encrypted blob, group_id, author_id) |
| `l2_fts` | FTS5 full-text search (BM25, porter) |
| `embedding_cache` | Content hash to vector |
| `groups` + `group_members` | Group model (culture, security_policy, roles) |
| `access_log` + `audit` | Access tracking and audit trail |
| `integrity_canary` | Tamper detection |
| `schema_version` | Migration tracking |

## Replication Model

**Group-scoped items replicate. Private items do not.** This is by design.

Items with `group_id = NULL` (visibility = 'private') never enter the replication
engine. The replication engine's `on_receive()` enforces group membership, and the
API's `WriteNotification` only fires for items with a `group_id`. This means:

- L2 items written without a group are local-only (no P2P sync)
- L1 hot context (stored in `l1_hot` table) does not replicate
- Only group-scoped items participate in culture-governed replication

**R5 Personal Groups** (see `docs/design/R5-personal-groups.md`) is the planned
unification: every item belongs to a group, "private" = personal group encrypted
with a PSK that keepers store but cannot decrypt. Until R5 lands, private items
exist only on the device where they were created.

## Tombstone Replication

When an L2 item is deleted via the API, a **tombstone** is broadcast to peers:

- The `l2_delete` handler reads the item's `group_id` before deleting locally
- A `WriteNotification` with `item_type = "__tombstone__"` is dispatched
- The replication engine's eager-push path delivers the tombstone to peers
- On receive, `on_receive()` recognises the `__tombstone__` type and deletes
  the local copy (group membership is still enforced)

Tombstones travel through the existing replication pipeline -- no separate
protocol. The known limitation is that peers offline during the eager push
window will not see the tombstone until a full anti-entropy sync (which
currently does not propagate deletions). This will be addressed when
anti-entropy gains tombstone awareness.
