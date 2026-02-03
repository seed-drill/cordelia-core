# Cordelia Rust P2P Node -- Technical Specification

Single binary (`cordelia-node`). Fully decentralised P2P memory replication. Coutts Cardano topology ported to Rust. Adversarial assumption.

## 1. Crate Structure

```
cordelia-node/
  Cargo.toml              # workspace root
  crates/
    cordelia-node/        # binary -- CLI, config, spawns runtime
    cordelia-protocol/    # wire protocol, message types, mini-protocols
    cordelia-governor/    # peer state machine, promotion/demotion, churn
    cordelia-replication/ # culture-aware replication engine
    cordelia-storage/     # rusqlite wrapper, schema v4 compatible
    cordelia-crypto/      # AES-256-GCM, scrypt (ring), round-trip compat with TS
    cordelia-api/         # local node API (axum, unix socket or HTTP)
```

### Dependencies

| Crate | Dependency | Purpose |
|-------|-----------|---------|
| all | `tokio` 1.x | Async runtime |
| protocol | `quinn` 0.11.x | QUIC transport (multiplexed, TLS 1.3 built-in) |
| protocol | `serde` + `serde_json` | Wire serialisation |
| protocol | `bytes` + `tokio-util` | Codec framing |
| governor | `rand` 0.8.x | Churn randomisation |
| storage | `rusqlite` 0.32.x (`bundled`) | SQLite, same DB file as TS server |
| crypto | `ring` 0.17.x | AES-256-GCM, scrypt, Ed25519 |
| api | `axum` 0.7.x | Local HTTP/Unix socket API |
| node | `clap` 4.x | CLI |
| node | `tracing` | Structured logging |
| all | `proptest` | Property-based testing |

**Why QUIC**: Multiplexed streams (matches Coutts mini-protocol multiplexing), TLS 1.3 for free, connection migration (laptop changes WiFi). No custom multiplexer needed. Fallback: raw TCP + `rustls` + `LengthDelimitedCodec` if quinn proves problematic.

---

## 2. Peer Protocol

QUIC between peers. One bidirectional stream per mini-protocol. 4-byte big-endian length prefix + serde JSON.

### Handshake (stream 0, one round-trip)

```rust
struct HandshakePropose {
    magic: u32,            // 0xC0DE11A1
    version_min: u16,
    version_max: u16,
    node_id: [u8; 32],    // Ed25519 pubkey hash
    timestamp: u64,
    groups: Vec<GroupId>,
}

struct HandshakeAccept {
    version: u16,          // 0 = rejected
    node_id: [u8; 32],
    timestamp: u64,
    groups: Vec<GroupId>,
    reject_reason: Option<String>,
}
```

Group intersection computed. Only intersecting groups replicate. No overlap = peer-sharing only.

### Keep-Alive (30s interval)

```rust
struct Ping { seq: u64, sent_at_ns: u64 }
struct Pong { seq: u64, sent_at_ns: u64, recv_at_ns: u64 }
```

3 missed pings = dead. RTT recorded for governor.

### Peer-Sharing (gossip, every 5 min from 2-3 random warm/hot peers)

```rust
struct PeerShareRequest { max_peers: u16 }
struct PeerShareResponse { peers: Vec<PeerAddress> }

struct PeerAddress {
    node_id: [u8; 32],
    addrs: Vec<SocketAddr>,
    last_seen: u64,
    groups: Vec<GroupId>,
}
```

### Memory-Sync (catch-up + moderate/taciturn discovery)

```rust
struct SyncRequest {
    group_id: GroupId,
    since: Option<String>,  // ISO8601
    limit: u32,
}

struct SyncResponse {
    items: Vec<ItemHeader>,
    has_more: bool,
}

struct ItemHeader {
    item_id: String,
    item_type: String,
    checksum: String,       // SHA-256 of encrypted blob
    updated_at: String,
    author_id: String,
    is_deletion: bool,
}
```

Receiver compares locally: unknown -> queue fetch. Different checksum -> fetch (last-writer-wins by `updated_at`). Deletion -> mark deleted + propagate.

### Memory-Fetch (batch up to 100)

