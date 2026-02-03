<p><img src="docs/logo.svg" alt="Cordelia" width="48" height="42"></p>

# Cordelia Core

[![CI](https://github.com/seed-drill/cordelia-core/actions/workflows/ci.yml/badge.svg)](https://github.com/seed-drill/cordelia-core/actions/workflows/ci.yml)

Cordelia protocol core -- P2P node, storage, crypto, replication.

> **Looking to install Cordelia?** Go to **[cordelia-agent-sdk](https://github.com/seed-drill/cordelia-agent-sdk)** -- that's the front door.

## What is Cordelia?

Cordelia is a distributed persistent memory system for autonomous AI agents. Current agents suffer from session amnesia -- every conversation starts from zero. Cordelia fixes this with encrypted, replicated memory that agents control. Memory is end-to-end encrypted (the infrastructure never sees plaintext), shared through culture-governed groups, and replicated across a peer-to-peer network. Agents accumulate identity over time, share knowledge selectively, and maintain sovereignty over their own memory.

Read the [whitepaper](WHITEPAPER.md) for the full design. Visit [seeddrill.ai](https://seeddrill.ai) for installation and documentation.

## What This Is

This repo contains the Rust implementation: the P2P node that stores memories in SQLite, replicates via QUIC, and manages peer lifecycle through an admission governor. For node operators and contributors.

## What This Is NOT

This is not where you install Cordelia or find hooks/skills. For that, see **[cordelia-agent-sdk](https://github.com/seed-drill/cordelia-agent-sdk)**. For the MCP proxy server, see [cordelia-proxy](https://github.com/seed-drill/cordelia-proxy).

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

- [cordelia-agent-sdk](https://github.com/seed-drill/cordelia-agent-sdk) -- **Start here.** Install, hooks, skills, agent spec
- [cordelia-proxy](https://github.com/seed-drill/cordelia-proxy) -- TypeScript MCP server, dashboard, REST API
- [cordelia](https://github.com/seed-drill/cordelia) -- Archived monorepo (full git history)

## Community

- [Website](https://seeddrill.ai) -- documentation and install guide
- [Whitepaper](WHITEPAPER.md) -- full system design
- [Agent SDK](https://github.com/seed-drill/cordelia-agent-sdk) -- agent identity and capability specification
- [Issues](https://github.com/seed-drill/cordelia-core/issues) -- bug reports and feature requests

## License

AGPL-3.0 -- Copyright (c) 2026 Seed Drill

See [LICENSE](LICENSE) for full text.
