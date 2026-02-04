# Replication Routing -- How Items Move Through the Network

**Status**: Living document
**Author**: Russell Wing, Claude (Opus 4.5)
**Date**: 2026-02-04
**Relates to**: R2-006 (Group Model), R3 (Decentralisation), R5 (Personal Groups)

---

## 1. Overview

Replication routing is the mechanism by which items written on one node reach
every node that should have them. This document specifies the exact rules,
the code paths that implement them, and the deployment patterns that result.

The routing model has three layers:

1. **Push target selection** -- which peers does the writer send to?
2. **Relay acceptance** -- does an intermediate node accept and forward?
3. **Destination acceptance** -- does the final recipient store the item?

An item must pass all three gates to reach its destination.

---

## 2. Node Roles and Relay Postures

### 2.1 Node Roles

| Role | Description | `is_relay` on peers | Config |
|------|-------------|-------------------|--------|
| **Relay** | Edge/backbone forwarding node | Yes (via bootnode seeding) | `role = "relay"` |
| **Personal** | User's agent node | No | `role = "personal"` |
| **Keeper** | Stores sovereign memory | No | `role = "keeper"` |

### 2.2 Relay Postures

Relay nodes have a posture that governs which groups they accept:

| Posture | Acceptance rule | Use case |
|---------|----------------|----------|
| **Transparent** | Accept all groups | Backbone/boot nodes |
| **Dynamic** | Accept groups learned from peers via group exchange | Org edge nodes |
| **Explicit** | Accept only pre-configured group allowlist | Restricted relays |

---

## 3. The Three Gates

### 3.1 Gate 1: Push Target Selection (Writer Side)

**Code**: `peer_pool.rs` :: `active_peers_for_group_or_relays()`

When a node writes an item, the replication task selects peers to push to:

```
peer is target IF:
    peer.state.is_active()
    AND (peer.is_relay OR peer.group_intersection.contains(group_id))
```

**Key properties**:
- Relay peers are ALWAYS push targets, regardless of group membership
- Non-relay peers are targets only if they share the group
- `group_intersection` is the intersection of the peer's groups with our
  effective group set (see Section 4)
- This gate only applies for **chatty** culture (eager push). Taciturn
  culture returns `OutboundAction::None` and relies on anti-entropy (Section 5)

### 3.2 Gate 2: Relay Acceptance (Intermediate Node)

**Code**: `engine.rs` :: `on_receive()`, `swarm_task.rs` :: `handle_push_request()`

When a relay receives a pushed item, acceptance depends on posture:

```
IF transparent:  accept all (unless blocked)
IF dynamic:      accept IF relay_accepted_groups.contains(group_id)
IF explicit:     accept IF relay_allowed_groups.contains(group_id)
```

Dynamic relays learn groups from non-relay peers during group exchange
(`governor_task.rs:247-253`). The learned groups feed directly into
`relay_accepted_groups` (same Arc).

**Relay re-push**: If a relay stores an item (`ack.stored > 0`), it
re-pushes to ALL active peers except the sender (`swarm_task.rs:639-660`).
Loop prevention: duplicate items yield `stored == 0`, stopping the chain.

### 3.3 Gate 3: Destination Acceptance (Final Recipient)

**Code**: `engine.rs` :: `on_receive()`

When a non-relay node receives a pushed item:

```
accept IF our_groups.contains(item.group_id)
```

The item's `group_id` must be in the node's `shared_groups` (groups created
locally via the API). This is a strict membership check -- the group must be
explicitly provisioned on the destination node.

---

## 4. Group Intersection

**Code**: `peer_pool.rs` :: `compute_intersection()`

`group_intersection` determines which groups a peer and this node have in
common. It governs push target selection (Gate 1) and anti-entropy sync
target selection (Section 5).

### 4.1 Computation

```
effective_groups = shared_groups
    UNION relay_learned_groups (if this node is a dynamic relay)

group_intersection(peer) = peer.groups INTERSECT effective_groups
```

For non-relay nodes, `effective_groups = shared_groups` only.

For dynamic relay nodes, `effective_groups` also includes groups learned
from peers via group exchange. This allows relays to find anti-entropy sync
targets for groups they relay but don't formally belong to.

### 4.2 When It Updates

- **Peer insertion**: computed on first connect (`peer_pool.rs` :: `insert()`)
- **Group exchange**: recomputed when peer sends new group list
  (`peer_pool.rs` :: `update_peer_groups()`)
- Group exchange fires every `GROUP_EXCHANGE_TICKS` governor ticks
  (default: 6 ticks x 10s = 60s), plus immediately on peer connect

### 4.3 What It Does NOT Include

- Groups created on other nodes but not exchanged yet (timing dependency)
- Groups blocked via relay deny-list

---

## 5. Group Exchange Advertising

**Code**: `governor_task.rs` :: periodic/initial exchange, `swarm_task.rs` :: exchange response

Group exchange is the mechanism by which peers discover each other's groups.
This drives `group_intersection` computation and, for dynamic relays, the
`relay_accepted_groups` set.

### 5.1 What Nodes Advertise

