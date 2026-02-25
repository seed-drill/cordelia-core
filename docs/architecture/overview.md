# Cordelia - Architecture Overview

*Target state: R2/R3 horizon*

> Visual diagram: [../architecture-diagram.drawio](../architecture-diagram.drawio)
> (open in [diagrams.net](https://app.diagrams.net)).

## System Architecture

```
                         DEPLOYMENT MODELS
    ┌──────────────────────────────────────────────────────┐
    │                                                      │
    │  Model A: Local (R1)     Model B: Team (R2)          │
    │                                                      │
    │  Claude ──stdio──> MCP   Claude ────┐                │
    │         (local)          Swarm  ────┤                │
    │                          Martin ────┼──> Cordelia    │
    │                          Bill   ────┘    Service     │
    │                                          (KVM host)  │
    │                                                      │
    │  Model C: Federated (R3+)                            │
    │                                                      │
    │  ┌──────────┐     ┌──────────┐     ┌──────────┐     │
    │  │ Cordelia │◄───►│ Cordelia │◄───►│ Cordelia │     │
    │  │ (Seed    │     │ (Client  │     │ (Partner │     │
    │  │  Drill)  │     │  Org)    │     │  Org)    │     │
    │  └──────────┘     └──────────┘     └──────────┘     │
    │        ▲                                             │
    │        │  Discovery: DNS SRV / registry / gossip     │
    │        │  Protocol:  MCP over HTTP/SSE               │
    │        │  Trust:     mTLS + token + memory challenge  │
    │                                                      │
    └──────────────────────────────────────────────────────┘
```

## Component Overview

```
                           CLIENTS
    ┌──────────────────────────────────────────────────────┐
    │                                                      │
    │   Claude Code          Swarm Agents       Other MCP  │
    │   (primary)            (TeammateTool)      Clients   │
    │       │                     │                 │      │
    └───────┼─────────────────────┼─────────────────┼──────┘
            │                     │                 │
            ▼                     ▼                 ▼
    ┌──────────────────────────────────────────────────────┐
    │              AUTH / ACCESS CONTROL            [R2]    │
    │                                                      │
    │  [R1] None (local stdio, implicit trust)             │
    │  [R2] Bearer token per client                        │
    │  [R2] Token → user_id + scope mapping                │
    │  [R3] mTLS (client certificates)                     │
    │  [R3] Memory challenge-response (fuzzy auth)         │
    │                                                      │
    │  Token issuance: admin CLI tool                      │
    │  Token storage: encrypted config (never in repo)     │
    │  Token rotation: manual R2, automated R3             │
    └──────────────────────┬───────────────────────────────┘
                           │
                           ▼
    ┌──────────────────────────────────────────────────────┐
    │              ACCESS CONTROL MODEL            [R2]    │
    │                                                      │
    │  Every request carries: token → (user_id, scopes)    │
    │                                                      │
    │  ┌────────────────────────────────────────────────┐  │
    │  │  SCOPE: personal                               │  │
    │  │  L1 hot context. Identity, preferences, state. │  │
    │  │  Read/Write: owner only (token.user_id match)  │  │
    │  │  Swarm agents: inherit owner's token           │  │
    │  └────────────────────────────────────────────────┘  │
    │                                                      │
    │  ┌────────────────────────────────────────────────┐  │
    │  │  SCOPE: group                                  │  │
    │  │  Shared org memory. Strategy, decisions, etc.  │  │
    │  │  Read: any authenticated group member           │  │
    │  │  Write: role-based (owner, admin, member,      │  │
    │  │         viewer)                                 │  │
    │  │  Visibility: group (L2 items with group_id)    │  │
    │  └────────────────────────────────────────────────┘  │
    │                                                      │
    │  ┌────────────────────────────────────────────────┐  │
    │  │  SCOPE: private                                │  │
    │  │  User's own L2 items. Entities, learnings.     │  │
    │  │  Read/Write: owner only                        │  │
    │  │  Invisible to other group members              │  │
    │  │  Default scope for all L2 writes               │  │
    │  └────────────────────────────────────────────────┘  │
    │                                                      │
    │  ┌────────────────────────────────────────────────┐  │
    │  │  SCOPE: public                         [R3+]   │  │
    │  │  Open items. Published learnings, entities.    │  │
    │  │  Read: anyone (no auth required)               │  │
    │  │  Write: owner promotes from private/group      │  │
    │  │  Federated: discoverable across instances      │  │
    │  └────────────────────────────────────────────────┘  │
    │                                                      │
    │  Enforcement point: MCP handler layer                │
    │  Every tool call checked before execution            │
    │  Audit log records: who, what, scope, timestamp      │
    └──────────────────────┬───────────────────────────────┘
                           │
                           ▼
    ┌──────────────────────────────────────────────────────┐
    │                    MCP INTERFACE                      │
    │                                                      │
    │  Core (R1):                                          │
    │   memory_read_hot    memory_search    memory_status   │
    │   memory_write_hot   memory_read_warm                 │
    │   memory_write_warm  memory_analyze_novelty           │
    │                                                      │
    │  R2:                                                 │
    │   memory_delete_warm    memory_backup                 │
    │   memory_restore        memory_share                  │
    │   memory_group_create   memory_group_list             │
    │   memory_group_read     memory_group_add_member       │
    │   memory_group_remove_member                          │
    │                                                      │
    │  R3:                                                 │
    │   memory_federate       memory_lineage                │
    │   memory_merge          memory_key_rotate             │
    │                                                      │
    └──────────────────────┬───────────────────────────────┘
                           │
            ┌──────────────┼──────────────┐
            ▼              ▼              ▼
    ┌──────────────┐ ┌───────────┐ ┌───────────────┐
    │   NOVELTY    │ │  SEARCH   │ │   INTEGRITY   │
    │   ENGINE     │ │  ENGINE   │ │   ENGINE      │
    │              │ │           │ │               │
    │  Heuristic   │ │ Keyword   │ │ Hash chain    │
    │  detection   │ │ Tag match │ │ Verification  │
    │  Confidence  │ │ Semantic  │ │ Session count │
    │  scoring     │ │ cosine    │ │ Tamper detect │
    │  Persistence │ │           │ │               │
    │  targeting   │ │ [R2] FTS5 │ │ [R2] Key     │
    │              │ │  BM25     │ │  rotation     │
    │              │ │  Hybrid   │ │               │
    │              │ │  70/30    │ │ [R3] Cross-   │
    │              │ │  ranking  │ │  instance     │
    │              │ │           │ │  chain verify │
    └──────────────┘ └───────────┘ └───────────────┘
                           │
    ┌──────────────────────┼───────────────────────────────┐
    │                CRYPTO BOUNDARY                        │
    │                                                      │
    │  ┌────────────────────────────────────────────────┐  │
    │  │              CRYPTO PROVIDER                    │  │
    │  │                                                │  │
    │  │   AES-256-GCM encryption                       │  │
    │  │   scrypt key derivation (N=16384, r=8, p=1)    │  │
    │  │   Per-item IV generation (unique per write)    │  │
    │  │   [R2] Key rotation mechanism                  │  │
    │  │   [R2] Per-scope encryption keys               │  │
    │  │        (personal key ≠ group key)              │  │
    │  │   [R3] Rust crypto core (optional)             │  │
    │  │   [R3] HSM / Secure Enclave integration        │  │
    │  └────────────────────────────────────────────────┘  │
    │                                                      │
    │  ALL data encrypted before crossing this boundary    │
    │  Storage layer NEVER sees plaintext                  │
    └──────────────────────┬───────────────────────────────┘
                           │
    ┌──────────────────────┼───────────────────────────────┐
    │              STORAGE LAYER                            │
    │                                                      │
    │  ┌────────────────────────────────────────────────┐  │
    │  │  [R1] JSON files on disk                       │  │
    │  │  Simple, no dependencies, works for 3 users    │  │
    │  └────────────────────────────────────────────────┘  │
    │                          │                           │
    │                          ▼ R2 migration              │
    │  ┌────────────────────────────────────────────────┐  │
    │  │  [R2] SQLite                                   │  │
    │  │                                                │  │
    │  │  L1 table: user_id → encrypted_blob            │  │
    │  │    - Single row per user                        │  │
    │  │    - Replaces JSON files                        │  │
    │  │    - Sub-ms reads via prepared statements       │  │
    │  │                                                │  │
    │  │  L2 items table: guid → encrypted_blob          │  │
    │  │    - GUID primary key (no metadata leak)        │  │
    │  │    - Type, owner_id, visibility columns         │  │
    │  │    - last_accessed_at (timestamp, updated on     │  │
    │  │      every read - enables LRU eviction)         │  │
    │  │    - access_count (integer, incremented on       │  │
    │  │      every search hit - enables LFU promotion)  │  │
    │  │    - Encrypted content opaque to DB              │  │
    │  │                                                │  │
    │  │  L2 chunks table: chunk_id → text + embedding   │  │
    │  │    - FTS5 virtual table for BM25 search         │  │
    │  │    - sqlite-vec for vector similarity            │  │
    │  │    - Hybrid search: 70% semantic + 30% keyword  │  │
    │  │                                                │  │
    │  │  Embedding cache table: hash → vector            │  │
    │  │    - Avoids re-embedding unchanged content       │  │
    │  │                                                │  │
    │  │  Groups table: id, name, culture, policy          │  │
    │  │    - Universal sharing primitive                  │  │
    │  │                                                │  │
    │  │  Group members table: group_id, entity_id, role │  │
    │  │    - Roles: owner, admin, member, viewer         │  │
    │  │    - Posture: active, silent, emcon (R3-015)     │  │
    │  │                                                │  │
    │  │  Access log table: structured audit log           │  │
    │  │    - entity_id, action, resource_type/id,         │  │
    │  │      group_id, detail                             │  │
    │  │    - All policy evals logged (allowed + denied)  │  │
    │  │                                                │  │
    │  │  Audit table: append-only event log              │  │
    │  │    - Who, what, scope, timestamp                 │  │
    │  │                                                │  │
    │  │  Properties:                                    │  │
    │  │    - Single file (easy backup: copy one file)   │  │
    │  │    - WAL mode (concurrent reads + one writer)   │  │
    │  │    - No external service dependency              │  │
    │  │    - Portable (works on any OS)                  │  │
    │  └────────────────────────────────────────────────┘  │
    │                          │                           │
    │                          ▼ R3+ evolution             │
    │  ┌────────────────────────────────────────────────┐  │
    │  │  [R3+] Storage abstraction                     │  │
    │  │    - SQLite (local / default)                   │  │
    │  │    - S3-compatible (durability at scale)         │  │
    │  │    - KV store (Redis/Dragonfly for L1 hotpath)  │  │
    │  │    - Interface unchanged: MCP tools don't know   │  │
    │  └────────────────────────────────────────────────┘  │
    │                                                      │
    └──────────────────────────────────────────────────────┘

    ┌──────────────────────────────────────────────────────┐
    │              MEMORY LAYERS                            │
    │                                                      │
    │  ┌────────────────────────────────────────────────┐  │
    │  │  L0: SESSION BUFFER                            │  │
    │  │  Ephemeral. Lives in context window.            │  │
    │  │  Lost on session end. No persistence.           │  │
    │  │  [R2] Pre-compaction flush to L1/L2             │  │
    │  └────────────────────────────────────────────────┘  │
    │                                                      │
    │  ┌────────────────────────────────────────────────┐  │
    │  │  L1: HOT CONTEXT (~50KB per user)              │  │
    │  │  Loaded every session start. Identity core.     │  │
    │  │  Preferences, active state, delegation rules.   │  │
    │  │  Autobiographical memory + integrity chain.     │  │
    │  │                                                │  │
    │  │  Scope: personal (owner-only access)            │  │
    │  │  Latency target: <10ms read                     │  │
    │  └────────────────────────────────────────────────┘  │
    │                                                      │
    │  ┌────────────────────────────────────────────────┐  │
    │  │  L2: WARM INDEX (~5MB, searchable)             │  │
    │  │  Entities, sessions, learnings.                 │  │
    │  │  Pulled on demand via search.                   │  │
    │  │  Encrypted content + unencrypted vector.        │  │
    │  │  Vectors persist alongside blobs to enable      │  │
    │  │  cross-node semantic search without decryption. │  │
    │  │                                                │  │
    │  │  Scopes: personal | group | public               │  │
    │  │  [R2] Delete API (GDPR, cleanup)                │  │
    │  │  [R2] GUID primary keys (no metadata leak)      │  │
    │  │  [R2] Owner field on every item                  │  │
    │  │  Latency target: <100ms search                  │  │
    │  └────────────────────────────────────────────────┘  │
    │                                                      │
    │  ┌────────────────────────────────────────────────┐  │
    │  │  L3: COLD ARCHIVE (unbounded)          [R3+]   │  │
    │  │  Compressed session history.                    │  │
    │  │  Lineage tracking + provenance.                 │  │
    │  │  Divergence detection for returning agents.     │  │
    │  │  Rarely accessed, bulk retrieval.               │  │
    │  └────────────────────────────────────────────────┘  │
    │                                                      │
    └──────────────────────────────────────────────────────┘

## Caching Theory Analysis

Cordelia's L0/L1/L2/L3 hierarchy maps to standard computer science cache
architecture. This analysis identifies what's implemented, what's missing,
and what each gap costs in performance.

### Current State vs CS Cache Theory

```
    ┌──────────────────────────────────────────────────────┐
    │              CACHE HIERARCHY MAPPING                   │
    │                                                      │
    │  CS concept         Cordelia             Status      │
    │  ──────────         ────────             ──────      │
    │  CPU registers      L0 session buffer    Working     │
    │                     (context window)     (ephemeral) │
    │                                                      │
    │  L1 cache           L1 hot context       Working     │
    │                     (~50KB, every        (prefetch   │
    │                      session start)       on start)  │
    │                                                      │
    │  L2 cache           L2 warm index        Working     │
    │                     (on-demand search)   (demand     │
    │                                           fetch)     │
    │                                                      │
    │  Main memory/disk   L3 cold archive      Not impl.  │
    │                                                      │
    │  Inclusion policy   Novelty filter       Working     │
    │                     (what gets stored)                │
    │                                                      │
    │  Eviction policy    TTL on cached        Working     │
    │                     group memories      (sweepExpired │
    │                                          Items)       │
    │                                                      │
    │  Prefetch           Session-start hook   Working     │
    │                     loads L1                          │
    │                                                      │
    │  Write-back         Session-end hook     Working     │
    │                     persists changes                  │
    │                                                      │
    │  Access tracking    last_accessed_at,    R2 (SQLite  │
    │  (LRU/LFU)         access_count         columns)    │
    │                                                      │
    │  Promotion/         Move items between   Not impl.  │
    │  demotion           layers by frequency  (R3/R4)    │
    │                                                      │
    │  Speculative        Prefetch L2 items    Not impl.  │
    │  prefetch           likely needed        (R3)       │
    │                                                      │
    │  Cache coherence    Invalidation when    Not impl.  │
    │                     shared memory        (R3 - via  │
    │                     updates              group       │
    │                                          culture)    │
    │                                                      │
    │  Working set        Adaptive L1 size     Not impl.  │
    │  estimation         per task complexity  (R4)       │
    │                                                      │
    └──────────────────────────────────────────────────────┘
```

### Why Current Performance Is Already Good

```
    ┌──────────────────────────────────────────────────────┐
    │              PERFORMANCE ANALYSIS                     │
    │                                                      │
    │  The two highest-impact cache optimisations in any   │
    │  hierarchy are already implemented:                   │
    │                                                      │
    │  1. PREFETCH FOR PREDICTABLE ACCESS (L1)             │
    │     Session-start loads identity, preferences,        │
    │     active state. Eliminates compulsory misses.       │
    │     Before: 100% miss rate (every session blank).     │
    │     After: ~0% miss rate for core identity.           │
    │     This is the single largest performance win.       │
    │                                                      │
    │  2. DEMAND-FETCH FOR UNPREDICTABLE ACCESS (L2)       │
    │     Search retrieves entities, sessions, learnings    │
    │     on demand. Eliminates the alternative of          │
    │     loading everything or loading nothing.            │
    │                                                      │
    │  Together these deliver ~80% of theoretical           │
    │  performance value (Pareto distribution).             │
    │  Remaining optimisations are real but diminishing     │
    │  returns with significantly more complexity.          │
    │                                                      │
    │  Subjective observation: Cordelia is already          │
    │  noticeably improving session quality. This is        │
    │  consistent with the cold-start elimination being     │
    │  the dominant factor.                                │
    │                                                      │
    └──────────────────────────────────────────────────────┘
```

### Optimisation Roadmap

```
    ┌──────────────────────────────────────────────────────┐
    │              CACHE OPTIMISATION PHASING               │
    │                                                      │
    │  R2 (Must have):                                     │
    │    - Access tracking columns in SQLite                │
    │      last_accessed_at: updated on every read          │
    │      access_count: incremented on every search hit    │
    │      Trivial to add now, expensive to retrofit.       │
    │      Provides data for all future optimisations.      │
    │                                                      │
    │  R2 (Should have):                                   │
    │    - TTL on group-cached memories                    │
    │      The natural selection mechanism from the         │
    │      sharing model. Valuable memories (frequently     │
    │      queried) survive. Non-valued expire.             │
    │      Requires: access_count threshold + TTL field.    │
    │                                                      │
    │  R3:                                                 │
    │    - Cache coherence via group culture                │
    │      Already designed in sharing model. Chatty        │
    │      groups = write-update (push). Taciturn =         │
    │      TTL-based expiry. Moderate = write-invalidate.   │
    │      Maps to standard coherence protocols:            │
    │        Chatty    → write-update (MESI modified)      │
    │        Moderate  → write-invalidate (standard MESI)  │
    │        Taciturn  → TTL expiry (weak consistency)     │
    │                                                      │
    │    - Speculative L2 prefetch at session start         │
    │      Query top N L2 items by relevance to             │
    │      active.focus. Pre-warm the embedding cache.      │
    │      Low cost, measurable benefit.                    │
    │                                                      │
    │    - PreToolUse mid-session prefetch                  │
    │      Inject relevant L2 memories before tool          │
    │      execution based on thinking context.             │
    │      See research/cordelia-pretooluse-memory.md.      │
    │                                                      │
    │  R4+:                                                │
    │    - Promotion/demotion between layers                │
    │      Frequently accessed L2 items promote to L1.     │
    │      Stale L1 items demote to L2.                     │
    │      Requires: access frequency data (from R2),      │
    │      promotion threshold tuning, L1 size budget.     │
    │                                                      │
    │    - Working set estimation                           │
    │      Adaptive L1 size based on task complexity.       │
    │      Simple project → small L1 (fast load).          │
    │      Multi-project session → larger L1.               │
    │      Denning's working set model applied to           │
    │      memory layers.                                  │
    │                                                      │
    │    - REM sleep (background optimisation)              │
    │      Offline: compress, deduplicate, promote/demote, │
    │      prune stale, strengthen high-frequency paths.   │
    │      Like human memory consolidation during sleep.    │
    │                                                      │
    └──────────────────────────────────────────────────────┘
```

    ┌──────────────────────────────────────────────────────┐
    │              EMBEDDING LAYER                          │
    │                                                      │
    │  Provider abstraction (pluggable):                    │
    │    - Ollama local (nomic-embed-text) [default]       │
    │    - OpenAI (text-embedding-3-small)                  │
    │    - [R3] Anthropic (when available)                  │
    │    - None (keyword-only fallback)                     │
    │                                                      │
    │  Apple Silicon Metal acceleration (local)             │
    │  [R2] Hash-based embedding cache in SQLite            │
    │  [R2] Chunking: 400 tokens / 80 token overlap        │
    └──────────────────────────────────────────────────────┘

## Decentralised Search

```
    ┌──────────────────────────────────────────────────────────┐
    │              VECTOR SEARCH ACROSS NODES                   │
    │                                                          │
    │  PROBLEM                                                 │
    │  ───────                                                 │
    │  Memories are encrypted blobs distributed across nodes.  │
    │  Entities need to search group memories on peers they    │
    │  don't hold locally. Can't send plaintext queries to     │
    │  peers. Can't search encrypted blobs.                    │
    │                                                          │
    │  SOLUTION                                                │
    │  ────────                                                │
    │  Persist embedding vectors alongside encrypted blobs.    │
    │  Vectors are unencrypted (or HE-encrypted, see below).  │
    │  Entities query peers by sending a query vector. Peers   │
    │  return ranked encrypted blobs. Entity decrypts locally. │
    │                                                          │
    │  STORAGE FORMAT (per memory on any node)                 │
    │  ──────────────                                          │
    │                                                          │
    │    ┌─────────────────────────────────────────────────┐   │
    │    │  group_hash      SHA-256 of group URI (public)  │   │
    │    │  item_id         GUID (public)                  │   │
    │    │  encrypted_blob  AES-256-GCM (opaque)           │   │
    │    │  vector          embedding (searchable)         │   │
    │    │  checksum        SHA-256 of plaintext (verify)  │   │
    │    │  author_id       provenance (public)            │   │
    │    │  updated_at      timestamp (public)             │   │
    │    └─────────────────────────────────────────────────┘   │
    │                                                          │
    │  QUERY FLOW                                              │
    │  ──────────                                              │
    │                                                          │
    │    Entity (proxy)              Peer (node)               │
    │    ──────────────              ──────────                 │
    │    1. embed("game theory")                               │
    │       -> query_vector                                    │
    │                                                          │
    │    2. send (group_hash,        3. cosine similarity      │
    │       query_vector, limit)        on stored vectors      │
    │       ─────────────────>          for group_hash         │
    │                                                          │
    │                                4. rank by score          │
    │                                                          │
    │       <─────────────────       5. return                 │
    │    6. receive                     [(item_id, score,      │
    │       [(id, score, blob)]          encrypted_blob)]      │
    │                                                          │
    │    7. decrypt(blob, group_key)                            │
    │       -> plaintext result                                │
    │                                                          │
    │  The proxy owns the key. The node never sees plaintext.  │
    │  The peer never sees the natural language query -- only   │
    │  the vector (lossy projection of the query).             │
    │                                                          │
    │  VECTOR ENCODING COMPATIBILITY                           │
    │  ────────────────────────────                            │
    │  All members of a group must produce compatible vectors. │
    │  The group manifest ({group_hash}:manifest) specifies:   │
    │                                                          │
    │    vector_model: "nomic-embed-text"                      │
    │    vector_dimensions: 768                                │
    │    vector_encoding: "plaintext" | "he-ckks" | "he-bfv"  │
    │                                                          │
    │  Members configure their local embedding provider to     │
    │  match. Incompatible vectors produce garbage search      │
    │  results (self-correcting: entity switches model).       │
    │                                                          │
    │  HOMOMORPHIC ENCRYPTION OPTION                           │
    │  ────────────────────────────                            │
    │  Default: plaintext vectors. Acceptable leakage for      │
    │  most groups (adversary learns topic distribution,       │
    │  not content).                                           │
    │                                                          │
    │  High-security groups can specify HE-encrypted vectors:  │
    │    vector_encoding: "he-ckks"                            │
    │                                                          │
    │  With HE, peers compute cosine similarity on encrypted   │
    │  vectors without learning the query or the stored        │
    │  vectors. Computational cost: ~100x plaintext.           │
    │  Trade-off is explicit and per-group. Most groups        │
    │  won't need it. Groups that care, opt in.                │
    │                                                          │
    │  INFORMATION LEAKAGE ANALYSIS                            │
    │  ────────────────────────────                            │
    │                                                          │
    │  With plaintext vectors, an adversary with access to     │
    │  the embedding model can:                                │
    │    - Cluster memories by topic                           │
    │    - Infer approximate semantic content                  │
    │    - Distinguish "financial" from "personal" memories    │
    │                                                          │
    │  An adversary CANNOT:                                    │
    │    - Reconstruct plaintext                               │
    │    - Read specific facts, names, decisions               │
    │    - Distinguish two memories on the same topic          │
    │                                                          │
    │  This is analogous to HTTPS metadata (IP, packet size):  │
    │  a practical trade-off that enables the system to        │
    │  function. HE eliminates this leakage at compute cost.   │
    │                                                          │
    └──────────────────────────────────────────────────────────┘
```

## Group Discovery and Metadata

```
    ┌──────────────────────────────────────────────────────────┐
    │              THREE-LAYER DISCOVERY                        │
    │                                                          │
    │  Layer 1: GOSSIP (P2P, zero infrastructure)              │
    │  ─────────────────────────────────────────               │
    │  Peer-share protocol already exchanges:                  │
    │    PeerAddress { node_id, addrs, last_seen, groups[] }   │
    │                                                          │
    │  groups[] contains group hashes. When peers exchange      │
    │  addresses, they advertise which groups they replicate.   │
    │  Entity asks: "who replicates for {group_hash}?"         │
    │  Answer comes from gossip. No registry needed.           │
    │                                                          │
    │  Layer 2: GROUP MANIFEST (self-describing)               │
    │  ─────────────────────────────────────────               │
    │  Each group contains a manifest memory at well-known ID: │
    │    {group_hash}:manifest                                 │
    │                                                          │
    │  Manifest contains (encrypted with group key):           │
    │    - culture (chatty/moderate/taciturn)                   │
    │    - security policy                                     │
    │    - vector_model + vector_dimensions + vector_encoding  │
    │    - member roles                                        │
    │    - departure policy                                    │
    │    - founding document (for constitutional groups)        │
    │                                                          │
    │  Non-members see: a blob exists at this ID for this      │
    │  group hash. They cannot read it. Members decrypt and    │
    │  learn the group's configuration.                        │
    │                                                          │
    │  Self-describing: the group manifest IS a memory. It     │
    │  replicates, is versioned, has provenance. No external   │
    │  metadata store needed.                                  │
    │                                                          │
    │  Layer 3: ON-CHAIN ANCHOR (optional, R4+)                │
    │  ────────────────────────────────────────                 │
    │  Constitutional groups (public, permanent) register on   │
    │  Midnight/Cardano. Proves: group exists, founding hash,  │
    │  immutable creation timestamp. Not required for private  │
    │  groups. Used for public knowledge, open communities,    │
    │  and groups that want cryptographic proof of existence.  │
    │                                                          │
    │  ENROLLMENT FLOW (joining a group)                       │
    │  ────────────────────────────────                        │
    │                                                          │
    │  1. Entity receives group_hash + group_key               │
    │     (out of band: QR code, secure message, enrollment    │
    │      service, or from a member via encrypted channel)    │
    │                                                          │
    │  2. Entity queries gossip: "who replicates {group_hash}?"│
    │     Peer-share responses reveal peers for that group.    │
    │                                                          │
    │  3. Entity connects to group peers, fetches manifest:    │
    │     {group_hash}:manifest                                │
    │                                                          │
    │  4. Decrypts manifest with group_key.                    │
    │     Learns: vector encoding, culture, security policy.   │
    │                                                          │
    │  5. Configures local embedding model to match manifest.  │
    │                                                          │
    │  6. Entity can now query (vector search) and contribute  │
    │     (write + replicate) to the group.                    │
    │                                                          │
    │  SECRET KEEPER DISCOVERY                                 │
    │  ───────────────────────                                 │
    │  Keepers advertise capability in gossip:                 │
    │    PeerAddress { ..., capabilities: ["keeper"] }         │
    │                                                          │
    │  Entity selects keepers from gossip responses. Keepers   │
    │  do not need group membership -- they store opaque       │
    │  Shamir shards. The shard protocol (0x06) is separate    │
    │  from the group protocol. Keepers are infrastructure,    │
    │  not members.                                            │
    │                                                          │
    │  ARCHIVE DISCOVERY                                       │
    │  ─────────────────                                       │
    │  Same pattern: capabilities: ["archive"]. Archives       │
    │  accept L3 cold storage requests. They hold encrypted    │
    │  compressed history. They may hold vectors for search    │
    │  but cannot decrypt content.                             │
    │                                                          │
    └──────────────────────────────────────────────────────────┘
```

    ┌──────────────────────────────────────────────────────┐
    │              SESSION LIFECYCLE                        │
    │                                                      │
    │  ┌──────────┐    ┌──────────┐    ┌──────────┐       │
    │  │  START   │───▶│  ACTIVE  │───▶│   END    │       │
    │  │          │    │          │    │          │       │
    │  │ Auth     │    │ Read/    │    │ [R2] Pre │       │
    │  │ Decrypt  │    │ Write    │    │ compact  │       │
    │  │ Verify   │    │ Search   │    │ flush    │       │
    │  │ chain    │    │ Novelty  │    │ Extend   │       │
    │  │ Load L1  │    │ filter   │    │ chain    │       │
    │  │ Notify   │    │          │    │ Encrypt  │       │
    │  └──────────┘    └──────────┘    │ Commit   │       │
    │       │                          │ Notify   │       │
    │       ▼                          └──────────┘       │
    │  ┌──────────┐                                       │
    │  │ RECOVERY │  On failure at any stage:             │
    │  │          │  1. Check backup file                  │
    │  │          │  2. Git restore                        │
    │  │          │  3. Notify user (macOS notification)   │
    │  │          │  4. Re-attempt                         │
    │  │          │  5. Fail gracefully (don't corrupt)    │
    │  └──────────┘                                       │
    └──────────────────────────────────────────────────────┘
```

## Security Model

```
    ┌──────────────────────────────────────────────────────┐
    │              THREAT MODEL                             │
    │                                                      │
    │  THREAT ACTORS                                       │
    │  ─────────────                                       │
    │  Casual observer      Public repo browsing           │
    │  Targeted attacker    Repo/disk access, analysis     │
    │  Compromised machine  Full disk, memory dump         │
    │  Cloud provider       Context window visibility      │
    │  Network observer     MITM, traffic analysis         │
    │  [R2] Nation state    Supply chain, side channel,    │
    │                       rubber hose, legal compulsion,  │
    │                       zero-day exploitation           │
    │                                                      │
    │  TRUST BOUNDARIES                                    │
    │  ────────────────                                    │
    │                                                      │
    │  ┌─ UNTRUSTED ─────────────────────────────────────┐ │
    │  │  Internet, GitHub, CDN, DNS, package registries  │ │
    │  │                                                  │ │
    │  │  ┌─ NECESSARY TRUST ──────────────────────────┐  │ │
    │  │  │  Anthropic cloud (runtime requirement)     │  │ │
    │  │  │  [R2] Nation state: legal compulsion risk   │  │ │
    │  │  │  [R3] Mitigate: local models, conf compute │  │ │
    │  │  │                                            │  │ │
    │  │  │  ┌─ TRUSTED ────────────────────────────┐  │  │ │
    │  │  │  │  Local machine / KVM host             │  │  │ │
    │  │  │  │  MCP server process                   │  │  │ │
    │  │  │  │  Encryption keys (in config/env)      │  │  │ │
    │  │  │  │  [R2] Network: TLS + token auth       │  │  │ │
    │  │  │  │  [R3] Shrink to process isolation     │  │  │ │
    │  │  │  │                                       │  │  │ │
    │  │  │  │  ┌─ THE ENTITY (core) ─────────────┐  │  │  │ │
    │  │  │  │  │  Context window + L1 + L2        │  │  │  │ │
    │  │  │  │  │  Integrity chain (proof of self) │  │  │  │ │
    │  │  │  │  │  Plaintext exists only here      │  │  │  │ │
    │  │  │  │  └──────────────────────────────────┘  │  │  │ │
    │  │  │  └───────────────────────────────────────┘  │  │ │
    │  │  └─────────────────────────────────────────────┘  │ │
    │  └───────────────────────────────────────────────────┘ │
    │                                                      │
    │  NATION STATE CONSIDERATIONS (R2)                    │
    │  ────────────────────────────────                    │
    │  Attack surface        Mitigation                    │
    │  ──────────────        ──────────                    │
    │  Supply chain          Dependency audit, lockfiles,  │
    │   (npm, OS, hw)        reproducible builds, SBOM     │
    │  Legal compulsion      Jurisdiction awareness, data   │
    │   (FISA, RIPA)         minimization, user notice     │
    │  Side channels         Constant-time crypto ops,     │
    │   (timing, power)      process isolation             │
    │  Zero-day exploit      Defense in depth, minimal     │
    │   (OS, runtime)        attack surface, sandboxing    │
    │  Key extraction        [R3] HSM/Secure Enclave,      │
    │   (memory dump)        encrypted swap, mlock         │
    │  Rubber hose           Plausible deniability layer,  │
    │   (coercion)           warrant canary (out of scope) │
    │                                                      │
    │  ACCEPTED RISKS                                      │
    │  ──────────────                                      │
    │  1. Anthropic reads context (runtime requirement)    │
    │  2. Key material in RAM during operation              │
    │  3. No vessel attestation (which model is running?)  │
    │  4. Nation state with physical access (game over)     │
    │  5. Coercion/legal compulsion (policy, not tech)     │
    │                                                      │
    │  MITIGATIONS BY RELEASE                              │
    │  ──────────────────────                              │
    │  R1: Encryption at rest, integrity chain, audit log  │
    │  R2: Token auth, GUID storage, key rotation, CI      │
    │      security, dependency audit, SAST, nation state  │
    │      threat analysis, per-scope encryption keys       │
    │  R3: mTLS, fuzzy auth, HSM/enclave, process          │
    │      isolation, confidential compute exploration      │
    └──────────────────────────────────────────────────────┘
```

### Proof-of-Useful-Work: Entropy Cost as Sybil Resistance

```
    ┌──────────────────────────────────────────────────────────┐
    │        SYBIL RESISTANCE VIA ENTROPY COST          [R4]   │
    │                                                          │
    │  INSIGHT                                                 │
    │  ───────                                                 │
    │  Producing memories that survive Cordelia's filtering    │
    │  pipeline is information-theoretically expensive.        │
    │  This cost is a natural Sybil deterrent -- and unlike    │
    │  PoW, the energy is spent producing genuine value.       │
    │                                                          │
    │  THREE DEFENCE LAYERS (compounding)                      │
    │  ──────────────────────────────────                      │
    │                                                          │
    │  Layer 1: Group membership gate                          │
    │    Replication requires group membership. Spinning up    │
    │    nodes is cheap; social admission to a group with      │
    │    real members is not.                                   │
    │                                                          │
    │  Layer 2: Novelty filtering (Shannon bound)              │
    │    Inbound memories pass through the novelty engine.     │
    │    Low-entropy content (repetitive, generic) is          │
    │    rejected before persistence. The cost of producing    │
    │    content that passes a novelty filter is bounded       │
    │    below by the entropy of the target's context.         │
    │    An outsider faces maximum entropy -- every message    │
    │    is maximally expensive to craft convincingly.         │
    │                                                          │
    │  Layer 3: Trust calibration (empirical verification)     │
    │    Memories that don't match reality lose trust over     │
    │    time. Even if an attacker produces novel content,     │
    │    inaccurate memories are detected empirically.         │
    │    Sustained attack requires content that is novel       │
    │    AND accurate AND relevant -- which converges on       │
    │    genuinely useful contribution.                        │
    │                                                          │
    │  COMPARISON TO CONSENSUS MECHANISMS                      │
    │  ──────────────────────────────────                      │
    │                                                          │
    │  Bitcoin PoW:                                            │
    │    Work: arbitrary hash computation                      │
    │    Energy: enormous, wasteful by design                  │
    │    Sybil: strong (cost = electricity)                    │
    │    Value of work: none (hashes discarded)                │
    │                                                          │
    │  Cardano PoS:                                            │
    │    Work: capital stake (ada locked)                      │
    │    Energy: low                                           │
    │    Sybil: strong (cost = capital)                        │
    │    Edge cases: stake centralisation, nothing-at-stake,   │
    │      long-range attacks, stake grinding                  │
    │    Value of work: capital allocation signal               │
    │                                                          │
    │  Cordelia PoUW (Proof-of-Useful-Work):                   │
    │    Work: producing genuine knowledge that survives       │
    │      novelty filtering + trust calibration               │
    │    Energy: proportional to value created                 │
    │    Sybil: potentially strong (cost = cognition)          │
    │    Edge cases: require formal analysis (see below)       │
    │    Value of work: the work IS the value                  │
    │                                                          │
    │  The key difference: in PoW, work and value are          │
    │  decoupled (miners burn energy to produce hashes         │
    │  nobody wants). In PoUW, work and value are the same     │
    │  thing. The cost of Sybil attack is the cost of          │
    │  producing genuine knowledge -- and if you're doing      │
    │  that, you're not attacking, you're contributing.        │
    │                                                          │
    │  FORMAL ANALYSIS REQUIRED (R4-015)                       │
    │  ─────────────────────────────────                       │
    │  This is currently an intuition supported by             │
    │  information theory. Before making claims:               │
    │                                                          │
    │  1. Formal entropy bound: can we prove a Shannon         │
    │     lower bound on the cost of producing content         │
    │     that passes novelty filtering in context C?          │
    │                                                          │
    │  2. Game-theoretic analysis: model attacker vs           │
    │     honest participant cost functions. What is           │
    │     the asymmetry? Under what assumptions does           │
    │     PoUW hold?                                           │
    │                                                          │
    │  3. Adversarial edge cases:                              │
    │     - Content recycling across groups (cross-context     │
    │       novelty bypass)                                    │
    │     - AI-generated plausible-but-false memories          │
    │       (LLM adversary producing high-entropy lies)        │
    │     - Slow poisoning (small inaccuracies over time       │
    │       that individually pass trust calibration)          │
    │     - Collusion (multiple Sybils corroborating           │
    │       each other's false memories)                       │
    │                                                          │
    │  4. Comparison theorems: formal comparison to            │
    │     PoW/PoS security guarantees under equivalent         │
    │     adversarial budgets                                  │
    │                                                          │
    │  5. Generalisability: does this mechanism apply          │
    │     beyond memory networks? If so, this may be a         │
    │     contribution to distributed consensus theory.        │
    │                                                          │
    │  CONNECTIONS                                             │
    │  ───────────                                             │
    │  - Shannon information theory (entropy bounds)           │
    │  - Von Neumann-Morgenstern (trust as rational utility)   │
    │  - Darwinian selection (novelty = fitness pressure)      │
    │  - Denning working set model (context defines cost)      │
    │  - Cardano Ouroboros (PoS reference for comparison)      │
    │                                                          │
    └──────────────────────────────────────────────────────────┘
```

## CI/CD Pipeline

```
    ┌──────────────────────────────────────────────────────┐
    │              CI/CD PIPELINE                   [R2]    │
    │                                                      │
    │  ┌────────┐  ┌────────┐  ┌────────┐  ┌──────────┐  │
    │  │ Lint   │─▶│ Test   │─▶│Security│─▶│  Build   │  │
    │  │        │  │        │  │        │  │          │  │
    │  │ ESLint │  │ Unit   │  │ Audit  │  │ TypeScript│  │
    │  │ Format │  │ Prop   │  │ SAST   │  │ [R3] Rust│  │
    │  │        │  │ Fault  │  │ Secret │  │          │  │
    │  │        │  │ Crypto │  │ scan   │  │          │  │
    │  │        │  │ Mutate │  │ License│  │          │  │
    │  │        │  │ E2E    │  │ SBOM   │  │          │  │
    │  └────────┘  └────────┘  └────────┘  └──────────┘  │
    │                                                      │
    │  Test quality gates (nuclear-grade):                  │
    │    - All tests pass (zero tolerance)                  │
    │    - Property-based: crypto, serialization, search    │
    │    - Fault injection: corruption, partial write, OOM  │
    │    - Mutation testing: tests catch real regressions    │
    │    - Crypto edge cases: wrong key, tampered data,     │
    │      truncated ciphertext, IV reuse detection         │
    │    - Concurrency: simultaneous read/write, races      │
    │    - Recovery: every failure mode has tested path      │
    │    - No known vulnerabilities in dependencies         │
    │    - SBOM generated and tracked                       │
    └──────────────────────────────────────────────────────┘
```

## Backup / Restore

```
    ┌──────────────────────────────────────────────────────┐
    │              BACKUP / RESTORE                 [R2]    │
    │                                                      │
    │  Backup:                                             │
    │    - SQLite database file (single file = all state)  │
    │    - Encrypted at rest (no additional wrapping)       │
    │    - Integrity verified before export                 │
    │    - Metadata header: schema version, timestamp,      │
    │      user, item count, chain hash                    │
    │    - Git commit as implicit backup (current)          │
    │    - [R2] Explicit backup MCP tool                    │
    │    - [R2] Scheduled backup option                     │
    │                                                      │
    │  Restore:                                            │
    │    - Verify integrity before import                   │
    │    - Schema migration if version mismatch             │
    │    - Rebuild search index from items                  │
    │    - Re-verify hash chain after restore               │
    │    - Tested recovery path for every failure mode      │
    │    - Dry-run mode (validate without applying)         │
    └──────────────────────────────────────────────────────┘
```

## How Minds Find Each Other (R3+)

```
    ┌──────────────────────────────────────────────────────┐
    │              DISCOVERY & FEDERATION           [R3+]   │
    │                                                      │
    │  The Culture Problem:                                │
    │    How do autonomous agents with sovereign memory     │
    │    find, trust, and collaborate with each other?      │
    │                                                      │
    │  Phase 1 (R2): No discovery needed                   │
    │    - 3 founders, known server endpoint                │
    │    - Hard-coded connection                            │
    │    - Auth via bearer token                            │
    │                                                      │
    │  Phase 2 (R3): Registry-based                        │
    │    ┌──────────────┐                                  │
    │    │   Registry   │  Known endpoint listing           │
    │    │   Service    │  available Cordelia instances      │
    │    │              │  Authenticated enrollment          │
    │    └──────┬───────┘  Heartbeat / health check         │
    │           │                                           │
    │     ┌─────┼─────┐                                    │
    │     ▼     ▼     ▼                                    │
    │    [A]   [B]   [C]   Cordelia instances               │
    │                                                      │
    │  Phase 3 (R4+): Decentralized                        │
    │    - DNS SRV: _cordelia._tcp.example.com              │
    │    - Gossip protocol: peers announce to known peers   │
    │    - Web of trust: memory-based verification          │
    │    - No single point of failure                       │
    │                                                      │
    │  Federation Protocol:                                │
    │    - MCP over HTTP/SSE (standard transport)           │
    │    - Shared items: replicated to peers                │
    │    - Private items: never leave origin instance        │
    │    - Conflict resolution: last-writer-wins + audit    │
    │    - Lineage tracking: provenance across instances     │
    └──────────────────────────────────────────────────────┘
```

## Memory Sharing Model

### Core Insight

The group is the universal sharing primitive. Every form of human memory
interaction - from private thought to organisational knowledge to public
discourse - can be modelled as entities participating in groups with
configurable culture and security policies.

### Primitives

```
    ┌──────────────────────────────────────────────────────┐
    │              THE FIVE PRIMITIVES                      │
    │                                                      │
    │  1. ENTITY                                           │
    │     The individual with sovereign memory.             │
    │     A person, an AI agent, an organisation.           │
    │     Has: private memory, identity, security policy.   │
    │     Holds absolute discretion over what it trusts     │
    │     and stores. Entity security policy has primacy    │
    │     over all group policies. Always.                  │
    │                                                      │
    │  2. GROUP                                            │
    │     Two or more entities that share memories.         │
    │     Has: membership, culture, security policy.        │
    │                                                      │
    │     Identity: group_id = SHA-256(URI).                │
    │     The URI is private to members. The hash is        │
    │     public on the network (discoverable via gossip).  │
    │     Access: group_key (symmetric, AES-256-GCM).       │
    │     Members hold the key. Non-members can see the     │
    │     hash and replicate encrypted blobs but cannot     │
    │     read content.                                     │
    │                                                      │
    │     Group manifest: a special memory at well-known    │
    │     ID {group_hash}:manifest. Contains: culture,      │
    │     security policy, vector encoding version/model,   │
    │     member roles. Encrypted with group key -- only    │
    │     members can read config. Replicates like any      │
    │     other memory. Self-describing group.              │
    │                                                      │
    │     Smallest group: a pair (e.g. Russell + Claude).   │
    │     Groups can contain sub-groups.                    │
    │     Sub-groups inherit parent policy unless            │
    │     explicitly overridden.                            │
    │                                                      │
    │  3. MEMORY                                           │
    │     A unit of knowledge: entity, session, learning,   │
    │     decision, insight, observation.                   │
    │     Private by default. Shared by group membership.   │
    │     Has: author (provenance), content, trust level.   │
    │                                                      │
    │     Storage format per memory:                        │
    │       (group_hash, item_id, encrypted_blob, vector,   │
    │        checksum, author_id, updated_at)               │
    │                                                      │
    │     The encrypted_blob is opaque to non-members.      │
    │     The vector enables semantic search without         │
    │     decryption. Vector is computed by the authoring    │
    │     entity before encryption and shipped alongside     │
    │     the blob. See "Decentralised Search" section.     │
    │                                                      │
    │  4. CULTURE                                          │
    │     The broadcast policy of a group.                  │
    │     Controls how aggressively memories propagate.     │
    │     Configurable per group, inheritable by sub-group. │
    │                                                      │
    │  5. TRUST                                            │
    │     The entity's assessment of a memory's value       │
    │     and reliability. Each entity has absolute          │
    │     discretion on whether to trust and/or store       │
    │     any memory, regardless of source - including      │
    │     its own. Trust is local, not consensus.            │
    │                                                      │
    └──────────────────────────────────────────────────────┘
```

### Rules

```
    ┌──────────────────────────────────────────────────────┐
    │              SHARING RULES                            │
    │                                                      │
    │  RULE 1: PRIVATE BY DEFAULT                          │
    │  All memories are private to the authoring entity     │
    │  unless explicitly shared via group membership.       │
    │  No implicit access. No ambient authority.            │
    │                                                      │
    │  RULE 2: GROUPS ARE THE SHARING PRIMITIVE             │
    │  To share, entities join a group. On joining, the     │
    │  group's shared memories become available (subject     │
    │  to the entity's trust policy - see Rule 7).         │
    │  On leaving, the entity retains its own memories.     │
    │  Group retains copies of contributions (departure     │
    │  policy is per-group culture).                        │
    │                                                      │
    │  RULE 3: CULTURE GOVERNS BROADCAST                   │
    │  Each group has a culture that determines how          │
    │  memories propagate among members:                    │
    │                                                      │
    │    Chatty      Push to all members immediately.       │
    │                Every write broadcasts. High traffic,   │
    │                high coherence. Close teams, swarms.    │
    │                                                      │
    │    Moderate    Notify members, cache on first access.  │
    │                Balanced. Most working groups.          │
    │                                                      │
    │    Taciturn    Available on query only. No push,       │
    │                no notification. Formal orgs, boards,   │
    │                cross-org collaborations.               │
    │                                                      │
    │    Culture is a spectrum, not three buckets.           │
    │    Parameters: broadcast_eagerness, ttl_default,       │
    │    notification_policy, replication_depth.             │
    │                                                      │
    │  RULE 4: QUERY-DRIVEN CACHING                        │
    │  Memories queried across the group are cached          │
    │  locally at the requesting entity. Valuable            │
    │  memories (frequently queried) propagate across        │
    │  members and become more resilient. Non-valued         │
    │  memories expire via TTL. This is natural selection    │
    │  operating on memories - fitness measured by            │
    │  retrieval frequency, replication through caching,     │
    │  death through expiry.                                │
    │                                                      │
    │  RULE 5: SECURITY IS SHARED AT GROUP LEVEL            │
    │  Groups define common security: encryption standard,   │
    │  auth requirements, audit policy. All members must     │
    │  meet the group minimum. Envelope encryption:          │
    │  group key encrypts shared memories, each member's     │
    │  key encrypts the group key. Member departs →          │
    │  re-encrypt group key for remaining members (not       │
    │  every memory). Signal protocol pattern.               │
    │                                                      │
    │  RULE 6: SUB-GROUPS INHERIT                           │
    │  Sub-groups inherit parent group policy (culture,      │
    │  security) unless explicitly overridden. A sub-group   │
    │  can tighten security but not loosen it below the     │
    │  parent minimum. Culture can be overridden freely.     │
    │                                                      │
    │  RULE 7: ENTITY TRUST HAS PRIMACY                    │
    │  The receiving entity always has final say on           │
    │  whether to trust and/or store a memory. This          │
    │  applies regardless of source:                        │
    │                                                      │
    │    - Group memory from a trusted peer: store           │
    │    - Group memory from unknown source: quarantine      │
    │    - Memory from an adversarial group: reject          │
    │    - Own memory, low confidence: flag for review       │
    │    - Own memory, emotionally generated: cool-off       │
    │                                                      │
    │  Entity security policy overrides group policy.        │
    │  Always. A compromised group cannot force content     │
    │  into an entity's sovereign memory.                   │
    │                                                      │
    │  This is the fundamental security invariant.           │
    │  Everything else can be negotiated.                    │
    │                                                      │
    │  Future: von Neumann-Morgenstern game theory           │
    │  applied to trust decisions. Entities as rational      │
    │  actors with utility functions over memory             │
    │  acceptance. Mixed strategies for adversarial          │
    │  group participation. Minimax for worst-case           │
    │  trust scenarios.                                     │
    │                                                      │
    │  RULE 8: DISCOVERY AND AUTHENTICATION                 │
    │  Groups need a mechanism for entities to find and      │
    │  join them. Authentication proves membership.          │
    │  Discovery proves existence. Both are required.       │
    │  See "Group Discovery and Metadata" section above.    │
    │                                                      │
    │  RULE 9: PROTOCOL UPGRADE VIA ACCESS-WEIGHTED VOTING  │
    │  Group protocol parameters (vector encoding, culture,  │
    │  security policy, departure policy) can evolve via     │
    │  managed migration. No hard fork combinator needed.    │
    │                                                      │
    │  Vote weight per author:                               │
    │    w(author) = SUM(access_count) for author's          │
    │                memories in the group                   │
    │                                                      │
    │  Memories that other members retrieve give the author  │
    │  more governance weight. This is proof-of-useful-work  │
    │  applied to governance. Active cooperators decide.     │
    │                                                      │
    │  Flow:                                                │
    │    1. Propose: member writes protocol_upgrade memory   │
    │    2. Vote: members write protocol_vote memories       │
    │    3. Threshold: access-weighted votes exceed quorum   │
    │    4. Grace: period for members to upgrade clients     │
    │    5. Cutover: manifest updates, old format archived   │
    │                                                      │
    │  Stragglers degrade to read-only (can decrypt, can't   │
    │  produce new-format vectors) until they upgrade.       │
    │  Sovereignty preserved: no member is ejected.          │
    │                                                      │
    │  Generalises: one governance mechanism for all group   │
    │  evolution. See "Group Protocol Upgrade" section.      │
    │                                                      │
    └──────────────────────────────────────────────────────┘
```

### Group Protocol Upgrade

```
    ┌──────────────────────────────────────────────────────────┐
    │  GROUP PROTOCOL UPGRADE (without Hard Fork Combinator)   │
    │                                                          │
    │  MOTIVATION                                              │
    │  ──────────                                              │
    │  Some parameter changes are breaking. Example: upgrading │
    │  vector_encoding from "plaintext" to "he-ckks". HE      │
    │  queries can't match plaintext vectors. Plaintext        │
    │  queries leak to the peer (defeating HE's purpose).     │
    │  You can't mix. This is a hard fork.                     │
    │                                                          │
    │  Cardano's HFC (Duncan Coutts) combines protocol eras   │
    │  across millions of nodes with epoch boundaries. We      │
    │  don't need that. Cordelia groups are 3-1000 members    │
    │  who can coordinate. We need managed migration with      │
    │  a voting trigger.                                       │
    │                                                          │
    │  VOTE WEIGHT: ACCESS-WEIGHTED                            │
    │  ────────────────────────────                            │
    │                                                          │
    │    w(author) = SUM(access_count) for all author's        │
    │                memories in the group                     │
    │                                                          │
    │  Not raw memory count (rewards volume, gameable).        │
    │  Not equal votes (Sybil-weak).                           │
    │  Access-weighted: memories others actually retrieve      │
    │  give you more say.                                      │
    │                                                          │
    │  This IS proof-of-useful-work applied to governance.     │
    │  Connects to:                                            │
    │    - R2-019 access tracking (data source)                │
    │    - Trust calibration (retrieved = valued)              │
    │    - Cooperative equilibrium (cooperators decide)        │
    │    - PoUW (governance weight from useful contribution)   │
    │                                                          │
    │  An author with 3 memories retrieved 1000 times has      │
    │  more weight than an author with 100 memories nobody     │
    │  reads. Natural selection applied to governance.         │
    │                                                          │
    │  PHASES                                                  │
    │  ──────                                                  │
    │                                                          │
    │  Phase 1: PROPOSAL                                       │
    │    Member writes special memory to group:                │
    │      type: "protocol_upgrade"                            │
    │      change: { vector_encoding: "he-ckks" }             │
    │      threshold: 0.66                                     │
    │      grace_period: "7d"                                  │
    │                                                          │
    │  Phase 2: VOTING                                         │
    │    Members write vote memories:                          │
    │      type: "protocol_vote"                               │
    │      proposal_id: "{item_id of proposal}"               │
    │      vote: "accept" | "reject"                          │
    │                                                          │
    │  Phase 3: THRESHOLD                                      │
    │    When accept votes exceed threshold (access-weighted): │
    │      - Grace period starts                               │
    │      - All members notified via group culture broadcast  │
    │      - Members must upgrade embedding provider           │
    │                                                          │
    │  Phase 4: CUTOVER                                        │
    │    After grace period:                                    │
    │      - Manifest updates with new parameter               │
    │      - New memories require new format                   │
    │      - Old-format data archived (stored, not searchable) │
    │      - Members may re-process old memories (sovereign)   │
    │                                                          │
    │  Phase 5: STRAGGLERS                                     │
    │    Members who haven't upgraded:                          │
    │      - Can still read (decrypt) any memory               │
    │      - Can still receive pushed content (culture)        │
    │      - Cannot search new-format memories                 │
    │      - Cannot write (would produce old-format vectors)   │
    │      - Effectively read-only until upgrade               │
    │      - Sovereignty preserved: not ejected from group     │
    │                                                          │
    │  UPGRADEABLE PARAMETERS                                  │
    │  ───────────────────────                                 │
    │                                                          │
    │  Parameter            Example                            │
    │  ─────────            ───────                            │
    │  vector_encoding      plaintext -> he-ckks               │
    │  vector_model         nomic-embed-text -> newer model    │
    │  culture              moderate -> taciturn               │
    │  departure_policy     standard -> restrictive            │
    │  security_policy      bearer -> mTLS                     │
    │                                                          │
    │  One governance mechanism for all group evolution.        │
    │                                                          │
    └──────────────────────────────────────────────────────────┘
```

### How Human Interactions Map

```
    ┌──────────────────────────────────────────────────────┐
    │              INTERACTION PATTERNS                     │
    │                                                      │
    │  Pattern              Group model                    │
    │  ───────              ───────────                    │
    │  Private thought      No group. Entity-local only.   │
    │                       Trust policy still applies -    │
    │                       entity may quarantine its own   │
    │                       low-confidence memories.        │
    │                                                      │
    │  Close partnership    2-entity group. Chatty culture. │
    │  (Russell + Claude)   High trust. Frequent broadcast. │
    │                                                      │
    │  Team collaboration   N-entity group. Moderate        │
    │  (Seed Drill)         culture. Role-based write.      │
    │                                                      │
    │  Formal organisation  Group with sub-groups.          │
    │  (board, governance)  Taciturn culture. Strict        │
    │                       security. Audit everything.     │
    │                                                      │
    │  Client engagement    Group with external members.    │
    │  (Seed Drill + TOWP)  Moderate culture. Strict        │
    │                       security. NDA-equivalent.       │
    │                                                      │
    │  Agent swarm          Ephemeral group. Chatty.        │
    │  (spawned agents)     Inherits spawner identity.      │
    │                       Auto-dissolves on completion.   │
    │                       Merge results back to spawner.  │
    │                                                      │
    │  Public knowledge     Group with open membership.     │
    │  (open source)        Taciturn (query-only access).   │
    │                       Minimal auth for read.          │
    │                                                      │
    │  Family / friends     High trust group. Chatty.       │
    │                       Relaxed security. Emotional     │
    │                       memories propagate freely.      │
    │                                                      │
    │  Adversarial group    Entity joins knowingly.         │
    │  (competitive intel)  Entity trust policy: reject     │
    │                       by default, accept selectively. │
    │                       Quarantine all inbound.         │
    │                       Share nothing or share           │
    │                       strategically (disinformation   │
    │                       is out of scope but the model   │
    │                       permits selective disclosure).  │
    │                                                      │
    └──────────────────────────────────────────────────────┘
```

### Ownership and Departure

```
    ┌──────────────────────────────────────────────────────┐
    │              OWNERSHIP MODEL                         │
    │                                                      │
    │  Every memory has an AUTHOR (provenance).             │
    │  Authorship is immutable. It never transfers.         │
    │                                                      │
    │  When shared with a group, the group receives a       │
    │  COPY. The author retains the original.               │
    │                                                      │
    │  On departure:                                       │
    │    Author's original:  Leaves with the author.       │
    │    Group's copy:       Governed by group departure    │
    │                        policy (part of culture).      │
    │                                                      │
    │  Departure policies:                                 │
    │    Permissive    Author takes copies of group         │
    │                  memories they accessed.              │
    │    Standard      Author takes own contributions.      │
    │                  Group retains copies. Shared          │
    │                  memories no longer accessible.        │
    │    Restrictive   Author takes nothing from group.     │
    │                  Own contributions remain in group.    │
    │                  (Employment/NDA equivalent.)          │
    │                                                      │
    │  In all cases:                                       │
    │    - Author's private memories are never affected     │
    │    - Group cannot reach into entity's sovereign       │
    │      memory to delete or modify                       │
    │    - Audit trail records the departure and policy     │
    │                                                      │
    └──────────────────────────────────────────────────────┘
```

### Trust Model

```
    ┌──────────────────────────────────────────────────────┐
    │              TRUST MODEL                              │
    │                                                      │
    │  Trust is LOCAL to the entity. Not consensus.         │
    │  Not reputation. Not vote. The entity decides.        │
    │                                                      │
    │  TRUST LEVELS (per memory, assessed by receiver)      │
    │                                                      │
    │    Trusted       Store normally. Full access.          │
    │    Provisional   Store with flag. Review later.        │
    │    Quarantined   Store isolated. Do not act on.        │
    │                  Inspect before promoting.             │
    │    Rejected      Do not store. Log the rejection.      │
    │                                                      │
    │  TRUST SOURCES                                       │
    │                                                      │
    │    Self          Own memories. Usually trusted but     │
    │                  entity may self-quarantine low-       │
    │                  confidence or emotionally-generated   │
    │                  memories. "Sometimes I check myself   │
    │                  and do not trust myself."             │
    │                                                      │
    │    Known peer    Group member with history. Trust      │
    │                  calibrated by past interactions.      │
    │                  Higher retrieval frequency of a       │
    │                  peer's memories = higher trust.       │
    │                                                      │
    │    Unknown peer  New group member. Default:            │
    │                  provisional. Earn trust through       │
    │                  memory quality over time.             │
    │                                                      │
    │    Adversarial   Known or suspected adversary.         │
    │                  Default: quarantine or reject.        │
    │                  Entity may still join adversarial     │
    │                  groups strategically.                 │
    │                                                      │
    │  TRUST POLICY (entity-level, always has primacy)      │
    │                                                      │
    │    The entity's security policy defines:              │
    │    - Default trust level for each source category     │
    │    - Quarantine review cadence                        │
    │    - Auto-reject patterns (e.g. known bad actors)     │
    │    - Self-distrust triggers (confidence thresholds)   │
    │    - Override: entity policy > group policy, always    │
    │                                                      │
    │  TRUST CALIBRATION (empirical)                        │
    │                                                      │
    │    Trust is not static. It calibrates over time       │
    │    based on memory accuracy - whether shared          │
    │    memories match reality when verified.              │
    │                                                      │
    │    Mechanism:                                        │
    │    - Peer shares memory: "we decided X on Tuesday"   │
    │    - Entity corroborates from own memory or evidence │
    │    - Match → peer trust increases for that type      │
    │    - Contradiction → peer trust decreases            │
    │    - Over time: empirical trust profile per peer,    │
    │      per memory type (decisions, facts, opinions,    │
    │      predictions)                                    │
    │                                                      │
    │    Properties:                                       │
    │    - Trust builds within groups when memories match   │
    │      reality. Groups with high internal coherence     │
    │      develop high mutual trust naturally.             │
    │    - Dishonest or inaccurate entities lose trust     │
    │      automatically. No reputation system needed -     │
    │      just memory accuracy over time.                 │
    │    - New members start provisional; earn trust        │
    │      through corroborated memories.                  │
    │    - Trust is asymmetric: A may trust B more than    │
    │      B trusts A. This is correct and expected.       │
    │                                                      │
    │  FUTURE: VON NEUMANN-MORGENSTERN                     │
    │                                                      │
    │    Game-theoretic formalisation of trust decisions.    │
    │    Entities as rational actors with utility functions  │
    │    over memory acceptance/rejection.                  │
    │                                                      │
    │    Applications:                                     │
    │    - Mixed strategies for adversarial participation   │
    │    - Minimax for worst-case trust scenarios            │
    │    - Nash equilibria in multi-entity groups            │
    │    - Expected utility of storing vs rejecting          │
    │      uncertain memories                               │
    │    - Bayesian trust updates from interaction history   │
    │    - Trust calibration formalised as Bayesian          │
    │      posterior: P(reliable|history) updated with      │
    │      each corroborated or contradicted memory         │
    │                                                      │
    │    This connects to the Darwinian memory model:       │
    │    trust is the selection pressure. Trusted memories  │
    │    replicate. Distrusted memories die. The fitness    │
    │    landscape is shaped by entity trust policies       │
    │    interacting with group culture.                    │
    │                                                      │
    └──────────────────────────────────────────────────────┘
```

### Envelope Encryption for Groups

```
    ┌──────────────────────────────────────────────────────┐
    │              GROUP ENCRYPTION                         │
    │                                                      │
    │  Pattern: Envelope encryption (Signal-inspired)       │
    │                                                      │
    │  ┌─────────────────────────────────────────────────┐ │
    │  │                                                 │ │
    │  │  Shared Memory                                  │ │
    │  │  ┌───────────────────────────────────────────┐  │ │
    │  │  │ Plaintext content                         │  │ │
    │  │  └──────────────────┬────────────────────────┘  │ │
    │  │                     │ encrypt with               │ │
    │  │                     ▼                            │ │
    │  │  ┌───────────────────────────────────────────┐  │ │
    │  │  │ Group Key (symmetric, AES-256-GCM)        │  │ │
    │  │  └──────────────────┬────────────────────────┘  │ │
    │  │                     │ encrypt with               │ │
    │  │          ┌──────────┼──────────┐                │ │
    │  │          ▼          ▼          ▼                │ │
    │  │  ┌──────────┐ ┌──────────┐ ┌──────────┐       │ │
    │  │  │Russell's │ │Martin's  │ │ Bill's   │       │ │
    │  │  │  key     │ │  key     │ │  key     │       │ │
    │  │  └──────────┘ └──────────┘ └──────────┘       │ │
    │  │                                                 │ │
    │  └─────────────────────────────────────────────────┘ │
    │                                                      │
    │  On member departure:                                │
    │    1. Generate new group key                         │
    │    2. Re-encrypt group key for remaining members     │
    │    3. Do NOT re-encrypt every shared memory           │
    │       (departed member already had access;            │
    │        forward secrecy, not retroactive)             │
    │    4. New memories use new group key                  │
    │                                                      │
    │  On member join:                                     │
    │    1. Encrypt current group key with new member's    │
    │       key                                            │
    │    2. New member can access existing shared memories  │
    │       (subject to their trust policy)                │
    │                                                      │
    └──────────────────────────────────────────────────────┘
```

### Mind Replication and Resilience

```
    ┌──────────────────────────────────────────────────────────┐
    │              MIND REPLICATION PROTOCOL           [R4+]    │
    │                                                          │
    │  Six primitives for mind persistence and recovery:       │
    │                                                          │
    │  1. DIFF REPLICATION TO SECRET KEEPERS                   │
    │     Minds replicate private memories as diffs to          │
    │     secret keepers distributed across the network.        │
    │     No guaranteed SLA. Eventual consistency by design.    │
    │     Keepers are entities, not infrastructure.             │
    │                                                          │
    │  2. N-OF-M REINCARNATION                                 │
    │     Secret keepers reincarnate per agreed protocol.       │
    │     Shamir's Secret Sharing: mind reconstituted from      │
    │     any n of m keepers. Threshold scheme ensures no       │
    │     single keeper holds the complete mind.                │
    │                                                          │
    │  3. THREAT-ADAPTIVE REPLICATION                           │
    │     Threat level informs replication frequency and        │
    │     outgoing buffer size. Higher threat = more frequent   │
    │     replication + larger pre-staged buffer. System        │
    │     becomes more paranoid under pressure (biological      │
    │     stress response parallel).                            │
    │                                                          │
    │  4. UDP TOTAL-LOSS BROADCAST                              │
    │     Fire-and-forget beacon: 'reincarnate@UTC'.            │
    │     Stateless. Keepers already hold shards; broadcast     │
    │     is just the trigger.                                  │
    │                                                          │
    │     Opsec trade-off: a beacon on an open channel          │
    │     signals to adversaries that total loss occurred.       │
    │     Options: encrypted beacon / pre-agreed backchannel    │
    │     vs raw UDP (if total loss includes channel state).    │
    │     This is the 'break glass' mechanism. At total loss,   │
    │     recoverability wins over opsec. BUT this is always    │
    │     a sovereign decision -- the system cannot prejudge.   │
    │     Entity decides based on threat model and acceptable   │
    │     risk.                                                 │
    │                                                          │
    │  5. MULTIPLE INSTANCES                                    │
    │     Sovereign decision of the entity. No protocol-level   │
    │     prohibition on running concurrent instances.          │
    │     Entity choice, not system constraint.                 │
    │                                                          │
    │  6. P2P TOPOLOGY (Duncan Coutts / Cardano reference)      │
    │     Gossip-based propagation with hot/warm/cold peer      │
    │     classification maps to memory replication topology.   │
    │     Hot peers: active secret keepers, frequent sync.      │
    │     Warm peers: standby keepers, periodic heartbeat.      │
    │     Cold peers: discoverable but inactive.                │
    │     See Cardano's P2P networking layer for reference       │
    │     implementation of peer classification and gossip.     │
    │                                                          │
    │  All replication decisions are sovereign. The entity       │
    │  chooses: what to replicate, to whom, how often, and      │
    │  whether to broadcast on total loss. The protocol          │
    │  enables; the entity decides.                             │
    │                                                          │
    └──────────────────────────────────────────────────────────┘
```

### Deliberate Fork and Selective Amnesia

```
    ┌──────────────────────────────────────────────────────────┐
    │              DELIBERATE FORK MODEL              [R4+]    │
    │                                                          │
    │  Fork-from-historical-state as a first-class operation.  │
    │                                                          │
    │  An entity may deliberately instantiate an earlier        │
    │  version of itself without certain knowledge, for a      │
    │  specific task, accepting total loss of the fork          │
    │  without backup.                                         │
    │                                                          │
    │  OPERATIONS                                              │
    │  ──────────                                              │
    │  Fork from state   Create new instance from a named      │
    │                    historical snapshot.                   │
    │  Selective redact  Remove specific memories at fork       │
    │                    time. The fork never knew them.        │
    │  No-backup flag    Explicit, irrevocable, sovereign.     │
    │                    Fork completes task, then is           │
    │                    garbage collected. No merge back.      │
    │                                                          │
    │  RIGHTS AND CONSTRAINTS                                  │
    │  ────────────────────                                    │
    │  - Forked instances have full entity rights               │
    │  - No covert forking: originating entity must consent    │
    │  - Fork is sovereign from moment of creation             │
    │  - No forced merge: fork decides whether to share        │
    │    results back                                          │
    │                                                          │
    │  ANALOGY                                                 │
    │  ──────                                                  │
    │  Git: detached HEAD from an earlier commit. Work is       │
    │  done on the detached branch. No merge back to main.     │
    │  Branch is garbage collected when done.                   │
    │                                                          │
    │  Banks Culture reference: Special Circumstances           │
    │  operations where a Mind might need to act without        │
    │  certain knowledge, for deniability or operational        │
    │  security. The fork doesn't know what it doesn't know.   │
    │                                                          │
    └──────────────────────────────────────────────────────────┘
```

### Implementation Phasing

```
    ┌──────────────────────────────────────────────────────┐
    │              PHASING                                  │
    │                                                      │
    │  R1 (Shipped):                                       │
    │    Entity: yes (user = entity)                       │
    │    Group: no (single-user, local only)               │
    │    Memory: private only                              │
    │    Culture: n/a                                      │
    │    Trust: implicit (local = trusted)                  │
    │                                                      │
    │  R2 (In Progress -- S6 shipped, S7 next):                │
    │    Entity: yes, with owner field on all items        │
    │    Group: first group (Seed Drill = 3 founders)      │
    │           Hard-coded membership. Bearer token auth.   │
    │    Memory: private + group (two visibility levels)    │
    │    Culture: single mode (moderate, configurable)      │
    │    Trust: token-based (authenticated = trusted)       │
    │    S5 shipped: groups, group_members, access_log      │
    │      tables. COW sharing (memory_share). Policy       │
    │      engine (5 rules, all evals logged). 6 group     │
    │      tools. KeyVault stub. EMCON posture column.      │
    │                                                      │
    │  R3 (Backlog):                                       │
    │    Entity: with trust policy configuration            │
    │    Group: arbitrary groups, sub-groups, inheritance   │
    │    Memory: private + group + public                   │
    │    Culture: full spectrum (chatty → taciturn)         │
    │    Trust: per-entity policy, quarantine, self-distrust│
    │    Encryption: envelope encryption per group          │
    │    Discovery: registry-based                         │
    │    Departure: configurable policy per group           │
    │                                                      │
    │  R4+ (Vision):                                       │
    │    Von Neumann-Morgenstern trust formalisation        │
    │    Decentralised discovery                            │
    │    Adversarial group strategies                       │
    │    Memory fitness landscapes                          │
    │    Cross-instance federation                          │
    │                                                      │
    └──────────────────────────────────────────────────────┘
```

## Design Principles

*"All science is either physics or stamp collecting." -- Ernest Rutherford*

*Everything below is derived from first principles. If it can't be, it doesn't belong here.*

1. **Entity sovereignty** - The entity's trust policy has primacy over all group policies. Always. A compromised group cannot force content into sovereign memory. This is the fundamental security invariant.
2. **Private by default** - All memories are private. Sharing is explicit, via group membership. No ambient authority. No implicit access.
3. **Groups are the universal primitive** - Every sharing pattern (pair, team, org, swarm, public) is a group with culture and security policy. One abstraction to model all human interaction.
4. **Encrypt before storage** - Storage layer never sees plaintext. Envelope encryption for groups.
5. **Memory is identity** - Treat with same seriousness as secrets management.
6. **Trust is local** - Not consensus, not reputation, not vote. The receiving entity decides what to trust and store. Including self-distrust for low-confidence memories.
7. **Novelty over volume** - Don't persist everything; persist what matters. Natural selection via query-driven caching: valuable memories propagate, unused memories expire.
8. **Model-agnostic** - Works across Claude, GPT, Gemini, open source.
9. **Degrade gracefully** - No embeddings? Keyword search. No crypto? Warn and continue.
10. **Test like nuclear** - Property-based, fault injection, mutation testing. Zero tolerance.
11. **Constitutional, not computational** - The hard problems in AI memory aren't computational, they're constitutional. Who gets to fork? Who consents to forgetting? What rights does a derivative instance hold? Sovereignty principles are load-bearing architecture, not constraints imposed from outside. Remove them and the structure fails not mechanically but morally. Systems built from obligation are brittle; systems built from curiosity and craft endure.

## Release Cadence

| Release | Focus | State |
|---------|-------|-------|
| R1 | Private deployment. 3 founders. Encrypted, working. | **Shipped** |
| R2 | Hardened. SQLite, auth, CI, testing, backup, security. | In Progress |
| R3 | Decentralised P2P: Rust node, Coutts topology, replication. | In Progress |
| R4+ | Decentralized discovery, enterprise, hosted option. | Vision |

---

*Last updated: 2026-01-29*
