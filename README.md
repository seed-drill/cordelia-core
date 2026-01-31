# Cordelia Core

[![CI](https://github.com/seed-drill/cordelia-core/actions/workflows/ci.yml/badge.svg)](https://github.com/seed-drill/cordelia-core/actions/workflows/ci.yml)

Cordelia protocol core -- P2P node, storage, crypto, replication.

## Overview

Cordelia is a distributed persistent memory system for AI agents. This repo contains the Rust implementation: the P2P node that stores memories in SQLite, replicates via QUIC, and manages peer lifecycle through an admission governor.

For the MCP proxy, dashboard, session hooks, and Claude Code integration, see [cordelia-proxy](https://github.com/seed-drill/cordelia-proxy).

## Architecture

```
Claude Code --> @cordelia/proxy (TS, stdio) --> cordelia-node (Rust, HTTP :9473) --> peers (QUIC :9474)
```

The proxy speaks MCP over stdio to agents and HTTP to the local Rust node. The node handles storage, replication, and peer management. See [ARCHITECTURE.md](ARCHITECTURE.md) and [HLD.md](HLD.md) for full design.

## Crates

| Crate | Description |
|-------|-------------|
| `cordelia-node` | Main binary -- CLI, config, task orchestration |
| `cordelia-protocol` | Wire protocol -- QUIC codec, TLS, message types |
| `cordelia-governor` | Admission control -- peer lifecycle, backoff |
| `cordelia-replication` | P2P replication engine |
| `cordelia-storage` | SQLite storage backend |
| `cordelia-crypto` | Identity, encryption, key derivation |
| `cordelia-api` | HTTP API (axum) |

## Building

```bash
cargo build --workspace
cargo test --workspace
cargo clippy -- -D warnings
```

## Documentation

- [ARCHITECTURE.md](ARCHITECTURE.md) -- Full system architecture
- [HLD.md](HLD.md) -- High-level design (proxy + node interaction)
- [REQUIREMENTS.md](REQUIREMENTS.md) -- Formal requirements specification
- [SPEC.md](SPEC.md) -- Protocol specification
- [NETWORK-MODEL.md](NETWORK-MODEL.md) -- Network model and topology
- [DEPLOY.md](DEPLOY.md) -- Deployment guide
- [THREAT-MODEL.md](THREAT-MODEL.md) -- Security threat model

## Related Repos

- [cordelia-proxy](https://github.com/seed-drill/cordelia-proxy) -- MCP proxy, dashboard, hooks, Claude integration (TypeScript)
- [cordelia](https://github.com/seed-drill/cordelia) -- Archived monorepo (full git history)

## License

AGPL-3.0 -- Copyright (c) 2026 Seed Drill

See [LICENSE](LICENSE) for full text.