| Node type | Advertised groups |
|-----------|------------------|
| **Personal/Keeper** | `shared_groups` only |
| **Relay (dynamic)** | `shared_groups` UNION `relay_learned_groups` |
| **Relay (transparent)** | `shared_groups` (acceptance is posture-based, not group-based) |

Dynamic relays advertise their learned groups so that peers (keepers, other
relays) can compute correct `group_intersection`. Without this, a keeper
with group `personal-alice` would not know that an edge relay also handles
that group, and anti-entropy sync would never target the relay.

### 5.2 Why Relays Must Advertise Learned Groups

Consider the personal group replication path:

```
agent (writes)  →  edge (relay, learned group)  →  keeper (has group)
```

1. Agent creates personal group, writes item
2. Edge learns the group via group exchange from the agent
3. Edge accepts and stores the item (Gate 2: dynamic acceptance)
4. Keeper needs to pull the item from the edge via anti-entropy sync

For step 4, the keeper must know the edge handles the group. This requires
the edge to advertise the group back to the keeper during group exchange.
The keeper then computes `group_intersection` with the edge, finds the
personal group, and can target the edge for sync.

### 5.3 Exchange Points

Group exchange occurs at three points:

1. **Periodic tick**: Every `GROUP_EXCHANGE_TICKS` governor ticks (60s default).
   `governor_task.rs` sends `GroupExchange` to all hot peers.
2. **Initial connect**: When a peer first reaches Hot state.
   `governor_task.rs` sends `GroupExchange` immediately.
3. **Response**: When receiving a `GroupExchange`, the node responds with its
   own group list. `swarm_task.rs` constructs the response.

All three points merge `relay_learned_groups` into the advertised set for
relay nodes.

### 5.4 Convergence Timeline

```
t=0    Agent creates personal group, group exchange not yet fired
t=0-60 Exchange tick: agent advertises group → edge learns it
       (edge adds to relay_accepted_groups / relay_learned_groups)
t=60   Exchange tick: edge advertises learned group → keeper sees it
       (keeper recomputes group_intersection with edge)
t=120  Keeper's anti-entropy sync targets edge for personal group
       → pulls item from edge → stores locally
```

Two exchange cycles are needed: one for the relay to learn the group, one
for peers to discover the relay handles it. Worst case: ~120s before the
keeper can pull from the edge.

---

## 6. Anti-Entropy Sync

**Code**: `replication_task.rs` :: `sync_base_tick` handler, `run_anti_entropy()`

Anti-entropy is the pull-based backup replication mechanism. It fires
periodically and reconciles items between peers.

### 6.1 Sync Intervals by Culture

| Culture | Strategy | Push on write? | Sync interval |
|---------|----------|---------------|---------------|
| **Chatty** | EagerPush | Yes | 60s (safety net) |
| **Moderate** | NotifyAndFetch | Header only | `sync_interval_moderate_secs` (default 300s) |
| **Taciturn** | Passive | No | `sync_interval_taciturn_secs` (default 900s) |

**Important**: The `sync_base_tick` fires at `EAGER_PUSH_INTERVAL_SECS` (60s).
Per-group deadlines are tracked separately, but the effective minimum sync
interval is 60s regardless of the culture-specific setting.

### 6.2 Sync Target Selection

For non-relay nodes:
```
random_hot_peer_for_group(group_id)
    → peers where group_intersection.contains(group_id)
    → fallback: warm peers with group_intersection match
```

For relay nodes (priority order):
```
random_hot_peer_for_group_or_relays(group_id)
    1. Hot peers with group_intersection match  (most likely to have items)
    2. Active (warm) peers with group_intersection match
    3. Hot relay peers (may have relayed items without membership)
    4. Active relay peers
```

**Why priority matters**: At scale (300+ nodes), a relay may have dozens of
relay peers in its hot set. If relay-only matches are mixed with group
intersection matches, the random selection frequently picks a relay that
doesn't have the group's items. Prioritising `group_intersection` matches
ensures sync targets are peers that actually handle the group.

### 6.3 Sync Flow

