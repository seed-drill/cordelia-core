# Cordelia - Formal Requirements Specification

**Version**: 2.0
**Status**: Version 1.0
**Derived from**: Architecture diagram (arch-diag.drawio), [HLD](../architecture/hld.md) (v2), [Architecture Overview](../architecture/overview.md), schema_v4.sql
**Audience**: Development team (Martin, Russell), CI/CD pipeline, future auditors

---

## Notation

- **FR**: Functional Requirement
- **NFR**: Non-Functional Requirement (performance, security, reliability)
- **IR**: Interface Requirement (contracts between components)
- **TR**: Test Requirement (verification criteria)

Priority: **P0** = invariant (must never be violated), **P1** = must-have for release, **P2** = should-have.

Each requirement is **testable**. The "Verify" field describes how.

---

## 1. Entity Layer

### FR-ENT-001: Model Agnosticism (P0)

The system SHALL NOT depend on any specific LLM provider. Any MCP-capable model (Claude, GPT, Gemini, local models) SHALL be able to use Cordelia via the MCP tool interface.

**Verify**: Integration test with mock MCP client (no LLM dependency in test harness).

### FR-ENT-002: Entity Sovereignty (P0)

An entity SHALL have exclusive control over its own memory. No group policy, peer, or administrator SHALL be able to force content into an entity's sovereign memory without the entity's explicit acceptance.

**Verify**: Attempt to write to entity's L1 from a different entity's token. Must return 403. Attempt group-push to entity that has rejected trust threshold. Must be silently dropped.

### FR-ENT-003: Entity Identity

An entity SHALL be identified by an Ed25519 keypair. The `node_id` SHALL be `SHA-256(public_key)` (32 bytes, hex-encoded).

**Verify**: Generate keypair, compute node_id, verify SHA-256 of public key matches. Round-trip: Rust and TypeScript implementations produce identical node_id from same key material.

---

## 2. Proxy Layer (@cordelia/proxy -- TypeScript)

### FR-PRX-001: MCP Protocol Compliance (P1)

The proxy SHALL implement the MCP protocol over stdio transport using `@modelcontextprotocol/sdk`. All 25 MCP tools SHALL be registered and callable via JSON-RPC.

**Verify**: MCP conformance test: send `tools/list` request, verify 25 tools returned with correct schemas. Call each tool with valid input, verify well-formed response.

### FR-PRX-002: Tool Set Completeness (P1)

The proxy SHALL expose the following tool categories:

| Category | Tools | Count |
|----------|-------|-------|
| L1 Hot Context | `memory_read_hot`, `memory_write_hot` | 2 |
| L2 Warm Index | `memory_read_warm`, `memory_write_warm`, `memory_delete_warm`, `memory_search` | 4 |
| Analysis | `memory_analyze_novelty` | 1 |
| System | `memory_status`, `memory_backup`, `memory_restore` | 3 |
| Groups | `memory_group_create`, `memory_group_list`, `memory_group_read`, `memory_group_add_member`, `memory_group_remove_member` | 5 |
| Sharing | `memory_share` | 1 |

Remaining tools are reserved for R3+ (federate, lineage, merge, key_rotate, etc.).

**Verify**: Unit test: tool registry contains exactly the specified tools. Each tool handler is callable and returns the correct response schema.

### FR-PRX-003: L0 In-Memory Cache (P1)

The proxy SHALL maintain an in-memory cache (L0) containing:
- L1 hot context for the session duration (no TTL expiry during session)
- Recent L2 search results with configurable TTL (default: 5 minutes)

Cache entries SHALL be invalidated on write-through.

**Verify**: Read L1 twice in succession -- second read must not hit the node (mock verifies zero HTTP calls). Write L1 then read -- must reflect new value. Wait >TTL on L2 cache entry, read again -- must hit node.

### FR-PRX-004: Novelty Engine (P1)

The proxy SHALL run novelty analysis before persistence decisions. The engine SHALL score content against the following signal types: correction, preference, entity_new, decision, insight, blocker, reference, working_pattern, meta_learning.

Scores below the configurable threshold (default: 0.7) SHALL NOT be persisted.

**Verify**: Property-based test: random inputs with known signal patterns score above threshold. Known low-novelty inputs ("ok", "got it") score below threshold.

### FR-PRX-005: Embedding Generation (P2)

The proxy SHALL optionally generate vector embeddings via a configurable provider (default: Ollama). When no provider is available, the proxy SHALL fall back to keyword-only search with no error.

**Verify**: Start proxy without Ollama running. Perform search. Must return FTS5 results without error. Start with Ollama. Perform search. Must return hybrid results.

### FR-PRX-006: Node Fallback (P1)

The proxy SHALL detect whether a Rust node is running (via `GET /api/v1/status`). If the node is unreachable, the proxy SHALL fall back to local SQLite storage with a logged warning. When the node becomes available again, the proxy SHALL resume using it.

**Verify**: Start proxy without node -- operations succeed against local SQLite. Start node -- subsequent operations route to node. Kill node -- operations degrade to local with warning in logs.

---

## 3. Encryption Boundary

### FR-ENC-001: Encryption Before Storage (P0)

ALL L2 item content SHALL be encrypted (AES-256-GCM) by the proxy BEFORE transmission to the Rust node. The Rust node SHALL NEVER receive, store, or process plaintext memory content.

**Verify**: Intercept HTTP request from proxy to node on L2 write. Verify `data` field is a valid `EncryptedPayload` JSON structure with `_encrypted: true`, `iv`, `authTag`, `ciphertext` fields. Attempt to decode `ciphertext` as UTF-8 -- must fail (not plaintext).

### FR-ENC-002: Encryption Payload Format (P0)

Encrypted payloads SHALL conform to:

```json
{
  "_encrypted": true,
  "version": 1,
  "iv": "<base64, 12 bytes>",
  "authTag": "<base64, 16 bytes>",
  "ciphertext": "<base64>"
}
```

**Verify**: Schema validation test. Round-trip: encrypt, serialize, deserialize, decrypt. Verify plaintext matches. Cross-language: TypeScript encrypt, Rust verify structure (and vice versa for future proxy-in-Rust).

### FR-ENC-003: Key Derivation (P0)

Encryption keys SHALL be derived via scrypt with parameters: N=16384, r=8, p=1, output=32 bytes. The passphrase SHALL be sourced from `CORDELIA_ENCRYPTION_KEY` environment variable.

**Verify**: Derive key from known passphrase. Compare output to pre-computed test vector. Verify 32-byte output length.

### FR-ENC-004: Per-Item IV (P0)

Each encryption operation SHALL generate a fresh 12-byte random IV. No IV SHALL be reused across encryptions with the same key.

**Verify**: Encrypt the same plaintext 1000 times. Verify all 1000 IVs are unique. Verify all 1000 ciphertexts are different.

### FR-ENC-005: Scope-Aware Keys (P1)

Personal memories and group memories SHALL use different encryption keys. A compromise of a group key SHALL NOT expose personal memories.

**Verify**: Encrypt personal item with personal key. Attempt decrypt with group key. Must fail (auth tag mismatch).

### FR-ENC-006: Key Version Tracking (P1)

