# R4-030: Group Metadata Replication -- The /etc/group Problem

*Design document. Pre-launch protocol decision.*

> "The ship's manifest is not cargo. But without the manifest, the cargo
> is just mass in a hold -- you don't know what it is, where it goes,
> or who it belongs to."

---

## Table of Contents

1. [Problem Statement](#1-problem-statement)
2. [Constraints (Inherited from R2-006)](#2-constraints)
3. [What Propagates Today](#3-what-propagates-today)
4. [The AD vs Unix Spectrum](#4-the-ad-vs-unix-spectrum)
5. [Options](#5-options)
6. [Recommendation](#6-recommendation)
7. [Wire Format](#7-wire-format)
8. [Security Analysis](#8-security-analysis)
9. [Conflict Resolution](#9-conflict-resolution)
10. [Migration and Rollout](#10-migration-and-rollout)
11. [What This Does NOT Do](#11-what-this-does-not-do)
11a. [Access Model and Key Architecture](#11a-access-model-and-key-architecture)
12. [Design Decisions (Resolved)](#12-design-decisions-resolved)
13. [Open Questions](#13-open-questions)

---

## 1. Problem Statement

Group metadata (name, culture, membership) is local-only. The `GroupExchange`
protocol carries bare group IDs. This means:

- A new node joining a group must have the group pre-provisioned in its local
  SQLite before it can apply the correct replication strategy.
- Culture changes made on one node have no effect on peer behavior.
- Membership changes (adds, removes) are invisible to the network.
- The portal can create groups, but they don't reach local proxies without
  a separate provisioning mechanism.

This is the "manifest without cargo" problem. We replicate the L2 items
(cargo) tagged with a `group_id`, but we don't replicate the group
definition (manifest) that gives those items meaning.

### 1.1 Why This Matters for Launch

Three deployment scenarios require group metadata to flow:

1. **Hosted portal** -- User registers at portal.seeddrill.ai, creates/joins
   groups. Their local proxy needs those group definitions.
2. **Self-hosted org** -- Org runs their own portal + stack. Groups must
   propagate to org members' local nodes.
3. **Agent/swarm** -- Orchestrator creates groups at deploy time. Peer nodes
   need the group record to apply correct culture.

Scenario 4 (pure local, no network) is unaffected -- groups are created
and consumed locally.

### 1.2 The Protocol Window

Post-launch protocol changes carry high risk: version negotiation, backward
compatibility, coordinated rollout across nodes we don't control. If we're
going to add group metadata to the wire protocol, the window is now.

---

## 2. Constraints

Inherited from R2-006 Section 1. All apply without modification:

1. **Entity security primacy** -- entity policy overrides group policy, always
2. **Private by default** -- nothing propagates unless explicitly shared
3. **Groups are universal** -- the only sharing primitive
4. **Encrypt before storage** -- at rest, encrypted blobs only
5. **Memory is identity** -- memories are sovereign property
6. **Trust is local** -- `tau_ij` is per-entity, never shared
7. **Novelty over volume** -- don't replicate garbage just because it exists

Additional constraint for this design:

8. **Manifest is not cargo** -- group metadata is structural, not content.
   It governs how cargo flows. It must be lightweight, verifiable, and
   resistant to manipulation. Bloated manifests are a protocol smell.

---

## 3. What Propagates Today

| Data | Replicated? | Via |
|------|------------|-----|
| L2 items (encrypted blobs) | Yes | `memory-sync`, `memory-fetch`, `memory-push` |
| Group IDs (bare strings) | Yes | `group-exchange`, `peer-share` |
| Group metadata (name, culture) | No | -- |
| Group membership (entity + role) | No | -- |
| Group security_policy | No | -- (field is always `'{}'`) |
| L1 hot context | No | By design, never crosses P2P |
| FTS/vec indexes | No | Local only |
| Entity posture (active/silent/emcon) | No | Stored locally, not on wire |

The `GroupExchange` protocol (Protocol 5) is:

```
Request:  { groups: ["seed-drill", "gcu-testing"] }
Response: { groups: ["seed-drill", "shared-xorg"] }
```

Bare IDs. Both sides learn the intersection and can target anti-entropy
sync for shared groups. But neither side learns the other's culture or
membership.

---

## 4. The AD vs Unix Spectrum

### Active Directory Model (Avoid)

AD replicates the full group object: name, members (potentially thousands),
nested group membership, ACLs, schema extensions. The replication unit is
the group object. This leads to:

- **Bloat**: a group with 500 members generates a 500-entry replication object
- **Conflict hell**: concurrent membership changes on different DCs require
  conflict resolution for multi-valued attributes
- **Security surface**: the group object itself becomes an attack vector
  (add yourself as admin on a replica, wait for convergence)
- **Brittleness**: partial replication failures leave inconsistent state

### Unix Model (Aspire)

`/etc/group` is a flat file. Each line: `groupname:x:gid:member1,member2`.
The group is a label. Membership is a flat list. No nesting. No ACLs on
the group itself. Authorization is checked at the resource, not the group.

Properties we want:
- **Small** -- group metadata is bytes, not kilobytes
- **Flat** -- no nested groups, no inheritance chains
- **Idempotent** -- applying the same group definition twice is a no-op
- **Verifiable** -- a node can validate a group definition independently

### Where Cordelia Should Sit

Closer to Unix. The group is a policy label (culture) attached to a
membership list. The heavy objects (L2 items) flow separately. The
manifest is small, the cargo is large.

But we need one thing Unix doesn't have: **distributed authority**. In Unix,
root edits `/etc/group`. In Cordelia, group mutations can originate from
any node where an owner/admin is present. This is the hard problem.

---

## 5. Options

### Option A: Extend GroupExchange with Metadata

Extend the existing `GroupExchange` protocol to include group metadata
alongside IDs.

**What changes:**
- `GroupExchange` request/response carries `Vec<GroupDescriptor>` instead
  of `Vec<GroupId>`
- `GroupDescriptor` = `{ id, name, culture, updated_at, member_count, checksum }`
- Peers merge received descriptors into local `groups` table (upsert)
- Membership is NOT included (see Option B for why)

**Pros:**
- Minimal protocol addition (extends existing message, same protocol ID)
- Group metadata is tiny (~200 bytes per group)
- Runs on existing schedule (every 6 governor ticks = 60s)
- Culture propagation means all peers converge on replication strategy

**Cons:**
- No membership propagation (just metadata)
- LWW on `updated_at` for conflict resolution (simple but lossy)
- Every peer gets every group descriptor (no scoping)

### Option B: Separate Group Metadata Sync Protocol

New Protocol 6: `/cordelia/group-meta/1`. Dedicated request/response for
group metadata including membership.

**What changes:**
- `GroupMetaRequest { group_ids: Vec<String> }` -- "give me full metadata for these groups"
- `GroupMetaResponse { groups: Vec<GroupFull> }`
- `GroupFull` = `{ id, name, culture, security_policy, updated_at, members: Vec<MemberRecord> }`
- `MemberRecord` = `{ entity_id, role, posture, joined_at }`

**Pros:**
- Full fidelity -- culture, security_policy, and membership all replicate
- Targeted -- only request groups you care about
- Separates concerns from item replication

**Cons:**
- New protocol = new version negotiation complexity
- Membership lists scale with group size (the AD problem)
- A group with 100 members = ~5KB per sync. 50 groups = 250KB every cycle.
- Membership conflicts are hard (two admins add different members concurrently)
- Security: a compromised peer could inject fake members

### Option C: Portal Provisioning Only (No Protocol Change)

Don't replicate group metadata over P2P. The portal provisions groups to
devices at enrollment and on change via HTTP push.

**What changes:**
- Portal `POST /api/groups` to device proxy on enrollment
- Portal watches for group mutations and pushes changes to enrolled devices
- Local-only groups (created via MCP tools) stay local

**Pros:**
- Zero protocol risk
- Portal is the authority -- no distributed conflict resolution
- Works with what we just built (proxy HTTP endpoints)
- Simple mental model: portal manages groups, network carries items

**Cons:**
- Requires portal for any cross-device group. Pure P2P groups don't work.
- Devices must be reachable from portal (or poll for changes)
- Single point of authority (contradicts sovereignty principle)
- Doesn't work for scenario 2 (self-hosted org without our portal)

### Option D: Hybrid -- Lightweight Metadata in GroupExchange + Enrollment Bootstrap

Extend `GroupExchange` with a lightweight `GroupDescriptor` (no membership).
Use enrollment to bootstrap full group state. Membership propagates
implicitly via item author provenance.

**What changes:**
- `GroupExchange` carries `Vec<GroupDescriptor>` (id, name, culture, updated_at, checksum)
- At enrollment, portal pushes full group state (metadata + membership) via HTTP
- Membership is NOT on the wire protocol -- nodes infer active members from
  L2 item `author_id` fields and `peer-share` group lists
- Explicit membership is local-only (managed by portal, MCP tools, or API)

**Pros:**
- Group metadata (culture) propagates -- all peers agree on replication strategy
- No membership bloat on wire -- the manifest stays small
- Enrollment handles the bootstrap case (new device gets full state)
- Pure P2P groups work (create locally, culture propagates via exchange)
- Membership divergence is tolerable (each node has its own view, like DNS)
- Keeps the Unix character: group = label + culture, membership = local policy

**Cons:**
- Membership is eventually consistent (nodes may disagree about who's in a group)
- Removed members may still be seen as members by peers until next enrollment push
- No wire-level membership verification

---

## 6. Recommendation

**Option D: Hybrid.**

Rationale:

### 6.1 The manifest/cargo separation is correct

Group metadata (name, culture) is the manifest. It tells nodes how to
handle items. It's small (~200 bytes per group), changes rarely, and
convergence is important (all nodes should apply the same replication
strategy). This belongs on the wire.

Membership is closer to cargo -- it's operational state that varies per
node's perspective. A relay doesn't need to know every member of a group
to forward items. A keeper doesn't need the member list to store items.
Only the entity's own node needs to know "am I in this group?" -- and it
already knows, because the entity told it.

### 6.2 Membership on the wire is the AD trap

Putting `Vec<MemberRecord>` in the protocol means:
- Message size scales with group size
- Conflict resolution for concurrent adds/removes
- Authority problem: who is allowed to assert "entity X is a member"?
- Privacy: broadcasting membership lists reveals social graph

None of these problems exist if membership stays local.

### 6.3 Implicit membership is sufficient

A node can infer "who's active in this group" from:
- **`peer-share`**: peers advertise their group IDs (already on wire)
- **`author_id`**: L2 items arriving for a group reveal who's contributing
- **Local state**: the entity's own membership is authoritative locally

This is how IRC worked before services: you know who's in a channel by
who's talking. The channel topic (culture) propagates, the nick list
(membership) is derived from presence.

### 6.4 Enrollment handles the bootstrap

The cold-start problem ("new device, no groups") is solved by portal
provisioning at enrollment. This is Option C as a complement, not a
replacement. The portal pushes full state (metadata + membership) via
HTTP. After that, the P2P layer keeps culture in sync.

### 6.5 Protocol change is minimal

We're extending an existing message type, not adding a new protocol.
The `GroupExchange` already fires every 60s. Adding ~200 bytes of metadata
per group is negligible.

---

## 7. Wire Format

### 7.1 Current GroupExchange (ERA_0)

```rust
struct GroupExchange {
    groups: Vec<GroupId>,  // Vec<String>
}
```

### 7.2 Proposed GroupExchange (ERA_0, v1.1)

```rust
struct GroupExchange {
    groups: Vec<GroupId>,                       // backward compat
    descriptors: Option<Vec<GroupDescriptor>>,  // new, optional
}

struct GroupDescriptor {
    id: String,                    // opaque UUID, portal-generated
    culture: String,               // raw JSON, max 4KB
    updated_at: String,            // ISO 8601
    checksum: String,              // SHA-256 of canonical(id + culture)
    owner_id: Option<String>,      // entity ID of group owner
    owner_pubkey: Option<String>,  // hex-encoded Ed25519 public key
    signature: Option<String>,     // hex-encoded Ed25519 signature
}
```

**No name on wire.** Group names are display metadata, not protocol data.
The portal distributes `id -> name` mappings out-of-band during enrollment.
Group IDs should be UUIDs (opaque). This prevents metadata leakage to peers --
a connected peer sees opaque IDs and replication policy, nothing human-readable.

The `descriptors` field is `Option` for backward compatibility. A peer
running old code ignores unknown fields (serde `#[serde(default)]`).
A peer running new code sends both `groups` (for old peers) and
`descriptors` (for new peers).

### 7.6 Owner Signing

The group owner signs the descriptor with their Ed25519 private key.
The signing payload is `canonical(id + "\n" + culture + "\n" + updated_at)`.
The signature, owner ID, and public key are included in the descriptor.

**Trust anchor:** The portal (secret keeper) is the trust anchor. During
group enrollment, the portal distributes the owner's public key to members.
We don't care who runs the portal -- all that matters is that the entities
using it trust it. Same model as a CA.

**Lazy signing:** The owning node signs its groups on first GroupExchange
if they're unsigned. The signature is persisted in storage for subsequent
exchanges. Non-owner nodes forward the stored signature as-is.

**Verification rules:**
1. If a descriptor has a signature, verify it against the owner's public key
2. If a descriptor is unsigned but the local copy is signed, reject it
   (prevents downgrade to unsigned)
3. If both local and incoming are signed, reject if owner_pubkey differs
   (prevents owner hijack)
4. If neither is signed, accept (graceful upgrade path)

### 7.3 Size Budget

| Field | Typical size |
|-------|-------------|
| `id` | 36 bytes (UUID) |
| `culture` | 50-200 bytes |
| `updated_at` | 24 bytes |
| `checksum` | 64 bytes |
| `owner_id` | 20-40 bytes |
| `owner_pubkey` | 64 bytes (hex-encoded 32-byte Ed25519) |
| `signature` | 128 bytes (hex-encoded 64-byte Ed25519) |
| **Total per group** | **~400-560 bytes** |

10 groups = ~5KB. 100 groups = ~50KB. At 60s intervals this is trivial.

Compare: a single L2 item blob is typically 1-10KB. Group metadata is
noise-level bandwidth.

### 7.4 No Name on Wire

Group names are display metadata with no protocol function. Including them
would leak human-readable information to any connected peer (e.g., "Project X
Finance Team"). Group IDs should be UUIDs -- opaque to the protocol.

The portal is the naming authority. It distributes `id -> name` mappings to
authorized clients during enrollment or via periodic polling. The node stores
the name locally for display but never transmits it over P2P.

### 7.5 No security_policy on Wire

`security_policy` is excluded from the wire format. It is currently unused
(`'{}'` everywhere) and its semantics are undefined. When we design the
distributed policy engine (R4+), we can add it then with proper schema.
Including it now as an opaque JSON blob is asking for trouble.

### 7.5 No Membership on Wire

Explicitly not included. See Section 6.2.

---

## 8. Security Analysis

### 8.1 Threat: Rogue Group Injection

A malicious peer sends a `GroupDescriptor` for a group that doesn't exist,
or modifies the culture of an existing group (e.g., changes `taciturn` to
`chatty` to trigger eager push and exfiltrate data).

**Mitigation:**
- **Owner signing (primary)**: the group owner signs descriptors with their
  Ed25519 private key. Only the owner can produce valid signatures. Peers
  verify before accepting. A rogue peer cannot forge a descriptor because
  they don't have the owner's private key.
- `checksum` field: `SHA-256(canonical(id + name + culture))`. Quick
  integrity check. The signature covers integrity too, but the checksum
  is cheap for comparison without pulling out the crypto.
- **Downgrade protection**: once a group has a signed descriptor, unsigned
  updates are rejected. Prevents an attacker from stripping the signature.
- **Owner hijack protection**: if the local copy has a known owner pubkey,
  incoming descriptors with a different pubkey are rejected.
- **Culture change protection (soft)**: culture changes from the network
  are accepted if signed by the owner (log a warning if eagerness increases).
  We expect participants to be adults. The response to bad behaviour is key
  recycling and access revocation, not protocol-level prevention. This
  follows the TCP/IP principle: the network is dumb, endpoints are smart.
- **Entity sovereignty**: if a node's owner has explicitly set a group's
  culture locally, network propagation should not override it. Local
  config is authoritative.

### 8.2 Threat: Group Metadata Poisoning

A peer floods the network with thousands of fake `GroupDescriptor` entries,
consuming storage and bandwidth.

**Mitigation:**
- **Only accept descriptors for groups we hold items for**, or groups
  advertised by peers we have positive trust scores with (`tau_ij >= theta`).
- **Rate limit**: max 100 descriptors per `GroupExchange`. Excess is dropped.
- **TTL**: group records not referenced by any L2 item for 30 days can be
  garbage collected.

### 8.3 Threat: Membership Inference

Since membership is not on the wire, an adversary cannot learn the full
member list of a group from protocol traffic. They can only infer active
participants from:
- `peer-share` group ID lists (reveals "this peer is in group X")
- `author_id` on replicated items (reveals "this entity contributed to group X")

This is inherent to any replication system -- if you can see traffic, you
can infer participants. The mitigation is EMCON posture (suppress all
outbound) and relay topology (org-internal traffic doesn't cross to backbone).

### 8.4 Threat: Replay of Stale Group Metadata

A peer replays an old `GroupDescriptor` with a stale `updated_at` to
revert a culture change.

**Mitigation:**
- LWW with `updated_at`: only accept descriptors with `updated_at` newer
  than local record. Stale descriptors are ignored.
- If a peer persistently replays stale metadata, trust score degrades
  and the peer is excluded from the group's sync topology.

---

## 9. Conflict Resolution

### 9.1 Group Metadata Conflicts

Same as L2 items: **last-writer-wins by `updated_at`**.

This is acceptable because:
- Group metadata changes are rare (culture is set at creation, rarely modified)
- There is typically one authority (the group owner/admin) making changes
- The consequence of a wrong merge is a temporary culture mismatch, not data loss

### 9.2 When LWW Is Not Enough

If two admins change a group's culture simultaneously on different nodes:
- Both changes propagate via `GroupExchange`
- Each node applies the one with the later `updated_at`
- Within 60s (one exchange cycle), all nodes converge

The losing write is silently dropped. This is the same model as DNS
propagation -- the last SOA serial wins. For group metadata (which changes
rarely), this is fine.

### 9.3 Membership Conflicts (Not Applicable)

Since membership is not on the wire, there are no distributed membership
conflicts. Each node's local membership is authoritative for that node.
The portal handles the "source of truth" role for managed deployments.

---

## 10. Migration and Rollout

### 10.1 Wire Compatibility

The `descriptors` field is `Option<Vec<GroupDescriptor>>` with
`#[serde(default)]`. Old nodes ignore it. New nodes send both `groups`
and `descriptors`.

**No protocol version bump required.** This is an additive extension
to an existing message type. ERA_0 protocol constants are unchanged.

### 10.2 Rollout Sequence

1. **Proxy**: Add `GroupDescriptor` handling to `NodeStorageProvider`
   (accept incoming descriptors, upsert to local `groups` table)
2. **Node**: Extend `GroupExchange` in `cordelia-protocol` messages
3. **Node**: Extend `swarm_task.rs` to populate descriptors from local
   `groups` table on exchange, and merge received descriptors
4. **Portal**: Add enrollment group push (Option C bootstrap)
5. **Tests**: Extend replication integration tests for descriptor propagation

### 10.3 Enrollment Bootstrap (Option C)

At enrollment time, the portal pushes the enrolling entity's groups:

```
POST /api/groups          { id, name, culture }     -- for each group
POST /api/groups/:id/members  { entity_id, role }   -- for each membership
```

This uses the HTTP endpoints we built today. The portal knows which groups
the entity belongs to (it's the management layer). The local proxy receives
full state at enrollment, then the P2P layer keeps culture in sync.

---

## 11. What This Does NOT Do

- **Does not replicate membership.** Membership is local. The portal
  manages it for hosted deployments. MCP tools manage it locally.
- **Does not add a new protocol.** Extends existing `GroupExchange`.
- **Does not replicate security_policy.** Dead field, no defined semantics.
- **Does not solve group creation authority.** Any node can create a group
  locally. Whether that group propagates depends on whether peers hold
  items for it. There is no "permission to create a group" on the network.
- **Does not implement EMCON.** Posture suppression is a separate concern
  (R4-031 or later).
- **Does not implement departure policy (yet).** The model is documented
  in Section 11a.4 -- key rotation on member removal. Implementation
  requires envelope encryption (R2-009 KeyVault, future sprint).

---

## 11a. Access Model and Key Architecture

### 11a.1 Two Keys, Two Jobs

Each group involves two cryptographic keys serving distinct purposes:

| Key | Type | Purpose | Held by |
|-----|------|---------|---------|
| **Group symmetric key** | AES-256-GCM (ephemeral, rotatable) | Encrypts/decrypts L2 items (memory data). Confidentiality gate. | All group members |
| **Owner Ed25519 keypair** | Asymmetric (long-lived) | Signs GroupDescriptor (culture/policy). Authenticity gate. | Group owner only |

The symmetric key controls **who can read and write group data**.
The owner keypair controls **who can define the group's policy**.

### 11a.2 Write Access

**All group members can write.** Any entity holding the group symmetric
key can encrypt and publish L2 items to the group. The `author_id` field
on `FetchedItem` records who wrote each item, but write permission is
implicit in key possession -- if you can encrypt, you're authorized.

The owner controls the group definition (culture, replication policy)
via the signed GroupDescriptor. But the group is a collaborative space,
not a broadcast channel. Think shared directory: the owner sets
permissions, everyone with access can create files.

### 11a.3 Enrollment Flow

1. Entity requests to join group via the **portal** (the trust anchor)
2. Portal verifies authorization and distributes:
   - Group UUID (opaque identifier)
   - Current group symmetric key + `key_version`
   - Owner's Ed25519 public key (for descriptor verification)
   - Group display name (out-of-band, never on P2P wire)
3. Entity's node stores all locally
4. Node can now:
   - **Decrypt** existing L2 items using the symmetric key
   - **Encrypt** new L2 items for the group
   - **Verify** GroupDescriptor signatures using the owner's pubkey
   - **Participate** in GroupExchange with peers (culture propagation)

### 11a.4 Key Revocation on Member Removal

When a member is removed from a group:

1. Portal revokes the member's access
2. Group symmetric key is **rotated** (new key version)
3. Portal distributes the new key to remaining members
4. The removed member:
   - **Cannot decrypt** new items (encrypted with the new key)
   - **Cannot encrypt** new items (doesn't have the new key)
   - **Can still read** items encrypted with the old key they possessed
     (this is inherent -- you can't un-know a key)
5. For forward secrecy of old items, re-encryption with the new key is
   possible but expensive (R2-009 envelope encryption design)

This is the same model as revoking a user's access to a shared drive --
they can't see new files, but they may have copies of old ones. The
cryptographic enforcement is stronger than ACL-based revocation because
the removed member literally cannot produce valid ciphertexts for the group.

### 11a.5 Trust Verification

Entities should periodically poll the portal to verify:
- The owner's public key hasn't been rotated (key compromise response)
- Their group symmetric key is current (haven't missed a rotation)
- Their membership hasn't been revoked

The P2P layer provides the fallback: if the portal is unreachable,
GroupExchange keeps culture synchronized. But key distribution is
exclusively the portal's responsibility -- keys never traverse the
P2P wire.

---

## 12. Design Decisions (Resolved)

### 12.1 Culture Downgrade Protection -- Soft

**Decision:** Soft enforcement. Accept all culture changes from the network,
log a warning if eagerness increases. No protocol-level prevention.

**Rationale:** We expect participants to be adults. The TCP/IP model: the
network is dumb, endpoints are smart. Bad behaviour is handled by key
recycling and access revocation, not by making the protocol more complex.
The trust model (`tau_ij`) handles the feedback loop -- a peer that abuses
culture changes sees its trust score drop and eventually loses sync
privileges.

Hard enforcement creates coordination problems: how does a legitimate
culture upgrade propagate if the network blocks eagerness increases?
You'd need out-of-band coordination for every culture change. Not worth it.

### 12.2 Group Deletion = Key Revocation

**Decision:** Group deletion is key revocation. When a group is deleted,
the group key is recycled and access is revoked for all members. No
tombstone protocol, no wire-level deletion mechanism.

**Rationale:** A group with revoked keys is effectively dead -- no new
items can be encrypted for it, existing items become unreadable as keys
rotate. The group record can linger in local SQLite until GC'd (no items
referencing it for 30 days). This is simpler than tombstone replication
and more secure -- deletion is a cryptographic fact, not a metadata flag
that a malicious peer could suppress.

For portal-managed groups, the portal also sends explicit `DELETE` to
enrolled devices as a courtesy (belt and braces).

### 12.3 Maximum Group Count per Node

**Decision:** 1000 groups per node, soft limit (warning at 500,
hard reject at 1000). Revisit if legitimate use cases exceed this.

### 12.4 Culture Schema Versioning -- Not Needed

**Decision:** No `culture_version` field. JSON is self-describing.

**Rationale:** Unknown fields in the `culture` JSON are ignored by
`#[serde(default)]` / serde's deny-unknown-fields-off. A node that
receives a culture with fields it doesn't understand applies defaults
for those fields (which is always `moderate` -- the safe fallback).
This is how every REST API handles forward compatibility. Adding a
version field creates a coordination problem (who bumps it, what do
old nodes do?) without solving a real problem. If it works for TCP/IP
options, HTTP headers, and DNS record types, it works for culture JSON.

## 13. Open Questions

### 13.1 Key Recycling Mechanics

Group deletion triggers key revocation (12.2), but the envelope encryption
design (R2-009) is still a stub. Key recycling requires:
- Per-group encryption keys (not yet implemented)
- Key rotation mechanism
- Re-encryption of items for remaining members on member departure

This is an R4+ dependency. For launch, group deletion is a local storage
operation (delete the group record and its members). Key revocation is
aspirational until envelope encryption lands.

### 13.2 Portal Push Reliability

The enrollment bootstrap (Section 10.3) assumes the portal can reach the
device's proxy via HTTP. For devices behind NAT or firewalls, this may
fail. Options:
- Device polls portal for group updates (pull model)
- Portal queues pushes and retries on next device check-in
- Rely on P2P culture propagation as fallback (the whole point of Option D)

For launch, the P2P fallback is sufficient. Portal push is best-effort.

---

*Last updated: 2026-02-03*
