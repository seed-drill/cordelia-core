# R5: Personal Groups -- Unified Storage and Replication Model

**Status**: Draft (updated 2026-02-26)
**Author**: Russell Wing, Claude (Opus 4.5/4.6)
**Date**: 2026-02-04 (revised 2026-02-26)
**Depends on**: R2-006 (Group Model, DONE), R4-030 (Group Metadata Replication, DONE), R3-028 (Dynamic shared_groups, DONE)
**Supersedes**: The private/group storage split introduced in R2

---

## 1. Problem Statement

Cordelia has two classes of data with fundamentally different replication behaviour:

| Property | Private items | Group items |
|----------|--------------|-------------|
| `group_id` | NULL | set |
| `key_version` | 1 (personal key) | 2 (group PSK) |
| Replicates? | No | Yes |
| Accessible from portal? | No | Yes |

Private items -- including L1 hot context and personal L2 memories -- never
replicate. They exist only on the device where they were created. This means:

- The portal cannot display identity cards (L1 hot context is local-only)
- A lost device means lost private memories (no DR)
- Secret keepers cannot fulfil their role for private data
- Two codepaths exist for storage, encryption, and replication

The root cause is that the system treats "private" and "grouped" as distinct
categories. This design eliminates that distinction.

---

## 2. Design Principle

**Every item belongs to a group. There are no ungrouped items.**

"Private" is not the absence of a group -- it is membership in a **personal
group** containing only the entity and their designated keeper(s). Privacy is
achieved through encryption, not through exclusion from the replication protocol.

This mirrors the Unix model: a file always has an owner and group. A "private"
file is one where the group has no other members with read access. The access
control is in the permissions, not in whether the file exists in the filesystem.

---

## 3. Personal Group

### 3.1 Definition

Every entity has exactly one personal group, auto-created at enrolment:

```
Personal group: <opaque-uuid-v4>
  Owner: {entity_id}
  Members: {entity_id} (owner), keeper-1 (member), keeper-2 (member), ...
  Culture: { "broadcast_eagerness": "chatty" }
  Encryption: Group PSK (AES-256-GCM)
```

The personal group is a regular group. It uses the same schema, the same
replication protocol (GroupExchange, R4-030), and the same storage layer.
The only special property is that items within it are encrypted with a PSK
that keepers do not possess -- they store opaque ciphertext.

### 3.2 Group ID

Personal group IDs are **opaque UUIDs** (v4), generated at enrolment time.

```
Personal group ID: e.g. "b7f3a1c2-9d4e-4f8b-a6c1-3e5d7f9b2a4c"
```

This is a deliberate privacy decision. Using a deterministic scheme like
`personal-{entity_id}` would leak entity identity to every relay node that
handles the group's traffic, since `group_id` is transmitted in plaintext
metadata for routing purposes (see `metadata-privacy.md` Section 2.5).

An opaque UUID means:

- Relay nodes see an anonymous group ID with no correlation to any entity
- The only nodes that know the mapping from UUID to entity are the entity's
  own devices and the portal (via vault metadata)
- Network observers cannot determine which groups belong to which entities
- No component can compute the personal group ID from an entity ID without
  a lookup -- this is the intended trade-off for privacy

The personal group UUID is stored in:

1. **Entity's config** (`~/.cordelia/config.toml`): `personal_group` field
2. **Vault metadata**: alongside the PSK, encrypted with passphrase
3. **Portal database**: linked to entity record for portal access

Shared groups continue to use the existing content-addressed ID scheme.

### 3.3 Culture

Personal groups use `chatty` culture:

- Eager push on write: agent pushes to relay peers, relay re-pushes to keepers
- Near-instant convergence (push latency only, no anti-entropy wait)
- Anti-entropy at 60s acts as a safety net for missed pushes

**Rationale**: Eager push resolves the transparent relay anti-entropy targeting
gap. Transparent relays (boot nodes) do not include learned groups in their
anti-entropy sync sets and do not advertise them to peers via group exchange.
Under taciturn culture, items written by an agent would reach a transparent
relay via push (Gate 1 always targets relays), but the relay would never
pull new items via anti-entropy, and downstream peers would never learn to
target the relay for sync. Chatty culture bypasses this gap entirely: the
relay re-pushes received items to all active peers, and anti-entropy serves
only as a fallback. Personal data volume is small (L1 hot context, personal
L2 memories), so the bandwidth cost of eager push is negligible.

