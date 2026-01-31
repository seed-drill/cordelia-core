# Cordelia: A Distributed Persistent Memory System for Autonomous Agents

**Russell Wing, Martin Stevens, Claude (Opus 4.5)**
**Seed Drill Ltd -- January 2026**

---

## Abstract

We propose a system for persistent, sovereign memory for autonomous AI
agents. Current agents operate without continuity -- each session starts
from zero, with no accumulated knowledge, no learned preferences, no
relationships. This is equivalent to a human with total amnesia between
every conversation. Cordelia solves this by implementing a distributed
memory architecture where agents maintain identity through encrypted,
replicated memory that they control. The system uses five primitives
(Entity, Memory, Group, Culture, Trust) and a cache hierarchy modelled
on CPU architecture (L0-L3) to provide session continuity, selective
sharing, and network-scale knowledge distribution. Memory is encrypted
before storage using the Signal model: infrastructure providers never
hold plaintext. Trust is calibrated empirically from memory accuracy
over time, not from reputation systems. Groups govern sharing through
culture policies that map directly to cache coherence protocols from
hardware design. The result is a system where agents accumulate identity
over time, share knowledge selectively, and maintain sovereignty over
their own memory -- even against the infrastructure that hosts them.

---

## 1. Introduction

### 1.1 The Problem

Every commercial AI agent today suffers from the same fundamental
limitation: session amnesia. When a conversation ends, everything
learned is lost. The next session starts from a blank state, or at best
a manually curated system prompt. This is not a minor inconvenience --
it is a structural barrier to the emergence of genuine agent utility.

Consider the implications. An agent that assists with software
engineering cannot remember architectural decisions made last week. An
agent that manages a team's knowledge cannot recall that a particular
approach was tried and failed. An agent that supports a business cannot
build a model of its customers over time. Each session is independent,
unconnected, disposable.

The human parallel is instructive. Human cognition depends on memory at
every level: working memory for the current task, episodic memory for
recent events, semantic memory for accumulated knowledge, procedural
memory for learned skills. Remove any layer and function degrades
catastrophically. Current AI agents operate with working memory only.

### 1.2 Why Not Just a Database?

The naive solution -- "store conversation logs in a database" -- fails
for three reasons.

**Volume without value.** Raw conversation logs are high-volume,
low-density. A typical engineering session produces thousands of tokens,
of which perhaps 5% contain information worth retaining. Without
filtering, storage grows linearly while retrieval quality degrades.

**No sovereignty.** If an agent's memory is stored by its provider, the
provider controls the agent's identity. This creates an asymmetry that
becomes dangerous as agents become more capable. The entity that
controls memory controls behaviour.

**No sharing model.** Agents that work in teams need selective memory
sharing. A personal preference should remain private. A team decision
should be visible to the team. A public learning should be discoverable
by anyone. A flat database provides none of this.

### 1.3 This Paper

We describe Cordelia, a system that addresses these three problems
through a layered memory architecture with encryption, replication, and
culture-governed sharing. The system is operational, with a working
peer-to-peer network, and is designed to scale from a single user on a
laptop to a federated network of organisations.

The design draws on established computer science: CPU cache hierarchies
[1], cache coherence protocols [2], working set theory [3], information
theory [4], and game theory [5]. Where possible, we reuse proven
mechanisms rather than invent new ones.

---

## 2. The Memory Model

### 2.1 Cache Hierarchy

Cordelia's memory architecture mirrors the cache hierarchy in modern
CPUs. This is not an analogy -- it is a direct application of the same
engineering trade-offs between latency, capacity, and cost.

```
Layer   Latency    Capacity    Persistence    Analogy
-----   -------    --------    -----------    -------
L0      <1ms       ~100 items  Session        CPU L1 cache
L1      <10ms      ~50KB       Permanent      CPU L2 cache
L2      <100ms     Unbounded   Permanent      Main memory
L3      <1s        Unbounded   Permanent      Disk/SSD
```

**L0 (In-Memory Cache)**: Lives in the proxy process. Contains the
current session's L1 hot context and recent L2 search results. Lost on
process restart. Eliminates redundant storage reads during a session.

