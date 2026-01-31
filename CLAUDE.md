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

The MCP proxy (TypeScript) lives in [cordelia-proxy](https://github.com/seed-drill/cordelia-proxy). HLD.md describes the proxy-node interaction boundary.

## MANDATORY: Safety Principles

1. **Memory is sacred** -- storage changes require extreme care, always have rollback path
2. **Fail safe** -- preserve existing data on failure, never overwrite good data with bad
3. **Test isolation** -- tests must never touch production data paths
