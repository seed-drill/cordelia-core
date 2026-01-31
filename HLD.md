# Cordelia - High Level Design

*For Martin. For Russell. For not stepping on each other.*

---

## 1. System Overview

Cordelia is a distributed persistent memory system. Three layers:

```
    AGENTS                     NODE                      NETWORK
    ------                     ----                      -------

    Claude Code ─┐
    Swarm Agent ─┤─ stdio ─> @cordelia/proxy ─ HTTP ─> cordelia-node ─ QUIC ─> peers
    Other MCP   ─┘            (TypeScript)              (Rust)
                              Port: n/a (stdio)         API: 9473
                                                        P2P: 9474
```

**@cordelia/proxy** (R3-021): Thin MCP proxy. Speaks stdio to agents, HTTP to local
Rust node. Has L0 in-memory cache. Receives peer notifications, translates to cache
invalidations. Agents see standard MCP tools; P2P is invisible. ~300 lines.

**cordelia-node** (Rust): The node. Stores everything in SQLite. Replicates to peers
via QUIC. Manages peer lifecycle (governor). Exposes HTTP API for local clients.

---

## 2. Component Map

```
    ┌─────────────────────────────────────────────────────────────────┐
    │                      @cordelia/proxy (TS)                       │
    │                                                                 │
    │  ┌───────────┐  ┌────────────┐  ┌──────────┐  ┌────────────┐  │
    │  │ MCP       │  │ Novelty    │  │ L0 Cache │  │ Node       │  │
    │  │ Handler   │  │ Engine     │  │ (in-mem) │  │ Client     │  │
    │  │           │  │           │  │          │  │ (HTTP)     │  │
    │  │ 25 tools  │  │ Pattern   │  │ L1 hot   │  │            │  │
    │  │ stdio     │  │ matching  │  │ context  │  │ Bearer     │  │
    │  │ transport │  │ Scoring   │  │ Recent   │  │ token      │  │
    │  │           │  │           │  │ L2 hits  │  │ Retry      │  │
    │  └─────┬─────┘  └─────┬─────┘  └────┬─────┘  └─────┬──────┘  │
    │        │              │             │              │           │
    │        └──────────────┴─────────────┴──────────────┘           │
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

#### Status

```
POST /api/v1/status
  Response: { "node_id": "3d2238...", "entity_id": "russell",
              "uptime_secs": 3600,
              "peers_warm": 2, "peers_hot": 1,
              "groups": ["seed-drill"] }

POST /api/v1/peers
  Response: { "warm": 2, "hot": 1, "total": 3 }
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

---

## 4. Work Packages

### WP1: @cordelia/proxy (R3-021) -- Martin

**What**: New TypeScript package. Thin MCP proxy that speaks stdio to
agents and HTTP to the local Rust node.

**Scope**:
- MCP server via `@modelcontextprotocol/sdk` (stdio transport)
- HTTP client to `localhost:9473` (bearer token auth)
- L0 in-memory cache (L1 hot context + recent L2 search results)
- 25 MCP tools (same interface as current `server.ts`)
- Encryption/decryption (reuse existing `crypto.ts`)
- Novelty engine (reuse existing `novelty.ts`)
- Schema validation (reuse existing `schema.ts`)
- Embeddings (reuse existing `embeddings.ts`, optional)