```rust
struct FetchRequest { item_ids: Vec<String> }
struct FetchResponse { items: Vec<FetchedItem> }

struct FetchedItem {
    item_id: String,
    item_type: String,
    encrypted_blob: Vec<u8>,  // opaque, stored as-is, NEVER decrypted by P2P layer
    checksum: String,
    author_id: String,
    group_id: GroupId,
    key_version: u32,
    parent_id: Option<String>,
    is_copy: bool,
    updated_at: String,
}
```

### Memory-Push (0x06, unsolicited item delivery)

Used by replication engine to push items to hot peers without a prior request. Distinct from Memory-Fetch (0x04) and Memory-Sync (0x05) which are request-response.

Sender opens a new QUIC stream with protocol byte `0x06`, writes a `FetchResponse` containing items, then finishes the send side. Receiver stores items via `engine.on_receive()` and replies with a `PushAck`.

```rust
// Sender writes FetchResponse (same as Memory-Fetch response)
// Receiver replies:
struct PushAck {
    stored: u32,    // items successfully stored
    rejected: u32,  // items rejected (dedup, policy, etc.)
}
```

Culture dispatch determines when push is used:
- **EagerPush** (chatty): full item via 0x06 to all hot group peers
- **NotifyAndFetch** (moderate): header via 0x05 SyncResponse
- **Passive** (taciturn): nothing (peers discover via anti-entropy)

### Protocol Byte Summary

| Byte | Protocol | Direction |
|------|----------|-----------|
| 0x01 | Handshake | Bidirectional |
| 0x02 | Keep-Alive | Bidirectional |
| 0x03 | Peer-Sharing | Request-Response |
| 0x04 | Memory-Fetch | Request-Response |
| 0x05 | Memory-Sync | Request-Response |
| 0x06 | Memory-Push | Unsolicited (sender-initiated) |

---

## 3. Peer Governor

Background tokio task, ticks every 10s.

### State Machine

```
              promote            promote
   COLD -----------------> WARM -----------------> HOT
    ^                    |                   |
    |       demote       |      demote       |
    |<-------------------|<------------------|
    |                                        |
    +---- adversarial_demote (any -> BANNED) -+
```

### Data

```rust
struct PeerInfo {
    node_id: NodeId,
    addrs: Vec<SocketAddr>,
    state: PeerState,       // Cold | Warm | Hot | Banned { until, reason }
    groups: Vec<GroupId>,
    rtt_ms: Option<f64>,
    last_activity: Instant,
    items_delivered: u64,
    connection: Option<quinn::Connection>,
}

struct GovernorTargets {
    hot_min: usize,    // 2
    hot_max: usize,    // 20
    warm_min: usize,   // 10
    warm_max: usize,   // 50
    cold_max: usize,   // 100
}
```

### Tick Logic

1. **Reap dead**: No keep-alive 90s -> Hot->Warm, Warm->Cold
2. **Promote Cold->Warm**: If `warm < warm_min`, connect (prefer group overlap)
3. **Promote Warm->Hot**: If `hot < hot_min` or warm outperforms worst hot. Score = `items_delivered / time` weighted by RTT
4. **Demote Hot->Warm**: If `hot > hot_max`, demote worst. Stale (no items 30 min) first
5. **Churn**: Every hour, cycle ~20% warm<->cold
6. **Ban**: Protocol violation -> Banned (1h, escalating)

Bootnodes: in config, added to cold on startup, no special authority. Empty peer list -> immediately promote all bootnodes to warm.

---

## 4. Replication Engine

### Culture Dispatch

```rust
enum ReplicationStrategy {
    EagerPush,       // chatty: send full item to all hot group peers
    NotifyAndFetch,  // moderate: send header, peers pull on demand
    Passive,         // taciturn: nothing, peers discover on periodic sync
}
```

Reads `groups.culture` JSON -> `broadcast_eagerness` field.

### On Local Write

```
culture = load_group_culture(group_id)
hot_peers = governor.hot_peers_for_group(group_id)

EagerPush      -> send FetchedItem to all hot group peers
NotifyAndFetch -> send ItemHeader to all hot group peers
Passive        -> do nothing
```

### On Remote Receive

1. Validate group membership
2. Dedup: same checksum -> skip
3. Conflict: last-writer-wins by `updated_at` (CRDT merge R4)
4. Store encrypted blob (no decryption)
5. Log to `access_log`

### Deletions

