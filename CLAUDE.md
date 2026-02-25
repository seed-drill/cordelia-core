# Cordelia -- Persistent Memory for AI Agents

**cordelia-core** -- Rust P2P node: storage, replication, wire protocol, governor.

Push back when something is wrong. Flag technical debt, architectural concerns, and safety issues before implementing.

## Team

| Name | Role | GitHub |
|------|------|--------|
| Russell Wing | Co-Founder | @russwing |
| Martin Stevens | Co-Founder | @budgester |

## Cross-Repo Architecture

| Repo | Purpose | Language | Visibility |
|------|---------|----------|------------|
| cordelia-core | Rust P2P node, storage, replication | Rust | public |
| cordelia-proxy | MCP server, HTTP sidecar, dashboard | TypeScript | public |
| cordelia-agent-sdk | Installer, hooks, skills | Shell/JS | public |
| cordelia-portal | OAuth portal, device enrollment, vault | JS/Express | private |

## Current Status

R3 near-complete (S10 remaining: MCP proxy package). Portal PS8-9 next. E2E CI pipeline green (16/0, 5m15s on self-hosted runner).

**Delivery Board:** https://github.com/orgs/seed-drill/projects/1

**Priority items:**

1. [Review P2P network design](https://github.com/seed-drill/cordelia-core/issues/7) -- memory/group propagation lifecycle, documentation gaps (review posted)
2. [Group invites](https://github.com/seed-drill/cordelia-portal/issues/2) -- invite-by-link, user directory, entity discovery
3. [Vault + device polish](https://github.com/seed-drill/cordelia-portal/issues/3) -- passphrase strength, device removal
4. [E2E test harness](https://github.com/seed-drill/cordelia-core/issues/5) -- Docker orchestrator optimisation

**Recently completed:**

- [GroupExchange investigation](https://github.com/seed-drill/cordelia-core/issues/6) -- root cause was jq test bug, propagation works (105s 2-hop)
- [P2P replication e2e test](https://github.com/seed-drill/cordelia-core/issues/4) -- CI smoke suite, 7-node Docker topology, org-wide runner

## Shared Conventions

- Commit format: `type: description` (feat/fix/docs/refactor/chore), under 72 chars
- Co-author line: `Co-Authored-By: Claude <model> <noreply@anthropic.com>`
- Never commit secrets (.env, credentials, keys)
- Never force push to main
- No emojis unless requested

## What Goes Where

- P2P protocol, storage, replication -> cordelia-core
- MCP tools, search, encryption, dashboard -> cordelia-proxy
- Install scripts, hooks, agent integration -> cordelia-agent-sdk
- Web UI, OAuth, enrollment, vault -> cordelia-portal
- Strategy, roadmap, actions, backlog -> seed-drill/strategy-and-planning

---

# Cordelia Core -- Claude Instructions

## Project Structure

Rust workspace with 7 crates under `crates/`. See README.md for crate descriptions.

## Build Commands

```bash
cargo build --workspace          # Build all crates
cargo test --workspace           # Run all tests
cargo clippy -- -D warnings      # Lint (must pass clean)
cargo fmt --check                # Format check
```

## Key Architecture

- **cordelia-node**: Main binary, orchestrates all subsystems
- **cordelia-protocol**: QUIC wire protocol with mini-protocols (ping, push, pull, governor)
- **cordelia-governor**: Admission control with exponential backoff, prevents same-tick oscillation
- **cordelia-replication**: Push-based replication, group-aware, culture-driven broadcast
- **cordelia-storage**: SQLite with schema v4, WAL mode
- **cordelia-crypto**: Ed25519 identity, scrypt KDF, AES-256-GCM
- **cordelia-api**: HTTP API on port 9473 for local proxy communication

## Conventions

- All public APIs documented with rustdoc
- Error handling via `thiserror` for library crates, `anyhow` for binary
- Async runtime: tokio (full features)
- Tests: unit tests in-module, integration tests in `tests/` where needed
- Property-based testing with `proptest` where appropriate

## Related Repo

The MCP proxy (TypeScript) lives in [cordelia-proxy](https://github.com/seed-drill/cordelia-proxy). [docs/architecture/hld.md](docs/architecture/hld.md) describes the proxy-node interaction boundary.

## MANDATORY: Safety Principles

1. **Memory is sacred** -- storage changes require extreme care, always have rollback path
2. **Fail safe** -- preserve existing data on failure, never overwrite good data with bad
3. **Test isolation** -- tests must never touch production data paths

## MANDATORY: P2P Network Design Rules

These are architectural invariants. Do not violate them without explicit team approval.

### Copy-on-Write (CoW)

- **No hard deletes** -- all deletions use tombstone markers (soft-delete), never `DELETE FROM`
- Items: `is_tombstone=true` on the L2 item row (existing pattern in replication engine)
- Groups: `culture = "__deleted__"` sentinel on the group descriptor row
- Members: `posture = "removed"` on the group_members row (never delete the row)
- **GC is the only path to physical deletion**, and only after the retention window (`TOMBSTONE_RETENTION_DAYS`, default 7). GC runs daily in the replication task.
- **CoW on share**: sharing personal memory creates a copy (`is_copy=true`, `parent_id` links to original). Original is never modified.

### Replication Semantics

- **LWW (Last-Writer-Wins)** by `updated_at` for both items and group descriptors
- **Three-gate routing**: (1) peer is target, (2) peer has group, (3) local node has group
- **Culture-driven broadcast**: chatty = eager push, taciturn = anti-entropy only
- **Checksums are mandatory** -- reject items/descriptors that fail checksum verification
- **Signatures on group descriptors** -- Ed25519, verified on merge

### Separation of Concerns

- **P2P layer**: descriptor propagation, item replication, anti-entropy sync
- **Portal layer**: identity, membership, invitation, OAuth
- **Proxy layer**: encryption, search, MCP interface, key management
- **Members are local-only** (R4-030) -- membership does not replicate via GroupExchange. Only group descriptors (culture, signature) propagate between peers.

### Metadata Visibility

- Item content is encrypted (AES-256-GCM at proxy layer) -- nodes and relays see only blobs
- Metadata is plaintext on the wire: `group_id`, `author_id`, `item_type`, `updated_at`, `checksum`
- This is a known and accepted trade-off ("manifest is not cargo")
- A compromised relay can observe traffic patterns but cannot read content or forge items