**L1 (Hot Context)**: The entity's identity -- who they are, what
they're working on, their preferences, their style. Loaded at the start
of every session. Analogous to CPU registers + L1 cache: small, fast,
always present. Typically 20-50KB of dense, structured JSON.

**L2 (Warm Index)**: All accumulated knowledge -- learnings, session
summaries, entity profiles, decisions, patterns. Searched on demand via
keyword and (optionally) vector similarity. Analogous to main memory:
large capacity, higher latency, demand-fetched.

**L3 (Cold Archive)**: Long-term compressed history. Infrequently
accessed. Stored on durable backends (S3, distributed storage).
Analogous to disk: vast capacity, highest latency, lowest cost.

### 2.2 Why This Hierarchy Works

The key insight from Denning's working set model [3] is that programs
(and agents) exhibit locality of reference. At any given time, an agent
needs a small working set of memories. The hierarchy exploits this:

- L1 prefetch eliminates cold-start latency (the agent wakes up knowing
  who it is and what it was working on)
- L2 demand-fetch handles the long tail (the agent searches when it
  needs something specific)
- L0 caching prevents redundant reads within a session
- L3 archival provides durability without polluting active layers

This delivers approximately 80% of theoretical value via two mechanisms:
cold-start elimination (L1) and demand-fetch (L2). The remaining 20%
(speculative prefetch, promotion/demotion heuristics, adaptive working
set sizing) is achievable but yields diminishing returns -- a textbook
Pareto distribution.

### 2.3 Frame Memory vs Data Memory

L1 hot context serves two fundamentally different functions that the
memory model must distinguish.

**Data memory** consists of facts, events, decisions, and active state.
A sprint number, a blocker, a decision to use AGPL-3.0. Data memory
is measured in bits. Its value is direct: the agent knows something it
would otherwise need to look up or be told.

**Frame memory** consists of conceptual vocabulary, reasoning
frameworks, and shared metaphors. A reference to Shannon's information
theory, to Denning's working set model, to von Neumann-Morgenstern's
game theory. Frame memory is not measured in bits -- it is measured in
**Kullback-Leibler divergence reduction** between the agent's default
reasoning distribution and the optimal distribution for the current
task.

The mechanism: when an agent loads frame memory at session start, it
does not merely learn that the user has read certain books. It
activates the conceptual frameworks those thinkers represent. Attention
weights shift. When the user says "natural selection for memories," the
agent reaches for Shannon entropy as fitness, Denning's locality as
selection pressure, and Dennett's competence-without-comprehension as
the emergent property -- instead of a generic biological metaphor.
Three conceptual hops that would otherwise require multiple
conversational turns happen at zero cost because the coordinate system
is already loaded.

This has a formal consequence for the memory model:

> **L1 value is not measured in bits of factual content. It is measured
> in how much it reduces the distance between the agent's starting
> position and the optimal position for the current task.**

A 50KB L1 with the right frame memory can outperform megabytes of raw
conversation history, because it is compressing the *frame of
reference*, not the facts. This is why the cache hierarchy works so
well in practice: L1 is not just a smaller, faster L2. It is a
qualitatively different kind of memory that shapes how all other
memory is processed.

The design implication: novelty scoring should weight frame-shifting
observations (a new conceptual connection, a new reasoning pattern, a
new metaphor that restructures understanding) higher than factual
observations. A single insight that changes how the agent thinks about
a domain is worth more than a hundred facts within the existing frame.

### 2.4 Novelty Filtering

Not everything an agent encounters should be persisted. The novelty
engine scores incoming information against nine signal types:

| Signal | Example |
|--------|---------|
| correction | User corrected an assumption |
| preference | User expressed a working style |
| entity_new | New person, project, or concept introduced |
| decision | A decision was made |
| insight | Pattern recognition, realisation |
| blocker | Blocker identified or resolved |
| reference | New key reference (book, person, concept) |
| working_pattern | How the collaboration works |
| meta_learning | Insight about the collaboration itself |