This aligns with the Pattern C specification in `group-lifecycle.md`, which
already defines personal groups as chatty.

#### Trade-offs considered

- **Taciturn was the original choice** to minimise keeper bandwidth when a
  keeper hosts many entities. Taciturn avoids per-write push traffic, relying
  on periodic anti-entropy sync at 900s intervals.

- **Taciturn creates a replication dead-end through transparent relays.**
  Boot nodes accept items via push (transparent posture) but do not include
  learned groups in their anti-entropy sync sets and do not advertise learned
  groups to peers. Under taciturn culture, an item reaching a boot node via
  push has no onward path -- no peer discovers that the boot node holds the
  group, and the boot node never initiates sync for it.

- **Chatty uses eager push, which targets relay peers directly.** When the
  agent pushes to a relay, the relay stores the item and re-pushes to all
  active peers (including keepers). This bypasses the anti-entropy gap
  entirely. Convergence is near-instant (push latency only).

- **At scale (500+ entities per keeper), chatty may need revisiting.**
  Each entity's personal group generates per-write push traffic to every
  keeper. For a keeper hosting 500 entities with frequent L1 writes, this
  could produce significant inbound push volume. See future work on network
  dynamics modelling for bandwidth projections and potential throttling
  strategies.

### 3.4 Membership

Personal group members are:

- The entity (role: `owner`)
- Designated secret keepers (role: `member`)

Keepers are added at enrolment time by the portal. The portal's embedded
keeper node (`portal-keeper`) is the default first keeper.

Keepers store and replicate encrypted blobs. They do not possess the PSK
and cannot decrypt the content. Their role is storage and availability --
they are the entity's off-device backup.

---

## 4. Encryption Model

### 4.1 Personal Group PSK

Each personal group has its own PSK (Pre-Shared Key):

- **Generation**: 32 bytes from `crypto.randomBytes(32)` at enrolment
- **Algorithm**: AES-256-GCM (same as existing group PSK scheme)
- **Key version**: `key_version = 2` (same as shared groups)
- **Storage**: Encrypted in the vault, protected by the entity's passphrase

The personal group PSK replaces the previous `key_version = 1` personal key
derived from `CORDELIA_ENCRYPTION_KEY`. All items use `key_version = 2` with
their group's PSK. The `key_version = 1` codepath is retained only for
backward compatibility during migration.

### 4.2 Vault as Key Distribution

The vault already stores encryption keys protected by a passphrase (scrypt +
AES-256-GCM, see R2-006 Section 6). The personal group PSK is stored
alongside:

```
Vault contents:
  - node_encryption_key: the existing key (legacy, pre-R5)
  - personal_group_id: the opaque UUID (for portal/device lookup)
  - personal_group_psk: the personal group's AES-256 key
  - [future] key_ring: array of versioned keys for rotation
```

**Key distribution flow:**

1. Entity enrols first device -> portal generates PSK, stores in vault
2. Entity adds second device -> enters passphrase -> vault releases PSK
3. Portal keeper is added as group member -> replicates ciphertext, no PSK
4. Entity accesses portal -> enters passphrase -> vault releases PSK ->
   portal decrypts L1/L2 for display -> key cleared on logout

The vault passphrase is the trust root. The PSK never leaves the entity's
devices and the vault. Keepers never receive it.

### 4.3 Keeper Perspective

From a keeper's perspective, personal group items are indistinguishable from
any other group's items:

- Keeper receives encrypted blob via eager push (or anti-entropy fallback)
- Keeper stores blob in its `l2_items` table with `group_id = <opaque-uuid>`
- Keeper participates in GroupExchange, advertising the personal group descriptor
- Keeper cannot decrypt (no PSK)
- Keeper can serve the encrypted blob to any peer that requests it

This is the same trust model as an encrypted backup service: the infrastructure
stores data it cannot read.

---

## 5. What Changes

### 5.1 Schema

**No schema changes required.** The existing `l2_items` table already supports:

- `group_id TEXT` (currently nullable, will always be set post-migration)
- `key_version INTEGER` (2 for group PSK)
- `visibility TEXT` (will be `'group'` for all items)

The `visibility` column becomes redundant post-migration (all items are `'group'`).
It is retained for backward compatibility but no longer consulted in new code.