Every L2 item SHALL carry a `key_version` field (integer, default 1). On key rotation, new writes SHALL use the new version. Reads SHALL use the version recorded on the item.

**Verify**: Write item with key_version=1. Rotate key. Write new item (key_version=2). Read both items. Both must decrypt correctly using their respective key versions.

---

## 4. Rust Node (cordelia-node)

### 4.1 HTTP API (cordelia-api)

### IR-API-001: API Endpoint Contract (P0)

The Rust node SHALL expose the following HTTP API on `localhost:9473`:

| Method | Path | Request | Response |
|--------|------|---------|----------|
| POST | /api/v1/l1/read | `{ user_id }` | `{ data }` |
| POST | /api/v1/l1/write | `{ user_id, data }` | `{ ok: true }` |
| POST | /api/v1/l2/read | `{ item_id }` | `{ data, type, meta }` |
| POST | /api/v1/l2/write | `{ item_id, type, data, meta }` | `{ ok: true }` |
| POST | /api/v1/l2/delete | `{ item_id }` | `{ ok: true }` |
| POST | /api/v1/l2/search | `{ query, limit }` | `{ results: [{ id, type, score }] }` |
| POST | /api/v1/groups/create | `{ id, name, culture, security_policy }` | `{ ok: true }` |
| POST | /api/v1/groups/list | `{}` | `{ groups: [...] }` |
| POST | /api/v1/groups/read | `{ group_id }` | `{ group, members }` |
| POST | /api/v1/groups/items | `{ group_id, since, limit }` | `{ items, has_more }` |
| POST | /api/v1/status | `{}` | `{ node_id, entity_id, uptime_secs, peers_warm, peers_hot, groups }` |
| POST | /api/v1/peers | `{}` | `{ warm, hot, total }` |

**Verify**: Contract test suite: for each endpoint, send valid request, verify response shape matches schema. Send malformed request, verify 400. Send without auth, verify 401.

### IR-API-002: Bearer Token Authentication (P0)

Every API request SHALL include `Authorization: Bearer <token>`. The token SHALL be loaded from `~/.cordelia/node-token`. Requests without a valid token SHALL receive HTTP 401.

**Verify**: Send request without Authorization header -- 401. Send with invalid token -- 401. Send with valid token -- 200.

### IR-API-003: API Versioning (P1)

All endpoints SHALL be prefixed with `/api/v1/`. Future versions SHALL use `/api/v2/` etc. Breaking changes SHALL NOT be made to existing version paths.

**Verify**: Request to `/api/v1/status` -- 200. Request to `/api/v2/status` (non-existent) -- 404.

### IR-API-004: Content-Type (P1)

All requests and responses SHALL use `Content-Type: application/json`.

**Verify**: Send request with `Content-Type: text/plain` -- 415 or 400. Verify all responses include `Content-Type: application/json` header.

### IR-API-005: Write Notification Side Effect (P1)

On successful L2 write, the API layer SHALL emit a `WriteNotification` to the replication task. This is the trigger for culture-governed replication to peers.

**Verify**: Write L2 item via API. Verify replication task receives notification (mock/channel assertion). Verify notification contains item_id, group_id, and culture policy.

### 4.2 Storage (cordelia-storage)

### FR-STO-001: SQLite WAL Mode (P0)

The storage layer SHALL use SQLite in WAL (Write-Ahead Logging) mode. This allows concurrent readers with a single writer.

**Verify**: Open database, verify `PRAGMA journal_mode` returns `wal`. Spawn concurrent readers + one writer. Verify no SQLITE_BUSY errors under normal load.

### FR-STO-002: Schema V4 Compliance (P1)

The database SHALL implement schema v4 with all tables defined in `schema_v4.sql`:

| Table | Purpose | Key Constraint |
|-------|---------|----------------|
| `l1_hot` | Entity identity | PK: user_id |
| `l2_items` | All memories | PK: id (GUID), type CHECK, visibility CHECK |
| `l2_fts` | FTS5 search | porter + unicode61 tokenizer |
| `l2_index` | Aggregate index | Single-row (id=1) |
| `embedding_cache` | Vector cache | Composite PK (hash, provider, model) |
| `groups` | Group definitions | PK: id |
| `group_members` | Membership | Composite PK (group_id, entity_id), role CHECK, posture CHECK |
| `access_log` | Audit trail | AUTOINCREMENT |
| `audit` | System events | AUTOINCREMENT |
| `integrity_canary` | Tamper detection | Single-row (id=1) |
| `schema_version` | Migration tracking | -- |

**Verify**: Create fresh database. Verify all tables exist. Verify CHECK constraints reject invalid values (e.g., type='invalid', visibility='secret', role='superuser', posture='hiding').

### FR-STO-003: GUID Primary Keys (P0)

L2 item IDs SHALL be opaque GUIDs that leak no metadata (no timestamp, no entity ID, no sequential counter). This prevents traffic analysis.

**Verify**: Generate 1000 item IDs. Verify no sequential pattern. Verify no embedded timestamp. Verify no common prefix correlating to entity.

### FR-STO-004: Copy-on-Write Sharing (P1)

When a memory is shared to a group, the system SHALL create a copy (`is_copy=1`) with `parent_id` pointing to the original. The original SHALL NOT be modified. The `author_id` on the copy SHALL match the original author (provenance is immutable).

**Verify**: Create item A (author=russell, is_copy=0). Share to group. Verify new item B exists with parent_id=A.id, is_copy=1, author_id=russell. Verify item A unchanged.

### FR-STO-005: Access Tracking (P1)

Every L2 read SHALL increment `access_count` and update `last_accessed_at`. These columns feed governance voting weight and TTL-based natural selection.

**Verify**: Write item. Read it 5 times. Verify access_count=5. Verify last_accessed_at is within 1 second of now.

### FR-STO-006: FTS5 Search (P1)

The `l2_fts` virtual table SHALL support BM25-ranked keyword search with porter stemming and unicode61 tokenization.

**Verify**: Insert items with known text. Search for stemmed term (e.g., "running" matches "run"). Verify results ordered by BM25 relevance. Verify unicode characters are searchable.

### FR-STO-007: Integrity Canary (P1)

The `integrity_canary` table SHALL contain a single row with a known value. On startup, the node SHALL verify the canary. A missing or altered canary indicates database tampering.

**Verify**: Write canary. Verify read matches. Manually corrupt canary value. Verify startup check detects corruption and logs alert.

### FR-STO-008: Schema Migration (P1)

The storage layer SHALL support forward migration from any previous schema version to v4. Migration SHALL be idempotent (running it twice produces no error and no data change).

**Verify**: Create v2 database. Run migration. Verify v4 schema. Run migration again. Verify no error, no data change.

### FR-STO-009: Indexes (P1)

The following indexes SHALL exist for query performance:

- `idx_l2_items_group` on `l2_items(group_id)` WHERE group_id IS NOT NULL
- `idx_l2_items_parent` on `l2_items(parent_id)` WHERE parent_id IS NOT NULL
- `idx_l2_items_author` on `l2_items(author_id)` WHERE author_id IS NOT NULL
- `idx_access_log_entity` on `access_log(entity_id)`
- `idx_access_log_group` on `access_log(group_id)` WHERE group_id IS NOT NULL

