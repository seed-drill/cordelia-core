# Member Removal Design

**Status:** R4 soft removal implemented. R5 hard removal planned.
**Issue:** [cordelia-core#15](https://github.com/seed-drill/cordelia-core/issues/15)
**References:** [R2-006 group model](R2-006-group-model.md), [R5 personal groups](R5-personal-groups.md), [R4-030 group metadata](R4-030-group-metadata-replication.md)

---

## 1. R4 Behaviour (Current)

### 1.1 Soft removal

`POST /api/v1/groups/remove_member` sets `posture = "removed"` on the member row (CoW soft-delete). The row is retained in SQLite but filtered from `list_members` and `get_membership` responses.

This is local-only (R4-030). Membership does not replicate -- portal must call the endpoint on every node that has the group.

### 1.2 Portal orchestration sequence

```
Portal                  Node A (owner)         Node B (member)        Node C (removed)
  |                         |                       |                       |
  |-- remove_member(C) ---->|                       |                       |
  |-- remove_member(C) ----------------------->|                       |
  |-- groups/delete ---------------------------------------------------------------->|
  |                         |                       |                       |
  |  (C stops replicating)  |                       |                       |
```

Step 3 (`groups/delete` on the removed member's node) is critical -- it writes a tombstone and removes the group from `shared_groups`, which stops the node from participating in replication.

### 1.3 What soft removal does NOT do

| Concern | Current state | Risk |
|---------|--------------|------|
| Items already replicated to removed member | Persist as encrypted blobs | Low -- content encrypted at proxy layer |
| Items authored by removed member | Remain in group on all nodes | None -- items belong to the group |
| Encryption key | Unchanged | Medium -- removed member holds current key |
| Notification to other members | None | Low -- membership is local, no protocol impact |
| Automatic cleanup on other nodes | None | Low -- portal handles orchestration |

---

## 2. Threat Model

### 2.1 What a removed member retains

After soft removal + `groups/delete` on their node:

- **Encrypted blobs** of all items replicated before removal (stored in local SQLite)
- **Group encryption key** (current `key_version`) -- can decrypt all historical items
- **Metadata**: group ID, member entity IDs, item types, timestamps, checksums
- **Group descriptor**: culture, security policy, owner identity

### 2.2 What a removed member cannot do

- **Receive new items** -- `shared_groups` cleared, no replication path
- **Push items to the group** -- replication three-gate model rejects (no group in `shared_groups`)
- **Re-join without portal** -- membership requires portal API call with auth
- **Forge items** -- checksum verification would fail
- **Forge group descriptors** -- Ed25519 signature verification (only owner can sign)

### 2.3 Residual risk: key compromise

The removed member holds the current group PSK. If they extract it (the key lives in the proxy's keychain, not in the Rust node), they could:

1. Decrypt all historical items they already hold
2. Decrypt items encrypted with the same `key_version` if they somehow obtain the ciphertext

**Mitigation (R5):** Key rotation on member removal. New `key_version` means future items are encrypted with a key the removed member never received.

### 2.4 Risk assessment

| Threat | Likelihood | Impact | Mitigation |
|--------|-----------|--------|------------|
| Removed member reads historical items | Certain (they have the blobs + key) | Low (they already saw them as a member) | Accept for R4. Key rotation in R5. |
| Removed member decrypts future items | Very low (no replication path for ciphertext) | Medium | Replication gates prevent delivery. Key rotation in R5 eliminates entirely. |
| Removed member re-joins | Low (requires portal compromise) | Medium | Portal auth + invitation flow. |
| Removed member infers activity from metadata | Certain (metadata is plaintext) | Low (accepted trade-off, see memory-architecture.md) | Accept. Onion routing out of scope. |

**Conclusion:** Soft removal is sufficient for R4. The primary risk (historical key access) is acceptable because (a) the member already had legitimate access, and (b) item content is encrypted -- the Rust node never sees plaintext.

---

## 3. R5 Hard Removal (Planned)

### 3.1 Departure policy

The group's `culture.departure_policy` governs removal behaviour:

- **permissive**: Member retains copies of authored items. Clean exit, no key rotation.
- **standard**: Member loses access. Group key rotated. Future items use new key version. Historical items remain readable via key ring.
- **restrictive**: Member loses access. Immediate full re-encryption of all items. Nuclear option -- high cost, use sparingly.

### 3.2 Standard removal flow

```
Portal                  Vault                   All remaining nodes
  |                       |                          |
  |-- rotate_key(group) ->|                          |
  |<-- new PSK (v2) ------|                          |
  |-- distribute_key(v2) --------------------------->|
  |-- remove_member(entity) ------------------------>|
  |                       |                          |
  |  Future items use key_version=2                  |
  |  Old items readable via key ring (v1 retained)   |
```

1. Portal requests key rotation from vault
2. Vault generates new PSK, increments `key_version`, retains old key in ring
3. Portal distributes new PSK to all remaining members' proxies
4. Portal calls `remove_member` on all nodes
5. Removed member's node gets `groups/delete` (tombstone)
6. All future writes use `key_version = 2`
7. Reads check item's `key_version` against the key ring

### 3.3 Key ring

Each proxy maintains a key ring per group:

```json
{
  "group_id": "team-alpha",
  "keys": [
    { "version": 2, "key": "<base64>", "created_at": "..." },
    { "version": 1, "key": "<base64>", "created_at": "...", "retired_at": "..." }
  ]
}
```

- Encryption: always latest version
- Decryption: match item's `key_version` to ring entry
- Removed member has v1 only -- cannot decrypt v2+ items

### 3.4 Restrictive removal

Same as standard, plus:
1. Re-encrypt all existing items with new key version
2. Old key versions purged from vault (no backward compatibility)
3. Extremely expensive for large groups -- O(items) re-encryption
4. Only appropriate for highly sensitive groups (e.g., compliance, legal)

### 3.5 Prerequisites for R5

- [ ] Vault key management API (store/rotate/distribute PSKs)
- [ ] Proxy key ring implementation (multi-version decrypt)
- [ ] `key_version` column on L2 items (exists in schema, not yet used)
- [ ] Portal removal orchestration (multi-node, key distribution)
- [ ] `departure_policy` enforcement in culture parsing

---

## 4. Implementation Notes

### 4.1 CoW invariant

All removal operations follow Copy-on-Write:
- `remove_member`: sets `posture = "removed"` (not DELETE)
- `groups/delete`: writes tombstone culture (not DELETE)
- Physical deletion only via GC after retention window

### 4.2 Schema

`group_members.posture` CHECK constraint includes `'removed'` (schema v7). The `removed` posture is filtered by `list_members` and `get_membership` but the row remains queryable via direct SQL for audit/compliance.

### 4.3 Idempotency

`remove_member` is idempotent -- calling it on an already-removed member returns `404` (second call finds no row with `posture != 'removed'`). Re-adding a removed member via `add_member` does an upsert that resets the role (but posture stays `removed` -- the member must be explicitly re-activated via `update_posture`).

`add_member` on a previously-removed member resets posture to `active` (upsert reactivates).
