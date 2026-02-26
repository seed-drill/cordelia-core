# Group Lifecycle -- From Creation to Deletion

How groups are created, discovered, joined, used, and removed across the Cordelia P2P network.

**Audience**: Portal developers, proxy integrators, contributors.
**Prerequisites**: Familiarity with [Architecture Overview](../architecture/overview.md) and node roles (relay, keeper, personal).

---

## 1. Create

A group starts on a single node and propagates outward via GroupExchange.

### 1.1 Local creation

The portal or proxy calls the node API:

```
POST /api/v1/groups/create
{
  "group_id": "team-alpha",
  "name": "Team Alpha",
  "culture": "{\"broadcast_eagerness\":\"chatty\"}",
  "security_policy": "{}"
}
```

This does two things:
1. Writes a row to the local `groups` table (SQLite)
2. Adds the group ID to `shared_groups` (in-memory list used by replication)

The group now exists on this node only.

### 1.2 Descriptor signing

On the next GroupExchange cycle (within 60s), the node's `build_descriptors()` function:
1. Queries `storage.list_groups()` -- finds the new group
2. If the group has no signature and the node owns it, signs the descriptor with Ed25519
3. Includes the `GroupDescriptor` in the exchange message

The descriptor contains: `id`, `culture`, `updated_at`, `checksum` (SHA-256), `owner_id`, `owner_pubkey`, `signature`. This is ~200 bytes -- lightweight by design.

### 1.3 Propagation

```
t=0      Group created on agent-alpha-1
t=0-60   GroupExchange tick: agent sends descriptor to edge-alpha
         Edge stores descriptor via merge_descriptors() -> write_group()
t=60     GroupExchange tick: edge sends descriptor to keeper-alpha-1
         (edge includes it because build_descriptors() queries storage)
t=60-120 Keeper stores descriptor
```

After two exchange cycles (~120s worst case), every node within 2 hops has the descriptor. CI measurement: 105s for agent -> edge -> keeper.

**What propagates**: Descriptor only (id, name, culture, signature). NOT membership. NOT items.

### 1.4 Multi-node provisioning

For a group to be usable on a node, the group must be in that node's `shared_groups`. Descriptor propagation puts the group in storage but NOT in `shared_groups`. There are two paths:

- **API creation**: Portal calls `groups/create` on each participating node. This adds to both storage and `shared_groups`.
- **Config provisioning**: Group listed in `config.toml` `[groups]` section. Loaded into `shared_groups` at startup.

Dynamic relay nodes (edges) don't need provisioning -- they learn groups automatically from peers via GroupExchange and add them to `relay_accepted_groups`.

---

## 2. Invite (Add Members)

Membership is local-only (R4-030). Each node manages its own membership table independently.

### 2.1 Portal-driven invitation

The portal is the trust anchor for membership:

```
Portal                          Node A              Node B
  |                               |                   |
  |-- groups/create ------------->|                   |
  |-- groups/create --------------------------------->|
  |                               |                   |
  |-- groups/add_member --------->|                   |
  |   (entity_id: "alice")        |                   |
  |-- groups/add_member --------------------------------->|
  |   (entity_id: "alice")        |                   |
  |                               |                   |
  |   [GroupExchange propagates descriptors between nodes]
```

The portal calls `groups/create` and `groups/add_member` on every node that should participate. This is explicit provisioning -- no magic.

### 2.2 L1 auto-stub

`group_members.entity_id` has a foreign key to `l1_hot.user_id`. When `add_member` is called, a minimal L1 stub (`{}`) is auto-created if the entity doesn't already have an L1 entry. Existing L1 data is never overwritten.

```
POST /api/v1/groups/add_member
{ "group_id": "team-alpha", "entity_id": "alice", "role": "member" }
```

The portal can optionally call `l1/write` beforehand to provision richer identity data, but it's not required.

### 2.3 Roles and postures

| Role | Permissions |
|------|------------|
| `owner` | Full control, can delete group, signs descriptor |
| `admin` | Add/remove members, update policies |
| `member` | Read/write items |
| `viewer` | Read-only |