**Verify**: Query `sqlite_master` for all indexes. Verify each listed index exists.

### 4.3 Governor (cordelia-governor)

### FR-GOV-001: Peer State Machine (P1)

The governor SHALL manage peer lifecycle through four states: `Cold -> Warm -> Hot`. Additionally, peers may be `Banned` with exponential backoff.

| Transition | Condition |
|------------|-----------|
| Cold -> Warm | Successful handshake |
| Warm -> Hot | Sufficient item delivery score |
| Hot -> Warm | Stale (30m no items) OR churn rotation |
| Warm -> Cold | Dead (90s inactivity) |
| Any -> Banned | Protocol violation, repeated failure |
| Banned -> Cold | After ban duration (exponential backoff) |

**Verify**: Unit test each transition. Inject events (handshake success, timeout, protocol violation). Verify resulting state.

### FR-GOV-002: Peer Scoring (P1)

Peer score SHALL be computed as: `items_delivered / elapsed * (1 / (1 + rtt_ms / 100))`. Higher scores indicate more useful, lower-latency peers.

**Verify**: Set known item count, elapsed time, RTT. Compute score. Verify against expected value. Verify peer with lower RTT scores higher given same delivery rate.

### FR-GOV-003: Churn Rotation (P1)

The governor SHALL rotate 20% of warm peers every 1 hour. This prevents eclipse attacks where an adversary surrounds a node with colluding peers.

**Verify**: Set up 10 warm peers. Advance clock 1 hour. Verify 2 peers demoted. Verify new peers promoted from cold pool.

### FR-GOV-004: Governor Tick (P1)

The governor SHALL tick every 10 seconds, evaluating timeouts and computing promotions/demotions. The tick SHALL return a list of actions (connect, disconnect, state transitions).

**Verify**: Create governor with known peer set. Call tick(). Verify returned actions match expected state transitions.

### FR-GOV-005: Configurable Targets (P1)

The governor SHALL accept configurable targets:

| Parameter | Default | Range |
|-----------|---------|-------|
| `hot_min` | 2 | 1-20 |
| `hot_max` | 20 | hot_min-100 |
| `warm_min` | 10 | 1-50 |
| `warm_max` | 50 | warm_min-200 |
| `cold_max` | 100 | -- |

**Verify**: Set hot_min=5. Verify governor promotes 5th peer to hot when available. Set hot_min=1. Verify only 1 hot peer maintained.

### FR-GOV-006: Ban Backoff (P1)

Ban duration SHALL use exponential backoff: `base_duration * 2^(escalation_count - 1)` with a configurable base (default: 1 hour) and maximum (default: 24 hours).

**Verify**: Ban peer once -- duration 1h. Ban again -- 2h. Again -- 4h. Verify cap at 24h.

### 4.4 QUIC Transport

### FR-QIC-001: QUIC Protocol (P0)

Node-to-node communication SHALL use QUIC (quinn) over UDP port 9474. Transport SHALL use TLS 1.3 with self-signed certificates.

**Verify**: Start two nodes. Verify QUIC connection established. Capture UDP traffic on port 9474. Verify TLS handshake present.

### FR-QIC-002: Mini-Protocol Multiplexing (P1)

Five mini-protocols SHALL be multiplexed on QUIC streams via a single-byte protocol prefix:

| Byte | Protocol | Direction |
|------|----------|-----------|
| 0x01 | Handshake | Bidirectional (stream 0 only) |
| 0x02 | Keep-Alive | Bidirectional |
| 0x03 | Peer-Share | Request/Response |
| 0x04 | Memory-Sync | Request/Response |
| 0x05 | Memory-Fetch | Request/Response |

**Verify**: For each protocol, open stream, send prefix byte + valid message, verify correct handler dispatched. Send unknown prefix byte (0xFF) -- connection must not crash (log warning, close stream).

### FR-QIC-003: Wire Format (P1)

All messages SHALL use: 4-byte big-endian length prefix + serde JSON payload. Maximum message size: 16MB.

**Verify**: Encode message. Verify first 4 bytes are big-endian length. Verify remaining bytes are valid JSON. Send message exceeding 16MB -- verify rejection.

### FR-QIC-004: Handshake Protocol (P0)

On connection, the initiator SHALL send `HandshakePropose` with protocol magic (`0xC0DE11A1`), version range, node_id, and group list. The responder SHALL reply with `HandshakeAccept` containing negotiated version and group intersection.

A mismatched magic SHALL result in `version: 0` (rejection) with `reject_reason`.

**Verify**: Normal handshake -- verify version negotiated. Send wrong magic -- verify rejection. Send version range with no overlap -- verify rejection with reason.

### FR-QIC-005: Keep-Alive (P1)

Peers SHALL exchange Ping/Pong messages at 15-second intervals. 3 consecutive missed pongs SHALL trigger dead-peer detection (demotion via governor).

RTT SHALL be measured from `sent_at_ns` to `recv_at_ns` (nanosecond resolution).

**Verify**: Start keep-alive. Verify Ping sent every 15s. Reply with Pong. Verify RTT computed. Suppress 3 Pongs. Verify governor receives dead-peer notification.

### FR-QIC-006: Peer Sharing (P1)

Peers SHALL exchange known peer addresses every 300 seconds. Response SHALL include `node_id`, addresses, `last_seen`, and group memberships.

**Verify**: Request peers with max_peers=5. Verify response contains <= 5 entries. Verify each entry has valid node_id (32 bytes hex), at least one address, and last_seen timestamp.

### FR-QIC-007: Memory Sync (Anti-Entropy) (P1)

Peers SHALL periodically exchange item headers (id, type, checksum, updated_at, author_id, is_deletion) for shared groups. Missing or divergent items SHALL trigger fetch.

**Verify**: Node A has items {X, Y}. Node B has items {Y, Z}. Sync. Verify A receives Z header, B receives X header. Verify subsequent fetch retrieves missing items.

### FR-QIC-008: Memory Fetch (P1)

Batch item retrieval by ID. Maximum 100 items per request. Response SHALL include encrypted blob, checksum, author_id, group_id, key_version, parent_id, is_copy, updated_at.

**Verify**: Request 3 known items. Verify 3 returned with all fields populated. Request >100 items -- verify rejection or truncation to 100. Request non-existent ID -- verify empty result for that ID.

### FR-QIC-009: Connection Idle Timeout (P1)

QUIC connections SHALL timeout after 300 seconds of inactivity. Keep-alive prevents timeout during active relationships.

**Verify**: Establish connection. Do nothing for 300s. Verify connection closed. Establish connection with keep-alive. Verify connection survives 300s.

### 4.5 Replication

### FR-REP-001: Culture-Governed Dispatch (P1)

On L2 write, replication behaviour SHALL be determined by the item's group culture:

| Culture | Behaviour |
|---------|-----------|
| `chatty` | Eager push to all hot peers in group |
| `moderate` | Notify peers (header only), they fetch on demand |
| `taciturn` | No active push. Anti-entropy sync only. Items expire via TTL. |

