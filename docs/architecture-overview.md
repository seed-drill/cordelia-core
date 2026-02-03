# Cordelia Architecture Overview

See [architecture-diagram.drawio](architecture-diagram.drawio) for the visual diagram
(open in [diagrams.net](https://app.diagrams.net)).

## Component Summary

```
Entity (Human + LLM)
  |
  | stdio (MCP / JSON-RPC)
  v
@cordelia/proxy (TypeScript)         cordelia-portal
  - 25 MCP tools                       - Auth, enrollment, vault
  - HTTP REST sidecar (:3847)  <----   - Device management
  - Novelty engine                     - Groups UI
  - Embedding (Ollama)
  - Encryption boundary (AES-256-GCM)
  |
  | SQLite (WAL, schema v4)
  |
  v
cordelia-node (Rust)
  - cordelia-api (Axum, 11 endpoints)
  - Governor (cold/warm/hot/banned)
  - Replication (culture-governed, anti-entropy)
  - QUIC transport (quinn, UDP 9474)
```

## Key Principles

- **Proxy is the universal API gateway.** Portal, Claude Code, and any MCP-capable
  client all talk to the proxy. The node is dumb transport (TCP/IP analogy).
- **Encryption boundary sits in the proxy.** The node never sees plaintext.
- **Portal never talks to the node directly.** All memory and group operations
  go through the proxy HTTP sidecar.
- **Entity sovereignty.** Each entity holds its own keys and controls its own data.

## Data Flow

1. **Claude Code** connects to proxy via stdio (MCP JSON-RPC)
2. **Portal** connects to proxy via HTTP REST (localhost:3847)
3. **Proxy** reads/writes SQLite directly for memory operations
4. **Proxy** optionally connects to node for P2P network status
5. **Node** handles QUIC transport, governor, and replication

## Schema (SQLite v4)

| Table | Purpose |
|-------|---------|
| `l1_hot` | Entity identity (~50KB) |
| `l2_items` | All memories (encrypted blob, group_id, author_id) |
| `l2_fts` | FTS5 full-text search (BM25, porter) |
| `embedding_cache` | Content hash to vector |
| `groups` + `group_members` | Group model (culture, security_policy, roles) |
| `access_log` + `audit` | Access tracking and audit trail |
| `integrity_canary` | Tamper detection |
| `schema_version` | Migration tracking |