| Posture | Behaviour |
|---------|-----------|
| `active` | Normal: sends and receives broadcasts |
| `silent` | Receives inbound, no outbound broadcasts |
| `emcon` | Radio silence (emergency only) |

---

## 3. Bootstrap (New Member Sync)

When a new node joins a group, it needs to receive existing items. There is no explicit "join and sync" command -- items arrive via the normal replication mechanisms.

### 3.1 Chatty groups

```
t=0    Portal creates group on new node (shared_groups updated)
t=0-60 GroupExchange: new node advertises group to peers
       Peers recompute group_intersection -- new node now included
t=60+  Anti-entropy sync: new node or peers initiate SyncRequest
       SyncResponse returns item headers, FetchRequest pulls missing items
```

**Expected bootstrap time**: 60-120s for items to start arriving. Full convergence depends on item count and batch size (100 items per fetch, 60s sync interval).

For a group with 500 items: ~5 sync cycles = ~5 minutes to full convergence.

### 3.2 Taciturn groups

```
t=0    Portal creates group on new node
t=0-60 GroupExchange: node advertises, edge learns
t=60   GroupExchange: edge advertises to other peers
t=120+ Anti-entropy sync (900s interval): first sync may not fire for 15 minutes
```

**Expected bootstrap time**: Up to 15 minutes for first items. Full convergence is much slower.