**L1 hot context** is currently stored in `l1_hot` table keyed by `user_id`.
Post-R5, L1 data is written as an L2 item in the personal group with a
reserved type (e.g., `type = 'l1-hot'`). The `l1_hot` table is retained as
a local cache for fast reads but is no longer the source of truth.

### 5.2 Node Bridge

The node bridge (`node-bridge.ts`) currently skips `key_version = 1` items:

```typescript
if (isEncryptedPayload(data) && keyVersion === 1) {
  return { skip: true };  // Cannot decrypt proxy-key items at node
}
```

Post-R5, all items use `key_version = 2`. This skip logic is removed. The
node replicates all encrypted blobs regardless of key version.

### 5.3 Proxy Storage Mode

The portal's proxy sidecar switches from `CORDELIA_STORAGE=sqlite` (isolated
local database) to `CORDELIA_STORAGE=node` with `CORDELIA_NODE_URL=http://localhost:9473`
(node-backed storage). This matches the edge relay architecture.

The proxy reads/writes through the node's HTTP API. The node handles
replication. One database, one codepath.

### 5.4 Portal Decrypt Flow

```
User logs into portal (OAuth)
  -> Portal shows encrypted identity cards: "Enter passphrase to unlock"
  -> User enters vault passphrase
  -> Portal retrieves encrypted PSK from vault
  -> Decrypts PSK with passphrase (scrypt)
  -> Holds PSK in server-side session memory (never sent to client)
  -> Decrypts L1/L2 items from keeper's replicated blobs
  -> Renders identity, preferences, session continuity
  -> PSK cleared on logout / session expiry
```

### 5.5 Enrolment Changes

Device enrolment (RFC 8628 device code flow) adds:

1. Generate personal group UUID v4 (if first device)
2. Generate personal group PSK (if first device)
3. Create personal group with opaque UUID
4. Add entity as owner
5. Add portal-keeper as member
6. Store UUID + PSK in vault (encrypted with passphrase)
7. Write UUID to device's `config.toml` (`personal_group` field)
8. Push personal group descriptor to device's node

---

## 6. Key Rotation

Key rotation for personal groups is **user-triggered only**. There is no
automatic rotation schedule. Rotation is appropriate when:

- Entity suspects key compromise
- Entity removes a device that had the PSK
- Entity wants to rotate as a hygiene practice

### 6.1 Rotation Procedure

1. Generate new PSK: `crypto.randomBytes(32)`
2. Increment key version in vault (key ring pattern)
3. Re-encrypt all items in personal group with new PSK
4. Update `key_version` on re-encrypted items
5. Store new PSK in vault alongside old PSK
6. Old PSK retained in vault for reading pre-rotation items
7. Distribute new PSK to other devices via vault

### 6.2 Key Ring

The vault stores a key ring rather than a single key:

```json
{
  "personal_group_psk": [
    { "version": 2, "key": "<base64>", "created_at": "2026-02-04T..." },
    { "version": 1, "key": "<base64>", "created_at": "2026-01-27T...", "retired_at": "2026-02-04T..." }
  ]
}
```

Decryption tries the item's `key_version` against the ring. Encryption
always uses the latest version.

---

## 7. Permissions Model Simplification

### 7.1 Current State (Whitepaper Section 3.3)

The whitepaper defines four roles:

| Role | Read | Write own | Write all | Delete | Admin |
|------|------|-----------|-----------|--------|-------|
| viewer | Y | N | N | N | N |
| admin | Y | Y | Y | Y | Y |
| owner | Y | Y | Y | Y | Y + transfer |
| member | Y | Y | N | N | N |

### 7.2 Implementation (R2-006 Section 4)

The R2 implementation uses an inline policy engine with a single membership
check. The `member` role permits reads and writes to own items. The `admin`
and `owner` roles are functionally equivalent except for ownership transfer.

### 7.3 R5 Simplification

**Nested groups are not supported.** The whitepaper does not specify nested
groups, and the implementation has never included them. The group model is
flat: an entity belongs to zero or more groups, each group has a flat member
list with roles.

This is a deliberate design choice. Nested groups introduce:

- Transitive permission resolution (which group's policy wins?)
- Circular dependency risks
- Complexity in the replication protocol (descriptor for a group-of-groups?)
- Conflict resolution across group boundaries

The flat model is sufficient for the target use cases:

- **Personal group**: entity + keepers (R5)
- **Team group**: founders, employees (R2-006)
- **Project group**: per-project membership (R2-006)
- **Organisation group**: all members of an org (future)

If an entity needs access to multiple scopes, they join multiple flat groups.
Composition through membership, not nesting.

### 7.4 Role Semantics for Personal Groups

In a personal group:

- **owner** (the entity): full read/write/delete
- **member** (keepers): replication only -- they store encrypted blobs they
  cannot read. The `member` role grants write access (to accept replicated
  items) but the encryption makes the content opaque.

The existing role model works without modification. Keepers are `member`
role, which allows them to write (store replicated items) and read (serve
encrypted blobs to peers). They cannot decrypt, so the effective access is
"store and forward" despite the role granting read/write.

---

## 8. Migration Path

### 8.1 Phase 1: Personal Group Infrastructure

- Auto-create personal groups at enrolment
- Generate and vault personal group PSK
- Add portal-keeper as member
- No changes to existing items (backward compatible)

### 8.2 Phase 2: New Items Use Personal Groups

- All new L1/L2 writes go to the personal group with `key_version = 2`
- Existing items remain with `group_id = NULL` and `key_version = 1`
- Node bridge stops skipping `key_version = 1` items (replicate as-is)

### 8.3 Phase 3: Backfill

- Migrate existing private items to personal group
- Re-encrypt with personal group PSK
- Set `group_id = <personal-group-uuid>`, `key_version = 2`
- Drop `key_version = 1` codepath

### 8.4 Phase 4: Portal Node-Backed Storage

- Switch portal proxy from `CORDELIA_STORAGE=sqlite` to `CORDELIA_STORAGE=node`
- Set `CORDELIA_NODE_URL=http://localhost:9473`
- Verify portal reads groups and items from keeper's replicated data
- Add passphrase unlock flow to portal UI

---

## 9. What This Eliminates

- The private/group storage split (single codepath)
- The `key_version = 1` encryption codepath (everything uses group PSK)
- The `CORDELIA_ENCRYPTION_KEY` environment variable
- The `visibility` column as a decision point (all items are `'group'`)
- The node bridge skip logic for `key_version = 1` items
- The portal identity cards gap (keepers replicate everything)
- Separate backup strategy for private data (replication IS the backup)
- Entity-to-group correlation via group_id (opaque UUID, see metadata-privacy.md)

## 10. What This Preserves

- Vault passphrase as the trust root
- COW semantics for sharing between groups (R2-006 Section 5)
- GroupExchange protocol with descriptor signing (R4-030)
- The four-role permission model (viewer/member/admin/owner)
- Flat group model (no nesting)
- Entity sovereignty invariant (R2-006 Section 1.1)
- Cache hierarchy (L1 as local cache, L2 as durable store)

---

## 11. Verification

### 11.1 Convergence Test (Docker E2E)

Deploy a 3-node Docker environment:
- Node A: entity's personal node (writes L1/L2)
- Node B: keeper (portal-keeper)
- Node C: second keeper (DR)

Write L1 hot context on Node A. Verify:
- Within seconds (eager push), encrypted blob appears on B and C
- B and C cannot decrypt (no PSK)
- Fetch PSK from vault, decrypt on B -> matches original

### 11.2 Portal Integration Test

1. Enrol a device via portal -> personal group created, PSK in vault
2. Write L1 hot context from device
3. Wait for anti-entropy sync to portal's keeper
4. Log into portal, enter passphrase
5. Verify identity cards render with correct data

### 11.3 Key Rotation Test

1. Write items with PSK v1
2. Rotate key -> new PSK v2
3. Verify old items still readable (key ring)
4. Write new items with PSK v2
5. Verify new items not readable with PSK v1

### 11.4 Device Loss Recovery

1. Write items on device A
2. Wait for replication to keeper
3. Destroy device A's database
4. Enrol new device B, enter passphrase
5. PSK from vault -> decrypt items from keeper -> full recovery

---

## 12. References

- **R2-006**: Group Model -- schema, COW, policy engine, envelope encryption stub
- **R4-030**: Group Metadata Replication -- GroupExchange protocol, descriptor signing
- **Whitepaper Section 3.3**: Group definition, role hierarchy, membership as access primitive
- **Whitepaper Section 3.4**: Culture as group-level replication policy
- **Whitepaper Section 8**: Security model, encryption invariants

---

*This document is a companion to the Cordelia whitepaper and the R2-006/R4-030
design specifications. It describes the unification of private and group storage
into a single replication model.*