**Verify**: Set group culture to "chatty". Write item. Verify push to all hot peers. Set to "taciturn". Write item. Verify no push (only available via sync).

### FR-REP-002: Anti-Entropy Sync (P1)

The replication task SHALL periodically (configurable, default: 300s) select a random warm or hot peer and run memory-sync to detect divergence.

**Verify**: Two nodes with divergent state. Wait for sync interval. Verify convergence. Verify only items in shared groups are synced.

### FR-REP-003: Tombstone Replication (P1)

Deletions SHALL replicate as headers with `is_deletion: true`. Receiving nodes SHALL mark items as deleted (soft delete) or remove them per group policy.

**Verify**: Delete item on Node A. Verify sync sends deletion header. Node B processes it. Verify item no longer returned by search on Node B.

### FR-REP-004: Conflict Resolution (P1)

When two nodes have different versions of the same item (same `id`, different `checksum`), the system SHALL resolve by:
1. Last-writer-wins based on `updated_at` timestamp
2. If timestamps equal, higher `checksum` (lexicographic) wins

**Verify**: Write item on A and B simultaneously with different content. Sync. Both nodes converge to same version.

### 4.6 Crypto (cordelia-crypto)

### FR-CRY-001: AES-256-GCM (P0)

Encryption SHALL use AES-256-GCM with 12-byte IV and 16-byte authentication tag.

**Verify**: Encrypt known plaintext. Verify ciphertext length = plaintext length + 12 (IV) + 16 (tag) + overhead. Decrypt. Verify roundtrip. Tamper with ciphertext -- verify decryption fails.

### FR-CRY-002: Ed25519 Identity (P0)

Node identity SHALL use Ed25519 keypairs (via `ring`). Key generation SHALL produce a 32-byte public key and 64-byte keypair.

**Verify**: Generate keypair. Verify public key length = 32 bytes. Sign message. Verify signature. Tamper with message -- verify signature invalid.

### FR-CRY-003: Cross-Language Round-Trip (P0)

Encrypted payloads produced by the TypeScript proxy SHALL be decryptable by the Rust crypto crate, and vice versa. Same key + same plaintext SHALL produce structurally identical (though not bitwise identical due to random IV) encrypted payloads.

**Verify**: TypeScript encrypts test vector. Rust decrypts. Verify match. Rust encrypts. TypeScript decrypts. Verify match.

### 4.7 Protocol (cordelia-protocol)

### FR-PRT-001: Protocol Magic (P0)

Protocol magic SHALL be `0xC0DE11A1`. Any handshake with different magic SHALL be rejected.

**Verify**: Send handshake with correct magic -- accepted. Send with `0xDEADBEEF` -- rejected.

### FR-PRT-002: Message Roundtrip (P1)

All message variants SHALL serialize to JSON and deserialize back without loss. Tagged union via `serde(tag = "type")`.

**Verify**: For each Message variant, serialize to JSON, deserialize, assert equality.

### FR-PRT-003: Codec Length Prefix (P1)

The codec SHALL use 4-byte big-endian length prefix. Decoder SHALL reject messages where payload length exceeds 16MB (`16 * 1024 * 1024` bytes).

**Verify**: Encode message. Read 4 bytes. Verify matches payload length. Craft 16MB+1 byte message. Verify rejection.

---

## 5. Primitives

### FR-PRM-001: Five Primitives (P0)

The system SHALL implement exactly five primitives:

| Primitive | Storage | Semantics |
|-----------|---------|-----------|
| **Entity** | `l1_hot` + `l2_items` (type='entity') | Sovereign. Holds own keys. |
| **Memory** | `l2_items` | Encrypted blob + vector. Immutable author provenance. COW via parent_id. |
| **Group** | `groups` + `group_members` | Universal sharing primitive. group_id = SHA-256(URI). |
| **Culture** | `groups.culture` JSON | Per-group replication policy (chatty/moderate/taciturn). |
| **Trust** | Derived from `access_log` + accuracy | Not stored directly. Computed empirically. |

**Verify**: For each primitive, verify storage location, CRUD operations, and constraint enforcement.

### FR-PRM-002: Nine Rules (P0)

The system SHALL enforce these rules at all times:

1. **Entity sovereignty**: No external force overrides entity decisions
2. **Private by default**: New items are visibility='private' unless explicitly set
3. **Groups are universal**: All sharing goes through group membership
4. **Encrypt before storage**: Node never sees plaintext (see FR-ENC-001)
5. **Memory is identity**: Memory defines the entity (L1 hot = who you are)
6. **Trust is local**: Each entity computes its own trust independently
7. **Novelty over volume**: Persistence gated by novelty score
8. **Model-agnostic**: No LLM dependency (see FR-ENT-001)
9. **Protocol upgrade via access-weighted voting**: Changes require weighted consensus

**Verify**: Each rule maps to one or more testable FR above. Rule 2: write item without specifying visibility, verify default='private'. Rule 3: attempt to share without group -- must fail.

---

## 6. Groups and Access Control

### FR-GRP-001: Group Roles (P1)

Group members SHALL have one of four roles with hierarchical permissions:

| Role | Read | Write own | Write all | Delete | Admin | Transfer ownership |
|------|------|-----------|-----------|--------|-------|--------------------|
| viewer | Y | N | N | N | N | N |
| member | Y | Y | N | N | N | N |
| admin | Y | Y | Y | Y | Y | N |
| owner | Y | Y | Y | Y | Y | Y |

**Verify**: For each role, attempt each operation. Verify allowed/denied matches table.

### FR-GRP-002: Group Postures (P1)

Members SHALL have one of three postures:

| Posture | Behaviour |
|---------|-----------|
| `active` | Full participation in replication |
| `silent` | Read-only, reduced network traffic |
| `emcon` | Emergency communications only, isolated |

**Verify**: Set member to `silent`. Verify no outbound replication for that member. Set to `emcon`. Verify no inbound or outbound except emergency messages.

### FR-GRP-003: Group ID Derivation (P1)

Group IDs SHALL be `SHA-256(URI)` where URI is a human-readable identifier. The hash is public (discoverable via gossip). The URI is private to members.

**Verify**: Create group with URI "seed-drill://team/founders". Verify group_id = SHA-256 of that URI. Non-members who see the hash cannot derive the URI.

### FR-GRP-004: Culture Configuration (P1)

Each group SHALL have a `culture` JSON field containing at minimum:

```json
{
  "broadcast_eagerness": "chatty" | "moderate" | "taciturn",
  "ttl_default": <seconds>
}
```

**Verify**: Create group with each eagerness level. Verify replication behaviour matches FR-REP-001. Set TTL. Verify items expire after TTL.

---

## 7. Node Roles

### FR-ROL-001: Single Binary (P0)

ALL node roles (personal, bootnode, edge relay, secret keeper, archive) SHALL be the same Rust binary. Role is determined by `[capabilities]` configuration, not by build variant.

**Verify**: Build binary once. Run with default config -- personal node. Run with `relay=true` -- edge relay. Same binary hash.

### FR-ROL-002: Capabilities Advertisement (P1)

Nodes SHALL advertise their capabilities in gossip (PeerAddress). Peers can discover which nodes offer keeper, archive, or relay services.

