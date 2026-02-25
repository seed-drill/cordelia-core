# Cordelia Core -- Documentation

Master index for all cordelia-core documentation. See also the [Whitepaper](../WHITEPAPER.md) for the full system design.

## Architecture -- "How it works"

System design and theory. Audience: engineers, contributors.

| Document | Description |
|----------|-------------|
| [Overview](architecture/overview.md) | Full system architecture: components, deployment models, storage, auth, caching |
| [High-Level Design](architecture/hld.md) | Proxy-node interaction, component map, interface contracts |
| [Network Model](architecture/network-model.md) | P2P topology, governor Markov chain, ship classes, stability analysis |
| [Threat Model](architecture/threat-model.md) | Trust boundaries, attack surface, security controls |
| [Architecture Diagram](architecture-diagram.drawio) | Visual diagram (open in [diagrams.net](https://app.diagrams.net)) |

## Design Decisions -- "Why we decided this"

Append-only decision records. Audience: contributors.

| Document | Description |
|----------|-------------|
| [R2-006 Group Model](design/R2-006-group-model.md) | Seven constraints, schema, COW semantics, culture object |
| [R3 Decentralisation Pivot](design/R3-decentralisation-pivot.md) | Why fully decentralised, Cardano topology port, Rust decision |
| [R4-030 Group Metadata Replication](design/R4-030-group-metadata-replication.md) | GroupExchange protocol, descriptor signing, manifest/cargo separation |
| [R5 Personal Groups](design/R5-personal-groups.md) | "Every item belongs to a group" unification, PSK encryption, vault |
| [Game Theory](design/game-theory.md) | Bayesian trust, cooperation equilibrium, adversarial analysis |
| [Memory Architecture](design/memory-architecture.md) | Three-domain model (values/procedural/interrupt), schema-free wire protocol |
| [Replication Routing](design/replication-routing.md) | Three-gate routing model, relay behaviour, deployment patterns, timing |
| [Member Removal](design/member-removal.md) | Soft removal (R4), threat model, hard removal with key rotation (R5) |

## Reference -- "Look it up"

Specifications and lookup tables. Audience: integrators, operators.

| Document | Description |
|----------|-------------|
| [Protocol Specification](reference/protocol.md) | Crate structure, peer protocol, message types, mini-protocols |
| [Requirements](reference/requirements.md) | Formal requirements (FR/NFR/IR/TR) with verification criteria |
| [API Reference](reference/api.md) | HTTP endpoints on port 9473 -- request/response shapes, defaults, errors, side effects |
| [Config Reference](reference/config.md) | config.toml fields, defaults, valid ranges, role-based caps, examples |

## Operations -- "How to run it"

Deployment and operational procedures. Audience: operators.

| Document | Description |
|----------|-------------|
| [Deployment Guide](operations/deploy.md) | Fly.io deployment, CI/CD, boot nodes, proxy |
| [Device Enrollment](operations/enrollment.md) | RFC 8628 device auth flow, portal-proxy-node sequence |

## Guides -- "How to use it"

User journeys and walkthroughs. Audience: users, portal developers.

| Document | Description |
|----------|-------------|
| [Group Lifecycle](guides/group-lifecycle.md) | Create, invite, bootstrap, use, leave, delete -- end-to-end journey ([#8](https://github.com/seed-drill/cordelia-core/issues/8)) |

## Testing

Test documentation lives with the test code:

| Document | Description |
|----------|-------------|
| [E2E Testing Guide](../tests/e2e/E2E-TESTING.md) | Docker topology, CI smoke tests, API helpers, troubleshooting |

## Rules

- **Root level**: only README.md, CLAUDE.md, WHITEPAPER.md, LICENSE, config files
- **docs/README.md**: this file -- every doc listed with one-line description
- **docs/design/**: append-only decision records (R-prefixed), never restructure
- **docs/reference/**: lookup docs, keep current with code
- **New docs**: go in the appropriate category, add to this index
- **Test docs**: stay with tests (e.g. `tests/e2e/E2E-TESTING.md`)