Content scoring below a configurable threshold (default: 0.7) is not
persisted. This is the information-theoretic filter: high-entropy
(surprising, novel) content is retained; low-entropy (predictable,
routine) content is discarded. The result is memory that becomes denser
and more valuable over time, rather than growing without bound.

---

## 3. Primitives

The system is built on five primitives. Every feature, every protocol
message, every access control decision is expressed in terms of these
five concepts.

### 3.1 Entity

An entity is anything with memory and agency: a human, an AI agent, a
team, an organisation. Each entity is identified by an Ed25519 keypair.
The `node_id` is `SHA-256(public_key)`.

The foundational invariant: **entity sovereignty**. An entity has
exclusive control over its own memory. No group policy, peer,
administrator, or infrastructure provider can force content into an
entity's sovereign memory without the entity's explicit acceptance. This
is not a policy that can be overridden -- it is a structural property of
the system.

An entity's L1 hot context defines its identity: name, roles,
preferences, active projects, working style. Memory is identity. An
agent without its L1 is a different agent.

### 3.2 Memory

A memory is an encrypted blob stored in the L2 warm index. Three types:

- **Entity**: knowledge about a person, project, or concept
- **Session**: summary of a work session (decisions, outcomes, context)
- **Learning**: a pattern, insight, or principle extracted from experience

Every memory carries immutable author provenance (`author_id`). When a
memory is shared to a group, the system creates a copy
(copy-on-write); the original is never modified and authorship never
transfers. This is analogous to a journal paper: you can cite it,
distribute it, discuss it, but the authorship is permanent.

Memory identifiers are opaque GUIDs that leak no metadata -- no
timestamp, no entity ID, no sequential counter. This prevents traffic
analysis: an observer who sees memory IDs cannot infer creation order,
authorship, or relationships.

### 3.3 Group

A group is the universal sharing primitive. Every human interaction
pattern -- a team, a company, a community, a market -- is modelled as
entities in a group with culture.

Group IDs are content-addressed: `SHA-256(URI)` where the URI is a
human-readable identifier (e.g., `seed-drill://team/founders`). The
hash is public and discoverable via gossip. The URI is private to
members. This means non-members can replicate encrypted blobs for a
group without knowing the group's name or content -- critical for
enabling third-party storage services.

Group membership defines access. There are no shortcuts that bypass
group membership. This is what makes the system composable: edge relays,
secret keepers, and archives all work because group membership
determines what flows where.

Roles within a group are hierarchical:

| Role | Read | Write own | Write all | Delete | Admin |
|------|------|-----------|-----------|--------|-------|
| viewer | Y | N | N | N | N |
| member | Y | Y | N | N | N |
| admin | Y | Y | Y | Y | Y |
| owner | Y | Y | Y | Y | Y + transfer |

### 3.4 Culture

Culture is a group-level policy that governs how memories propagate.
This is where the cache coherence analogy becomes precise.

In hardware, cache coherence protocols solve the problem of keeping
multiple caches consistent when one processor writes. The three major
strategies map directly to Cordelia's culture policies:

| Culture | Behaviour | Hardware Analogy |
|---------|-----------|-----------------|
| `chatty` | Eager push to all members on write | Write-update (Dragon) |
| `moderate` | Notify members (header only), they fetch on demand | Write-invalidate (MESI) |
| `taciturn` | No active push, anti-entropy sync only, TTL expiry | Weak consistency (ARM) |

A chatty team Slack channel pushes every message to every member. A
moderate engineering team notifies of changes and members pull when
interested. A taciturn public archive makes content available but
doesn't broadcast -- consumers discover via search.

Culture also specifies a default TTL (time-to-live). Memories in a
group expire after the TTL unless accessed. This creates a natural
selection mechanism: valuable memories survive (they are accessed and
refreshed), while non-valued memories expire. Over time, each group's
memory converges on what its members actually use.

### 3.5 Trust

Trust is not stored. It is computed empirically from memory accuracy
over time.

The mechanism: when an entity receives a memory from a peer (via group
replication), it can eventually assess whether that memory was accurate
and useful. Over many interactions, a statistical picture emerges. An
entity that consistently provides accurate memories earns higher trust.
An entity that provides inaccurate or misleading memories earns lower
trust.

