# Cordelia - High Level Design

**Version 1.0 -- Seed Drill (https://seeddrill.ai)**

---

## 1. System Overview

Cordelia is a distributed persistent memory system. Four components:

```
    AGENTS                     PROXY                    NODE                      NETWORK
    ------                     -----                    ----                      -------

    Claude Code ─┐                                     cordelia-node (Rust)
    Swarm Agent ─┤─ stdio ─> @cordelia/proxy (TS)      │
    Other MCP   ─┘            │                        ├─ HTTP API :9473
                              ├─ MCP stdio (agents)    ├─ QUIC P2P :9474
    Browser ─────── HTTP ───> ├─ HTTP :3847 (dashboard)├─ SQLite storage
                              ├─ HTTP client ──────────┘
                              └─ Encryption boundary
```

**@cordelia/proxy** (TypeScript): MCP proxy + dashboard HTTP server. Three
interfaces: (1) MCP stdio to agents, (2) HTTP server for dashboard/enrollment/admin,
(3) HTTP client to local Rust node. Holds encryption keys. Role-configurable via TOML.

**cordelia-node** (Rust): The node. Stores everything in SQLite. Replicates to peers
via QUIC. Manages peer lifecycle (governor). Exposes HTTP API for local clients.
Never holds plaintext.

**Repos**:
- `cordelia-core` (Rust) -- node, protocol, storage, crypto, all 7 crates
- `cordelia-proxy` (TypeScript) -- proxy, dashboard, hooks, skills, scripts

---

## 2. Component Map

```
    ┌─────────────────────────────────────────────────────────────────┐
    │                      @cordelia/proxy (TS)                       │
    │                                                                 │
    │  ┌───────────┐  ┌────────────┐  ┌──────────┐  ┌────────────┐  │
    │  │ MCP       │  │ Dashboard  │  │ L0 Cache │  │ Node       │  │
    │  │ Handler   │  │ HTTP       │  │ (in-mem) │  │ Client     │  │
    │  │           │  │ Server     │  │          │  │ (HTTP)     │  │
    │  │ 25 tools  │  │           │  │ L1 hot   │  │            │  │
    │  │ stdio     │  │ Dashboard  │  │ context  │  │ Bearer     │  │
    │  │ transport │  │ Enrollment │  │ Recent   │  │ token      │  │
    │  │           │  │ Admin API  │  │ L2 hits  │  │ Retry      │  │
    │  └─────┬─────┘  └─────┬─────┘  └────┬─────┘  └─────┬──────┘  │
    │        │              │             │              │           │
    │  ┌─────┴──────────────┴─────────────┴──────────────┘           │
    │  │                                                              │
    │  │  ┌────────────┐  ┌────────────┐  ┌────────────┐            │
    │  │  │ Encryption │  │ Novelty    │  │ Schema     │            │
    │  │  │ Boundary   │  │ Engine     │  │ Validation │            │
    │  │  │ AES-256-GCM│  │ Pattern    │  │ Zod types  │            │
    │  │  │ scrypt KDF │  │ matching   │  │            │            │
    │  │  └────────────┘  └────────────┘  └────────────┘            │
    │  │                                                              │
    │  │  Role Config (from TOML):                                    │
    │  │  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐   │
    │  │  │personal│ │boot    │ │relay   │ │keeper  │ │archive │   │
    │  │  │(default│ │node    │ │        │ │        │ │        │   │
    │  │  │minimal)│ │+status │ │+relay  │ │+shards │ │+L3 API │   │
    │  │  └────────┘ └────────┘ └────────┘ └────────┘ └────────┘   │
    │  └──────────────────────────────────────────────────────────┘  │
    │                          │                                     │
    └──────────────────────────┼─────────────────────────────────────┘
                               │ HTTP (localhost:9473)
                               │ Bearer token auth
                               ▼
    ┌─────────────────────────────────────────────────────────────────┐
    │                      cordelia-node (Rust)                       │
    │                                                                 │
    │  ┌──────────────────────────────────────────────────────────┐  │
    │  │  cordelia-api (Axum HTTP server)                         │  │
    │  │                                                          │  │
    │  │  POST /api/v1/l1/{read,write}                           │  │
    │  │  POST /api/v1/l2/{read,write,delete,search}             │  │
    │  │  POST /api/v1/groups/{create,list,read,items}           │  │
    │  │  POST /api/v1/device/{begin,poll,register}              │  │
    │  │  POST /api/v1/status                                    │  │
    │  │  POST /api/v1/peers                                     │  │
    │  │                                                          │  │
    │  │  On L2 write: broadcasts WriteNotification to repl task │  │
    │  └──────────────────────────┬───────────────────────────────┘  │
    │                              │                                  │
    │    ┌─────────────────────────┼──────────────────────────────┐  │
    │    │                         ▼                              │  │
    │    │  ┌──────────────┐  ┌──────────────┐  ┌─────────────┐ │  │
    │    │  │  Governor    │  │  Replication  │  │  QUIC       │ │  │
    │    │  │  Task        │  │  Task         │  │  Transport  │ │  │
    │    │  │              │  │               │  │             │ │  │
    │    │  │  Peer state: │  │  Write        │  │  Accept     │ │  │
    │    │  │  cold/warm/  │  │  dispatch     │  │  loop       │ │  │
    │    │  │  hot/banned  │  │  (culture-    │  │             │ │  │
    │    │  │              │  │   governed)   │  │  Dial       │ │  │
    │    │  │  Churn       │  │               │  │  (outbound) │ │  │
    │    │  │  Backoff     │  │  Anti-entropy │  │             │ │  │
    │    │  │  Promotion   │  │  sync loop    │  │  5 mini-    │ │  │
    │    │  │  Demotion    │  │               │  │  protocols  │ │  │
    │    │  └──────────────┘  └──────────────┘  └─────────────┘ │  │
    │    │           4 async tokio tasks                         │  │
    │    └──────────────────────────────────────────────────────┘  │
    │                              │                                  │
    │    ┌─────────────────────────┼──────────────────────────────┐  │
    │    │                         ▼                              │  │
    │    │  ┌──────────────┐  ┌──────────────┐  ┌─────────────┐ │  │
    │    │  │  cordelia-   │  │  cordelia-   │  │  cordelia-  │ │  │
    │    │  │  storage     │  │  crypto      │  │  protocol   │ │  │
    │    │  │              │  │              │  │             │ │  │
    │    │  │  SQLite      │  │  AES-256-GCM │  │  Message    │ │  │
    │    │  │  WAL mode    │  │  Ed25519     │  │  types      │ │  │
    │    │  │  Schema v4   │  │  scrypt KDF  │  │  Codec      │ │  │
    │    │  │  FTS5 search │  │              │  │  (len+JSON) │ │  │
    │    │  └──────────────┘  └──────────────┘  └─────────────┘ │  │
    │    │           Shared libraries                            │  │
    │    └──────────────────────────────────────────────────────┘  │
    │                                                                 │
    └─────────────────────────────────────────────────────────────────┘
```

---

## 3. Interface Contracts

### 3.1 Proxy <-> Node (HTTP API)

This is the **primary boundary**. Both sides must agree on this contract.
The API already exists in `cordelia-api`. The proxy is a client of it.

**Auth**: `Authorization: Bearer <token>` on every request.
Token loaded from `~/.cordelia/node-token`.

**Content-Type**: `application/json` for all requests and responses.

#### L1 Hot Context

```
POST /api/v1/l1/read
  Request:  { "user_id": "russell_wing" }
  Response: { "data": <encrypted_blob_or_json> }

POST /api/v1/l1/write
  Request:  { "user_id": "russell_wing", "data": <blob> }
  Response: { "ok": true }
```

#### L2 Warm Index

```
POST /api/v1/l2/read
  Request:  { "item_id": "ml21eqjx-kspne0" }
  Response: { "data": <item_json>, "type": "learning",
              "meta": { "owner_id", "visibility", "group_id",
                        "author_id", "key_version", "checksum" } }

POST /api/v1/l2/write
  Request:  { "item_id": "...", "type": "entity",
              "data": <item_json>,
              "meta": { "owner_id", "visibility", "group_id",
                        "author_id", "key_version" } }
  Response: { "ok": true }
  Side effect: WriteNotification broadcast -> replication task

POST /api/v1/l2/delete
  Request:  { "item_id": "..." }
  Response: { "ok": true }

POST /api/v1/l2/search
  Request:  { "query": "game theory", "limit": 20 }
  Response: { "results": [{ "id", "type", "score" }] }
```

#### Groups

```
POST /api/v1/groups/create
  Request:  { "id": "seed-drill", "name": "Seed Drill",
              "culture": { "broadcast_eagerness": "moderate",
                           "ttl_default": 86400 },
              "security_policy": "..." }
  Response: { "ok": true }

POST /api/v1/groups/list
  Response: { "groups": [{ "id", "name", "culture", "created_at" }] }

POST /api/v1/groups/read
  Request:  { "group_id": "seed-drill" }
  Response: { "group": { "id", "name", "culture", "security_policy" },
              "members": [{ "entity_id", "role", "posture", "joined_at" }] }

POST /api/v1/groups/items
  Request:  { "group_id": "seed-drill", "since": "2026-01-30T00:00:00Z",
              "limit": 100 }
  Response: { "items": [ItemHeader], "has_more": bool }
```

#### Device Enrollment

```
POST /api/v1/device/begin
  Request:  { "client_id": "cordelia-proxy" }
  Response: { "device_code": "ABCD-EFGH",
              "verification_uri": "https://dash.cordelia.seeddrill.ai/enroll",
              "expires_in": 600,
              "interval": 5 }

POST /api/v1/device/poll
  Request:  { "device_code": "ABCD-EFGH" }
  Response (pending):  { "status": "pending" }
  Response (complete): { "status": "complete",
                         "bearer_token": "ck_<64hex>",
                         "entity_id": "russell",
                         "node_url": "http://127.0.0.1:9473" }
  Response (expired):  { "status": "expired" }

POST /api/v1/device/register
  Request:  { "github_token": "<oauth_token>", "device_code": "ABCD-EFGH" }
  Response: { "ok": true, "entity_id": "russell" }
  Note: Called by dashboard after GitHub OAuth, not by proxy directly.
```

#### Status

```
POST /api/v1/status
  Response: { "node_id": "3d2238...", "entity_id": "russell",
              "role": "personal",
              "capabilities": { "keeper": false, "archive": false, "relay": false },
              "uptime_secs": 3600,
              "peers_warm": 2, "peers_hot": 1,
              "groups": ["seed-drill"],
              "version": "0.1.0" }

POST /api/v1/peers
  Response: { "peers": [{ "node_id", "addr", "state", "rtt_ms",
                           "score", "groups", "capabilities" }],
              "summary": { "warm": 2, "hot": 1, "cold": 5, "total": 8 } }
```

### 3.2 Node <-> Peer (QUIC P2P)

Five mini-protocols, multiplexed on QUIC streams via protocol byte prefix:

| Byte | Protocol | Purpose |
|------|----------|---------|
| 0x01 | Handshake | Identity exchange, version negotiation |
| 0x02 | Keep-Alive | Ping/pong, RTT measurement |
| 0x03 | Peer-Share | Exchange known peer addresses |
| 0x04 | Memory-Sync | Header-based diff for anti-entropy |
| 0x05 | Memory-Fetch | Bulk item retrieval by ID |

Wire format: 4-byte big-endian length prefix + serde JSON payload. Max 16MB.

### 3.3 Proxy internal: Encryption Boundary

The proxy handles encryption/decryption. The node stores opaque blobs.
The proxy owns the user's key (via `CORDELIA_ENCRYPTION_KEY`).

```
  Agent -> proxy: "write this learning"
  Proxy: validate schema, encrypt content, compute checksum
  Proxy -> node: POST /api/v1/l2/write { data: encrypted_blob }
  Node: store blob, broadcast to peers
  Peers: receive and store blob (never decrypt)
```

This means: **the Rust node never holds plaintext**. The crypto boundary
lives in the proxy. The node is a dumb (but reliable) encrypted blob store
with replication.

### 3.4 Novelty Engine (proxy-side)

Stays in the proxy. Pattern matching + confidence scoring.
No dependency on the Rust node. Runs before persistence decisions.

### 3.5 Embedding / Search

**R3 approach**: FTS5 keyword search only in the Rust node. The proxy
can optionally run local embeddings (Ollama) and do client-side hybrid
ranking. This keeps the Rust node simple.

**R4 approach**: Move embedding + vector search into the Rust node
(rust-bert or candle). Not now.

### 3.6 Dashboard HTTP API (port 3847)

The proxy serves a dashboard HTTP server for human interaction.
This is Martin's primary interface contract -- the API he builds the UI against.

**Auth**: GitHub OAuth 2.0 or session cookie. API keys for CLI uploads.

#### Authentication

```
GET  /auth/status
  Response: { "authenticated": bool, "entity_id": "russell",
              "github_id": "russwing", "org": "seed_drill" }

GET  /auth/github
  Redirects to GitHub OAuth. Query: ?redirect=/dashboard
  Scopes: read:user, read:org

GET  /auth/github/callback
  Handles OAuth code exchange. Sets session cookie.
  Redirects to original ?redirect target.

POST /auth/logout
  Clears session cookie.
  Response: { "ok": true }
```

#### Entity Profile

```
GET  /dash/api/profile/:entityId
  Auth: session cookie (own profile) or admin role
  Response: { "entity_id", "github_id", "name", "org", "roles",
              "session_count", "created_at", "last_active" }

POST /dash/api/profile/:entityId/api-key
  Auth: session cookie (own profile only)
  Response: { "api_key": "ck_<64hex>" }
  Note: Regenerates key. Old key invalidated immediately.

DELETE /dash/api/profile/:entityId
  Auth: session cookie (own profile only)
  Response: { "ok": true, "items_deleted": 0 }
  Query: ?delete_items=true to also delete L2 items

GET  /dash/api/profile/:entityId/export
  Auth: session cookie (own profile only)
  Response: JSON blob of L1 + all owned L2 items (decrypted)
  Note: Proxy decrypts before serving. Large response.
```

#### Memory (Dashboard View)

```
GET  /dash/api/hot/:entityId
  Auth: session cookie
  Response: { "data": <decrypted_l1_json> }

GET  /dash/api/l2/search
  Auth: session cookie
  Query: ?q=game+theory&type=learning&limit=20
  Response: { "results": [{ "id", "type", "name", "tags", "score",
                             "group_id", "created_at" }] }

GET  /dash/api/l2/item/:id
  Auth: session cookie + visibility check
  Response: { "data": <decrypted_item>, "meta": { ... } }
```

#### Groups (Dashboard View)

```
GET  /dash/api/groups
  Auth: session cookie
  Response: { "groups": [{ "id", "name", "role", "member_count",
                            "culture", "created_at" }] }
  Note: Only groups the authenticated entity is a member of.

GET  /dash/api/groups/:groupId
  Auth: session cookie + membership check
  Response: { "group": { ... }, "members": [...],
              "recent_items": [...], "stats": { ... } }

GET  /dash/api/groups/:groupId/items
  Auth: session cookie + membership check
  Query: ?since=2026-01-30T00:00:00Z&limit=50
  Response: { "items": [ItemHeader], "has_more": bool }
```

#### Node Status (Dashboard View)

```
GET  /dash/api/status
  Auth: session cookie
  Response: { "node": { "id", "role", "capabilities", "uptime",
                         "version" },
              "peers": { "warm", "hot", "cold", "total" },
              "storage": { "l1_users", "l2_items", "groups",
                           "db_size_bytes" },
              "encryption": { "enabled", "provider" } }

GET  /dash/api/peers
  Auth: session cookie + admin role
  Response: { "peers": [{ "node_id", "addr", "state", "rtt_ms",
                           "score", "groups", "capabilities",
                           "connected_since" }] }
```

#### Admin (Multi-Tenant)

```
GET  /dash/api/admin/entities
  Auth: session cookie + admin role
  Response: { "entities": [{ "id", "github_id", "name", "org",
                              "session_count", "last_active",
                              "l2_item_count" }] }

GET  /dash/api/admin/stats
  Auth: session cookie + admin role
  Response: { "entities": N, "groups": N, "l2_items": N,
              "db_size_bytes": N, "uptime_secs": N,
              "peers": { "warm", "hot", "cold" } }

GET  /dash/api/admin/audit
  Auth: session cookie + admin role
  Query: ?since=2026-01-30T00:00:00Z&limit=100
  Response: { "entries": [{ "timestamp", "entity_id", "action",
                             "target_type", "target_id", "group_id",
                             "result" }] }
```

#### Enrollment (Device Authorization)

```
GET  /enroll
  Serves enrollment page. Entity enters device code, authenticates
  via GitHub OAuth, dashboard calls node to complete registration.

POST /enroll/verify
  Request: { "device_code": "ABCD-EFGH" }
  Auth: session cookie (GitHub OAuth must be complete)
  Response: { "ok": true, "entity_id": "russell" }
  Side effect: Calls POST /api/v1/device/register on Rust node.
```

### 3.7 Role-Specific API Surfaces

The proxy exposes additional endpoints based on its configured role.
Default (`personal`) exposes only the base dashboard. Other roles add:

| Role | Additional Endpoints | Purpose |
|------|---------------------|---------|
| `personal` | (base only) | Default. Personal entity dashboard. |
| `bootnode` | `/dash/api/peers` (all users) | Peer visibility for ops. |
| `keeper` | `/dash/api/keeper/*` | Shard management, enrollment monitoring. R4. |
| `archive` | `/dash/api/archive/*` | Storage usage, retention, lineage. R4. |

Keeper and archive endpoints are defined here for completeness but
implemented in R4:

```
GET  /dash/api/keeper/shards
  Response: { "shards": [{ "entity_id", "shard_id", "stored_at",
                            "size_bytes", "healthy": bool }],
              "total": N, "capacity": N }

GET  /dash/api/keeper/enrollments
  Response: { "recent": [{ "entity_id", "device_code", "status",
                            "timestamp" }] }

GET  /dash/api/archive/stats
  Response: { "total_items": N, "total_bytes": N,
              "groups": [{ "id", "item_count", "bytes" }],
              "retention_policy": { ... } }

GET  /dash/api/archive/lineage/:itemId
  Response: { "chain": [{ "id", "author_id", "timestamp",
                           "parent_id" }] }
```

### 3.8 Static Dashboard Pages

Martin builds these. The proxy serves them from `/dashboard/`.

| Page | URL | Purpose |
|------|-----|---------|
| Landing | `/` | Marketing/status (unauthenticated) |
| Login | `/login` | GitHub OAuth trigger |
| Dashboard | `/dashboard` | Authenticated entity view (identity, memory, groups) |
| Admin | `/admin` | Admin panel (entities, stats, audit) -- role-gated |
| Enroll | `/enroll` | Device enrollment (enter code, OAuth, complete) |
| Groups | `/groups` | Group management (list, create, members, culture) |
| Peers | `/peers` | Peer visibility (admin/bootnode only) |

---

## 4. Work Packages (Updated)

### WP1: @cordelia/proxy MCP + Core (R3) -- Russell

**What**: Proxy MCP server, encryption boundary, node client, novelty engine.
Russell builds this weekend using existing TypeScript code as base.

**Scope**:
- MCP server via `@modelcontextprotocol/sdk` (stdio transport)
- HTTP client to `localhost:9473` (bearer token auth)
- L0 in-memory cache (L1 hot context + recent L2 search results)
- 25 MCP tools (same interface as current `server.ts`)
- Encryption/decryption boundary (AES-256-GCM, scrypt)
- Novelty engine (pattern matching, scoring)
- Schema validation (Zod types)
- Role-aware config loading from TOML
- Embeddings (Ollama, optional)

**Does NOT include**:
- Dashboard HTML/CSS/JS (that's Martin, WP2)
- Enrollment flow UI (that's Martin, WP2)
- Admin panel UI (that's Martin, WP2)
- P2P networking (that's the Rust node)
- SQLite storage (that's the Rust node)

**Deliverables**:
- `src/server.ts` -- MCP server entry point
- `src/node-client.ts` -- HTTP client for Rust node API
- `src/cache.ts` -- L0 in-memory cache with TTL
- `src/tools.ts` -- MCP tool handlers
- `src/crypto.ts` -- encryption boundary
- `src/novelty.ts` -- novelty engine
- `src/schema.ts` -- Zod schemas
- `src/config.ts` -- TOML config loader
- `src/http-server.ts` -- Dashboard HTTP server (API routes, static serving)
- Full test suite (unit + integration)

**Key design decision**: The http-server.ts provides all `/dash/api/*`
routes (section 3.6) with JSON responses. Martin builds the HTML/JS
frontend that calls these APIs. The proxy serves Martin's dashboard
as static files from `/dashboard/`.

### WP2: Dashboard + Integration + Infrastructure -- Martin

**What**: Dashboard UI, enrollment flow, admin panel, CI/CD, deployment.
Martin starts Monday with complete API contract (section 3.6).

**Scope**:
- Dashboard HTML/CSS/JS (calls `/dash/api/*` endpoints)
- Enrollment page (`/enroll` -- device code entry + GitHub OAuth)
- Admin panel (`/admin` -- entity management, stats, audit)
- Group management UI (`/groups` -- list, create, members, culture)
- Peer visibility UI (`/peers` -- for bootnode/admin roles)
- CI/CD pipeline (GitHub Actions for both repos)
- Deployment (Fly.io, Docker, install script updates)
- Integration testing (proxy <-> Rust node end-to-end)

**Does NOT include**:
- MCP protocol handling (that's Russell, WP1)
- Encryption/decryption logic (that's Russell, WP1)
- Rust node changes (that's Russell, WP3)
- Novelty engine (that's Russell, WP1)

**Interface contract**: Section 3.6 above. Martin's frontend is a client
of the dashboard API. All state lives in the Rust node; the proxy is
stateless except for L0 cache and session cookies.

### WP3: Peer Replication Protocol (R3) -- Russell

**What**: Complete the culture-governed replication between Rust nodes.

**Scope** (all in Rust, all in `cordelia-core/crates/`):
- `cordelia-replication/src/engine.rs`: wire dispatch + conflict resolution
- `cordelia-node/src/replication_task.rs`: anti-entropy sync loop
- `cordelia-node/src/mini_protocols.rs`: memory-push handler (inbound)
- `cordelia-api`: add device enrollment endpoints (section 3.1)
- `cordelia-api`: add detailed peers endpoint (section 3.1)
- `cordelia-protocol/src/messages.rs`: add any missing message variants

**Key tasks**:
1. Wire `WriteNotification` -> `dispatch_outbound` -> QUIC stream to hot peers
2. Implement inbound push handler
3. Complete anti-entropy: periodic sync headers -> diff -> fetch missing
4. Culture-aware dispatch: chatty=eager push, moderate=notify+fetch, taciturn=passive
5. Tombstone handling: deletions replicate as `is_deletion=true` headers
6. Device enrollment endpoints on Rust API
7. Integration test: two nodes, write on A, verify appears on B

### WP4: Shared -- API Contract Verification

**What**: Ensure all work packages agree on the HTTP API contracts.

**How**:
- Russell builds proxy API routes against section 3.6 contract (WP1)
- Russell ensures Rust API returns exact shapes from section 3.1 (WP3)
- Martin builds dashboard frontend against section 3.6 contract (WP2)
- Cross-language contract tests in CI verify both sides (TR-CI-005)
- Integration test: dashboard -> proxy API -> node API -> SQLite

**Shared artifacts**: This HLD, sections 3.1 and 3.6. The contracts.

---

## 5. Dependency Graph

```
    WP1 (proxy core)        WP2 (dashboard)         WP3 (replication)
    Russell (weekend)       Martin (from Monday)     Russell (ongoing)
        │                       │                        │
        ▼                       ▼                        ▼
    ┌────────┐           ┌──────────┐              ┌────────┐
    │ Proxy  │           │Dashboard │              │ Repl   │
    │ MCP +  │           │ HTML/JS  │              │ (Rust) │
    │ API    │           │ Enroll   │              │        │
    │ routes │           │ Admin    │              │        │
    └───┬────┘           └────┬─────┘              └───┬────┘
        │                     │                        │
        │        ┌────────────┘                        │
        │        │ calls /dash/api/*                   │
        ▼        ▼                                     │
    ┌─────────────────┐                                │
    │ Proxy HTTP      │                                │
    │ Server :3847    │                                │
    │ (serves API +   │                                │
    │  static files)  │                                │
    └────────┬────────┘                                │
             │                                         │
             │ calls /api/v1/*                          │
             ▼                                         │
        ┌───────────────┐                              │
        │ Rust Node     │<─────────────────────────────┘
        │ HTTP API      │
        │ Port 9473     │
        └───────┬───────┘
                │
           ┌────┴────┐
           │ SQLite  │
           └─────────┘
```

**Repo boundaries**:
- WP1 + WP2: `cordelia-proxy` repo
- WP3: `cordelia-core` repo

Martin works in `cordelia-proxy/dashboard/` (new files).
Russell works in `cordelia-proxy/src/` (proxy core) and `cordelia-core/crates/` (Rust).
No source file conflicts between WP1 and WP2.

---

## 6. Development Workflow

### Repos
- `cordelia-proxy` (TypeScript) -- Russell (WP1) + Martin (WP2)
- `cordelia-core` (Rust) -- Russell (WP3)

### Branching
- Martin: branches in `cordelia-proxy` for dashboard work
- Russell: `main` in `cordelia-proxy` for proxy core (ships first)
- Russell: branches in `cordelia-core` for replication work
- Both merge to `main` via PR

### Testing independently
- Russell (proxy): `npm test` in `cordelia-proxy` (mocked node)
- Russell (Rust): `cargo test` in `cordelia-core`
- Martin: browser tests against `npm run dashboard:dev` (proxy running)

### Integration testing
- Start Rust node: `cordelia-node` (listens on 9473 + 9474)
- Start proxy: `npm start` in `cordelia-proxy` (stdio MCP + HTTP :3847)
- Test: MCP tool call -> proxy -> HTTP -> Rust node -> SQLite
- Test: Dashboard -> /dash/api/* -> proxy -> Rust node -> SQLite
- Two-node test: write on node A, verify replication to node B

---

## 7. Multi-Tenant Model

Cordelia supports two deployment models. The architecture is identical;
the difference is operational.

### Model A: Self-Hosted (Single Tenant)

```
    ┌─────────────────────────────────────────┐
    │  Org Infrastructure                      │
    │                                          │
    │  ┌──────────┐    ┌────────────────────┐ │
    │  │ Proxy    │───>│ Node               │ │
    │  │ :3847    │    │ :9473 (API)        │ │
    │  │          │    │ :9474 (P2P)        │ │
    │  └──────────┘    │ SQLite             │ │
    │                   └────────────────────┘ │
    │                                          │
    │  All entities in one org.                │
    │  Trust boundary = network.               │
    │  Encryption keys managed by org.         │
    │                                          │
    │  ./install.sh and go.                    │
    └─────────────────────────────────────────┘
```

- One node + one proxy per org
- All entities share the instance
- No tenant isolation needed -- trust boundary is the org network
- Groups provide internal access control (viewer/member/admin/owner)
- Org manages own encryption keys
- This is the Cordelia Foundation offering: AGPL, run it yourself

### Model B: Managed (Multi-Tenant)

```
    ┌─────────────────────────────────────────────────────┐
    │  Seed Drill Infrastructure                           │
    │                                                      │
    │  ┌──────────────────────────────────────────┐       │
    │  │  Proxy :3847                              │       │
    │  │                                           │       │
    │  │  Org A entities ──> org_id scoping        │       │
    │  │  Org B entities ──> org_id scoping        │       │
    │  │  Org C entities ──> org_id scoping        │       │
    │  │                                           │       │
    │  │  Session cookie carries org_id.           │       │
    │  │  All queries scoped to org_id.            │       │
    │  │  Cross-org visibility = impossible.       │       │
    │  └─────────────────┬─────────────────────────┘       │
    │                     │                                 │
    │  ┌─────────────────┴─────────────────────────┐       │
    │  │  Node :9473                                │       │
    │  │                                            │       │
    │  │  Groups enforce org isolation.             │       │
    │  │  Each org = top-level group.               │       │
    │  │  Org admin = group owner.                  │       │
    │  │  Seed Drill never holds encryption keys.   │       │
    │  │                                            │       │
    │  │  Per-org encryption: each org has own key. │       │
    │  │  Proxy decrypts per-session.               │       │
    │  │  Node stores opaque blobs.                 │       │
    │  └────────────────────────────────────────────┘       │
    │                                                      │
    │  + Keeper nodes (shard backup, R4)                    │
    │  + Archive nodes (L3 cold store, R4)                  │
    │  + Bootnode (always-on peer discovery)                │
    └─────────────────────────────────────────────────────┘
```

- Seed Drill runs infrastructure for multiple orgs
- Each org = a top-level group with org_id
- Strict isolation: all queries scoped by org_id, enforced at proxy + node
- Each org manages own encryption keys (Seed Drill never has them)
- Per-org OAuth app or SSO federation
- Metering and billing per org (R4+)
- This is the Seed Drill Ltd commercial offering

### Isolation Guarantees

| Concern | Self-Hosted | Managed |
|---------|-------------|---------|
| Data isolation | Network boundary | org_id scoping in every query |
| Auth | Single OAuth app | Per-org OAuth app or SSO |
| Encryption keys | Org manages | Org manages (Seed Drill never holds) |
| Admin visibility | Org admin sees all | Org admin sees only own org |
| Data residency | Org's servers | Seed Drill infra (UK, configurable) |
| Billing | N/A | Per-org metering |
| Groups | Internal only | Org-scoped, cross-org via federation (R4+) |

### Architectural Invariant

**Seed Drill never holds plaintext or encryption keys, even in managed mode.**
The proxy decrypts in the entity's session only. Managed mode means Seed Drill
runs the node infrastructure. The same Signal model: infrastructure provider
that is structurally unable to read the content.

### Multi-Tenant Implementation

The group model already provides the primitives:

1. **Org = top-level group**. Creating an org creates a group with `org_id`.
   All entity membership is through this group.
2. **org_id on session**. OAuth callback resolves entity -> org. Session
   cookie carries org_id. Proxy scopes all node API calls by org_id.
3. **Node-level scoping**. The Rust node's API accepts optional `org_id`
   parameter. When provided, all queries are filtered. When absent
   (self-hosted), no filtering -- backward compatible.
4. **No cross-org leakage**. Group membership is the access primitive.
   Entities can only see items in groups they belong to. Org isolation
   is a consequence of group isolation.

---

## 8. Storage Architecture

### Single SQLite Database

Every Cordelia node has one SQLite file (WAL mode). The storage layer
never sees plaintext -- everything arrives encrypted from the proxy.

```
SQLite database (single file, WAL mode)
│
├── l1_hot              Entity identity + active state
│   user_id (PK) -> encrypted blob
│   One row per entity. ~50KB. Loaded every session.
│
├── l2_items            All memories
│   id (GUID, PK) -> encrypted blob + metadata
│   Main table. Every memory lives here.
│
├── l2_fts              Full-text search (BM25, porter stemming)
│   Mirrors l2_items for keyword search.
│
├── l2_index            Aggregate index blob
│
├── embedding_cache     Vector embeddings
│   (content_hash, provider, model) -> vector
│   Avoids re-embedding unchanged content.
│
├── groups              Group definitions
│   id (PK) -> name, culture (JSON), security_policy (JSON)
│
├── group_members       Membership
│   (group_id, entity_id) -> role, posture
│   Roles: owner | admin | member | viewer
│   Postures: active | silent | emcon
│
├── devices             Enrolled devices (R3)
│   (device_id, entity_id) -> bearer_token_hash, enrolled_at,
│   last_seen, user_agent
│
├── access_log          Audit trail (who/what/when/group)
├── audit               System audit
├── integrity_canary    Tamper detection
├── schema_version      Migration tracking (currently v4)
└── [indexes]           group_id, parent_id, author_id, access_log
```

Schema: `cordelia-core/crates/cordelia-storage/src/schema_v4.sql`

### Primitives to Storage Mapping

| Primitive | Storage | Notes |
|-----------|---------|-------|
| **Entity** | `l1_hot` (identity) + `l2_items` type='entity' | L1 = who you are. L2 entities = what you know about others. |
| **Memory** | `l2_items` | Encrypted blob + metadata. Every fact, session, learning. |
| **Group** | `groups` + `group_members` | Culture and security are JSON columns on `groups`. |
| **Trust** | Derived from `access_log` + accuracy over time | Not stored directly -- computed empirically (M1). |
| **Culture** | `groups.culture` JSON | Governs replication: chatty/moderate/taciturn + TTL. |

### The l2_items Table (Core)

Every memory in the system lives in `l2_items`. Key columns:

```
id              GUID, opaque (no metadata leakage)
type            'entity' | 'session' | 'learning'
owner_id        Who created it (immutable)
visibility      'private' | 'group' | 'public'
data            AES-256-GCM encrypted blob (proxy encrypts, node stores)
group_id        Which group (NULL = private/personal)
author_id       Provenance (never transfers, even on COW copy)
parent_id       COW chain -- points to original if this is a copy
is_copy         0 = original, 1 = shared copy
key_version     Envelope encryption version (for key rotation)
checksum        Integrity verification
access_count    Retrieval count (feeds governance voting weight)
last_accessed_at  Natural selection: unused memories expire via TTL
```

### Encryption Boundary

```
Proxy (TypeScript)              Node (Rust)
─────────────────               ──────────
Holds entity's keys             NEVER sees plaintext
Encrypts before write    →      Stores encrypted blob
Decrypts after read      ←      Returns encrypted blob
Generates vectors        →      Stores vectors (unencrypted*)
                                Runs cosine similarity on vectors
                                Replicates blobs + vectors to peers
```

*Vectors unencrypted by default (bounded leakage: topic inferable,
content not). Groups can opt into HE-CKKS for zero-leakage search
at ~100x compute cost. See ARCHITECTURE.md "Decentralised Search".

### Storage by Node Role

All roles run the **same Rust binary**, same schema. Config determines
behaviour:

| Role | What it stores | Key difference |
|------|----------------|----------------|
| **Personal node** | Own L1 + own L2 + group items for joined groups | Default. Your laptop. |
| **Bootnode** | Same as personal, always-on | Higher uptime, peer catch-up source. |
| **Edge relay** | Items for both internal + public groups | Bridges org/public memory spaces. |
| **Secret keeper** | Shamir shards for entity backup | Encrypted shards only. Cannot read. R4. |
| **Archive** | Full group history, never expires TTL | Long-term durable storage. Seed Drill commercial. R4. |

No special builds, no role-specific schemas. The `[capabilities]`
config section advertises what the node provides to the network.

---

## 9. Crate Map (Rust)

For Martin's reference. 7 crates, clear responsibilities:

| Crate | Purpose | Owner |
|-------|---------|-------|
| `cordelia-protocol` | Wire types, codec, constants | Shared |
| `cordelia-governor` | Peer state machine (cold/warm/hot/banned) | Russell |
| `cordelia-replication` | Culture-aware replication engine | Russell |
| `cordelia-storage` | SQLite storage trait + implementation | Shared |
| `cordelia-crypto` | AES-256-GCM, Ed25519 identity, scrypt | Shared |
| `cordelia-api` | Axum HTTP API server | Shared |
| `cordelia-node` | Main binary, task orchestration | Russell |

"Shared" = either of us may need to touch it. Coordinate via PR.

---

## 10. Configuration

### Rust Node (`~/.cordelia/config.toml`)

```toml
[node]
role = "personal"               # personal | bootnode | relay | keeper | archive
identity_key = "~/.cordelia/node.key"
api_addr = "127.0.0.1:9473"
database = "~/.cordelia/cordelia.db"
entity_id = "russell"

[capabilities]
keeper = false
archive = false
relay = false
dashboard = true                # Expose dashboard HTTP on proxy

[network]
listen_addr = "0.0.0.0:9474"

[[network.bootnodes]]
node_id = "3d223867d707af7021ffd2d07bedd41acbf6f672ee42cd4433f649d721bf655e"
addr = "boot1.cordelia.seeddrill.ai:9474"

[governor]
hot_min = 2
warm_min = 10

[replication]
sync_interval_moderate_secs = 300

[keeper]
# Only read when capabilities.keeper = true
max_shards = 1000
shard_storage = "~/.cordelia/shards/"

[archive]
# Only read when capabilities.archive = true
storage_backend = "local"       # "local" | "s3"
retention_days = 3650
```

### Proxy (`~/.cordelia/proxy.toml`)

```toml
[proxy]
node_url = "http://127.0.0.1:9473"
node_token_file = "~/.cordelia/node-token"
encryption_key_file = "~/.cordelia/keyfile"    # Or platform keychain
dashboard_port = 3847
dashboard_enabled = true

[embedding]
provider = "none"               # "none" | "ollama" | "openai"
url = "http://localhost:11434"
model = "nomic-embed-text"

[cache]
l1_ttl_secs = 0                 # 0 = session duration
l2_ttl_secs = 300               # 5 minutes

[oauth]
github_client_id = "..."
github_client_secret = "..."    # Or via env: CORDELIA_GITHUB_SECRET
session_secret = "..."          # Or via env: CORDELIA_SESSION_SECRET
```

**Environment variable overrides** (for backward compatibility and Docker):

```
CORDELIA_NODE_URL           -> proxy.node_url
CORDELIA_NODE_TOKEN         -> inline token (bypasses file)
CORDELIA_ENCRYPTION_KEY     -> inline key (bypasses file)
CORDELIA_EMBEDDING_PROVIDER -> embedding.provider
CORDELIA_DASHBOARD_PORT     -> proxy.dashboard_port
CORDELIA_GITHUB_CLIENT_ID   -> oauth.github_client_id
CORDELIA_GITHUB_SECRET      -> oauth.github_client_secret
CORDELIA_SESSION_SECRET     -> oauth.session_secret
```

TOML file takes precedence over env vars when both are set.

---

## 11. What's Already Done vs TODO

| Component | Status | Notes |
|-----------|--------|-------|
| Rust P2P transport (QUIC) | Done | quinn, self-signed TLS |
| 5 mini-protocols | Done | handshake, keepalive, peer-share, sync, fetch |
| Governor (peer state machine) | Done | + reconnect backoff |
| Rust HTTP API | Done | 11 endpoints, bearer auth |
| Rust storage (SQLite) | Done | schema v4, WAL, FTS5 |
| Rust crypto | Done | AES-256-GCM, Ed25519, round-trip with TS |
| Bootnode deployment | Done | boot1.cordelia.seeddrill.ai:9474 |
| Proxy MCP server (existing) | Done | 187 tests, 25 tools, encryption |
| Proxy dashboard HTTP (existing) | Done | Auth, profile, L1/L2 API, admin |
| Repo split | Done | cordelia-core (Rust) + cordelia-proxy (TS) |
| Replication engine | Partial | Engine + config done, wire dispatch TODO |
| Replication task | Partial | Structure done, anti-entropy TODO |
| Memory push (inbound) | TODO | Receive pushed items from peers |
| Device enrollment endpoints | TODO | On Rust API (section 3.1) |
| Proxy TOML config | TODO | Replace env-only config |
| Dashboard enrollment page | TODO | Device code + OAuth flow |
| Dashboard group management | TODO | List, create, members, culture |
| Dashboard peer visibility | TODO | Governor status, health |
| Multi-tenant org scoping | TODO | org_id on session + queries |
| Integration test (proxy+node) | TODO | End-to-end |
| Two-node replication test | TODO | Write on A, read on B |

---

## 12. Deployment Vision: Internal / Public / Services

The network has two memory spaces, same technology, different trust:

```
    INTERNAL MEMORY SPACE               PUBLIC MEMORY SPACE
    (org intranet)                      (open internet)

    ┌──────────────────────┐            ┌───────────────────────────────┐
    │                      │            │                               │
    │  Org Nodes           │            │  Edge Relays                  │
    │  ┌──────┐ ┌──────┐  │            │  ┌──────┐                     │
    │  │Node A│ │Node B│  │  QUIC      │  │Relay │  (anyone can run)   │
    │  │(priv)│ │(priv)│──┼───────────>│  │      │                     │
    │  └──────┘ └──────┘  │            │  └──────┘                     │
    │                      │            │                               │
    │  Private groups,     │            │  Constitutional groups,       │
    │  sovereign memory,   │            │  public learnings,            │
    │  full encryption     │            │  shared knowledge             │
    │                      │            │                               │
    └──────────────────────┘            │  ┌─────────────────────────┐  │
                                        │  │  SECRET KEEPERS         │  │
                                        │  │  (Seed Drill = first)   │  │
                                        │  │                         │  │
                                        │  │  Shamir shard storage   │  │
                                        │  │  n-of-m reincarnation   │  │
                                        │  │  Enrollment/onboarding  │  │
                                        │  │  Key distribution       │  │
                                        │  │  SLA monitoring         │  │
                                        │  └─────────────────────────┘  │
                                        │                               │
                                        │  ┌─────────────────────────┐  │
                                        │  │  ARCHIVES               │  │
                                        │  │  (Seed Drill = first)   │  │
                                        │  │                         │  │
                                        │  │  L3 cold store          │  │
                                        │  │  Compressed history     │  │
                                        │  │  Lineage / provenance   │  │
                                        │  │  S3/durable backend     │  │
                                        │  └─────────────────────────┘  │
                                        │                               │
                                        └───────────────────────────────┘
```

### Why this works with current architecture

The Rust node is already the universal primitive. Every deployment type is
the same binary with different configuration:

| Deployment | What it is | Config difference |
|------------|-----------|-------------------|
| **Personal node** | Runs on your laptop, holds your memory | Default config |
| **Org node** | Runs on org infra, holds group memory | More groups, always-on |
| **Edge relay** | Bridges internal to public groups | Member of both internal + public groups |
| **Bootnode** | Always-on peer for discovery | `listen_addr` on public IP |
| **Secret keeper** | Holds Shamir shards for reincarnation | `[capabilities] keeper = true` |
| **Archive** | L3 cold store, durable backend | `[capabilities] archive = true`, S3 storage |

An edge relay is just a node that has membership in both an org's
private groups AND public constitutional groups. Group culture governs
what flows where. No special code needed -- group membership + culture
already handles selective forwarding.

### Intranet -> Internet Trend

Same trajectory as the web:

1. **Now (R3)**: 3 founders, private P2P. Pure intranet.
2. **R3+**: Seed Drill internal group + first client groups. Still private.
3. **R4**: Constitutional groups (public, anyone joins). First public memories.
   Seed Drill runs keeper + archive nodes as first service provider.
4. **R5+**: Other orgs run their own keepers/archives. Market forms.
   Internal/public boundary blurs as trust calibration proves out.

### Seed Drill as Service Provider

**What we sell (R4+)**:
- Keeper service: "Your mind is backed up. n-of-m shards across
  geographically distributed keepers. SLA: 99.99% recovery."
- Archive service: "Your history is preserved. L3 cold store.
  Lineage queries. Compliance-grade retention."
- Edge relay hosting: "We run the relay, you get the connectivity."

**What we don't sell**:
- Access to anyone's plaintext memory (we never have it)
- Decryption keys (held by entities, not by us)
- Control over anyone's trust policy (sovereignty is non-negotiable)

This is the Signal model: infrastructure provider that is structurally
unable to read the content. Revenue from reliability, not from data.

---

## 13. Martin's Full Trajectory

Martin's scope is larger than WP2. Here's the phased view:

### Phase 1: Dashboard + Integration (R3 -- from Monday)
Dashboard UI that calls proxy API (section 3.6). Enrollment page.
Admin panel. Group management. CI/CD pipeline. Deployment.
See WP2 (Section 4) for full spec.

### Phase 2: Operational Infrastructure (R3+)
- **Enrollment CLI**: `npx @cordelia/proxy enroll`
  RFC 8628 device authorization grant. See enrollment-sequence.md.
- **Token management**: Issue, rotate, revoke bearer tokens.
  Stored in `~/.cordelia/tokens/`.
- **Key distribution**: Envelope encryption key exchange when entities
  join groups. Signal-pattern: group key encrypted per member key.
- **Health monitoring**: `cordelia status --keepers` shows shard health.

### Phase 3: Keeper Infrastructure (R4)
- **Shard protocol**: Mini-protocol 0x06 on QUIC. Rust wire protocol
  (Russell). Operational wrapper (Martin).
- **Reincarnation workflow**: n-of-m shard reconstitution.
- **Keeper dashboard**: `/dash/api/keeper/*` endpoints + UI.

### Phase 4: Archive Infrastructure (R4)
- **L3 storage backend**: S3-compatible durable store.
- **Lineage API**: Provenance chain queries.
- **Compliance**: GDPR right-to-forget, audit export.
- **Archive dashboard**: `/dash/api/archive/*` endpoints + UI.

### Phase 5: Service Operations (R4+)
- **SLA monitoring**: Keeper availability, archive durability.
- **Billing**: Per-org keeper/archive subscription.
- **Customer onboarding**: Self-serve via seeddrill.ai.

### Work Split Principle

Russell builds the **protocols and engines** (Rust, wire formats, state
machines, game theory) and the **proxy core** (MCP, encryption, novelty).
Martin builds the **operational infrastructure** (dashboards, enrollment
workflows, key management, monitoring, deployment, CI/CD).

---

## 14. Non-Goals for R3

These are explicitly **not in scope** for the immediate sprint.
Captured here so we don't accidentally build them:

- Policy engine in Rust (R4 -- stays in proxy for now)
- Envelope encryption per group (Phase 2)
- mTLS between proxy and node (Phase 2, bearer token is fine for now)
- Vector search in Rust node (R4 -- proxy does client-side if needed)
- Sub-groups, group inheritance (R4)
- Trust calibration / quarantine (R4)
- L3 cold archive (Phase 4)
- Federation / cross-org discovery (R4+)
- Keeper/archive infrastructure (Phase 3-4)
- Public constitutional groups (R4)
- Billing / SLA monitoring (Phase 5)
- Proxy rewrite in Rust (evaluate when Rust MCP SDK stabilises)

---

## 15. Structural Decisions to Get Right Now

These R3 decisions affect whether the vision in sections 7 and 12 is possible:

1. **Node config must support capabilities** -- Add `[capabilities]`
   section to `config.toml` even if all are `false` for R3. Don't
   hardcode "all nodes are identical" into the binary. Keepers and
   archives advertise capabilities in gossip (PeerAddress).

2. **Storage trait must be abstract** -- `cordelia-storage` already
   defines a trait. Keep it. Archive nodes will implement a different
   backend (S3). Don't leak SQLite assumptions into the protocol layer.

3. **API must be versioned** -- Already `/api/v1/`. Keep it. Keepers
   and archives will add `/api/v1/keeper/` and `/api/v1/archive/`
   namespaces.

4. **Group membership is the access primitive** -- Don't add shortcuts
   that bypass group membership. Edge relays work because group
   membership determines what flows where. If we add any "all peers
   see everything" path, the internal/public distinction breaks.

5. **Encryption stays in the proxy** -- The Rust node never holds
   plaintext. This is what makes keeper/archive services trustworthy.
   If we move decryption into the node, we can't credibly offer
   "we can't read your data."

6. **Group ID = SHA-256(URI)** -- Groups are content-addressed. The
   hash is public (discoverable via gossip). The URI is private to
   members. Non-members can replicate encrypted blobs for a group
   without knowing the group name or content. This enables keepers,
   relays, and archives to serve groups they can't read.

7. **Vectors persist alongside encrypted blobs** -- Do NOT strip
   embeddings on save (reversing the R1 decision). The vector is the
   searchable metadata layer that enables cross-node semantic search
   without decryption. Storage format: (group_hash, item_id,
   encrypted_blob, vector, checksum). Group manifest specifies
   vector_model + vector_dimensions + vector_encoding.

8. **Vector encoding is a group-level decision** -- Default: plaintext
   vectors (bounded leakage, acceptable for most groups). Groups that
   need stronger privacy specify `vector_encoding: "he-ckks"` in
   their manifest. Homomorphic encryption on vectors at ~100x compute
   cost. The protocol supports both; the group decides.

9. **Proxy role via TOML** -- The proxy reads its role from config and
   exposes only the relevant API surfaces. Default is `personal`
   (minimal). This avoids keeper/archive code paths executing on
   personal nodes.

10. **Multi-tenant via groups** -- org_id is a group. No separate tenant
    table. Multi-tenant isolation is a consequence of group isolation.
    Self-hosted = no org scoping. Managed = org_id on every query.

---

*Last updated: 2026-01-31*
*Russell Wing and Claude (Opus 4.5)*
