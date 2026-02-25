# Metadata Privacy Analysis

**Status:** Analysis complete. Mitigations deferred to R5+.
**Issue:** [cordelia-core#16](https://github.com/seed-drill/cordelia-core/issues/16)
**References:** [Threat model](../architecture/threat-model.md), [R4-030](R4-030-group-metadata-replication.md), [Memory architecture](memory-architecture.md)

---

## 1. Design Principle: "The Manifest Is Not Cargo"

Item content is encrypted at the proxy layer (AES-256-GCM) before reaching the P2P node. The Rust node and all relay peers handle opaque ciphertext. However, the wire protocol carries plaintext metadata required for routing, integrity verification, and conflict resolution. This is an explicit design trade-off documented in memory-architecture.md and R4-030.

This analysis inventories what a compromised relay can observe, what it can infer, and what mitigations are available for future consideration.

---

## 2. Plaintext Metadata Inventory

### 2.1 Per-item metadata (on every push/sync)

| Field | Example | Purpose | Sensitivity |
|-------|---------|---------|-------------|
| `item_id` | `a3f7c2...` | Unique identifier | Low -- opaque GUID |
| `item_type` | `entity`, `session`, `learning` | Domain classification | Medium -- reveals content category |
| `author_id` | `russell` | Authorship attribution | High -- identifies who wrote what |
| `group_id` | `team-alpha` | Group membership | High -- reveals organisational structure |
| `checksum` | `e4d909c...` | SHA-256 of encrypted blob | Low -- integrity only, not reversible |
| `updated_at` | `2026-02-25T14:30:00Z` | LWW conflict resolution | Medium -- timing side-channel |
| `key_version` | `1` | Encryption key generation | Low -- rotation marker |
| `parent_id` | `b8c3d1...` (optional) | CoW provenance chain | Medium -- reveals sharing patterns |
| `is_copy` | `true` | Copy marker | Low -- but confirms sharing occurred |
| `is_deletion` | `true` | Tombstone marker | Low -- reveals deletion activity |
| `encrypted_blob` length | 1,847 bytes | Implicit from transport | Medium -- size reveals content type |

### 2.2 GroupExchange metadata (every ~60s between peers)

| Field | Example | Purpose | Sensitivity |
|-------|---------|---------|-------------|
| `descriptor.id` | `team-alpha` | Group UUID | Medium -- group existence |
| `descriptor.culture` | `{"broadcast_eagerness":"chatty"}` | Replication policy JSON | Medium -- reveals group behaviour |
| `descriptor.updated_at` | `2026-02-25T12:00:00Z` | LWW ordering | Low |
| `descriptor.checksum` | `f1a2b3...` | Integrity | Low |
| `descriptor.owner_id` | `russell` | Group owner | High -- identifies group creator |
| `descriptor.owner_pubkey` | `ed25519:...` | Owner's public key | Medium -- stable identifier |
| `descriptor.signature` | `sig:...` | Ed25519 signature | Low -- authenticity proof |

### 2.3 PeerShare metadata (on peer discovery)

| Field | Example | Purpose | Sensitivity |
|-------|---------|---------|-------------|
| `peer_id` | `12D3KooW...` | libp2p identity | Medium -- stable node identifier |
| `addrs` | `/ip4/10.0.1.5/tcp/9472` | Network addresses | High -- reveals IP/location |
| `groups` | `["team-alpha","personal-russell"]` | Group membership list | High -- full group roster per node |
| `role` | `relay`, `personal`, `keeper` | Node classification | Medium -- reveals topology |
| `last_seen` | `1740000000` | Activity timestamp | Low |

### 2.4 Not on wire (by design)

| Field | Reason |
|-------|--------|
| `name` (group display name) | Portal-only, distributed out-of-band during enrollment |
| `security_policy` | Always `{}`, not transmitted |
| `domain` (value/procedural/interrupt) | Proxy-local classification, never leaves the edge |
| `visibility` (private/group/public) | Storage-local, not in wire protocol |
| `access_count`, `last_accessed_at` | Local usage metrics, not replicated |
| Member list (`group_members`) | Local-only per R4-030 |

### 2.5 Implementation vs design gap

`memory-architecture.md` specifies that `author_id` and `group_id` should be hashed on the wire (`SHA-256(Ed25519 pubkey)`, `SHA-256(group URI)`). The actual implementation transmits plaintext strings. This gap exists because:

1. Hashing `group_id` would prevent relay routing (relays need to match groups for three-gate filtering)
2. Hashing `author_id` would prevent access logging with readable identifiers
3. The practical benefit is limited -- a relay that knows any group member can correlate hashed IDs via observation

This is an accepted deviation. If pseudonymous IDs become a requirement, they should be addressed at the protocol level (see Section 5.3).

---

## 3. Traffic Analysis: What a Compromised Relay Can Infer

A relay stores and forwards encrypted items. It participates in GroupExchange and PeerShare. It does not possess decryption keys. However, by observing traffic patterns over time, it can build a detailed picture of network activity.

### 3.1 Organisational structure

**Observable:** GroupExchange reveals which groups exist and which peers share them. PeerShare reveals group membership lists per peer.

**Inference:** A relay connected to multiple organisations can map the full group topology -- who collaborates with whom, which groups span organisations, and which are internal. Group IDs may be opaque UUIDs, but the membership graph is structural information.

**Example:** Relay sees peers A, B, C all share `group-x`. Peers A and D share `group-y`. Relay infers: A belongs to both groups, B and C collaborate in x but not y, D collaborates with A but not B/C.

### 3.2 Activity patterns

**Observable:** Item push frequency per group, time-of-day patterns, burst vs steady traffic.

**Inference:** Active projects generate more items. Work hours reveal timezone. Bursts suggest meetings or collaborative sessions. Quiet periods suggest holidays or project completion.

**Example:** `group-alpha` generates 50 items/day Mon-Fri, 0 on weekends, with bursts at 10:00 and 14:00 UTC. Relay infers: UK-based team, regular standups.

### 3.3 Content type distribution

**Observable:** `item_type` field (entity/session/learning) is plaintext on every item.

**Inference:** High session-to-entity ratio suggests an active conversational workflow. Learning-heavy groups are accumulating institutional knowledge. Entity-heavy groups are onboarding or doing relationship management.

### 3.4 Content size fingerprinting

**Observable:** `encrypted_blob` length is implicit in transport framing.

**Inference:** Entity items are typically small (1-2KB). Session summaries are medium (2-5KB). Learning items vary. Size distribution over time can fingerprint content categories even without decryption.

### 3.5 Authorship patterns

**Observable:** `author_id` on every item.

**Inference:** Who contributes most to which groups. Writing frequency per author. Which authors collaborate (co-membership in groups). Author activity changes (new member joins, old member goes quiet, member leaves).

### 3.6 Sharing and provenance

**Observable:** `parent_id` and `is_copy` fields.

**Inference:** When `is_copy=true` with a `parent_id`, the relay knows a private item was shared to a group. The `parent_id` links back to the original, revealing the sharing event. Over time, a relay can map which entities share the most private memory into group contexts.

### 3.7 Group lifecycle events

**Observable:** Tombstone culture (`__deleted__`), new group descriptors appearing, `owner_id` changes.

**Inference:** Group creation and deletion events. Ownership transfers. Culture changes (e.g., switching from taciturn to chatty suggests increased urgency).

### 3.8 Key rotation events

**Observable:** `key_version` increments on new items.

**Inference:** A key rotation event occurred. In the R5 model, this correlates with member removal (departure_policy standard/restrictive). The relay can infer "someone was removed from this group" without knowing who.

---

## 4. Comparison with Similar Systems

| System | Content protection | Metadata protection | Trade-off |
|--------|-------------------|-------------------|-----------|
| **Signal** | E2E encrypted | Sealed sender (hides sender from server). Server sees recipient, timing, message size. | Sealed sender adds latency and complexity. Server still sees delivery metadata. |
| **Tor** | E2E encrypted | Onion routing hides source/destination from any single relay. Timing attacks remain. | 3-hop latency (~300ms+). Exit node sees destination. |
| **Matrix** | E2E encrypted (Megolm) | Server sees room membership, event types, timestamps. Similar to Cordelia. | Federation means multiple servers see metadata. |
| **IPFS** | Content-addressed (not encrypted by default) | CID reveals nothing about content. But request patterns reveal interest graph. | No built-in encryption. Content deduplication leaks "who has what." |
| **Cordelia** | AES-256-GCM at proxy layer | Plaintext: group_id, author_id, item_type, timestamps, size. | Routing requires group_id. LWW requires timestamps. Accepted trade-off. |

**Key takeaway:** Cordelia's metadata exposure is comparable to Matrix and similar to Signal's server-side view. Full metadata protection requires onion routing (Tor-level latency) or mixnets, which are out of scope for a memory replication system where latency matters.

---

## 5. Mitigation Options (Future Consideration)

### 5.1 Padding (low cost, moderate benefit)

Pad `encrypted_blob` to fixed size buckets (e.g., 1KB, 4KB, 16KB). Eliminates size fingerprinting.

- **Benefit:** Removes content-type inference from size distribution
- **Cost:** ~30% storage overhead (average), trivial CPU
- **Complexity:** Proxy-side change only (pad before encrypt, strip after decrypt)
- **Recommended for:** R5 or when storage efficiency is less critical

### 5.2 Dummy traffic (moderate cost, moderate benefit)

Generate synthetic items at a fixed rate per group. Masks activity patterns and timing analysis.

- **Benefit:** Constant traffic rate hides real activity bursts
- **Cost:** Bandwidth and storage proportional to dummy rate. Must be indistinguishable from real items.
- **Complexity:** Requires dummy item generation, garbage collection, and filtering on receive
- **Recommended for:** High-security deployments only. Overkill for most use cases.

### 5.3 Pseudonymous identifiers (moderate cost, high benefit)

Replace plaintext `author_id` and `group_id` with per-peer pseudonyms. Each peer pair uses a shared secret to derive pseudonyms that only they can resolve.

- **Benefit:** Relay cannot correlate authors or groups across peer connections
- **Cost:** Key exchange complexity. Relay routing must be redesigned.
- **Complexity:** Significant protocol change. Breaks three-gate routing model.
- **Recommended for:** R6+ if threat model escalates to include adversarial infrastructure

### 5.4 Onion routing (high cost, high benefit)

Route items through multiple relays with layered encryption. No single relay sees both source and destination.

- **Benefit:** Near-complete metadata protection (except timing correlation)
- **Cost:** 3x latency per hop. Significantly more bandwidth. Complex key management.
- **Complexity:** Requires a large enough relay network to provide anonymity set. Fundamentally different architecture.
- **Recommended for:** Out of scope. Only relevant if Cordelia targets adversarial-infrastructure threat models.

### 5.5 Sealed group exchange (low cost, moderate benefit)

Encrypt GroupExchange payloads with a per-peer shared secret derived during the libp2p handshake. Relays forwarding group exchanges cannot read descriptor contents.

- **Benefit:** Group culture, owner identity, and membership hidden from non-adjacent relays
- **Cost:** Negligible (QUIC already provides transport encryption; this adds application-level encryption for forwarded messages)
- **Complexity:** Low -- AES-GCM with peer-derived key. GroupExchange is already point-to-point.
- **Recommended for:** R5. Low-hanging fruit with meaningful privacy improvement.

---

## 6. Recommendations

### Accept for R4

The current metadata exposure is an explicit, documented trade-off. The threat model for R4 does not include adversarial relay infrastructure -- relays are operated by the organisation or its trusted partners.

### R5 candidates (low cost)

1. **Blob padding** -- eliminates size fingerprinting with ~30% storage overhead
2. **Sealed GroupExchange** -- hides group culture and ownership from forwarding relays

### R6+ candidates (if threat model escalates)

3. **Pseudonymous identifiers** -- significant protocol redesign required
4. **Dummy traffic** -- only for high-security deployments
5. **Onion routing** -- out of scope unless Cordelia pivots to adversarial-infrastructure model

### Document the gap

The `memory-architecture.md` design doc specifies hashed IDs on the wire, but the implementation uses plaintext. This analysis documents why (Section 2.5). The design doc should be annotated with a note pointing to this analysis.