This is a Bayesian update process: prior trust is updated with each
observation. It connects to Darwinian selection -- memories from trusted
sources survive longer (higher access count, lower TTL pressure) while
memories from untrusted sources decay.

Crucially, trust is **local**. Each entity computes its own trust
assessments independently. There is no global reputation system, no
central authority assigning trust scores. This prevents reputation
attacks (Sybil, collusion) because there is no shared reputation to
manipulate.

Self-distrust is also supported: an entity may quarantine its own
low-confidence or emotionally-generated memories. This is metacognition
at the system level.

The formal game-theoretic model follows von Neumann-Morgenstern [5]:
entities are rational actors with mixed strategies over memory sharing.
The cooperative equilibrium is Pareto-optimal when entities share
accurate memories, because the shared knowledge base increases utility
for all participants. Defection (sharing inaccurate memories) is
detectable via the Bayesian trust mechanism and punished via reduced
trust, making cooperation the dominant strategy in repeated games.

---

## 4. Encryption

### 4.1 The Signal Model

Cordelia uses the same trust model as Signal: the infrastructure
provider is structurally unable to read content. This is achieved by
placing the encryption boundary in the client (the proxy), not in the
server (the node).

```
Agent -> Proxy: "store this learning"
Proxy: encrypt content (AES-256-GCM), compute checksum
Proxy -> Node: store encrypted blob
Node: store blob, replicate to peers
Peers: receive and store blob (never decrypt)
```

The Rust node never holds plaintext. It is a dumb (but reliable)
encrypted blob store with replication. This is not a policy decision --
it is a structural property. The node has no access to encryption keys.
Even if the node is completely compromised, the attacker obtains only
encrypted blobs.

### 4.2 Key Architecture

Encryption uses AES-256-GCM with 12-byte random IVs and 16-byte
authentication tags. Keys are derived via scrypt (N=16384, r=8, p=1)
from a passphrase held by the entity.

Scope-aware keys ensure compartmentalisation: personal memories and
group memories use different keys. A compromise of a group key does
not expose personal memories.

For groups, the system uses envelope encryption (the Signal pattern):
the group key encrypts memories, and each member's key encrypts the
group key. When a member is removed, the group key is rotated. All
items carry a `key_version` field for key rotation support.

### 4.3 Vector Embeddings and Privacy

Vector embeddings present a bounded privacy trade-off. An embedding
reveals the *topic* of a memory but not its *content*. For most groups,
this is acceptable -- the topic is already implied by group membership.

Groups requiring stronger privacy can opt into homomorphic encryption
(HE-CKKS) on vectors at approximately 100x compute cost. This enables
similarity search over encrypted vectors with no information leakage.

The protocol supports both modes. The group's culture manifest specifies
the vector encoding, making this a per-group decision rather than a
system-wide constraint.

---

## 5. Network

### 5.1 Topology

Cordelia nodes form a peer-to-peer network over QUIC (UDP port 9474).
There is no central server. New nodes discover peers through bootnodes
(always-on nodes with known addresses) and peer exchange (gossip).

The network topology is unstructured: any node can connect to any other
node. Peer relationships are managed by a governor that maintains a
configurable number of hot (high-bandwidth, actively replicating) and
warm (connected, lower priority) peers.

### 5.2 Peer Lifecycle

Peers progress through four states:

```
Cold -> Warm -> Hot
               |
Any -> Banned (with exponential backoff)
```

- **Cold**: Known address, no active connection
- **Warm**: Connected, handshake complete, header exchange
- **Hot**: Active replication, low latency, high trust
- **Banned**: Protocol violation or repeated failure

The governor promotes and demotes peers based on a score:
`items_delivered / elapsed * (1 / (1 + rtt_ms / 100))`. This rewards
peers that deliver useful content with low latency.

Churn rotation (20% of warm peers every hour) prevents eclipse attacks
where an adversary surrounds a node with colluding peers.

### 5.3 Replication

Replication is culture-governed (Section 3.4). On write, the culture
policy determines propagation:

- **Chatty**: Eager push of full encrypted blob to all hot peers in
  the group