See [#12](https://github.com/seed-drill/cordelia-core/issues/12) for a proposal to trigger immediate sync when a group is added.

### 3.3 Sync protocol detail

```
New node                    Peer
  |                           |
  |-- SyncRequest ----------->|  { group_id, since: null, limit: 100 }
  |<-- SyncResponse ----------|  { items: [headers...], has_more: true }
  |                           |
  |  [compute diff: which items we don't have]
  |                           |
  |-- FetchRequest ---------->|  { item_ids: ["id1", "id2", ...] }
  |<-- FetchResponse ---------|  { items: [full encrypted blobs] }
  |                           |
  |  [on_receive() validates: checksum, group membership, LWW]
  |  [stores items locally]
```

The `since` parameter enables incremental sync after the initial bootstrap. Subsequent syncs only request items newer than the last sync timestamp.

---

## 4. Use (Read/Write Items)

### 4.1 Writing

```
POST /api/v1/l2/write
{
  "item_id": "mem-123",
  "type": "learning",
  "data": { "content": "..." },
  "meta": {
    "group_id": "team-alpha",
    "author_id": "alice",
    "visibility": "group",
    "key_version": 1
  }
}
```

On write:
1. Storage layer computes `checksum = SHA-256(data)` and upserts the item
2. API emits a `WriteNotification` to the replication task
3. Replication engine checks culture:
   - **Chatty**: `OutboundAction::BroadcastItem` -- push full item to all active group peers and relay peers
   - **Taciturn**: `OutboundAction::None` -- item stays local until anti-entropy pulls it

### 4.2 Replication path (chatty)

```
agent-alpha-1        edge-alpha          keeper-alpha-1
  |                     |                     |
  |-- MemoryPush ------>|                     |
  |   [Gate 2: edge     |                     |
  |    accepts, stores] |                     |
  |                     |-- MemoryPush ------>|
  |                     |   [Gate 3: keeper   |
  |                     |    accepts, stores] |
  |                     |                     |
  |   PushAck(stored=1) |   PushAck(stored=1) |
```

- **Gate 1** (writer): push to edge (is_relay) and any hot peers with group_intersection match
- **Gate 2** (edge): dynamic relay accepts because it learned the group via GroupExchange
- **Gate 2 re-push**: edge re-pushes to all active peers except sender
- **Gate 3** (keeper): accepts because `team-alpha` is in `shared_groups`

### 4.3 Cross-org replication

For shared groups that span organisations:

```
agent-alpha-1 -> edge-alpha -> boot1 -> edge-bravo -> keeper-bravo-1
   (write)       (relay)     (relay)    (relay)        (store)
```

Boot nodes are transparent relays -- they accept and forward all groups. Edge nodes on both sides learn the group via GroupExchange. CI measurement: 2s for this path (all nodes pre-converged).

### 4.4 Group isolation

Org-internal groups do NOT cross the backbone:

```
agent-alpha-1 -> edge-alpha -> keeper-alpha-1    (reaches alpha)
                    X
              boot1 (never receives -- edge doesn't push internal groups to backbone)
                    X
              edge-bravo -> keeper-bravo-1       (never receives)
```

Isolation is topological: bravo's edge never learns the alpha-internal group because no bravo peer has it. The item physically cannot reach bravo.

### 4.5 Reading

```
POST /api/v1/l2/read
{ "item_id": "mem-123" }
```

Returns the encrypted blob, type, and metadata. Decryption happens at the proxy layer (AES-256-GCM), not the node.

### 4.6 Conflict resolution

Concurrent writes to the same `item_id` from different nodes are resolved by **Last-Writer-Wins** (LWW):

- Each item has an `updated_at` timestamp (ISO 8601)
- On `on_receive()`, if a local copy exists with a newer `updated_at`, the inbound item is rejected as `Duplicate`
- If the inbound item is newer, it overwrites the local copy
- String comparison on ISO 8601 timestamps is lexicographically correct for ordering

No vector clocks, no CRDTs. LWW is sufficient because:
- Concurrent writes to the same item are rare (items are typically authored by one entity)
- Group culture governs who writes what
- Memory items are append-mostly (new items, not frequent updates)

---

## 5. Leave / Remove Members

### 5.1 Soft removal (R4)

```
POST /api/v1/groups/remove_member
{ "group_id": "team-alpha", "entity_id": "alice" }
```

Sets the member's `posture` to `"removed"` (CoW soft-delete). The member row is retained in storage but filtered from `list_members` and `get_membership` responses. Effects:

- Removed on this node only (membership is local, R4-030)
- Portal must call `remove_member` on all nodes independently
- Items already replicated to the removed member's node persist (encrypted blobs)
- The removed member's node still has the group in `shared_groups` -- portal must also call `groups/delete` on their node to stop replication

### 5.2 What does NOT happen

- Items authored by the removed member are NOT tombstoned (they remain in the group)
- No notification to other members (membership is local-only)
- No key rotation (encryption key unchanged -- removed member can still decrypt historical items)
- No automatic cleanup of the removed member's data on other nodes

### 5.3 Threat model for soft removal

A removed member retains:
- Encrypted copies of all items replicated before removal
- The current group encryption key (can decrypt historical items)
- Knowledge of group ID, member IDs, and item metadata

A removed member cannot:
- Receive new items (replication stops after `shared_groups` removal)
- Forge items (checksum verification + group membership gates)
- Re-join without portal re-invitation

This is acceptable for R4 because item content is encrypted at the proxy layer -- the removed member's node holds opaque ciphertext. The real risk is key compromise, which requires R5 key rotation to mitigate.

### 5.4 Future: hard removal (R5)

When the R5 personal groups PSK model is implemented, `departure_policy` in group culture governs the response:

| Policy | On removal | Key rotation | Items |
|--------|-----------|-------------|-------|
| `permissive` | Clean exit, member retains authored copies | No | Unchanged |
| `standard` | Member loses access, authored items stay | Yes (future items only) | Unchanged |
| `restrictive` | Member loses access, immediate re-key | Yes (all items re-encrypted) | Re-encrypted |

Hard removal flow (standard/restrictive):
1. Portal removes member from all nodes
2. Rotate group PSK (`key_version` increment)
3. New PSK distributed to remaining members via vault
4. Future items encrypted with new key version
5. Removed member cannot decrypt post-rotation items
6. Historical items remain readable via key ring (old PSK retained)

See [member removal design](../design/member-removal.md) for the full threat model and R5 plan.

---

## 6. Delete (Tombstone Propagation)

### 6.1 Tombstone deletion

```
POST /api/v1/groups/delete
{ "group_id": "team-alpha" }
```

Effects:
1. Group culture overwritten with `"__deleted__"` sentinel (tombstone descriptor)
2. Members soft-removed (`posture = "removed"`)
3. Group removed from `shared_groups` (replication stops)
4. Tombstone descriptor propagates to peers via GroupExchange (LWW semantics)
5. L2 items with `group_id = "team-alpha"` are NOT deleted

### 6.2 Propagation via GroupExchange

The tombstone descriptor propagates automatically:

1. Originating node writes tombstone culture
2. Next GroupExchange round sends tombstone to connected peers
3. Receiving peers detect `is_tombstone()`, soft-remove local members, remove from `shared_groups`
4. Peers propagate the tombstone further on their next GroupExchange round
5. Full network propagation: ~60s per hop (GroupExchange interval)

### 6.3 Garbage collection

Tombstoned groups are retained in storage for `TOMBSTONE_RETENTION_DAYS` (default 7) to ensure propagation reaches all peers. A daily GC tick in the replication task purges tombstoned groups past retention -- this is the only path to physical row deletion (CoW invariant).

### 6.4 Items after group deletion

L2 items remain in storage after group deletion. They are orphaned (no group to replicate through) but not deleted. This preserves the CoW invariant and allows recovery if the deletion was accidental (within the retention window, re-creating the group with the same ID would resume replication).

---

## 7. Deployment Patterns Summary

### Pattern A: Shared cross-org group

```
agent-alpha-1 --> edge-alpha --> boot --> edge-bravo --> keeper-bravo-1
   (write)        (dynamic)   (transparent) (dynamic)      (store)
```

- Culture: chatty (eager push)
- Provisioning: `groups/create` on agents + keepers; edges learn automatically
- Convergence: 2-5s (CI measured)

### Pattern B: Org-internal group

```
agent-alpha-1 --> edge-alpha --> keeper-alpha-1
   (write)        (dynamic)      (store)
```

- Culture: chatty
- Provisioning: `groups/create` on agent + keeper; edge learns automatically
- Convergence: 2-5s within org; items never leave org
- Isolation: topological (no cross-org peer has the group)

### Pattern C: Personal group

```
agent-alpha-1 --> edge-alpha --> keeper-alpha-1
   (write)        (dynamic)  --> keeper-alpha-2
                                  (store)
```

- Culture: chatty (personal memory replicates immediately via eager push)
- Provisioning: created during enrollment on agent + keepers; edge learns automatically
- Convergence: 2-5s to keepers
- Encryption: group PSK stored in vault (R5), keepers store ciphertext only
- See [R5 Personal Groups, Section 3.3](../design/R5-personal-groups.md#33-culture) for the rationale behind chatty culture and trade-off analysis

---

## 8. Timing Reference

| Scenario | Expected latency | Notes |
|----------|-----------------|-------|
| Item push (chatty, 1-hop) | 1-2s | Direct push, pre-converged peers |
| Item push (chatty, 2-hop via relay) | 2-5s | Relay store-and-forward |
| Item push (chatty, cross-org via backbone) | 2-5s | Transparent relay, no group check |
| Group descriptor propagation (2-hop) | 60-120s | Two GroupExchange cycles |
| New member bootstrap (chatty, first items) | 60-120s | Anti-entropy after exchange |
| New member bootstrap (chatty, full convergence) | 5-10 min | Depends on item count |
| New member bootstrap (taciturn) | Up to 15 min | 900s sync interval |
| GroupExchange cycle | 60s | 6 governor ticks x 10s |
| Anti-entropy sync (chatty) | 60s | Safety net interval |
| Anti-entropy sync (taciturn) | 900s | 15 minutes |

---

## References

- [Replication Routing](../design/replication-routing.md) -- three-gate model, relay behaviour, full timing analysis
- [R4-030 Group Metadata Replication](../design/R4-030-group-metadata-replication.md) -- GroupExchange protocol design
- [R2-006 Group Model](../design/R2-006-group-model.md) -- schema, roles, COW semantics, culture
- [R5 Personal Groups](../design/R5-personal-groups.md) -- personal group unification, PSK encryption
- [E2E Testing Guide](../../tests/e2e/E2E-TESTING.md) -- CI test scenarios validating these flows