**Verify**: Start node with `keeper=true`. Exchange peer info. Verify capability present in PeerAddress.

### FR-ROL-003: Bootnode (P1)

A bootnode SHALL be an always-on personal node with a publicly reachable address. It SHALL accept incoming QUIC connections and serve as initial peer for new nodes.

**Verify**: Start bootnode. Dial from new node. Verify handshake succeeds. Verify peer-share returns other known peers.

---

## 8. Backup and Recovery

### FR-BAK-001: Backup with Manifest (P1)

Backup SHALL produce a copy of the SQLite database file plus a JSON manifest containing SHA-256 checksum, timestamp, schema version, and item counts.

**Verify**: Run backup. Verify .db file exists. Verify .manifest.json exists. Verify SHA-256 in manifest matches actual file hash.

### FR-BAK-002: Restore with Verification (P1)

Restore SHALL verify SHA-256 checksum before applying. On mismatch, restore SHALL abort with error.

**Verify**: Run backup. Corrupt backup file (flip one byte). Attempt restore. Verify abort with checksum mismatch error.

### FR-BAK-003: Restore Idempotency (P1)

Restoring the same backup twice SHALL produce the same result.

**Verify**: Backup. Restore. Verify state. Restore again. Verify identical state.

---

## 9. Dashboard HTTP API

### FR-DASH-001: Dashboard HTTP Server (P1)

The proxy SHALL serve a dashboard HTTP server on a configurable port (default: 3847). The server SHALL serve both JSON API endpoints (`/dash/api/*`) and static files (`/dashboard/`). The server SHALL be enabled/disabled via `proxy.toml` (`dashboard_enabled`).

**Verify**: Start proxy with `dashboard_enabled=true`. Verify port 3847 accepts connections. Start with `dashboard_enabled=false`. Verify port 3847 refuses connections. Set `dashboard_port=4000`. Verify port 4000 accepts connections.

### FR-DASH-002: Dashboard Authentication Endpoints (P1)

