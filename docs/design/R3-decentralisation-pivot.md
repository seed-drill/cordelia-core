# R3 Architectural Pivot: Decentralised P2P Network

**Decision**: 2026-01-29
**Decided by**: Russell Wing
**Status**: APPROVED

## Context

R3 was planned as hub-and-spoke: central Cordelia server on Martin's KVM, all founders connect to it. During S6 (cache coherence) design, we identified that:

1. A central server is a single point of failure and governance attack vector
2. The proxy pattern (MCP shim between agent and server) naturally enables peer-to-peer
3. Group culture already maps to replication protocols (chatty/moderate/taciturn)
4. COW semantics + hash chains are CRDT-friendly -- append-only, convergent
5. Entity trust primacy means no peer can force content into sovereign memory

The central server is architecturally wrong. It must be eliminated, not deferred.

## Decision

### 1. Fully decentralised P2P network from R3

No central server. Every Cordelia installation is a full peer node. Some nodes are always-on (bootnodes), others are intermittent. The always-on node is a peer with higher uptime, not a server.

Adversarial assumption (Satoshi model): we must assume the network may be seen adversarially. No single point of seizure, subpoena, or shutdown.

### 2. Rust for the P2P layer and eventually everything

- P2P networking layer: Rust from day one
- Port remaining TypeScript to Rust over time
- Not Haskell: it has been a millstone for Cardano developers despite sound theory
- Rust gives: memory safety, single binary, reverse engineering resistance, no runtime

### 3. Duncan Coutts' Cardano P2P topology as the network layer

Reuse the Coutts P2P design (not the Haskell implementation). Port the protocol to Rust.

Mapping:

| Cardano P2P | Cordelia |
|-------------|----------|
| Block propagation | Group memory replication |
| Hot peer | Active group member, chatty culture |
| Warm peer | Active group member, moderate culture |
| Cold peer | Inactive or taciturn member |
| Stake pool / relay | Bootnode (always-on peer) |
| Light client | Intermittent node (laptop) |
| Block fetch | Memory item fetch (write-invalidate pull) |
| Chain sync | Offline catch-up (reconnecting node) |

Russell and Martin run bootnodes until the network grows. Same as early Bitcoin.

### 4. Thin MCP proxy as separate package

```
Agent <--stdio--> @cordelia/proxy <--P2P--> Peer nodes
```

- `@cordelia/proxy`: thin, stateless (apart from L0 cache), disposable
- Speaks standard MCP to any agent (model-agnostic)
- Connects to local Cordelia node or directly to peers
- Coherence is invisible to the agent layer

### 5. Central website for onboarding and discovery only

- seeddrill.ai provides onboarding, docs, discovery tooling
- Not required to run Cordelia -- people can run without touching us at all
- No data flows through the website
- No governance authority over the network

## Architecture

### Node Structure

Every Cordelia node:
- Stores entity's sovereign memory (SQLite, encrypted)
- Serves MCP to local agents (stdio)
- Connects to peer nodes for group replication (Rust P2P)
- Applies culture-governed coherence
- Is a full participant, not a client

### Replication Protocol (Culture-Governed)

Group culture IS the replication protocol:

| Culture | Replication | Consistency | Cardano Analogue |
|---------|------------|-------------|-----------------|
| Chatty | Eager push (full item) | Strong | Block propagation |
| Moderate | Notify-and-fetch | Eventual | Compact blocks / block fetch |
| Taciturn | Periodic sync / TTL | Weak | Initial block download |

### Cache Hierarchy

| Layer | Location | Latency |
|-------|----------|---------|
| L0 | Proxy process memory | Sub-ms |
| L1 | Local node hot context | Disk I/O |
| L2 | Local node SQLite | Disk + query |
| Peer | Remote node via P2P | Network RTT |

### Security Model

- Entity trust primacy: no peer can force content into sovereign memory
- Encryption at rest: AES-256-GCM (already built)
- No central authority: consensus via group membership, not governance
- Adversarial assumption: design as if the network will be attacked

## What Changes

### R3-017 (Rust Assessment): DECIDED

Full Rust. Not hybrid, not deferred. P2P layer first, then port remaining TS.

### R3-018 (Team Deployment): REWRITTEN

Was: central server on KVM.
Now: peer network. Each founder runs a node. Russell + Martin run bootnodes.

### R3-009 (Rust Core): PROMOTED to R3

Was deferred to R4. Now core R3 deliverable.

### R3-001 (Federation): SUBSUMED

P2P network IS federation. Discovery via bootnode peer lists initially, gossip protocol later.

### New: @cordelia/proxy package

Thin MCP proxy. Separate from main node. ~300 lines.

### New: Coutts P2P protocol port

Rust implementation of Cardano P2P topology management. Hot/warm/cold peer classification. Connection management, churn, NAT traversal.

## Migration Path

1. **R3 (now)**: Rust P2P layer. TS MCP server behind proxy. Peer replication for groups.
2. **R4**: Port storage + crypto to Rust. TS becomes thin MCP wrapper only.
3. **R5**: Full Rust. Single binary. TS eliminated.

TypeScript remains the agent-facing MCP layer until Rust MCP SDK matures or we write our own.

## Risks

1. **Rust learning curve for Martin**: Mitigated -- Martin has systems background, Rust is learnable, P2P protocol is well-specified
2. **Timeline**: P2P is harder than hub-and-spoke. Accepted -- architectural debt costs more
3. **NAT traversal for intermittent nodes**: Solved problem in Cardano P2P, port the solution
4. **Conflict resolution**: COW + version chains handle most cases. True conflicts (concurrent writes to same item) need merge strategy -- last-writer-wins initially, CRDT merge in R4

## References

- Duncan Coutts, Cardano P2P networking: topology management, peer classification
- Satoshi Nakamoto, Bitcoin: adversarial network assumptions, no central authority
- Banks, The Culture: entity sovereignty, capability parity, Special Circumstances
- R2-006 group model design: groups as universal sharing primitive
- R4-012 mind replication protocol: Shamir shards, secret keepers, bootnode topology