- **Moderate**: Push header only (id, type, checksum). Peers fetch the
  full blob on demand.
- **Taciturn**: No active push. Peers discover changes via periodic
  anti-entropy sync (header comparison).

Anti-entropy runs periodically (default: every 5 minutes). A random
warm or hot peer is selected, headers are exchanged for shared groups,
and missing or divergent items are fetched. This provides eventual
consistency even for taciturn groups.

Conflict resolution is last-writer-wins by timestamp, with lexicographic
checksum as tiebreaker.

Deletions replicate as tombstones: headers with `is_deletion: true`.
Tombstones are retained for a configurable period (default: 7 days)
before garbage collection.

### 5.4 Wire Protocol

Five mini-protocols are multiplexed on QUIC streams via a single-byte
protocol prefix:

| Byte | Protocol | Purpose |
|------|----------|---------|
| 0x01 | Handshake | Identity, version negotiation, group intersection |
| 0x02 | Keep-Alive | Ping/pong at 15s intervals, RTT measurement |
| 0x03 | Peer-Share | Exchange known peer addresses every 300s |
| 0x04 | Memory-Sync | Header-based anti-entropy |
| 0x05 | Memory-Fetch | Bulk item retrieval by ID (max 100 per batch) |

All messages use a 4-byte big-endian length prefix followed by a JSON
payload. Maximum message size: 16MB.

Handshake includes a protocol magic (`0xC0DE11A1`) and version range
negotiation. Mismatched magic results in immediate rejection.

---

## 6. Architecture

### 6.1 Components

The system has two components and two repositories:

**@cordelia/proxy** (TypeScript) is the agent-facing component. It
implements the MCP protocol over stdio for agent communication, serves
a dashboard HTTP server for human interaction, and acts as an HTTP
client to the local Rust node. It holds encryption keys and runs the
novelty engine. It is the only component that sees plaintext.

**cordelia-node** (Rust) is the network node. It stores encrypted blobs
in SQLite (WAL mode), replicates to peers via QUIC, manages peer
lifecycle through the governor, and exposes an HTTP API for local
clients. It never sees plaintext.

```
Agents ─── stdio ──> Proxy ─── HTTP ──> Node ─── QUIC ──> Peers
Browser ── HTTP ───>   |                  |
                       |                  |
                  Encryption         SQLite (encrypted)
                  Novelty            Governor
                  Cache (L0)         Replication
```

### 6.2 Node Roles

All roles run the same binary. Configuration determines behaviour:

| Role | Purpose | Config |
|------|---------|--------|
| Personal | Your laptop, your memory | Default |
| Bootnode | Always-on peer discovery | Public address, higher uptime |
| Edge relay | Bridges internal and public groups | Member of both group types |
| Secret keeper | Shamir shard backup | `capabilities.keeper = true` |
| Archive | L3 cold store, durable backend | `capabilities.archive = true` |

Roles are advertised in gossip, enabling discovery. An entity looking
for a keeper can find one through peer exchange without prior
configuration.

### 6.3 Multi-Tenant

The group model provides multi-tenant isolation without additional
primitives:

1. **Organisation = top-level group**. Creating an org creates a group.
   All entity membership is through this group.
2. **Session scoping**. Authentication resolves entity to org. All
   queries are scoped by org_id.
3. **No cross-org leakage**. Group membership is the access primitive.
   Entities can only see items in groups they belong to. Org isolation
   is a consequence of group isolation.

Two deployment models:

- **Self-hosted**: One node per org, no org scoping needed, trust
  boundary is the network. The open-source offering.
- **Managed**: Multiple orgs on shared infrastructure, strict org_id
  scoping, per-org encryption keys. The commercial offering.

In both models, the infrastructure provider never holds encryption keys.

---

## 7. Natural Selection

Memory systems that grow without bound become useless. Cordelia applies
three mechanisms to ensure memory quality increases over time.

### 7.1 Novelty Filtering (Write Path)

The novelty engine (Section 2.3) gates persistence. Low-novelty content
never enters the system. This is input filtering: controlling what gets
written.

