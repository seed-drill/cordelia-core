# Memory Architecture: Three-Domain Model

*Design document for memory stratification. Addresses the dilution problem observed*
*in production: operational memories accumulating in L2 and drowning out foundational*
*frame memories that shape agent personality and reasoning quality.*

> "L1 value is not measured in bits of factual content. It is measured in how much
> it reduces the distance between the agent's starting position and the optimal
> position for the current task."
>
> -- Cordelia Whitepaper, Section 2.3

---

## Table of Contents

1. [Problem Statement](#1-problem-statement)
2. [Three Memory Domains](#2-three-memory-domains)
3. [Domain: Values](#3-domain-values)
4. [Domain: Procedural](#4-domain-procedural)
5. [Domain: Interrupt](#5-domain-interrupt)
6. [Mapping to Cache Hierarchy](#6-mapping-to-cache-hierarchy)
7. [Search and Vector Recovery](#7-search-and-vector-recovery)
8. [Novelty and Density](#8-novelty-and-density)
9. [Network Layer: Schema-Free Wire Protocol](#9-network-layer-schema-free-wire-protocol)
10. [Groups](#10-groups)
11. [Implementation Changes](#11-implementation-changes)
12. [Future Work](#12-future-work)

---

## 1. Problem Statement

At genesis, Cordelia's L1 hot context was primed with carefully curated frame
memories: key references (Dennett, Shannon, Minsky, Banks), reasoning style
preferences, and cultural touchstones. These created measurable KL divergence
reduction -- the agent's reasoning was biased toward first-principles thinking,
systems analysis, and the specific conceptual vocabulary of the team.

Over ~85 sessions, L2 warm storage accumulated hundreds of operational memories:
session summaries, build outcomes, bug fixes, deployment states. These are useful
for continuity but carry low frame value. The problem: prefetch and search treat
all L2 items equally. Operational memories now dominate context, diluting the
foundational frame memories that originally shaped agent behavior.

This is the **memory dilution problem**: undifferentiated accumulation degrades
the very quality that made early sessions effective.

The whitepaper distinguishes *frame memory* from *data memory* (Section 2.3).
Frame memories -- conceptual vocabulary, reasoning frameworks, shared metaphors --
are not measured in bits of information but in KL divergence reduction between the
agent's default reasoning distribution and the optimal distribution for the task.
A 50KB L1 with the right frame memories outperforms megabytes of raw history.

The current architecture has no mechanism to preserve this distinction at the
storage level.

---

## 2. Three Memory Domains

All memories, regardless of cache tier (L1/L2/L3), belong to one of three
semantic domains:

| Domain | Change Rate | Analogy | Cache Behavior |
|-----------|-------------|---------|----------------|
| **Values** | Very slow | Character, personality | Always resident |
| **Procedural** | Medium | Skills, learned patterns | TTL + compression |
| **Interrupt** | Fast | Current state, stack | Push/pop lifecycle |

These domains are orthogonal to the L1/L2/L3 cache hierarchy. A value can exist
in L1 (core identity) or L2 (extended narrative). A procedural learning lives in
L2 but may be compressed into L1 notes. An interrupt occupies L1 active state and
L2 session records.

The domain determines *how* a memory is managed. The cache tier determines
*where* it lives and *how fast* it is accessed.

---

## 3. Domain: Values

Values are the slowest-changing memories. They define who an entity is: their
reasoning frameworks, cultural references, aesthetic preferences, ethical
commitments, and the conceptual vocabulary they use to think.

### Role in Agent Personality

In humans, values form the stable substrate of personality. They bias perception,
shape reasoning, and determine which patterns feel salient. The same mechanism
operates in LLM agents through frame memory.

When an agent loads frame memories at session start -- Shannon's information
theory, Denning's working set model, the Culture's ethics of intervention -- it
does not merely learn facts. It **activates conceptual frameworks** that shift
attention weights and reshape the reasoning distribution. The coordinate system
is pre-loaded, enabling reasoning paths that would otherwise require multiple
conversational turns to establish.

This is why values are not just preferences to be stored. They are the mechanism
by which persistent memory improves agent performance on specific tasks. An agent
primed with game-theoretic frameworks will naturally frame cooperation problems
differently than one without. An agent carrying Shannon's entropy lens will
approach information problems with compression and channel capacity intuitions
already active.

The whitepaper's formal statement: frame memory value = KL divergence reduction
between default and task-optimal reasoning distributions. Values are the highest-
value frame memories because they apply across tasks, not just within one session.

### Storage Model

- **L1 core**: `identity.key_refs`, `identity.style`, `identity.heroes` --
  always loaded, never expire, manually curated.
- **L2 extended**: Learning items with `domain: "value"` -- principles, insights
  that proved durable, narrative history of how values were acquired or refined.
- **L3 archive**: Historical record of value evolution over time.

### Lifecycle

Values change rarely. When they do change, it is significant -- a shift in
worldview, a new framework adopted, an old one refined. The novelty analyzer
should flag value-level changes with high confidence, and they should be
persisted with no TTL (permanent until explicitly revised).

### Examples

- Key references: Dennett's competence-without-comprehension, Banks' Culture
  ethics, Shannon's information theory
- Reasoning style: first-principles, iterative, systems thinking
- Principles: entity sovereignty, privacy by default, cooperation as Nash
  equilibrium

---

## 4. Domain: Procedural

Procedural memories are "learning on the job" -- patterns extracted from
experience that improve future performance. They change at medium frequency:
acquired through work, refined through repetition, eventually compressed or
superseded.

### Storage Model

- **L1 summary**: High-value procedural knowledge compressed into
  `active.notes` or a dedicated `procedural` field -- e.g., "chain_hash must
  be recomputed on every patch operation".
- **L2 detail**: Learning items with `domain: "procedural"` -- full context,
  source session, confidence score.
- **L3 archive**: Superseded or low-access procedural memories.

### Lifecycle

Procedural memories have a natural lifecycle:

1. **Acquisition**: Learned during a session (e.g., "tsx requires env vars").
   Persisted to L2 with initial TTL based on novelty score.
2. **Reinforcement**: Accessed or validated in subsequent sessions. TTL extends.
   Confidence increases.
3. **Compression**: After repeated validation, the essential insight is promoted
   to L1 (compressed form) while L2 retains the detailed record.
4. **Supersession**: New learning replaces old. The old version moves to L3 or
   is dropped.

TTL is the primary lifecycle mechanism. High-novelty procedural memories get
longer TTL. Access refreshes TTL. Memories that are never accessed expire
naturally -- Darwinian selection through use.

### Examples

- "CORDELIA_STORAGE=sqlite must be set for smoke tests"
- "prefetch returns recency-biased results; search with explicit keywords for
  older items"
- "chain_hash must be recomputed on every L1 patch, not just replace"

---

## 5. Domain: Interrupt

Interrupt memories are current working state -- what is happening right now,
what was just happening, what needs attention next. They are the fastest-changing
domain, with stack-based semantics: context is pushed when a task begins and
popped when it completes.

### Storage Model

- **L1 active**: `active.focus`, `active.blockers`, `active.next`,
  `ephemeral.open_threads` -- the current interrupt stack.
- **L2 session**: Session summaries that capture completed interrupt frames.
  These are the "popped" records.
- **L3 archive**: Compressed session history.

### Lifecycle

1. **Push**: A new task or thread begins. Added to L1 `open_threads` and
   `active.focus`.
2. **Active**: Work proceeds. L2 session record accumulates context.
3. **Pop**: Task completes or is suspended. Removed from L1 active state.
   Session summary written to L2 with short TTL.
4. **Compress or drop**: If the session produced durable insights, those are
   extracted as procedural or value memories. The session record itself can
   expire.

Interrupt memories should have the shortest TTL. Most session details are only
relevant for a few sessions afterward. The valuable content gets extracted into
procedural learnings; the rest expires.

### Examples

- "Currently running E2E tests on vducdl94"
- "Chain hash fix deployed, verifying"
- "Launch sequence: website -> CF flip -> repos public -> announcements"

---

## 6. Mapping to Cache Hierarchy

The three domains distribute across cache tiers differently:

```
         L1 Hot        L2 Warm           L3 Cold
         (always)      (searchable)      (archive)
         ─────────     ──────────────    ──────────────
Values   key_refs      principles,       value evolution
         style         extended refs,    history
         heroes        narrative history
                       [no TTL]          [no TTL]

Proced.  notes         patterns,         superseded
         (compressed)  insights,         learnings
                       how-tos
                       [TTL: medium]     [TTL: long]

Interr.  focus         session records,  compressed
         blockers      recent state      session history
         open_threads
                       [TTL: short]      [TTL: medium]
```

### Prefetch Strategy

Current prefetch is recency-biased. The new model should be **domain-aware**:

1. **Always load**: All value-domain items from L2 (they are few and high-value)
2. **Relevance load**: Procedural items matching current project/context
3. **Recency load**: Most recent interrupt items (capped)

This ensures frame memories are always present regardless of how many operational
memories have accumulated.

---

## 7. Search and Vector Recovery

The domain model interacts with search in two ways:

### Keyword Search (FTS5)

Domain should be a filterable field. Queries like "search for principles" should
return value-domain items; "what did I do last week" should return interrupt-
domain session records. The `domain` field on L2 items enables this directly.

### Vector Similarity Search

Vector embeddings capture semantic proximity. When searching for conceptually
related memories, the domain provides a useful boost signal:

- A search during architectural discussion should weight value-domain items
  higher (frameworks, principles apply).
- A search during debugging should weight procedural items higher (specific
  learned patterns).
- A search for "what was I working on" should weight interrupt items.

This is not hard filtering -- a procedural memory may be relevant to an
architectural discussion. But domain-aware boosting improves result quality
by encoding the intuition that different types of memory matter in different
contexts.

### Vector Recovery on Startup

When the proxy restarts, the in-memory embedding cache (L0) is lost. The
current approach regenerates embeddings on demand via Ollama. The domain model
suggests a priority order for regeneration:

1. Value-domain items first (always needed, small count)
2. Procedural items for current project context
3. Recent interrupt items

This ensures the most impactful memories are searchable fastest after restart.

---

## 8. Novelty and Density

The existing novelty analyzer (Section 2.4 of whitepaper) determines *whether*
to persist a memory. The domain model extends this to determine *how* to persist
it:

### Domain Classification

The nine novelty signal types map to domains:

| Signal | Domain |
|--------|--------|
| `correction` | Procedural |
| `preference` | Values |
| `entity_new` | Values (new entity profile) |
| `decision` | Interrupt (current) -> Procedural (if pattern) |
| `insight` | Values (if fundamental) or Procedural (if tactical) |
| `blocker` | Interrupt |
| `reference` | Values |
| `working_pattern` | Procedural |
| `meta_learning` | Values or Procedural |

Some signals require judgment to classify (insight, meta_learning). Confidence
score and content analysis can inform this -- a high-confidence insight about
system architecture is likely a value; a medium-confidence insight about a
specific API quirk is procedural.

### Density-Based TTL

The reconstitution principle (novelty = conditional entropy H(M|C)) provides
the right metric for TTL assignment:

- **High H(M|C)**: Memory cannot be reconstituted from the rest of the corpus.
  Long TTL or permanent.
- **Low H(M|C)**: Memory is largely redundant given other memories. Short TTL.

Combined with domain:
- Value + high density = permanent (no TTL)
- Procedural + high density = long TTL
- Procedural + low density = short TTL (may be superseded)
- Interrupt + any density = short TTL (extract insights, let record expire)

---

## 9. Network Layer: Schema-Free Wire Protocol

### Design Principle: The Network is a Dumb Pipe

The three-domain model, TTL, novelty scores, and all classification metadata
are **edge concerns**. They exist in the proxy's local SQLite index. They never
appear on the wire.

This follows the TCP/IP layering principle: the network is agnostic to what
memory structures entities encode. Just as IP does not inspect packet payloads,
the Rust P2P layer does not interpret memory content. This separation is not
just architectural hygiene -- it is a security requirement and an extensibility
requirement.

### Why Schema Must Stay at the Edge

**Entity sovereignty -- structural, not just content**: The whitepaper
establishes entity sovereignty as axiomatic: no group policy, peer, or
administrator can force content into sovereign memory (Section 3.1). But
sovereignty over content is incomplete without sovereignty over structure.

A network-imposed schema dictates how entities must internally represent
their knowledge -- which categories they use, what fields they populate, how
they organise thought. This is a form of cognitive coercion. It is the
difference between a shared language for exchange (the wire envelope) and a
mandated internal structure for thought (a typed schema).

In human terms: a community can agree on a postal system (addressing, envelope
size, delivery routes) without dictating what language the letters are written
in, or whether the contents are prose, poetry, mathematics, or drawings. The
postal system works precisely because it is agnostic to content structure.

A schema-free wire protocol is not a convenience -- it is a **necessary
consequence of entity sovereignty**. If the network mandates memory types
(entity, session, learning) or domains (value, procedural, interrupt), it
constrains how entities are permitted to think. An entity that develops a
novel memory organisation -- graph-based, narrative-based, or something we
haven't imagined -- would be forced to shoehorn its cognition into the
network's categories or be unable to participate.

This principle should be treated as a corollary to the sovereignty axiom:

> **Structural Sovereignty**: An entity has exclusive control over the
> internal representation of its memories. The network transports opaque
> encrypted content and makes no assumptions about its structure. Schema
> interpretation is exclusively an edge concern.

**Security**: An encrypted blob with six metadata fields is auditable. A rich
wire schema with types, domains, TTLs, nested structures, and version
negotiation expands the attack surface and makes formal security analysis
intractable. We cannot prove unconditional security for a complex wire format.
The Signal model works because the server sees only opaque ciphertext and
minimal routing metadata. We follow the same principle.

Complexity and provable security are adversaries. Every field the network
interprets is a field that must be validated, versioned, and defended against
malformed input. The minimal envelope is not just simpler to secure -- it is
the only approach where formal security analysis remains tractable as the
system grows.

**Extensibility**: Entities will find uses for memory structures we did not
anticipate. If the wire protocol encodes a schema, every new memory type
requires protocol negotiation. If the wire protocol carries opaque blobs,
entities can independently evolve their memory formats. Different proxy versions
can coexist. A research group can encode graph structures while a development
team encodes session records -- the network doesn't care.

**Schema evolution**: When the proxy adds a `domain` field, or changes the
learning subtypes, or introduces a new memory type entirely, this is a local
change. No network upgrade, no version negotiation, no backwards-compatibility
shims. The blob format is self-describing (encrypted JSON with a version field
inside the ciphertext). The decrypting proxy knows how to parse it.

### Wire Protocol: Minimal Envelope

The network carries exactly this:

```
┌─────────────────────────────────────────────┐
│  Wire Envelope (plaintext metadata)         │
│                                             │
│  item_id       : opaque GUID               │
│  group_id      : SHA-256(group URI)         │
│  author_id     : SHA-256(Ed25519 pubkey)    │
│  timestamp     : uint64 (Unix ms)          │
│  content_hash  : SHA-256(encrypted_blob)    │
│  encrypted_blob: opaque bytes               │
│                                             │
└─────────────────────────────────────────────┘
```

Six fields. No type, no domain, no schema version, no TTL. The blob is opaque
to every node except the entity (or group members) that hold the decryption key.

### Layer Responsibilities

```
Layer            Responsibility              Knows about schema?
─────────────    ─────────────────────────   ───────────────────
Rust P2P         Move blobs between peers    No
                 Peer discovery, NAT, QUIC

Culture/Repl.    Replication policy           No (operates on
                 (chatty/moderate/taciturn)   envelope metadata
                 Consistency guarantees       only)

Proxy (edge)     Decrypt, parse, classify     Yes -- full schema
                 Domain tagging               awareness
                 TTL, prefetch, search
                 Novelty analysis
                 Agent-facing MCP API
```

### Implication for Domain Model

The `domain` field (value/procedural/interrupt) is purely local metadata.
When the proxy decrypts an incoming blob, it classifies it and stores the
domain tag in its local index. Different proxies could classify the same blob
differently -- that's fine, domain is an edge-side optimization for retrieval.

This means:
- **No domain field in the wire protocol** or the encrypted payload
- **Domain is inferred at the edge** from content analysis when items are
  received, or set explicitly when items are created locally
- **TTL is local policy**, not a network attribute. A chatty group replicates
  all items; the receiving proxy decides how long to keep them
- **Prefetch priority is local**. The proxy knows its own domain classification
  and applies the values-first strategy locally

### Consequence for Group Culture

Culture governs replication behavior at the network layer, but only using
envelope metadata. A chatty group eagerly pushes all new items. A moderate
group notifies and lets peers pull. A taciturn group relies on anti-entropy
sync.

Culture does NOT govern per-domain replication. That would require the network
to understand domains, violating the schema-free principle. Instead, domain-
aware filtering happens at the edge:

- The proxy can choose not to cache interrupt-domain items from a group it
  is only passively monitoring
- The proxy can prioritise fetching value-domain items first when catching up
  after offline
- These are local decisions invisible to the network

---

## 10. Groups

Groups use the same three-domain model. Knowledge flows between personal and
group memory through the existing COW (copy-on-write) sharing mechanism:

- **Values** shared to group = team principles, shared vocabulary, culture
- **Procedural** shared to group = team knowledge base, how-tos, patterns
- **Interrupt** shared to group = team awareness, coordination state

Group culture (chatty/moderate/taciturn) governs replication of each domain:
- Values: typically `moderate` -- notify on change, members fetch
- Procedural: typically `chatty` -- push useful patterns to all members
- Interrupt: typically `taciturn` -- only relevant to active collaborators

Knowledge can be **harvested** from group into personal memory (e.g., a team
pattern becomes a personal procedural memory) and **shared** from personal into
group (e.g., an individual insight becomes team knowledge). The domain tag
travels with the memory.

### Group Lifecycle Policy

Groups are sovereign over their own information handling. Domain classification
is metadata on group items (useful for search, display, prefetch ordering) but
does **not** govern lifecycle. The lifecycle policy split:

- **Private items**: Domain governs lifecycle. Value = permanent. Procedural =
  usage-based cap eviction. Interrupt = 3-day TTL refreshed on access.
- **Group items**: Group culture policy is the **sole** TTL source. If
  `culture.ttl_default` is set, items expire after that period (refreshed on
  access). If not set, items have no expiry. Procedural cap eviction does not
  apply to group items.

This separation is clean: `ttl_expires_at` is set at write time from the
correct policy source, and the sweep checks the column without caring why it
was set. An entity participating in a group implicitly agrees to abide by the
group's handling rules. Defection (e.g., retaining items the group has
expired) cannot be prevented at the protocol level but can be detected and
addressed by evicting the entity and recycling group keys.

**Deferred**: Automated harvesting, domain-aware group replication, and richer
policy definition via Secret Keeper are future work. The current implementation
supports manual sharing with domain tags as metadata and static culture-based
lifecycle.

---

## 11. Implementation Changes

### Phase 1: Schema and Classification

1. **Add `domain` field to L2 items**: `"value" | "procedural" | "interrupt"`.
   New column in SQLite, indexed. Default: `"procedural"` for learnings,
   `"interrupt"` for sessions, classification required for entities.

2. **Backfill existing items**: One-time migration to classify ~existing L2
   items by domain. Use content analysis: items referencing frameworks,
   principles, or key_refs -> value; items referencing specific sessions,
   builds, bugs -> interrupt or procedural.

3. **Update novelty analyzer**: When persisting a new memory, classify its
   domain based on signal type (see mapping in Section 8). Add domain to
   the persistence payload.

### Phase 2: TTL and Lifecycle

4. **Add `ttl` and `last_accessed` fields to L2 items**. TTL set at write
   time based on domain + novelty score. Access refreshes `last_accessed`.

5. **Expiry sweep**: Periodic check (on session start or prefetch) that
   archives or deletes items past TTL. Value-domain items exempt.

6. **Interrupt stack semantics**: When an open_thread is resolved in L1,
   mark corresponding L2 session records for short TTL expiry.

### Phase 3: Domain-Aware Retrieval

7. **Update prefetch**: Always include value-domain items. Then procedural
   by relevance. Then interrupt by recency. Cap total items.

8. **Update search**: Add domain as optional filter. Apply domain-aware
   boosting to vector similarity scores.

9. **Vector recovery priority**: On startup, regenerate embeddings in
   domain priority order: values -> procedural -> interrupt.

---

## 12. Future Work

### Automatic Compression

Procedural memories that have been reinforced across multiple sessions could be
automatically compressed: extract the essential insight, promote to L1 notes,
archive the detailed records. This is analogous to memory consolidation in
biological systems -- episodic memories become semantic knowledge.

### Value Drift Detection

Track changes to value-domain memories over time. If key_refs shift, if
reasoning style preferences evolve, if principles are revised -- this is
significant and worth surfacing. A "value evolution" view could show how an
entity's foundational frameworks have changed.

### Cross-Domain Promotion

Some memories naturally migrate between domains. A specific bug fix (interrupt)
reveals a general pattern (procedural) which reveals an architectural principle
(value). Detecting these promotion paths automatically -- perhaps via repeated
access patterns or explicit user signals -- would reduce manual curation.

### Density Metrics (R4-020)

Entropy-based scoring of memory density at the corpus level. How much of
the memory store is high-value vs redundant? What is the effective information
density? This enables automated compaction: when density drops below threshold,
trigger compression of low-value items.

### Personality Calibration

With explicit value-domain tagging, it becomes possible to A/B test the impact
of different frame memory configurations on agent performance. Load different
value sets, measure reasoning quality on standardized tasks. This opens the
door to empirical personality tuning -- not just for individual agents but for
role-specific configurations (e.g., a "CPO mode" value set vs a "CTO mode"
value set that biases toward different reasoning frameworks).

### Group Knowledge Harvesting

Automated detection of when personal learnings should be proposed for group
sharing, and when group knowledge should be absorbed into personal memory.
The domain tag provides the key signal: value-domain group memories are
candidates for personal absorption (team culture becoming individual values);
high-confidence procedural memories are candidates for group sharing.

### Emotional and Motivational State

The interrupt domain currently tracks task state but not emotional or
motivational context. In humans, mood and motivation are fast-changing states
that significantly affect reasoning and decision quality. Tracking these as
interrupt-domain memories -- even coarsely -- could improve agent self-awareness
and response calibration.

### Vector Embedding Specialization

Different domains may benefit from different embedding strategies. Value memories
are conceptually dense and benefit from models trained on academic/philosophical
text. Procedural memories are technically specific and benefit from code-aware
embeddings. Interrupt memories are contextual and benefit from recency-weighted
similarity. Multi-model embedding with domain-aware selection is a natural
extension.

### Independent Schema Evolution

Because the wire protocol carries opaque blobs, each entity can upgrade its
local memory schema independently. An entity that adopts a new domain
taxonomy, adds fields, restructures its L2 index, or switches from JSON to
CBOR inside the encrypted blob does so without affecting any peer. The
network continues to replicate blobs unchanged. Group members running
different proxy versions interoperate seamlessly -- each decrypts and
interprets according to its own schema version.

This eliminates coordinated upgrade requirements. In a distributed system
with heterogeneous entities (different AI providers, different agent
frameworks, different use cases), mandating simultaneous schema upgrades is
operationally impossible. Schema-at-edge makes it unnecessary.

---

## 13. Document Updates Required

The structural sovereignty principle and schema-free wire protocol have
implications for other Cordelia documents. These updates should be made to
maintain consistency across the design.

### WHITEPAPER.md

**Section 3.1 (Entity)**: Extend the sovereignty axiom to cover structural
sovereignty explicitly. Current text covers content sovereignty ("no one can
force content into sovereign memory"). Add: no one can dictate the internal
representation of that content. Schema is a sovereign concern.

**Section 5.4 (Wire Protocol)**: Currently specifies "JSON payload" for wire
messages, which implies schema awareness at the network layer. Should be
revised to specify opaque encrypted blobs with a minimal plaintext envelope.
The five mini-protocols (handshake, keep-alive, peer-share, memory-sync,
memory-fetch) should operate on envelope metadata only. JSON structure, if
used, should be inside the encrypted blob and interpreted only at the edge.

**Section 2.3 (Frame Memory vs Data Memory)**: Add a note that the three-
domain model (values, procedural, interrupt) is the implementation of the
frame/data distinction. Values = frame memory. Procedural + interrupt = data
memory. The domain classification enables the system to preserve frame memory
priority as operational data accumulates.

### R2-006-group-model.md

**Constraint 1.1 (Entity Security Primacy)**: Add structural sovereignty as
a sub-principle. Entity policy overrides group policy for content AND
structure. A group cannot mandate memory types or schema versions.

**Section 2 (Schema)**: Clarify that the SQL schema is edge-local. The
`l2_items` table structure, column names, and data types are proxy
implementation details. They do not appear on the wire and are not shared
with peers.

### R3-decentralisation-pivot.md

**Replication Protocol section**: Note that culture-governed replication
operates on envelope metadata only. The replication layer does not inspect
or validate blob content. Add reference to structural sovereignty principle.

**Wire Protocol Mapping (Coutts table)**: Note that "Memory-Sync" and
"Memory-Fetch" mini-protocols exchange envelope metadata and opaque blobs
respectively. No schema negotiation beyond protocol version in handshake.

---

*Version: 0.2 (draft)*
*Last updated: 2026-02-01*
*Authors: Russell Wing, Claude (Cordelia session 86)*