Soft-delete tombstones. Retained 7 days. Propagated via same culture strategy.

### Anti-Entropy Sync

Per-group background task. Chatty: real-time. Moderate: 5 min. Taciturn: 15 min. Random hot peer -> SyncRequest -> fetch missing.

---

## 5. Storage

`rusqlite` wrapping the SAME `cordelia.db` as TS MCP server. Schema v4.

```rust
trait Storage: Send + Sync {
    fn read_l2_item(&self, id: &str) -> Result<Option<L2ItemRow>>;
    fn write_l2_item(&self, item: &L2ItemWrite) -> Result<()>;
    fn read_l2_item_meta(&self, id: &str) -> Result<Option<L2ItemMeta>>;
    fn list_group_items(&self, group_id: &str, since: Option<&str>, limit: u32) -> Result<Vec<ItemHeader>>;
    fn read_group(&self, id: &str) -> Result<Option<GroupRow>>;
    fn write_group(&self, id: &str, name: &str, culture: &str, security_policy: &str) -> Result<()>;
    fn list_groups(&self) -> Result<Vec<GroupRow>>;
    fn list_members(&self, group_id: &str) -> Result<Vec<GroupMemberRow>>;
    fn get_membership(&self, group_id: &str, entity_id: &str) -> Result<Option<GroupMemberRow>>;
    fn log_access(&self, entry: &AccessLogEntry) -> Result<()>;
}
```

Schema auto-initialises on first run via `ensure_schema()` -- if the `schema_version` table doesn't exist, the full schema v4 SQL is executed. No manual migration required.

P2P layer does NOT need L1, FTS, or embedding ops. WAL mode + `busy_timeout = 5000ms` for concurrent access with TS process.

---

## 6. Node API (for @cordelia/proxy)

Unix socket (`~/.cordelia/node.sock`) or HTTP (`127.0.0.1:9473`). axum router, bearer token from `~/.cordelia/node-token`.

```
POST /api/v1/l1/read          { user_id }                    -> encrypted blob
POST /api/v1/l1/write         { user_id, data }              -> ok
POST /api/v1/l2/read          { item_id }                    -> { data, type, meta }
POST /api/v1/l2/write         { item_id, type, data, meta }  -> ok (triggers replication)
POST /api/v1/l2/delete        { item_id }                    -> ok (triggers tombstone)
POST /api/v1/l2/search        { query, limit }               -> results
POST /api/v1/groups/create    { group_id, name, culture, security_policy } -> ok (creates group + updates dynamic group list)
POST /api/v1/groups/list      {}                             -> groups
POST /api/v1/groups/read      { group_id }                   -> group + members
POST /api/v1/groups/items     { group_id, since?, limit? }   -> item headers
POST /api/v1/status           {}                             -> node_id, peers, uptime
POST /api/v1/peers            {}                             -> hot/warm/cold details
```

Groups are dynamic -- created via the API and immediately available to the replication engine. The node maintains a shared `Arc<RwLock<Vec<String>>>` of group IDs, updated on group creation, read by anti-entropy sync and culture dispatch.

API does NOT encrypt/decrypt. Passes encrypted blobs through.

---

## 7. Configuration

`~/.cordelia/config.toml`

```toml
[node]
identity_key = "~/.cordelia/node.key"
api_transport = "unix"
api_socket = "~/.cordelia/node.sock"
database = "~/cordelia/memory/cordelia.db"
entity_id = "russell"

[network]
listen_addr = "0.0.0.0:9474"

[[network.bootnodes]]
addr = "russell.cordelia.seeddrill.ai:9474"

[[network.bootnodes]]
addr = "martin.cordelia.seeddrill.ai:9474"

[governor]
hot_min = 2
hot_max = 20
warm_min = 10
warm_max = 50
cold_max = 100
churn_interval_secs = 3600
churn_fraction = 0.2

[replication]
sync_interval_moderate_secs = 300
sync_interval_taciturn_secs = 900
tombstone_retention_days = 7
max_batch_size = 100
```

---

## 8. Security

**Peer auth**: Ed25519 keypair per node. Self-signed X.509 in QUIC TLS. Trust: bootnodes by config, other peers by group membership.

**Invariants**:
1. L1 NEVER leaves the node
2. Private items NEVER replicate
3. Encryption keys NEVER leave the node
4. P2P layer NEVER decrypts -- stores opaque blobs