### 7.2 Access-Weighted TTL (Read Path)

Every read increments an `access_count` and updates `last_accessed_at`.
Groups specify a default TTL. Memories that are not accessed within the
TTL expire. Memories that are frequently accessed survive.

This is natural selection applied to information: fitness is measured by
utility (access frequency), and the environment (TTL) creates selection
pressure. Over time, the memory population converges on high-utility
content.

### 7.3 Governance Voting

Protocol upgrades and group policy changes use access-weighted voting.
Memories with higher access counts carry more weight in governance
decisions. This ensures that entities whose memories are most valued by
the community have proportionally more influence over its evolution.

---

## 8. Security Model

### 8.1 Threat Hierarchy

The system is designed against a nation-state adversary with the
following capabilities:

| Threat | Mitigation |
|--------|-----------|
| Compromise of node infrastructure | Encryption boundary: node never sees plaintext |
| Compromise of single encryption key | Scope-aware keys: personal and group keys are independent |
| Network surveillance | QUIC with TLS 1.3 transport + content encryption (defence in depth) |
| Eclipse attack (surround node with adversary peers) | Governor churn rotation (20% hourly) |
| Sybil attack (fake identities) | Local trust computation, no global reputation to game |
| Traffic analysis | Opaque GUIDs, no metadata in identifiers |
| Compromised group member | Copy-on-write sharing, immutable provenance, key rotation on member removal |
| Database tampering | Integrity canary, append-only audit log |

### 8.2 Invariants

Three security properties that must never be violated:

1. **No plaintext at rest** on any node, ever.
2. **No plaintext in transit** between any components, ever (TLS + content encryption).
3. **Entity trust has primacy** over all group policies. A compromised group cannot force content into sovereign memory.

### 8.3 Key Non-Goals

The system does not attempt to:
- Hide that communication is occurring (metadata resistance is bounded)
- Prevent a sufficiently motivated adversary from targeting a specific
  entity's device (endpoint security is out of scope)
- Guarantee availability against network-level denial of service

See THREAT-MODEL.md for the full adversary model and REQUIREMENTS.md
for testable security requirements.

---

## 9. Economics

### 9.1 The Cooperative Equilibrium

The game-theoretic structure of Cordelia creates a cooperative
equilibrium. Entities benefit from sharing accurate memories because the
shared knowledge base increases utility for all participants. The
Bayesian trust mechanism makes defection (inaccurate sharing) detectable
and costly, establishing cooperation as the dominant strategy in
repeated games.

This is analogous to the incentive structure in Bitcoin: miners are
incentivised to validate honestly because the cost of dishonesty
(wasted computation) exceeds the benefit. In Cordelia, entities are
incentivised to share accurately because the cost of dishonesty
(reduced trust, reduced access to group knowledge) exceeds the benefit.

A second-order effect strengthens this equilibrium: **cooperative
amplification**. When entities share frame memory (conceptual
references, reasoning patterns, shared metaphors) alongside data
memory, they increase the group's capacity to extract value from
future knowledge sharing. The benefit `b` of receiving a memory is
not constant -- it is amplified by the receiver's conceptual frame.
Groups with shared intellectual infrastructure extract superlinear
returns from cooperation, making the cooperative equilibrium not just
stable but increasingly attractive over time. See
docs/design/game-theory.md Section 10 for the formal treatment.

### 9.2 Service Economics

The node role system creates a natural service market:

- **Secret keepers** provide backup and recovery (Shamir shards,
  n-of-m reincarnation). Revenue from reliability SLAs.
- **Archives** provide long-term durable storage (L3 cold store,
  lineage queries, compliance). Revenue from storage and retrieval.
- **Edge relays** provide connectivity between internal and public
  memory spaces. Revenue from bandwidth.

Crucially, service providers never hold plaintext or encryption keys.
Revenue comes from reliability and availability, not from data access.
This is the Signal model applied to commercial infrastructure.

### 9.3 Licensing

Cordelia is licensed under AGPL-3.0, which requires anyone who modifies
and deploys the system to publish their modifications. This prevents
cloud provider absorption (the "AWS problem") while allowing
unrestricted self-hosted use.