**Does NOT include**:
- P2P networking (that's the Rust node)
- SQLite storage (that's the Rust node)
- Governor, replication, peer management
- Policy engine (moves to Rust node in R4)

**Interface contract**: Section 3.1 above. The proxy is a client.

**Key design decisions for Martin**:
1. This replaces the current `server.ts` for networked deployments.
   Local-only (stdio, no Rust node) remains as fallback.
2. The proxy should detect whether a Rust node is running
   (`GET /api/v1/status`). If not, fall back to local SQLite.
3. L0 cache: simple Map with TTL. L1 cached for session duration.
   L2 search results cached for 5 minutes (configurable).
4. Error handling: if Rust node is unreachable, degrade gracefully.
   Log warning, serve from L0 cache where possible.

**Existing code to reuse** (from `/Users/russellwing/cordelia/src/`):
- `crypto.ts` -- encryption/decryption (copy or import)
- `novelty.ts` -- novelty analysis (copy or import)
- `schema.ts` -- Zod schemas for L1/L2 validation
- `embeddings.ts` -- Ollama/null provider
- `integrity.ts` -- chain hash verification

**Test strategy**:
- Unit tests for HTTP client (mock Rust node responses)
- Unit tests for L0 cache (TTL, invalidation)
- Integration test: proxy <-> real Rust node on localhost
- MCP protocol conformance (existing MCP SDK test helpers)

**Files Martin creates** (new package, no conflicts):
```
cordelia/packages/proxy/
  package.json
  tsconfig.json
  src/
    index.ts          -- entry point, stdio MCP server
    node-client.ts    -- HTTP client for Rust node API
    cache.ts          -- L0 in-memory cache
    tools.ts          -- MCP tool handlers (thin, delegate to client)
    fallback.ts       -- local SQLite fallback when node unavailable
  test/
    node-client.test.ts
    cache.test.ts
    integration.test.ts
```

### WP2: Peer Replication Protocol (R3-020) -- Russell

**What**: Complete the culture-governed replication between Rust nodes.

**Scope** (all in Rust, all in `cordelia-node/crates/`):
- `cordelia-replication/src/engine.rs`: flesh out `on_local_write` dispatch
  and `on_receive` with full conflict resolution
- `cordelia-node/src/replication_task.rs`: complete anti-entropy sync loop,
  wire up write dispatch to actual QUIC streams
- `cordelia-node/src/mini_protocols.rs`: implement memory-push handler
  (inbound side of eager push)
- `cordelia-protocol/src/messages.rs`: add any missing message variants
- `cordelia-governor/src/lib.rs`: already done (reconnect backoff landed today)

**Does NOT include**:
- MCP proxy (that's Martin)
- TypeScript changes
- API changes (unless new endpoints needed for replication status)

**Key tasks**:
1. Wire `WriteNotification` -> `dispatch_outbound` -> QUIC stream to hot peers
2. Implement inbound push handler (peer sends us a `FetchResponse`,
   we call `on_receive`, reply `PushAck`)
3. Complete anti-entropy: periodic sync headers -> diff -> fetch missing
4. Culture-aware dispatch: chatty=eager push, moderate=notify+fetch, taciturn=passive
5. Tombstone handling: deletions replicate as `is_deletion=true` headers
6. Integration test: two nodes, write on A, verify appears on B

**Files Russell touches** (Rust only, no conflicts with WP1):
```
cordelia-node/crates/
  cordelia-replication/src/engine.rs
  cordelia-node/src/replication_task.rs
  cordelia-node/src/mini_protocols.rs
  cordelia-protocol/src/messages.rs  (if needed)
  cordelia-governor/src/lib.rs       (done - backoff fix)
```

### WP3: Shared -- API Contract Verification

**What**: Ensure WP1 and WP2 agree on the HTTP API contract.

**How**:
- Martin writes HTTP client tests against mock responses (WP1)
- Russell ensures the Rust API returns those exact shapes (WP2/existing)
- Any contract drift is caught by integration test: proxy <-> real node

**Shared artifact**: This HLD, section 3.1. The contract.

---

## 5. Dependency Graph

```
    WP1 (proxy)              WP2 (replication)
    Martin                   Russell
        │                        │
        │                        │
        ▼                        ▼
    ┌────────┐              ┌────────┐
    │ Proxy  │              │ Repl   │
    │ (TS)   │              │ (Rust) │
    └───┬────┘              └───┬────┘
        │                       │
        │   ┌───────────────┐   │
        └──>│ HTTP API      │<──┘
            │ (cordelia-api)│
            │ Port 9473     │
            └───────────────┘
                 │
            ┌────┴────┐
            │ SQLite  │
            │ (shared │
            │  file)  │
            └─────────┘
```

**No source file conflicts.** Martin works in `packages/proxy/` (new).
Russell works in `cordelia-node/crates/` (existing Rust).

Both depend on the HTTP API contract (section 3.1), which is already
implemented in `cordelia-api`. Changes to the API require coordination.

---

## 6. Development Workflow

### Branching
- Martin: `feature/proxy` branch
- Russell: `feature/replication` branch
- Both merge to `main` via PR
- No shared files = no merge conflicts

### Testing independently
- Martin: `npm test` in `packages/proxy/` (mocked node)
- Russell: `cargo test` in `cordelia-node/` (unit + integration)

### Integration testing
- Start Rust node: `cordelia-node` (listens on 9473 + 9474)
- Start proxy: `node packages/proxy/dist/index.js` (stdio)
- Test: MCP tool call -> proxy -> HTTP -> Rust node -> SQLite
- Two-node test: start two Rust nodes, write via proxy on node A,
  verify replication to node B, read via proxy on node B

### Shared SQLite file
Both the proxy (via HTTP to Rust node) and the Rust node access the
same SQLite file. WAL mode handles concurrent access. The proxy never
touches SQLite directly -- always via HTTP API.

---

## 7. Storage Architecture

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
├── access_log          Audit trail (who/what/when/group)
├── audit               System audit
├── integrity_canary    Tamper detection
├── schema_version      Migration tracking (currently v4)
└── [indexes]           group_id, parent_id, author_id, access_log
```

Schema: `cordelia-node/crates/cordelia-storage/src/schema_v4.sql`

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

## 8. Crate Map (Rust)

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

## 9. Configuration

### Rust Node (`~/.cordelia/config.toml`)

```toml
[node]
identity_key = "~/.cordelia/node.key"
api_transport = "http"          # or "unix"
api_addr = "127.0.0.1:9473"
database = "~/cordelia/memory/cordelia.db"
entity_id = "russell"

[network]
listen_addr = "0.0.0.0:9474"

[[network.bootnodes]]
node_id = "3d223867d707af7021ffd2d07bedd41acbf6f672ee42cd4433f649d721bf655e"
addr = "boot1.cordelia.seeddrill.io:9474"

[governor]
hot_min = 2
warm_min = 10

[replication]
sync_interval_moderate_secs = 300
```

### Proxy (environment variables)

```
CORDELIA_NODE_URL=http://127.0.0.1:9473
CORDELIA_NODE_TOKEN=<bearer token>
CORDELIA_ENCRYPTION_KEY=<passphrase>
CORDELIA_EMBEDDING_PROVIDER=ollama
CORDELIA_EMBEDDING_URL=http://localhost:11434
```

---

## 10. What's Already Done vs TODO

| Component | Status | Notes |
|-----------|--------|-------|
| Rust P2P transport (QUIC) | Done | quinn, self-signed TLS |
| 5 mini-protocols | Done | handshake, keepalive, peer-share, sync, fetch |
| Governor (peer state machine) | Done | + reconnect backoff (today) |
| Rust HTTP API | Done | 11 endpoints, bearer auth |
| Rust storage (SQLite) | Done | schema v4, WAL, FTS5 |
| Rust crypto | Done | AES-256-GCM, Ed25519, round-trip compatible with TS |
| Bootnode deployment | Done | boot1.cordelia.seeddrill.io:9474 |
| Replication engine | Partial | Engine + config done, wire dispatch TODO |
| Replication task | Partial | Structure done, anti-entropy TODO |
| Memory push (inbound) | TODO | Receive pushed items from peers |
| @cordelia/proxy | TODO | New package (WP1) |
| Integration test (proxy+node) | TODO | End-to-end |
| Two-node replication test | TODO | Write on A, read on B |

---

## 11. Deployment Vision: Internal / Public / Services

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

### Node Capabilities (config extension needed)

```toml
[capabilities]
relay = false       # Accept connections from external peers
keeper = false      # Accept and store Shamir shards
archive = false     # Accept L3 cold storage requests

[keeper]
max_shards = 1000
shard_storage = "~/.cordelia/shards/"
enrollment_token = "..."    # For entity enrollment API

[archive]
storage_backend = "s3"      # or "local"
s3_bucket = "cordelia-archive-prod"
s3_region = "eu-west-2"
retention_days = 3650       # 10 years
```

### Intranet -> Internet Trend

Same trajectory as the web:

1. **Now (R3)**: 3 founders, private P2P. Pure intranet.
2. **R3+**: Seed Drill internal group + first client groups. Still private.
3. **R4**: Constitutional groups (public, anyone joins). First public memories.
   Seed Drill runs keeper + archive nodes as first service provider.
4. **R5+**: Other orgs run their own keepers/archives. Market forms.
   Internal/public boundary blurs as trust calibration proves out.

The cooperative equilibrium proof (see `decisions/2026-01-31-cooperative-
equilibrium-proof.md`) is the theoretical foundation for why the public
space works: honest cooperation is the Nash equilibrium under Cordelia's
mechanism design. Without that proof, public memory is just a tragedy
of the commons waiting to happen.

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

## 12. Martin's Full Trajectory

Martin's scope is larger than WP1. Here's the phased view:

### Phase 1: @cordelia/proxy (R3 -- now)
MCP proxy. Get agents talking to Rust nodes. Bounded, testable.
See WP1 (Section 4) for full spec.

### Phase 2: Operational Infrastructure (R3+)
- **Enrollment CLI**: `cordelia enroll --keeper boot1.cordelia.seeddrill.io`
  Entity generates shards, distributes to keepers, receives confirmation.
- **Token management**: `cordelia token create --entity russell --scope group:seed-drill`
  Issue, rotate, revoke bearer tokens. Stored in `~/.cordelia/tokens/`.
- **Key distribution**: Envelope encryption key exchange when entities join groups.
  Signal-pattern: group key encrypted per member key. Martin implements the
  key exchange protocol; Russell implements the group key rotation trigger.
- **Health monitoring**: `cordelia status --keepers` shows shard health across
  keepers. Alerting when shards go stale or keepers go offline.

### Phase 3: Keeper Infrastructure (R4)
- **Shard protocol**: New mini-protocol (0x06) on QUIC layer for shard
  push/pull/verify. Rust implementation. Martin builds the operational
  wrapper; Russell builds the wire protocol.
- **Reincarnation workflow**: Entity triggers reincarnation (or dead-man
  switch). Keepers reconstitute from n-of-m shards. New node bootstraps
  from reconstituted state.
- **Shard rotation**: Periodic re-sharing to limit exposure window.
- **Keeper dashboard**: Web UI for Seed Drill ops team to monitor shard
  health, keeper availability, enrollment rate.

### Phase 4: Archive Infrastructure (R4)
- **L3 storage backend**: S3-compatible durable store for compressed
  session history.
- **Lineage API**: Query provenance chains across archived sessions.
- **Compliance features**: Retention policies, GDPR right-to-forget
  (delete lineage chain on entity request), audit export.
- **Archive dashboard**: Storage usage, query latency, retention stats.

### Phase 5: Service Operations (R4+)
- **SLA monitoring**: Keeper availability, archive durability, latency
  percentiles. Alerting to ops team.
- **Billing**: Per-entity keeper/archive subscription. Usage metering.
- **Customer onboarding**: Self-serve enrollment via seeddrill.io.
  Choose keeper count (3/5/7), archive retention (1y/5y/10y), pay.

### Work Split Principle

Russell builds the **protocols and engines** (Rust, wire formats, state
machines, game theory). Martin builds the **operational infrastructure**
(CLI tooling, dashboards, enrollment workflows, key management, monitoring,
deployment). Both contribute to the API contract between them.

This is CTO vs CPO division: Martin makes it run reliably for customers;
Russell makes it work correctly in theory and protocol.

---

## 13. Non-Goals for R3

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

---

## 14. Structural Decisions to Get Right Now

These R3 decisions affect whether the vision in Section 10 is possible:

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

---

*Last updated: 2026-01-31*
*Russell Wing and Claude (Opus 4.5)*