**Mitigations**: Sybil (group membership gate + entropy cost, see below), Eclipse (20%/hr churn + bootnodes), Replay (checksum dedup), Amplification (batch limits), Injection (membership check), DoS (bans + QUIC flow control).

### Sybil Resistance via Entropy Cost (Proof-of-Useful-Work)

Three defence layers, each independently effective, compounding in combination:

**Layer 1: Group membership gate.** Replication requires group membership. Spinning up nodes is cheap; getting admitted to a group with real members is not.

**Layer 2: Novelty filtering.** Inbound memories pass through the novelty engine. Low-entropy content (repetitive, generic, boilerplate) is rejected before persistence. An attacker must produce content that appears genuinely novel *in the context of the receiving entity's existing knowledge*. This is information-theoretically expensive: the cost of producing content that passes a novelty filter is bounded below by the entropy of the target's context. An outsider attacking a group they don't understand faces maximum entropy -- every message is maximally expensive to craft convincingly.

**Layer 3: Trust calibration.** Memories that don't match reality lose trust over time (see ARCHITECTURE.md trust model). Even if an attacker produces novel content, inaccurate memories are detected empirically and the source loses trust. Sustained attack requires producing content that is novel AND accurate AND relevant -- which converges on genuinely useful contribution.

**Comparison to existing consensus mechanisms:**

| Mechanism | Work type | Energy cost | Sybil resistance |
|-----------|-----------|-------------|-----------------|
| Bitcoin PoW | Arbitrary hash computation | Enormous (wasteful by design) | Strong |
| Cardano PoS | Capital stake (ada locked) | Low | Strong (but stake centralisation risk, nothing-at-stake edge cases) |
| Cordelia PoUW | Producing genuine knowledge | Proportional to value created | Potentially strong (requires formal analysis) |

The key insight: in Cordelia, the "work" required to participate is producing genuinely valuable information that survives novelty filtering and trust calibration. This is a **proof-of-useful-work** -- the energy expenditure is the cognitive/computational cost of generating real knowledge, not arbitrary computation or locked capital.

**Open questions (R4+ formal analysis required):**
- Can the entropy cost be formally bounded? (Shannon lower bound on novelty-passing content)
- How does this compare to PoW/PoS under adversarial assumptions? (game-theoretic analysis)
- What is the cost function for an attacker vs honest participant? (asymmetry analysis)
- Does this generalise beyond memory networks? (potential contribution to consensus theory)
- Edge cases: can an attacker recycle genuine content across groups? (cross-context novelty)

**Status:** Intuition supported by information theory. Layers 1-3 are implemented. Formal analysis required to make quantitative claims. Tagged R4-015 in backlog.

---

## 9. Implementation Status

All crates implemented. 52+ tests passing. End-to-end replication verified live.

| Crate | Status | Tests |
|-------|--------|-------|
| cordelia-storage | Complete (schema v4 auto-init, write_group) | Unit + integration |
| cordelia-crypto | Complete (AES-256-GCM, scrypt, TS round-trip) | Crypto round-trip |
| cordelia-protocol | Complete (wire types, codec, PushAck) | Codec + proptest |
| cordelia-governor | Complete (state machine, replace_node_id) | Unit + proptest |
| cordelia-api | Complete (axum, groups/create endpoint) | Handler tests |
| cordelia-replication | Complete (culture dispatch, push 0x06) | Engine tests |
| cordelia-node | Complete (binary, CLI, config, E2E) | Integration + live bootnode |

### Key S9/S11 Changes

- **Governor identity fix**: Bootnodes seeded with `SHA-256(addr)` fake NodeId, replaced with real handshake NodeId via `replace_node_id()`
- **Memory-Push 0x06**: New protocol for unsolicited item delivery (separate from request-response 0x04/0x05)
- **Dynamic groups**: `Arc<RwLock<Vec<String>>>` shared across governor, replication, and API tasks
- **Group creation API**: `POST /groups/create` writes to storage and updates dynamic group list
- **Schema auto-init**: `ensure_schema()` runs full schema v4 SQL on empty databases

---

## Output

This spec becomes `cordelia-node/SPEC.md`. Martin implements against it. Spec changes need discussion.