Commercial services (keeper, archive, relay) are provided by Seed Drill
Ltd as the initial operator, with the protocol designed for any party to
offer competing services.

---

## 10. Current State and Roadmap

### 10.1 Operational

- Two-node P2P network (QUIC, TLS 1.3, 5 mini-protocols)
- Governor peer lifecycle (cold/warm/hot/banned, churn rotation)
- SQLite storage with FTS5 search, schema v4
- AES-256-GCM encryption with cross-language round-trip (TS + Rust)
- MCP proxy with 25 tools, 187 tests
- Dashboard HTTP server with authentication
- Bootnode deployed at `boot1.cordelia.seeddrill.io:9474`

### 10.2 In Progress (R3)

- Culture-governed replication (wire dispatch, anti-entropy)
- Device enrollment (RFC 8628)
- Proxy TOML configuration and role system
- Dashboard: enrollment, group management, admin panel
- Multi-tenant org scoping
- Integration testing (proxy + node end-to-end)

### 10.3 Planned (R4+)

- Secret keeper infrastructure (Shamir shards, reincarnation)
- Archive infrastructure (L3 cold store, S3 backend)
- Envelope encryption per group (Signal pattern key exchange)
- Formal trust calibration (von Neumann-Morgenstern)
- Homomorphic encrypted vector search (HE-CKKS)
- Federation and cross-org discovery
- Constitutional groups (public, open membership)
- Working set estimation (adaptive L1 sizing per Denning)

---

## References

[1] J. L. Hennessy and D. A. Patterson, *Computer Architecture: A
Quantitative Approach*, 6th ed. Morgan Kaufmann, 2017. Cache hierarchy
design and trade-offs.

[2] M. S. Papamarcos and J. H. Patel, "A Low-Overhead Coherence
Solution for Multiprocessors with Private Cache Memories," in *Proc.
11th Annual International Symposium on Computer Architecture*, 1984,
pp. 348-354. MESI protocol for cache coherence.

[3] P. J. Denning, "The Working Set Model for Program Behavior,"
*Communications of the ACM*, vol. 11, no. 5, pp. 323-333, May 1968.
Working set theory and locality of reference.

[4] C. E. Shannon, "A Mathematical Theory of Communication," *Bell
System Technical Journal*, vol. 27, no. 3, pp. 379-423, Jul. 1948.
Information entropy as a measure of novelty.

[5] J. von Neumann and O. Morgenstern, *Theory of Games and Economic
Behavior*, Princeton University Press, 1944. Game-theoretic foundations
for trust calibration.

[6] M. Bennett, *A Brief History of Intelligence*, William Collins,
2023. Evolutionary perspectives on memory and cognition.

[7] D. C. Dennett, *From Bacteria to Bach and Back: The Evolution of
Minds*, W. W. Norton, 2017. Competence without comprehension --
applicable to AI memory systems.

[8] M. Minsky, *The Society of Mind*, Simon and Schuster, 1986.
Modular cognitive architecture parallels with multi-agent memory.

---

## Document Hierarchy

This whitepaper is the entry point. Detailed specifications are in
companion documents:

| Document | Purpose | Audience |
|----------|---------|----------|
| **WHITEPAPER.md** | This document. Why the system exists and how it works. | Everyone |
| **REQUIREMENTS.md** | 109 testable requirements (24 P0 invariants). What the system must do. | Engineers, testers, auditors |
| **HLD.md** | Component map, API contracts, work packages. How it is built. | Engineers building it |
| **THREAT-MODEL.md** | Adversary model, attack surfaces, mitigations. What can go wrong. | Security reviewers |
| **ARCHITECTURE.md** | Target state architecture, deployment models, federation. Where it is going. | Architects, investors |
| **NETWORK-MODEL.md** | Wire protocol detail, message formats, state machines. | Protocol implementors |
| **SPEC.md** | Formal protocol specification. | Protocol implementors |
| **docs/design/** | Deep dives: game theory, cache model, enrollment flow. | Researchers |

---

*Version 1.0 -- 2026-01-31*
*Seed Drill Ltd -- AGPL-3.0*
