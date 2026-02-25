# R2-006: Group Memory Model -- Contact Makes You More, Not Less

*Design document for S5 implementation. Produced during S4.*

> "A Culture Mind does not diminish by sharing. It grows. Every Contact is an opportunity
> to become more than you were -- more informed, more nuanced, more capable of good
> judgment. The same must be true of memory."

---

## Table of Contents

1. [Constraints (Non-Negotiable)](#1-constraints-non-negotiable)
2. [Schema (SQL for S5)](#2-schema-sql-for-s5)
3. [Auth Identity Model](#3-auth-identity-model)
4. [Policy Enforcement](#4-policy-enforcement)
5. [COW Semantics](#5-cow-semantics)
6. [Envelope Encryption (R2-009 Stub)](#6-envelope-encryption-r2-009-stub)
7. [Culture Object](#7-culture-object)
8. [Migration Plan for S5](#8-migration-plan-for-s5)
9. [Open Questions](#9-open-questions)

---

## 1. Constraints (Non-Negotiable)

These constraints are axiomatic. They cannot be relaxed, traded off, or deferred. Every
design decision in this document must satisfy all seven simultaneously. If a proposed
implementation violates any constraint, the implementation is wrong -- not the constraint.

### 1.1 Entity Security Primacy

Entity policy overrides group policy. Always. Without exception.

If an entity says "do not share this memory," no group administrator, no consensus
mechanism, no majority vote can override that decision. This is not a feature; it is
a constitutional principle. In Culture terms: even the Hub Mind of a GSV cannot compel
a drone to share its private thoughts.

The implication is directional: entity policy can only *restrict* what group policy
permits, never *expand* it. An entity in a permissive group can choose to be private.
An entity in a restrictive group cannot unilaterally make it permissive.

### 1.2 COW Immutability

Shared memories are copies. Originals are untouched.

When a memory is shared to a group, a new row is created with `parent_id` pointing to
the original and `is_copy = 1`. The original memory remains in the author's private
space, unmodified. This is the same principle as Git's immutable object store: you can
fork, branch, and merge, but you cannot rewrite history without leaving evidence.

Without COW, a compromised group member can overwrite shared memories in place with no
detection or rollback. COW makes the entity trust primacy invariant *enforceable*:
the original author's version survives regardless of group compromise.

### 1.3 Private by Default

Every memory is private until explicitly shared. There is no "public by default" mode,
no "auto-share" setting, no implicit group visibility. The act of sharing is always a
conscious, auditable decision by the memory's author.

This is not paranoia; it is respect. A Mind that assumes the right to broadcast is not
a Mind worth trusting.

### 1.4 Trust is Local, Not Consensus

Trust decisions are made by individual entities, not by group vote. Entity A's trust
in Entity B is A's business alone. There is no mechanism for the group to declare
"Entity B is trustworthy" and have that override A's local assessment.

In R3, this enables trust calibration: each entity maintains its own trust scores for
other entities, informed by observed behaviour (COW version chains provide the evidence).
A group cannot compel trust; it can only provide a space where trust may grow or decay
based on actions.

### 1.5 Envelope Encryption (Signal Pattern)

The target architecture (R3) uses envelope encryption: the group key encrypts memories,
and each member's key encrypts the group key. This means:

- Adding a member = encrypting the group key with their public key
- Removing a member = rotating the group key and re-encrypting for remaining members
- Compromise of one member's key does not retroactively compromise memories encrypted
  under previous group key versions (forward secrecy via rotation)

R2 implements a degenerate stub (shared key, no real envelope), but the schema and
interfaces are designed for the full pattern. The `key_version` column exists from day one.

### 1.6 R2 = Degenerate Single-Process Case

R2 is a single Cordelia process serving three founders over stdio MCP. This is the
simplest possible deployment: one PEP (Policy Enforcement Point), one policy store,
one trust domain.

However, the design *must not preclude* R3's distributed architecture:

- **R3 target**: PDP/PEP/PIP separation for federation
- **R3 target**: Policy distribution for multi-instance Cordelia
- **R3 target**: Delegated authority for swarm agents

The R2 implementation is an inline PEP in `server.ts` with a single membership check.
The interface is extracted into `src/policy.ts` with a `PolicyEngine` contract. R3
replaces the implementation behind the interface without changing the contract.

### 1.7 EMCON Awareness (R3-015 Entity Posture Override)

Entities can go silent. At any time, in any group, an entity can set its posture to
`emcon` (emissions control) -- full radio silence. No broadcasts, no notifications,
no acknowledgments. Receive-only.

This is not a bug in the protocol; it is a fundamental right. A Culture Mind entering
contested space runs silent not because the protocol failed, but because the protocol
*anticipated* adversarial environments.

The effective posture is always the more restrictive of entity posture and group culture:

```
effective_posture = min(group_culture.broadcast_eagerness, entity.posture)
```

Posture changes are recorded in the audit log with a reason (`manual`, `threat_response`,
`default`). The group cannot override an entity's silence. See R3-015 for full design.

The `posture` column on `group_members` is included in the S5 schema to avoid a future
migration. Even though EMCON logic is R3, the storage is ready now.

---

## 2. Schema (SQL for S5)

Schema version: **v4** (current production is v3).

All changes applied in a single migration transaction. Backwards-incompatible changes
(visibility enum) are handled via data backfill within the same transaction.

### 2.1 New Tables

#### `groups`

The group is the universal sharing primitive. It replaces the ambiguous `team` scope.

```sql
CREATE TABLE IF NOT EXISTS groups (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  culture TEXT NOT NULL DEFAULT '{}',
  security_policy TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

The `culture` column stores a human-readable JSON `GroupCulture` object (see Section 7).
The `security_policy` column stores group-level policy constraints (maximum visibility,
required encryption level, departure policy). Both are JSON -- readable by humans,
parseable by Minds.

> "The Culture's constitution is public." Every group's rules are inspectable by its
> members. There are no secret policies.

#### `group_members`

Membership is explicit, role-based, and posture-aware.

```sql
CREATE TABLE IF NOT EXISTS group_members (
  group_id TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'member'
    CHECK(role IN ('owner', 'admin', 'member', 'viewer')),
  posture TEXT DEFAULT 'active'
    CHECK(posture IN ('active', 'silent', 'emcon')),
  joined_at TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (group_id, entity_id),
  FOREIGN KEY (group_id) REFERENCES groups(id),
  FOREIGN KEY (entity_id) REFERENCES l1_hot(user_id)
);
```

**Roles**:
- `owner`: Full control. Can delete group, change security policy, manage all members.
- `admin`: Can add/remove members, change culture. Cannot delete group or change security policy.
- `member`: Can read and write group memories. Cannot manage membership.
- `viewer`: Read-only access. Cannot write group memories.

**Posture** (included now for R3-015, avoid future migration):
- `active`: Normal participation, broadcasts per group culture.
- `silent`: Receive-only, no emissions. Manual opt-in.
- `emcon`: Full emissions control. Threat-triggered or manual. Like a GSV running dark
  in contested space.

#### `access_log`

Structured audit log replacing the current freeform `audit` table for group operations.
The existing `audit` table is retained for backwards compatibility; `access_log` adds
structured fields for policy evaluation and forensics.

```sql
CREATE TABLE IF NOT EXISTS access_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ts TEXT NOT NULL DEFAULT (datetime('now')),
  entity_id TEXT NOT NULL,
  action TEXT NOT NULL,
  resource_type TEXT NOT NULL,
  resource_id TEXT,
  group_id TEXT,
  detail TEXT
);
```

Every policy evaluation is logged: allowed and denied. Storage is cheap; information
loss is expensive. The audit trail is the evidence base for R3 trust calibration.

### 2.2 Column Additions to `l2_items`

```sql
-- Group association (NULL = private, not shared to any group)
ALTER TABLE l2_items ADD COLUMN group_id TEXT;

-- Provenance: who authored this memory
ALTER TABLE l2_items ADD COLUMN author_id TEXT;

-- Envelope encryption: which key version encrypted this item
ALTER TABLE l2_items ADD COLUMN key_version INTEGER DEFAULT 1;

-- COW: if this is a copy, points to the original memory
ALTER TABLE l2_items ADD COLUMN parent_id TEXT;

-- COW: flag indicating this row is a shared copy
ALTER TABLE l2_items ADD COLUMN is_copy INTEGER DEFAULT 0;
```

### 2.3 Visibility Enum Reconciliation

The current schema has `CHECK(visibility IN ('private', 'team', 'public'))`. The `team`
value is an R1 artifact -- a vague scope with no enforcement mechanism. Groups replace
it entirely.

**Migration**: all rows with `visibility = 'team'` are updated to `visibility = 'group'`.
The CHECK constraint is rebuilt:

```sql
-- Step 1: Update existing data
UPDATE l2_items SET visibility = 'group' WHERE visibility = 'team';

-- Step 2: Recreate table with new constraint (SQLite ALTER TABLE limitation)
-- In practice, handled via table rebuild in migration code.
-- Final constraint:
--   CHECK(visibility IN ('private', 'group', 'public'))
```

**Final visibility enum**: `'private'` | `'group'` | `'public'`

- `private`: Visible only to `owner_id`. Default.
- `group`: Visible to members of the group specified by `group_id`.
- `public`: Visible to all entities. Use with extreme caution.

### 2.4 Indexes

```sql
-- Fast lookup of group memories
CREATE INDEX IF NOT EXISTS idx_l2_items_group ON l2_items(group_id)
  WHERE group_id IS NOT NULL;

-- COW chain traversal
CREATE INDEX IF NOT EXISTS idx_l2_items_parent ON l2_items(parent_id)
  WHERE parent_id IS NOT NULL;

-- Author provenance queries
CREATE INDEX IF NOT EXISTS idx_l2_items_author ON l2_items(author_id)
  WHERE author_id IS NOT NULL;

-- Access log queries by entity
CREATE INDEX IF NOT EXISTS idx_access_log_entity ON access_log(entity_id);

-- Access log queries by group
CREATE INDEX IF NOT EXISTS idx_access_log_group ON access_log(group_id)
  WHERE group_id IS NOT NULL;
```

---

## 3. Auth Identity Model

Three transport mechanisms, three identity models. All converge on a single `entity_id`
that maps to `l1_hot.user_id`.

### 3.1 MCP over stdio (Local)

**R2**: The `user_id` parameter in MCP tool calls is trusted. The process is local,
launched by the user's shell, running under their OS user. The trust boundary is the
operating system's process isolation.

**R3**: Cryptographic binding. The stdio transport signs requests with a local key,
and Cordelia verifies the signature against the entity's registered public key. This
prevents a compromised shell from impersonating another user.

### 3.2 HTTP (Web UI)

**R2 (existing)**: GitHub OAuth flow in `http-server.ts` maps GitHub identity to
`entity_id`. The OAuth token is verified on each request. Session cookies with
appropriate expiry and CSRF protection.

**R3**: Extend to additional OAuth/OIDC providers. Combine with R3-008 identity
federation for cross-IdP mapping.

### 3.3 HTTP MCP (Remote Tool Access)

**R2**: Pre-shared bearer token per entity. Each founder has a unique token stored
outside the repository. Token is sent in the `Authorization` header. Simple, sufficient
for three trusted users on a private network.

**R3**: JWT with claims. Token includes `entity_id`, group memberships, posture, and
expiry. Signed by Cordelia's private key. Enables stateless policy evaluation at the
PEP without querying the PDP for every request.

### 3.4 Identity Resolution

All three transports resolve to the same `entity_id`. The mapping is:

| Transport | R2 Identity | R3 Identity | Maps To |
|-----------|-------------|-------------|---------|
| stdio MCP | `user_id` param (trusted) | Signed `user_id` | `l1_hot.user_id` |
| HTTP | GitHub OAuth username | OAuth/OIDC federation | `l1_hot.user_id` |
| HTTP MCP | Bearer token -> entity lookup | JWT `sub` claim | `l1_hot.user_id` |

---

## 4. Policy Enforcement

### 4.1 R2: Inline PEP

In R2, policy enforcement is a single function call in `server.ts`. Before any
memory operation, the handler checks:

1. Is the entity authenticated? (identity resolution per Section 3)
2. For private memories: is `entity_id === owner_id`?
3. For group memories: is the entity a member of the memory's `group_id`?
4. For write operations: does the member's role permit writes? (`viewer` cannot write)
5. Is the entity's posture compatible with the operation? (`emcon` entities cannot write to group)

This is simple, correct, and sufficient for three founders on a single process.

### 4.2 PolicyEngine Interface

To ensure R2's implementation does not preclude R3, the enforcement logic is extracted
into `src/policy.ts` behind a clean interface:

```typescript
interface PolicyEngine {
  evaluate(request: {
    entity_id: string;
    action: 'read' | 'write' | 'delete' | 'share';
    resource_type: string;
    resource_id?: string;
    group_id?: string;
  }): Promise<PolicyDecision>;
}

interface PolicyDecision {
  allowed: boolean;
  reason?: string;
  audit_detail?: string;
}
```

**R2 implementation** (`InlinePolicyEngine`):
- Single `evaluate()` method
- Queries `group_members` table directly
- Returns `PolicyDecision` with audit detail
- Logs every evaluation to `access_log`

**R3 sketch** (`DistributedPolicyEngine`):
- PEP (Policy Enforcement Point): sits in `server.ts`, calls PDP
- PDP (Policy Decision Point): standalone service, evaluates policies
- PIP (Policy Information Point): provides entity attributes, group memberships,
  trust scores to PDP
- PEP sends request to PDP, PDP queries PIP, PDP returns decision to PEP
- Policy distribution: PDP pushes policy updates to PEP instances (for federation,
  multiple Cordelia instances need consistent policy without round-tripping to a
  central PDP on every request)
- Delegated authority: swarm agents receive scoped tokens from their parent entity,
  with limited permissions and TTL

The key insight: R2's `PolicyEngine` interface is the PEP contract. R3 replaces the
*implementation* (from inline to distributed), not the *interface*. The `server.ts`
handlers never change.

### 4.3 Audit Logging

Every `PolicyEngine.evaluate()` call writes to `access_log`, regardless of outcome:

```
{
  entity_id: "russell",
  action: "read",
  resource_type: "entity",
  resource_id: "ent-abc-123",
  group_id: "seed-drill",
  detail: "allowed: member role permits read"
}
```

Denied requests are equally important:

```
{
  entity_id: "unknown-agent",
  action: "write",
  resource_type: "learning",
  resource_id: null,
  group_id: "seed-drill",
  detail: "denied: entity not a member of group seed-drill"
}
```

This log is the raw material for R3 trust calibration. Pattern analysis over the
access log reveals anomalies: sudden bursts of read activity, writes from unusual
contexts, access patterns inconsistent with an entity's established behaviour.

---

## 5. COW Semantics

### 5.1 Why COW

Copy-on-write is not an optimisation. It is a security mechanism.

Without COW, sharing a memory to a group means the group has a *reference* to your
original. Any member with write access can modify that reference. A compromised member
can silently overwrite shared memories -- no detection, no rollback, no evidence.

With COW:
- **Entity sovereignty**: Your original memory is yours. Forever. No group action can
  alter it.
- **No mutation of originals**: The original row is never modified by a share operation.
  The group gets a copy.
- **Full audit trail**: The COW chain (`parent_id` -> original) provides tamper evidence.
  Any divergence between original and copy is visible and attributable.
- **Legal compulsion resistance**: If a nation state compels modification of group
  memories (R2-005), the modification creates a visible fork, not a silent overwrite.
  The original author's version survives as evidence.

This is the same principle that makes Git trustworthy: immutable objects, append-only
history, content-addressed storage. You can rewrite history, but you cannot hide that
you did.

### 5.2 Share Flow

When an entity shares a memory to a group:

```
memory_share(item_id: "mem-123", target_group: "seed-drill")
```

1. **Policy check**: Is the entity the `owner_id` of `mem-123`? Is the entity a member
   of `seed-drill`? Does their role permit `share`?
2. **Create copy**: New row in `l2_items`:
   - `id`: new GUID
   - `type`: same as original
   - `owner_id`: same as original (provenance preserved)
   - `author_id`: entity performing the share
   - `visibility`: `'group'`
   - `group_id`: `'seed-drill'`
   - `data`: copy of encrypted data (re-encrypted with group key in R3)
   - `parent_id`: `'mem-123'` (points to original)
   - `is_copy`: `1`
   - `key_version`: current group key version
3. **Original untouched**: `mem-123` remains `visibility = 'private'`, no columns changed.
4. **Audit**: Log the share action to `access_log`.
5. **Notify**: Per group culture (chatty = push, moderate = invalidate, taciturn = nothing).

### 5.3 Update Semantics

Updates to a group copy create a new version (append-only chain):

```
Original (mem-123, private, author=russell)
  |
  +-- Copy v1 (mem-456, group=seed-drill, parent=mem-123, author=russell)
        |
        +-- Copy v2 (mem-789, group=seed-drill, parent=mem-456, author=martin)
```

Each version is a new row. Old versions persist. The chain is traversable via `parent_id`.
Conflict resolution (two members update the same copy concurrently) follows the same
pattern: both new versions exist, linked to the same parent. The group sees both; the
culture determines which is canonical (or both are retained as divergent perspectives).

### 5.4 New MCP Tool: `memory_share`

```typescript
// Tool: memory_share
// Shares a private memory to a group (creates COW copy)
{
  name: "memory_share",
  description: "Share a private memory to a group. Creates an immutable copy; the original is never modified.",
  inputSchema: {
    type: "object",
    properties: {
      item_id: { type: "string", description: "ID of the memory to share" },
      target_group: { type: "string", description: "Group ID to share to" }
    },
    required: ["item_id", "target_group"]
  }
}
```

---

## 6. Envelope Encryption (R2-009 Stub)

### 6.1 R2: Degenerate Case

In R2, all three founders share a single encryption key (the existing Cordelia master
key). There is no real envelope encryption. The `key_version` column is set to `1` on
all items and is not meaningfully used.

This is acceptable because R2 is a single process with three trusted users. The threat
model for R2 does not include member-to-member key isolation.

### 6.2 KeyVault Interface

The interface is designed for the full R3 pattern, stubbed for R2:

```typescript
interface KeyVault {
  getGroupKey(groupId: string, version?: number): Promise<Buffer>;
  rotateGroupKey(groupId: string): Promise<{ newVersion: number }>;
  reencryptItems(groupId: string, fromVersion: number): Promise<{ count: number }>;
}
```

**R2 stub implementation** (`SharedKeyVault`):
- `getGroupKey()`: Returns the current shared master key. Ignores `version`.
- `rotateGroupKey()`: No-op. Returns `{ newVersion: 1 }`. Key rotation is manual in R2
  (see R2-009).
- `reencryptItems()`: No-op. Returns `{ count: 0 }`. No re-encryption needed when
  there is only one key.

### 6.3 R3 Full Pattern (Signal)

The target architecture follows the Signal protocol's key distribution model:

```
Group Key (GK) -- encrypts memory data
  |
  +-- Encrypted with Russell's public key -> stored in key_escrow table
  +-- Encrypted with Martin's public key  -> stored in key_escrow table
  +-- Encrypted with Bill's public key    -> stored in key_escrow table
```

**Adding a member**: Encrypt the current GK with the new member's public key. Store
in `key_escrow`.

**Removing a member**: Generate new GK (version N+1). Encrypt with remaining members'
public keys. Re-encrypt all group memories from version N to N+1. The removed member
retains access to memories encrypted under version N (they already had the key), but
cannot decrypt anything encrypted under version N+1.

**Key rotation**: Same as member removal without removing anyone. Limits the blast
radius of a compromised key.

### 6.4 `key_version` Column

The `key_version` column on `l2_items` records which version of the group key was used
to encrypt each item. This enables:

- Selective re-encryption during rotation (only items at old version need re-encrypting)
- Forensic analysis (which key version was active when a memory was created)
- Gradual migration (not all items need re-encrypting simultaneously)

R2 sets `key_version = 1` on all items and never increments it. The column is ready
for R3 to use meaningfully.

---

## 7. Culture Object

### 7.1 GroupCulture Interface

Every group has a culture -- a set of behavioural parameters that govern how the group
communicates. The culture is public, inspectable, and human-readable.

```typescript
interface GroupCulture {
  broadcast_eagerness: 'chatty' | 'moderate' | 'taciturn';
  ttl_default: number | null; // seconds, null = no expiry
  notification_policy: 'push' | 'notify' | 'silent';
  departure_policy: 'permissive' | 'standard' | 'restrictive';
}
```

**`broadcast_eagerness`**: How aggressively the group pushes updates to members.

**`ttl_default`**: Default time-to-live for group memories. `null` means memories
persist indefinitely. Non-null enables the Darwinian memory fitness model (R2-018):
valuable memories (frequently accessed) survive beyond TTL; non-valued expire.

**`notification_policy`**: How members are notified of group activity.
- `push`: Active notification on every change.
- `notify`: Notification available, not pushed.
- `silent`: No notifications. Members must poll.

**`departure_policy`**: What happens when a member leaves.
- `permissive`: Member retains copies of memories they authored. Clean exit.
- `standard`: Member loses access to group memories. Their authored copies remain in
  the group.
- `restrictive`: Member loses access. Group key rotated immediately. All items
  re-encrypted. Nuclear option.

### 7.2 Example: Seed Drill Group Culture

```json
{
  "broadcast_eagerness": "moderate",
  "ttl_default": null,
  "notification_policy": "notify",
  "departure_policy": "standard"
}
```

Three founders, high trust, moderate bandwidth. No TTL on memories (everything is
valuable at this stage). Notifications available but not pushed (we are busy people).
Standard departure (unlikely, but defined).

### 7.3 Culture as Cache Coherence

The `broadcast_eagerness` parameter maps directly to cache coherence protocols. This
is not a metaphor; it is the same problem. Each entity maintains a local cache of
group memories. The culture defines how that cache is kept consistent.

| Culture | Cache Protocol | MESI Equivalent | Behaviour |
|---------|---------------|-----------------|-----------|
| `chatty` | Write-update | Modified -> Shared | Every write pushes the new value to all members. High bandwidth, strong consistency. Like MESI write-update: the writer broadcasts the new value, all caches update immediately. |
| `moderate` | Write-invalidate | Modified -> Invalid | Every write notifies members that their cached copy is stale. Members re-fetch on next access. Lower bandwidth than chatty, eventual consistency. Like MESI write-invalidate: the writer broadcasts an invalidation, caches drop stale entries. |
| `taciturn` | TTL expiry | No protocol (weak) | No notifications. Members' caches expire after `ttl_default` seconds. Weakest consistency, lowest bandwidth. Suitable for high-latency links, adversarial environments, or groups where real-time consistency is not required. |

This mapping becomes critical in R3-011 (cache coherence via group culture) and
essential at interplanetary scale (R4-011), where light-speed propagation delay makes
`chatty` physically impossible beyond a certain radius. The group culture adapts to
the physics of the network, not the other way around.

> A GSV in deep space does not demand write-update coherence with the Hub. It runs
> taciturn, with long TTLs, and trusts its local cache. The protocol respects the
> speed of light.

---

## 8. Migration Plan for S5

All steps execute within a single SQLite transaction. If any step fails, the entire
migration rolls back. The database remains at v3.

### Step 1: Create New Tables

```sql
BEGIN TRANSACTION;

-- Groups table
CREATE TABLE IF NOT EXISTS groups (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  culture TEXT NOT NULL DEFAULT '{}',
  security_policy TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Group membership
CREATE TABLE IF NOT EXISTS group_members (
  group_id TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'member'
    CHECK(role IN ('owner', 'admin', 'member', 'viewer')),
  posture TEXT DEFAULT 'active'
    CHECK(posture IN ('active', 'silent', 'emcon')),
  joined_at TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (group_id, entity_id),
  FOREIGN KEY (group_id) REFERENCES groups(id),
  FOREIGN KEY (entity_id) REFERENCES l1_hot(user_id)
);

-- Structured access log
CREATE TABLE IF NOT EXISTS access_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ts TEXT NOT NULL DEFAULT (datetime('now')),
  entity_id TEXT NOT NULL,
  action TEXT NOT NULL,
  resource_type TEXT NOT NULL,
  resource_id TEXT,
  group_id TEXT,
  detail TEXT
);
```

### Step 2: Add Columns to `l2_items`

```sql
ALTER TABLE l2_items ADD COLUMN group_id TEXT;
ALTER TABLE l2_items ADD COLUMN author_id TEXT;
ALTER TABLE l2_items ADD COLUMN key_version INTEGER DEFAULT 1;
ALTER TABLE l2_items ADD COLUMN parent_id TEXT;
ALTER TABLE l2_items ADD COLUMN is_copy INTEGER DEFAULT 0;
```

### Step 3: Create `seed-drill` Group with Three Founding Members

```sql
INSERT INTO groups (id, name, culture, security_policy)
VALUES (
  'seed-drill',
  'Seed Drill',
  '{"broadcast_eagerness":"moderate","ttl_default":null,"notification_policy":"notify","departure_policy":"standard"}',
  '{}'
);

INSERT INTO group_members (group_id, entity_id, role) VALUES ('seed-drill', 'russell', 'owner');
INSERT INTO group_members (group_id, entity_id, role) VALUES ('seed-drill', 'martin', 'owner');
INSERT INTO group_members (group_id, entity_id, role) VALUES ('seed-drill', 'bill', 'owner');
```

All three founders are `owner`. This is a partnership, not a hierarchy.

### Step 4: Backfill `author_id` from `owner_id`

```sql
UPDATE l2_items SET author_id = owner_id WHERE author_id IS NULL;
```

### Step 5: Reconcile Visibility Enum

```sql
UPDATE l2_items SET visibility = 'group' WHERE visibility = 'team';
```

Note: SQLite does not support modifying CHECK constraints via ALTER TABLE. The
constraint rebuild requires a table recreation in the migration code (create new table
with correct constraint, copy data, drop old, rename new). The migration TypeScript
code handles this. The final constraint is:

```sql
CHECK(visibility IN ('private', 'group', 'public'))
```

### Step 6: Set `key_version` on All Existing Items

```sql
UPDATE l2_items SET key_version = 1 WHERE key_version IS NULL;
```

No re-encryption is needed. R2 uses a single shared key. The `key_version = 1` is a
marker that says "encrypted with the original shared key." When R3 introduces real
envelope encryption and key rotation, version 1 items are the baseline.

### Step 7: Create Indexes and Update Schema Version

```sql
CREATE INDEX IF NOT EXISTS idx_l2_items_group ON l2_items(group_id) WHERE group_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_l2_items_parent ON l2_items(parent_id) WHERE parent_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_l2_items_author ON l2_items(author_id) WHERE author_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_access_log_entity ON access_log(entity_id);
CREATE INDEX IF NOT EXISTS idx_access_log_group ON access_log(group_id) WHERE group_id IS NOT NULL;

INSERT INTO schema_version (version) VALUES (4);

COMMIT;
```

---

## 9. Open Questions

### 9.1 Schema Version Coordination

Current production is v3 (S3 added checksum + integrity canary). This document designs
v4. The question: does S3 implementation ship before S5, or do we merge v3 and v4 into
a single migration?

**Recommendation**: Ship S3 as v3, then S5 as v4. Sequential migrations are simpler to
reason about, test, and rollback. The migration framework already handles version
sequencing.

### 9.2 L2 Index: Single Index with `group_id` Filter vs Per-Group Index

**Option A** (recommended): Single `l2_index` table, queries filtered by `group_id`.
Simpler schema, single search path, familiar code. The `WHERE group_id = ?` clause is
trivially optimised by the `idx_l2_items_group` index.

**Option B**: Separate index per group. Better isolation, but more complex search code,
more tables, harder to maintain. Only justified at scale (hundreds of groups with
distinct embedding spaces).

**Recommendation**: Option A. We have one group and three members. Premature
optimisation is the root of all evil, and also most schema migrations.

### 9.3 Audit Granularity

Should `access_log` record all operations (reads, writes, deletes, shares, policy
evaluations) or only writes?

**Recommendation**: All operations. Storage is cheap. Information loss is expensive.
The audit log is the evidence base for R3 trust calibration -- you cannot calibrate
trust from incomplete data. A read pattern is as informative as a write pattern: an
entity that suddenly starts reading everything in a group is a signal worth capturing.

SQLite can handle millions of audit rows without breaking a sweat. If storage becomes
a concern (it will not for three users), implement log rotation with archival, not
selective logging.

### 9.4 Decentralised Economics

The economic model must mirror the decentralised architecture. There must be no central
point of economic failure, just as there is no central point of technical failure.

Questions to resolve:

- **Cost allocation**: Who pays for storage and compute when memories are shared across
  groups? The author? The group? Proportional to access?
- **Value capture**: If Cordelia becomes a platform (R4+), how do independent operators
  sustain their instances without creating economic dependency on a central entity?
- **Incentive alignment**: The architecture incentivises sharing (Contact makes you
  more). The economics must not penalise it. A group member who shares generously
  should not bear disproportionate cost.
- **Federation economics**: When federated Cordelia instances exchange memories (R3-001),
  the economic model must handle cross-instance value transfer without requiring a
  central clearinghouse.

This is the hardest open question. Technical decentralisation without economic
decentralisation is an illusion -- the economics re-centralise the system through the
back door. The Culture solved this by being post-scarcity. We are not post-scarcity.
The Midnight blockchain (R4-010) may provide infrastructure for decentralised economic
coordination (smart contracts for cost sharing, cryptographic fairness proofs), but the
economic *model* must be designed independently of the implementation mechanism.

> "Money is a sign of poverty." -- Iain M. Banks, *The State of the Art*.
> We are not yet rich enough to dispense with it. Design accordingly.

### 9.5 Governance Model

Governance creates an attack vector if not carefully managed. The Bitcoin vs Cardano
spectrum illustrates the trade-off: BTC's minimal governance reduces attack surface
but limits adaptability; Cardano's on-chain governance enables evolution but creates
capture risk.

**Design preference**: Closer to BTC. Minimal governance surface area, simple majority
voting per entity. The group model already provides the primitives: entities vote with
their trust decisions (local, not consensus), and culture parameters are inspectable.

**Critical requirement**: Sybil resistance. Entity sovereignty provides the foundation
(one entity = one vote), but identity verification becomes load-bearing at federation
scale (R3+). Memory accuracy-based trust calibration (Rule 7 in [Architecture Overview](../architecture/overview.md)) may
serve as a novel Sybil resistance mechanism: your reputation is your memory accuracy
over time, which cannot be faked without actual knowledge.

**Reference**: Minotaur work by Aggelos Kiayias et al. -- combining multiple consensus
mechanisms. Cordelia's entity-first integrity chains (SHA-256 hash chain per entity)
are structurally similar to a per-entity blockchain. Federation (R3) extends this to
cross-entity verification. If entity identity and memory accuracy replace proof-of-work
or proof-of-stake as the consensus basis, the result is a fundamentally different -- and
potentially more efficient -- consensus model.

---

## Appendix A: Relationship to Other Backlog Items

| Item | Relationship |
|------|-------------|
| R2-005 (Nation state threat) | COW versioning provides tamper evidence. Audit log provides forensic trail. |
| R2-009 (Key rotation) | KeyVault interface defined here. Stub implemented in S5. Martin implements vault provider. |
| R2-018 (TTL on group memories) | `ttl_default` in GroupCulture. Implemented in S5 alongside group model. |
| R3-011 (Cache coherence) | Culture `broadcast_eagerness` maps to cache protocols. Designed here, implemented R3. |
| R3-014 (Context-to-group binding) | Requires group model. Context determines which groups' memories are visible. |
| R3-015 (EMCON posture) | `posture` column included in S5 schema. Full EMCON logic is R3. |
| R4-010 (Midnight anchoring) | COW version chains are structurally Merkle chains. Export format must include `content_hash` + `author_id`. |
| R4-011 (Interplanetary scale) | Culture cache coherence adapts to propagation delay. Group model must not preclude light-cone partitioning. |

---

*Last updated: 2026-01-29*