The proxy SHALL expose the following authentication endpoints:

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/auth/status` | Return current auth status (authenticated, entity_id, github_id) |
| GET | `/auth/github` | Initiate GitHub OAuth 2.0 flow (redirect) |
| GET | `/auth/github/callback` | Handle OAuth code exchange, set session cookie |
| POST | `/auth/logout` | Clear session cookie |

**Verify**: Call `/auth/status` unauthenticated -- returns `{ authenticated: false }`. Complete OAuth flow -- returns `{ authenticated: true, entity_id, github_id }`. Call `/auth/logout` -- subsequent `/auth/status` returns `{ authenticated: false }`.

### FR-DASH-003: Entity Profile Endpoints (P1)

The proxy SHALL expose entity profile management:

| Method | Path | Auth | Purpose |
|--------|------|------|---------|
| GET | `/dash/api/profile/:entityId` | Session cookie (own profile or admin) | Get entity profile |
| POST | `/dash/api/profile/:entityId/api-key` | Session cookie (own profile only) | Regenerate API key |
| DELETE | `/dash/api/profile/:entityId` | Session cookie (own profile only) | Delete profile |
| GET | `/dash/api/profile/:entityId/export` | Session cookie (own profile only) | Export L1 + all owned L2 items (decrypted) |

API key regeneration SHALL invalidate the previous key immediately. Profile deletion SHALL optionally delete L2 items (`?delete_items=true`).

**Verify**: Get own profile -- 200 with entity details. Get another entity's profile without admin -- 403. Regenerate API key -- old key returns 401, new key returns 200. Delete profile -- entity no longer in `/auth/status`. Export -- returns JSON with L1 hot context + all owned L2 items decrypted.

### FR-DASH-004: Memory Dashboard Endpoints (P1)

The proxy SHALL expose memory viewing endpoints for the dashboard:

| Method | Path | Auth | Purpose |
|--------|------|------|---------|
| GET | `/dash/api/hot/:entityId` | Session cookie | Read L1 hot context (decrypted) |
| GET | `/dash/api/l2/search` | Session cookie | Search L2 index (query, type, limit) |
| GET | `/dash/api/l2/item/:id` | Session cookie + visibility check | Read L2 item (decrypted) |

Search results SHALL include: id, type, name, tags, score, group_id, created_at. Item read SHALL enforce visibility: private items only visible to owner, group items only visible to group members.

**Verify**: Read own L1 -- 200, decrypted JSON. Search L2 -- results respect visibility. Read private item as non-owner -- 403. Read group item as non-member -- 403. Read own private item -- 200.

### FR-DASH-005: Group Dashboard Endpoints (P1)

The proxy SHALL expose group management endpoints:

| Method | Path | Auth | Purpose |
|--------|------|------|---------|
| GET | `/dash/api/groups` | Session cookie | List groups entity belongs to |
| GET | `/dash/api/groups/:groupId` | Session cookie + membership | Group detail with members and stats |
| GET | `/dash/api/groups/:groupId/items` | Session cookie + membership | Paginated group items |

Group list SHALL only return groups the authenticated entity is a member of. Group detail SHALL include members, recent items, and stats.

**Verify**: List groups -- returns only joined groups. Request group user is not a member of -- 403. Get group detail -- includes member list and culture. Get items -- paginated, respects `since` and `limit` parameters.

### FR-DASH-006: Node Status Dashboard Endpoints (P1)

The proxy SHALL expose node status endpoints:

| Method | Path | Auth | Purpose |
|--------|------|------|---------|
| GET | `/dash/api/status` | Session cookie | Node overview (role, peers, storage, encryption) |
| GET | `/dash/api/peers` | Session cookie + admin role | Peer detail (node_id, addr, state, RTT, capabilities) |

Status SHALL aggregate node status, peer summary, storage counts, and encryption state into a single response.

**Verify**: Get status -- returns node info, peer summary, storage counts. Get peers without admin role -- 403. Get peers with admin role -- returns detailed peer list.

### FR-DASH-007: Admin Endpoints (P1)

The proxy SHALL expose admin endpoints, gated by admin role:

| Method | Path | Auth | Purpose |
|--------|------|------|---------|
| GET | `/dash/api/admin/entities` | Session cookie + admin | List all entities with stats |
| GET | `/dash/api/admin/stats` | Session cookie + admin | Aggregate system statistics |
| GET | `/dash/api/admin/audit` | Session cookie + admin | Paginated audit log (since, limit) |

**Verify**: Call any admin endpoint without admin role -- 403. Call with admin role -- 200. Audit log respects `since` and `limit` parameters. Stats include entity count, group count, L2 item count, DB size, uptime.

### FR-DASH-008: Static Dashboard Serving (P1)

The proxy SHALL serve static HTML/CSS/JS files from a configurable directory (default: `dashboard/`). Pages:

| Page | URL | Auth |
|------|-----|------|
| Landing | `/` | None |
| Login | `/login` | None |
| Dashboard | `/dashboard` | Required |
| Admin | `/admin` | Required + admin role |
| Enroll | `/enroll` | None (OAuth required during flow) |
| Groups | `/groups` | Required |
| Peers | `/peers` | Required + admin/bootnode role |

**Verify**: Request `/` -- serves landing page (200). Request `/dashboard` unauthenticated -- redirects to `/login`. Request `/admin` without admin role -- 403.

### FR-DASH-009: Role-Gated API Surfaces (P1)

The proxy SHALL expose additional API endpoints based on its configured role:

| Role | Additional Endpoints |
|------|---------------------|
| `personal` | Base dashboard only |
| `bootnode` | `/dash/api/peers` available to all authenticated users |
| `keeper` | `/dash/api/keeper/*` (R4) |
| `archive` | `/dash/api/archive/*` (R4) |

**Verify**: Start proxy as `personal` role. Request `/dash/api/keeper/shards` -- 404. Start as `keeper` role. Request `/dash/api/keeper/shards` -- 200 (or stub).

---

## 10. Authentication and Session Management

### FR-AUTH-001: GitHub OAuth 2.0 (P1)

The proxy SHALL implement GitHub OAuth 2.0 with scopes `read:user` and `read:org`. The OAuth client ID and secret SHALL be configurable via `proxy.toml` or environment variables (`CORDELIA_GITHUB_CLIENT_ID`, `CORDELIA_GITHUB_SECRET`).

**Verify**: Initiate OAuth flow. Verify redirect to GitHub with correct client_id and scopes. Complete callback. Verify session established with correct github_id.

### FR-AUTH-002: Session Cookies (P1)

The proxy SHALL issue signed HTTP-only secure cookies after successful OAuth. Sessions SHALL have a configurable TTL (default: 24 hours). The session secret SHALL be configurable via `proxy.toml` or `CORDELIA_SESSION_SECRET`.

**Verify**: Complete OAuth. Verify `Set-Cookie` header with `HttpOnly`, `Secure` flags. Verify session valid for duration. Expire session. Verify subsequent requests return 401.

### FR-AUTH-003: API Key Authentication (P1)

The proxy SHALL support API key authentication for programmatic access (CLI uploads, automation). API keys SHALL be formatted as `ck_<64hex>`. Keys SHALL be stored as SHA-256 hashes, never plaintext.

**Verify**: Generate API key. Use it in `Authorization: Bearer ck_<hex>` header. Verify 200. Use invalid key. Verify 401. Regenerate key. Verify old key returns 401, new key returns 200.

### FR-AUTH-004: Entity-GitHub Mapping (P1)

The proxy SHALL maintain a mapping from GitHub user ID to Cordelia entity ID. On first OAuth login, the proxy SHALL create a new entity profile if one does not exist. On subsequent logins, the proxy SHALL resolve to the existing entity.

**Verify**: First login with new GitHub account -- entity created. Second login with same account -- same entity resolved. Login with different GitHub account -- different entity.

### FR-AUTH-005: Authorization Checks (P0)

Every dashboard API endpoint SHALL enforce authorization:

| Check | Applies to |
|-------|-----------|
| Authenticated | All `/dash/api/*` endpoints |
| Own profile or admin | `/dash/api/profile/:entityId` |
| Own profile only | API key regen, delete, export |
| Group member | `/dash/api/groups/:groupId/*` |
| Visibility (private/group) | `/dash/api/l2/item/:id` |
| Admin role | `/dash/api/admin/*`, `/dash/api/peers` |

Any authorization failure SHALL return HTTP 403 with `{ "error": "forbidden" }`.

**Verify**: For each check, attempt access from an unauthorized context. Verify 403. Attempt from authorized context. Verify 200.

---

## 11. Device Enrollment

### FR-ENROLL-001: Device Authorization Grant (P1)

The system SHALL implement RFC 8628 Device Authorization Grant for enrolling new proxy instances to a Rust node. The Rust node SHALL expose:

```
POST /api/v1/device/begin
  Request:  { "client_id": "cordelia-proxy" }
  Response: { "device_code": "ABCD-EFGH",
              "verification_uri": "https://dash.cordelia.seeddrill.ai/enroll",
              "expires_in": 600, "interval": 5 }
```

Device codes SHALL be 8 alphanumeric characters (4-4 format). Codes SHALL expire after `expires_in` seconds (default: 600).

**Verify**: Call `/device/begin`. Verify response contains device_code, verification_uri, expires_in. Verify code is 8 chars in XXXX-XXXX format. Wait past expiry. Verify code is rejected.

### FR-ENROLL-002: Device Code Polling (P1)

The proxy SHALL poll the Rust node for enrollment completion:

```
POST /api/v1/device/poll
  Request:  { "device_code": "ABCD-EFGH" }
  Response: { "status": "pending" }
           | { "status": "complete", "bearer_token": "ck_<64hex>",
               "entity_id": "russell", "node_url": "http://127.0.0.1:9473" }
           | { "status": "expired" }
```

Polling SHALL respect the `interval` from the begin response (minimum 5 seconds). Polling faster than the interval SHALL return HTTP 429.

**Verify**: Begin enrollment. Poll before completion -- `pending`. Complete enrollment. Poll -- `complete` with bearer_token. Poll expired code -- `expired`. Poll at 1-second intervals -- 429.

### FR-ENROLL-003: Device Registration (P1)

The dashboard SHALL complete device registration after the entity authenticates via GitHub OAuth:

```
POST /api/v1/device/register
  Request:  { "github_token": "<oauth_token>", "device_code": "ABCD-EFGH" }
  Response: { "ok": true, "entity_id": "russell" }
```

This endpoint is called by the dashboard (not the proxy). It links the device code to the authenticated entity and generates the bearer token that the polling proxy will receive.

**Verify**: Begin enrollment on proxy. Authenticate on dashboard. Call register with valid device_code and github_token. Verify poll returns `complete`. Verify bearer_token works against node API.

### FR-ENROLL-004: Device Storage (P1)

The Rust node SHALL store enrolled devices in the `devices` table:

| Column | Type | Purpose |
|--------|------|---------|
| device_id | TEXT PK | Unique device identifier |
| entity_id | TEXT NOT NULL | Owning entity |
| bearer_token_hash | TEXT NOT NULL | SHA-256 of bearer token |
| enrolled_at | TEXT NOT NULL | ISO 8601 timestamp |
| last_seen | TEXT | Last API request timestamp |
| user_agent | TEXT | Client identifier |

Multiple devices per entity SHALL be supported. Each device has its own bearer token.

**Verify**: Enroll two devices for same entity. Verify both appear in `devices` table. Verify each has unique token. Use device A's token -- verify `last_seen` updates. Use device B's token -- verify independent tracking.

---

## 12. Multi-Tenant

### FR-MULTI-001: Org as Top-Level Group (P1)

In managed (multi-tenant) mode, each organization SHALL be represented as a top-level group. Creating an org SHALL create a group with the org's `org_id`. All entity membership within the org SHALL be through this group.

**Verify**: Create org "acme". Verify group "acme" created. Add entity to org. Verify entity is member of "acme" group. Entity can see group items. Non-member cannot.

### FR-MULTI-002: Org-Scoped Sessions (P1)

In managed mode, the session cookie SHALL carry the entity's `org_id`. All proxy-to-node API calls SHALL include `org_id` for scoping. The node SHALL filter all query results by `org_id` when provided.

**Verify**: Login as entity in org A. Search L2 -- results only from org A groups. Login as entity in org B. Search L2 -- results only from org B groups. Verify no cross-org leakage.

### FR-MULTI-003: Cross-Org Isolation (P0)

In managed mode, entities in org A SHALL NOT be able to read, search, or discover any memory, group, or entity belonging to org B. This invariant SHALL hold across all API endpoints (MCP tools, dashboard API, node API).

**Verify**: Create items in org A and org B. Attempt to read org B item from org A session -- 403 or empty. Attempt to search -- results exclude org B. Attempt to list groups -- org B groups absent. Attempt to fetch org B entity profile -- 403.

### FR-MULTI-004: Self-Hosted Compatibility (P1)

In self-hosted (single-tenant) mode, `org_id` scoping SHALL NOT be applied. The node SHALL process all queries without org filtering when no `org_id` is provided. Self-hosted mode SHALL be the default -- no configuration needed.

**Verify**: Start node without org scoping config. Create items across multiple groups. Search -- all items returned regardless of group. Verify no org_id present in queries.

### FR-MULTI-005: Per-Org Encryption Keys (P0)

In managed mode, each organization SHALL have its own encryption key. A compromise of org A's key SHALL NOT expose org B's data. Seed Drill infrastructure SHALL NEVER hold any org's encryption keys.

**Verify**: Encrypt item with org A key. Attempt decrypt with org B key -- failure. Inspect managed infrastructure -- no plaintext keys in config, env, or storage.

---

## 13. Non-Functional Requirements (NFR)

### NFR-PERF-001: L1 Read Latency (P1)

L1 hot context read SHALL complete in <10ms (local SQLite).

**Verify**: Benchmark 1000 L1 reads. Verify p99 < 10ms.

### NFR-PERF-002: L2 Search Latency (P1)

L2 keyword search (FTS5) SHALL complete in <100ms for databases up to 100,000 items.

**Verify**: Seed database with 100,000 items. Run 100 searches. Verify p99 < 100ms.

### NFR-PERF-003: Replication Latency (P2)

For `chatty` groups, write-to-replication-delivery SHALL complete in <5 seconds to hot peers.

**Verify**: Two-node test. Write on A. Measure time until item appears on B. Verify < 5s.

### NFR-SEC-001: No Plaintext at Rest (P0)

No plaintext memory content SHALL exist in the SQLite database or any persistent storage on the Rust node.

**Verify**: Write items via proxy. Open SQLite database directly. Scan all BLOB columns. Verify no readable plaintext (attempt JSON parse -- must fail or return encrypted structure).

### NFR-SEC-002: No Plaintext in Transit (P0)

Memory content SHALL be encrypted before transmission over any network (HTTP to local node, QUIC to peers). QUIC provides transport encryption; content encryption provides defence-in-depth.

**Verify**: Capture QUIC traffic. Verify TLS 1.3 encryption. Inspect payload (even after TLS termination in test) -- verify encrypted blob, not plaintext.

### NFR-SEC-003: Audit Trail Immutability (P1)

The `access_log` and `audit` tables SHALL be append-only. No UPDATE or DELETE operations SHALL be permitted on these tables.

**Verify**: Attempt UPDATE on access_log -- verify failure or policy block. Attempt DELETE -- verify failure. INSERT -- verify success.

### NFR-SEC-004: Secret Scanning (P1)

CI/CD SHALL scan every commit for credentials patterns: `.env`, `credentials.json`, `id_rsa`, AWS keys, API tokens, database passwords.

**Verify**: Add a file containing `AWS_SECRET_ACCESS_KEY=fake`. Verify CI fails with secret scan alert.

### NFR-REL-001: Graceful Degradation (P1)

Failure of any single peer SHALL not affect local operations. Failure of all peers SHALL not affect local read/write (data is sovereign).

**Verify**: Kill all peers. Verify local read/write/search still works. Verify warning logged. Restart peers. Verify replication resumes.

### NFR-REL-002: Crash Recovery (P1)

After unexpected process termination, the node SHALL recover to a consistent state on restart. SQLite WAL mode provides this guarantee.

**Verify**: Write item. Kill process (SIGKILL) mid-operation. Restart. Verify database integrity (no corruption). Verify committed items are present.

### NFR-REL-003: Connection Resilience (P1)

The node SHALL reconnect to peers after transient network failure with exponential backoff (see FR-GOV-006).

**Verify**: Establish connection. Simulate network partition (iptables/firewall). Restore network. Verify reconnection within backoff window.

---

## 14. CI/CD Requirements

### TR-CI-001: Lint Gate (P1)

All code SHALL pass linting before merge:
- TypeScript: ESLint (strict) + TypeScript strict mode
- Rust: `cargo clippy -- -D warnings`

**Verify**: CI pipeline stage "lint" passes. Introduce lint violation -- verify pipeline fails.

### TR-CI-002: Test Gate (P1)

All tests SHALL pass before merge:
- TypeScript: `npm test` (all 187+ tests)
- Rust: `cargo test --workspace`

**Verify**: CI pipeline stage "test" passes. Introduce failing test -- verify pipeline fails.

### TR-CI-003: Security Gate (P1)

Before merge:
- `npm audit` SHALL report no moderate+ vulnerabilities
- `cargo audit` SHALL report no known vulnerabilities
- Secret scan SHALL find no credential patterns
- License check SHALL verify all dependencies use approved licenses (MIT, Apache-2.0, ISC, BSD, GPL, AGPL)

**Verify**: CI pipeline stage "security" passes. Add dependency with known vulnerability -- verify pipeline fails.

### TR-CI-004: SBOM (P2)

CI SHALL generate a Software Bill of Materials (CycloneDX format) on every release build.

**Verify**: Release build produces `sbom.json`. Verify valid CycloneDX schema. Verify all direct dependencies listed.

### TR-CI-005: Cross-Language Contract Tests (P1)

CI SHALL run contract tests verifying TypeScript proxy and Rust node agree on:
- HTTP API request/response schemas
- Encryption payload format (encrypt in TS, verify structure in Rust)
- GUID format and validation rules

**Verify**: Contract test suite in CI. Change API response shape in Rust without updating proxy -- verify contract test fails.

### TR-CI-006: Property-Based Testing (P1)

Critical paths SHALL have property-based tests (fast-check for TS, proptest for Rust):
- Encryption roundtrip: `forall plaintext: decrypt(encrypt(plaintext)) == plaintext`
- Codec roundtrip: `forall msg: decode(encode(msg)) == msg`
- Governor invariants: `forall events: hot_count <= hot_max AND warm_count <= warm_max`

**Verify**: Property tests in test suite. Run with 10,000 iterations. Zero failures.

### TR-CI-007: Mutation Testing (P2)

Mutation testing (Stryker for TS, cargo-mutants for Rust) SHALL achieve >70% mutation kill rate on critical modules (crypto, storage, policy, governor).

**Verify**: Run mutation testing. Verify kill rate. Surviving mutants reviewed and either killed or documented as acceptable.

### TR-CI-008: Rust Formatting (P1)

All Rust code SHALL pass `cargo fmt --check`. No manual formatting overrides.

**Verify**: CI runs `cargo fmt --check`. Introduce formatting violation -- verify pipeline fails.

---

## 15. Configuration

### FR-CFG-001: Node Configuration (P1)

The Rust node SHALL be configured via `~/.cordelia/config.toml` with sections:

```toml
[node]
identity_key, api_transport, api_addr, database, entity_id

[network]
listen_addr

[[network.bootnodes]]
node_id, addr

[governor]
hot_min, warm_min

[replication]
sync_interval_moderate_secs

[capabilities]
relay, keeper, archive
```

**Verify**: Start node with each config variation. Verify behaviour matches config. Start with missing required field -- verify clear error message.

### FR-CFG-002: Proxy Configuration (P1)

The proxy SHALL be configured via `~/.cordelia/proxy.toml` with sections:

```toml
[proxy]
node_url, node_token_file, encryption_key_file, dashboard_port, dashboard_enabled

[embedding]
provider, url, model

[cache]
l1_ttl_secs, l2_ttl_secs

[oauth]
github_client_id, github_client_secret, session_secret
```

Environment variables SHALL override TOML values when set:

| Variable | TOML Path | Purpose |
|----------|-----------|---------|
| `CORDELIA_NODE_URL` | `proxy.node_url` | Rust node API URL |
| `CORDELIA_NODE_TOKEN` | (inline, bypasses file) | Bearer token |
| `CORDELIA_ENCRYPTION_KEY` | (inline, bypasses file) | Encryption passphrase |
| `CORDELIA_EMBEDDING_PROVIDER` | `embedding.provider` | Embedding provider |
| `CORDELIA_DASHBOARD_PORT` | `proxy.dashboard_port` | Dashboard HTTP port |
| `CORDELIA_GITHUB_CLIENT_ID` | `oauth.github_client_id` | OAuth client ID |
| `CORDELIA_GITHUB_SECRET` | `oauth.github_client_secret` | OAuth client secret |
| `CORDELIA_SESSION_SECRET` | `oauth.session_secret` | Session cookie secret |

TOML file SHALL take precedence over env vars when both are set (except inline token/key overrides).

**Verify**: Start proxy with TOML only -- correct behaviour. Start with env override -- env value used. Start without encryption key in either -- clear error on first write. Start without TOML file -- falls back to env vars only.

---

## 16. Traceability Matrix

| Architecture Component | Requirements |
|----------------------|--------------|
| Entity (diagram: purple) | FR-ENT-001, FR-ENT-002, FR-ENT-003 |
| Proxy (diagram: green) | FR-PRX-001 to FR-PRX-006, FR-ENC-001 to FR-ENC-006 |
| Encryption Boundary (diagram: yellow highlight) | FR-ENC-001 to FR-ENC-006, NFR-SEC-001, NFR-SEC-002, FR-MULTI-005 |
| Node - API (diagram: yellow) | IR-API-001 to IR-API-005 |
| Node - Governor | FR-GOV-001 to FR-GOV-006 |
| Node - Replication | FR-REP-001 to FR-REP-004 |
| Node - QUIC Transport | FR-QIC-001 to FR-QIC-009 |
| Node - Storage (diagram: red) | FR-STO-001 to FR-STO-009, FR-ENROLL-004 |
| Node - Crypto | FR-CRY-001 to FR-CRY-003 |
| Node - Protocol | FR-PRT-001 to FR-PRT-003 |
| SQLite Detail (diagram: centre) | FR-STO-001 to FR-STO-009, FR-ENROLL-004 |
| P2P Network (diagram: blue) | FR-QIC-001 to FR-QIC-009, FR-REP-001 to FR-REP-004 |
| Primitives (diagram: bottom right) | FR-PRM-001, FR-PRM-002 |
| Groups | FR-GRP-001 to FR-GRP-004 |
| Node Roles | FR-ROL-001 to FR-ROL-003, FR-DASH-009 |
| Backup/Recovery | FR-BAK-001 to FR-BAK-003 |
| Dashboard HTTP API | FR-DASH-001 to FR-DASH-009 |
| Authentication | FR-AUTH-001 to FR-AUTH-005 |
| Device Enrollment | FR-ENROLL-001 to FR-ENROLL-004 |
| Multi-Tenant | FR-MULTI-001 to FR-MULTI-005 |
| Performance | NFR-PERF-001 to NFR-PERF-003 |
| Security | NFR-SEC-001 to NFR-SEC-004, FR-ENC-001 to FR-ENC-006, FR-AUTH-005, FR-MULTI-003 |
| Reliability | NFR-REL-001 to NFR-REL-003 |
| CI/CD | TR-CI-001 to TR-CI-008 |
| Configuration | FR-CFG-001, FR-CFG-002 |

---

## 17. Requirement Counts

| Category | P0 | P1 | P2 | Total |
|----------|----|----|-----|-------|
| Entity (FR-ENT) | 2 | 0 | 0 | 3 |
| Proxy (FR-PRX) | 0 | 5 | 1 | 6 |
| Encryption (FR-ENC) | 4 | 2 | 0 | 6 |
| Node API (IR-API) | 2 | 3 | 0 | 5 |
| Storage (FR-STO) | 2 | 7 | 0 | 9 |
| Governor (FR-GOV) | 0 | 6 | 0 | 6 |
| QUIC (FR-QIC) | 2 | 7 | 0 | 9 |
| Replication (FR-REP) | 0 | 4 | 0 | 4 |
| Crypto (FR-CRY) | 3 | 0 | 0 | 3 |
| Protocol (FR-PRT) | 1 | 2 | 0 | 3 |
| Primitives (FR-PRM) | 2 | 0 | 0 | 2 |
| Groups (FR-GRP) | 0 | 4 | 0 | 4 |
| Roles (FR-ROL) | 1 | 2 | 0 | 3 |
| Backup (FR-BAK) | 0 | 3 | 0 | 3 |
| Dashboard (FR-DASH) | 0 | 9 | 0 | 9 |
| Authentication (FR-AUTH) | 1 | 4 | 0 | 5 |
| Enrollment (FR-ENROLL) | 0 | 4 | 0 | 4 |
| Multi-Tenant (FR-MULTI) | 2 | 3 | 0 | 5 |
| Configuration (FR-CFG) | 0 | 2 | 0 | 2 |
| Non-Functional (NFR) | 2 | 7 | 1 | 10 |
| Test (TR-CI) | 0 | 6 | 2 | 8 |
| **Total** | **24** | **80** | **4** | **109** |

P0 invariants (24): These must NEVER be violated. Any violation is a severity-1 bug.

New P0s added in v2.0:
- FR-AUTH-005: Authorization checks on every dashboard endpoint
- FR-MULTI-003: Cross-org isolation
- FR-MULTI-005: Per-org encryption keys

---

*Derived from architecture diagram v2 + HLD v2 (Martin + Russell + Claude, 2026-01-31)*
*Version 2.0 -- added Dashboard, Auth, Enrollment, Multi-Tenant requirements*
*Last updated: 2026-01-31*
*Russell Wing, Martin Stevens, and Claude (Opus 4.5)*
