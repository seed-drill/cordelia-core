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

The proxy speaks MCP over stdio to agents and HTTP to the local Rust node. The node handles storage, replication, and peer management. See the [architecture overview](docs/architecture/overview.md) and [HLD](docs/architecture/hld.md) for full design.

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

See [docs/README.md](docs/README.md) for the full documentation index, organised by audience:

- **Architecture**: [Overview](docs/architecture/overview.md), [HLD](docs/architecture/hld.md), [Network Model](docs/architecture/network-model.md), [Threat Model](docs/architecture/threat-model.md)
- **Design Decisions**: [Group Model](docs/design/R2-006-group-model.md), [Replication Routing](docs/design/replication-routing.md), [more...](docs/README.md#design-decisions----why-we-decided-this)
- **Reference**: [Protocol Spec](docs/reference/protocol.md), [Requirements](docs/reference/requirements.md)
- **Operations**: [Deployment](docs/operations/deploy.md), [Enrollment](docs/operations/enrollment.md)

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