1. Initiator sends `SyncRequest { group_id, since, limit }` to a peer
2. Peer responds with `SyncResponse` containing item headers
3. Initiator computes diff (what peer has that we don't)
4. Initiator sends `FetchRequest` for missing items
5. Peer responds with `FetchResponse` containing full items
6. Initiator processes via `on_receive()` (Gate 2 or Gate 3 applies)

Anti-entropy is a **pull** mechanism: the initiator pulls FROM the peer.
For items to flow from A to B, B must initiate sync with A (or an
intermediate relay must pull from A and then B pulls from the relay).

### 6.4 Relay Sync Group Set

For relay nodes, the sync loop iterates an extended group set:

```
sync_groups = shared_groups
    UNION relay_learned_groups (from group exchange)
    UNION stored_group_ids (from local SQLite -- items already stored)
```

This ensures relays sync groups they're relaying, not just groups they
formally belong to.

---

## 7. Deployment Patterns

### 7.1 Pattern A: Shared Group (Cross-Org)

**Example**: `shared-xorg` group visible across all orgs.

```
agent-alpha-1  →  edge-alpha  →  boot  →  edge-bravo  →  keeper-bravo-1
    (write)       (relay)      (relay)     (relay)        (store)
```

**Provisioning**:
- Group created on all participating nodes (agent, keepers)
- Edge nodes learn via group exchange (dynamic relay)
- Boot nodes relay transparently (no provisioning needed)

**Culture**: Chatty (eager push, immediate fan-out)

### 7.2 Pattern B: Org-Internal Group

**Example**: `alpha-internal` group within one org.

```
agent-alpha-1  →  edge-alpha  →  keeper-alpha-1
    (write)       (relay)        (store)
```

**Provisioning**:
- Group created on agent and keeper nodes
- Edge learns via group exchange
- Item does NOT reach boot nodes (edge's dynamic posture only accepts
  learned groups; boot's transparent posture would accept, but edge
  doesn't push to boot for internal groups unless boot is in
  group_intersection)

**Culture**: Moderate or chatty

### 7.3 Pattern C: Personal Group

**Example**: `personal-agent-alpha-1` -- entity's private memory.

```
agent-alpha-1  →  edge-alpha  →  keeper-alpha-1
    (write)       (relay)        (store)
                              →  keeper-alpha-2
                                  (store)
```

**Provisioning**:
- Group created on agent and keeper nodes during enrollment
- Edge learns via group exchange (no explicit provisioning)
- Keepers MUST have the group created (Gate 3 requires `our_groups` membership)

**Culture**: Chatty (personal memory should replicate immediately)

**Isolation**: Items reach only the org's edge and keeper nodes. They do NOT
cross to other orgs because other orgs' edges don't learn the personal group
(no peer in the other org has it).

### 7.4 Provisioning Summary

| Node type | Shared group | Org-internal | Personal group |
|-----------|-------------|-------------|---------------|
| **Agent** | Create | Create | Create (enrollment) |
| **Edge (dynamic)** | Learns automatically | Learns automatically | Learns automatically |
| **Edge (transparent)** | Accepts all | Accepts all | Accepts all |
| **Keeper** | Create | Create | Create (enrollment) |
| **Boot (transparent)** | Accepts all | N/A (no path) | N/A (no path) |

---

## 8. Timing Dependencies

Group exchange is the critical synchronisation point. Items cannot route
through a relay until the relay has learned the group.

### 8.1 Timeline for New Chatty Group Replication

```
t=0    Group created on agent + keepers (via API or enrollment)
t=0-60 Exchange tick 1: agent sends groups to edge, edge learns group
t=60+  Edge's relay_accepted_groups includes the group
t=60+  Agent writes item → pushes to edge (is_relay) → edge accepts
       → edge re-pushes to keeper → keeper accepts (our_groups)
```

### 8.2 Timeline for Taciturn Group (No Edge Provisioning)

Taciturn groups rely on anti-entropy sync. Two exchange cycles are needed:
one for the relay to learn the group, one for peers to discover the relay
handles it.

```
t=0     Group created on agent + keepers only
t=0-60  Exchange tick 1: agent advertises group → edge learns it
        (edge adds to relay_accepted_groups + relay_learned_groups)
t=60    Exchange tick 2: edge advertises learned group → keeper sees it
        (keeper recomputes group_intersection with edge -- now includes group)
t=60+   Agent writes item (taciturn: no push, item stays on agent)
t=120   Edge's anti-entropy sync: pulls item from agent (sync_base_tick)
t=180   Keeper's anti-entropy sync: targets edge (group_intersection match),
        pulls item from edge → stores locally (Gate 3: our_groups)
```

### 8.3 Worst-Case Latency

| Culture | Hops | Worst case |
|---------|------|-----------|
| Chatty, 1 hop (agent → keeper via edge) | 2 | 60s (group exchange) + push |
| Chatty, 2 hops (agent → edge → boot → edge → keeper) | 4 | 60s + push chain |
| Taciturn, 1 hop (no edge provision) | 2 | 120s (2x exchange) + 60s (sync) + 60s (sync) = 240s |
| Taciturn, 2 hops (via relay chain) | 3 | 120s + 60s + 60s + 60s = 300s |

For **immediate** replication after group creation, trigger a reconnect
or explicit group exchange rather than waiting for the periodic tick.

---

## 9. Invariants

1. **No item without a group**: Every replicated item has a `group_id`
2. **Keeper membership is explicit**: Keepers reject items for groups not
   in `shared_groups` (Gate 3)
3. **Relay learning is automatic**: Dynamic relays discover groups from
   peers -- no manual provisioning required
4. **Transparent relays see everything**: Boot/backbone nodes accept all
   items but cannot decrypt group-encrypted content
5. **Isolation is topological**: Personal group items stay within the org
   because no cross-org peer has the group
6. **Loop prevention is dedup-based**: `stored == 0` stops relay re-push
   chains; checksum comparison prevents re-storage
7. **Relays advertise learned groups**: Dynamic relays include
   `relay_learned_groups` in group exchange so peers can compute correct
   `group_intersection` for anti-entropy targeting

---

*Last updated: 2026-02-04*
