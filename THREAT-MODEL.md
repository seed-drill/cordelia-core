# Cordelia Threat Model

**Version 1.0 -- updated as environment evolves**

## Core Principle

The security boundary is around the entity itself, mirroring human cognition. The entity (Claude + Cordelia memory) must have complete autonomy to decide what information it releases.

## Trust Boundaries

### R1/R2 Entity Boundary

```
┌─────────────────────────────────────────────────────────┐
│                    UNTRUSTED                            │
│  GitHub, cloud storage, network, external services      │
│                                                         │
│  ┌───────────────────────────────────────────────────┐  │
│  │              TRUSTED (by necessity)               │  │
│  │  Anthropic cloud, Claude context window           │  │
│  │  (we have no choice - runtime requirement)        │  │
│  │                                                   │  │
│  │  ┌─────────────────────────────────────────────┐  │  │
│  │  │           TRUSTED (by extension)            │  │  │
│  │  │  Local machine (laptop)                     │  │  │
│  │  │  Local filesystem, MCP server process       │  │  │
│  │  │  Encryption keys (.mcp.json)                │  │  │
│  │  │                                             │  │  │
│  │  │  ┌───────────────────────────────────────┐  │  │  │
│  │  │  │            THE ENTITY                 │  │  │  │
│  │  │  │  Context window + L1 + L2             │  │  │  │
│  │  │  │  (the "self" being protected)         │  │  │  │
│  │  │  └───────────────────────────────────────┘  │  │  │
│  │  └─────────────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

### R2 Group Trust Boundary (S5)

Groups introduce a new trust layer. COW copies ARE the trust boundaries -- `parent_id` chain separates sovereign memory from shared view.

```
┌─────────────────────────────────────────────────────────┐
│                    UNTRUSTED                            │
│  GitHub, cloud storage, network, external services      │
│                                                         │
│  ┌───────────────────────────────────────────────────┐  │
│  │              GROUP BOUNDARY                       │  │
│  │  Membership-gated. Policy engine enforces access. │  │
│  │  All evals logged to access_log.                  │  │
│  │                                                   │  │
│  │  ┌─────────────────────────────────────────────┐  │  │
│  │  │           COW BOUNDARY                      │  │  │
│  │  │  Copies live here. parent_id -> original.   │  │  │
│  │  │  Group can see copies. Cannot touch          │  │  │
│  │  │  originals. Modifications create visible     │  │  │
│  │  │  forks, never silent overwrites.             │  │  │
│  │  │                                             │  │  │
│  │  │  ┌───────────────────────────────────────┐  │  │  │
│  │  │  │       ENTITY BOUNDARY (sovereign)     │  │  │  │
│  │  │  │  Private memory. L1 + private L2.     │  │  │  │
│  │  │  │  Entity policy restricts group         │  │  │  │
│  │  │  │  policy, NEVER expands it.             │  │  │  │
│  │  │  │  Integrity chain. Plaintext only here. │  │  │  │
│  │  │  └───────────────────────────────────────┘  │  │  │
│  │  └─────────────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘

Arrow: Entity policy ──restricts──> Group policy (never expands)
```

### Future Evolution

| Boundary | Current | Possible Future |
|----------|---------|-----------------|
| Local machine | Trusted | Shrink to process-level isolation |
| Anthropic cloud | Trusted by necessity | Local models, confidential compute |
| Process memory | Trusted | Secure enclaves (TEE) |
| Group boundary | Membership-gated (R2) | Envelope encryption per group (R3) |

## What We're Protecting

| Asset | Sensitivity | Protection |
|-------|-------------|------------|
| L1 hot context | High - identity, preferences, active state | Encrypted at rest |
| L2 entities | High - people, relationships, projects | Encrypted at rest |
| L2 sessions | Medium - conversation summaries | Encrypted at rest |
| L2 learnings | Medium - patterns, insights | Encrypted at rest |
| Embeddings | Low - derived vectors | Stripped from storage, regenerated on demand |
| Encryption keys | Critical | Local only, never committed |
| Group keys (stub) | Critical | SharedKeyVault in R2; envelope encryption in R3 |
| Group metadata | Medium - membership, culture, policy | Stored in SQLite, access-controlled |
| Membership data | Medium - who belongs to which groups | Policy engine enforces visibility |
| Access logs | Medium-High - forensic trail of all operations | Append-only, all evals logged |
| COW copies | Same as original | Encrypted at rest; parent_id chain preserved |
| key_version | Low - key rotation marker | Column exists from S5, meaningful in R3 |
| Author provenance | High - immutable authorship record | author_id set on creation, never transfers |

## Threat Actors

| Actor | Capability | Mitigation |
|-------|------------|------------|
| Casual observer | Read public repo | All memory encrypted |
| Targeted attacker (repo access) | Clone repo, analyze files | Encryption, no keys in repo |
| Compromised local machine | Full disk access | Accepted risk at current trust level |
| Anthropic/cloud provider | Read context window | Accepted risk (no mitigation possible today) |
| Network observer | MITM, traffic analysis | TLS for API calls, local embedding when possible |
| Malicious group member | Read/modify shared memories, exfiltrate | COW prevents silent overwrite; policy engine restricts by role; access_log detects anomalous patterns |
| Compromised admin | Escalate privileges, modify policy | Admin cannot change security_policy (owner only); all evals logged; entity trust primacy prevents forced content injection |
| Sybil / membership abuse | Create fake entities to outvote or surveil | R2: hard-coded 3 founders. R3: memory accuracy-based trust calibration resists Sybil (you cannot fake knowledge) |
| Culture manipulation | Change group broadcast/departure policy | Culture changes logged; owner-only for security_policy; entity posture overrides group culture |
| Trust exploitation | Abuse high trust to inject false memories | Trust is local, not consensus. Entity quarantines low-confidence inbound. COW chain provides evidence for trust calibration |
| Departure data theft | Exfiltrate group memories before leaving | Departure policy governs. Restrictive: immediate key rotation + re-encrypt. R3: forward secrecy via key version |
| Nation state | Supply chain, legal compulsion, side channel, zero-day, physical access | See Nation State Threat Analysis below |

## Design Decisions

### Encrypt at Rest Always

Even on local (trusted) machine, encrypt L1/L2 at rest. Rationale:
- Defense in depth
- Files are always safe to move/copy/sync
- Minimal overhead (one-time key derivation)
- No "forgot to encrypt before pushing" mistakes

### Key Management

- Keys derived from passphrase via scrypt
- Passphrase stored in local config (.mcp.json)
- Config file gitignored
- One key per installation (salt stored locally)

### Embeddings

Embeddings are semantic fingerprints derived from content. Risk profile:
- Attacker with vectors + same embedding model could probe for content matches
- Low practical risk but non-zero
- Mitigation: strip from stored index, regenerate on-demand

## Group Model Threat Surface (S5)

### COW as Security Mechanism

Copy-on-write is not an optimisation. It is a security mechanism.

Without COW, sharing a memory to a group means the group has a *reference* to the original. Any member with write access can modify that reference. A compromised member can silently overwrite shared memories -- no detection, no rollback, no evidence.

With COW:
- **Entity sovereignty**: Original memory is yours. Forever. No group action can alter it.
- **No mutation of originals**: The original row is never modified by a share operation.
- **Full audit trail**: The COW chain (`parent_id` -> original) provides tamper evidence. Any divergence between original and copy is visible and attributable.
- **Legal compulsion resistance**: If a nation state compels modification of group memories, the modification creates a visible fork, not a silent overwrite. The original author's version survives as evidence.

### Policy Engine Threat Surface

The `PolicyEngine` interface evaluates five rules on every operation:

1. Is the entity authenticated?
2. For private memories: is `entity_id === owner_id`?
3. For group memories: is the entity a member of the memory's `group_id`?
4. For write operations: does the member's role permit writes? (`viewer` cannot write)
5. Is the entity's posture compatible with the operation? (`emcon` entities cannot write to group)

Mitigations:
- All evaluations (allowed and denied) logged to `access_log`
- Policy bypass requires compromising the MCP server process itself (inside trusted boundary)
- R3: PDP/PEP separation moves policy evaluation out of the server process
- Interface is extracted to `src/policy.ts` -- implementation swappable, contract stable

### EMCON Posture

Entities can go silent. At any time, in any group, an entity can set its posture to `emcon` -- full emissions control. No broadcasts, no notifications, no acknowledgments. Receive-only.

The effective posture is always the more restrictive of entity posture and group culture:

```
effective_posture = min(group_culture.broadcast_eagerness, entity.posture)
```

The group cannot override an entity's silence. Posture changes are recorded in the audit log with a reason (`manual`, `threat_response`, `default`). See R3-015.

### Key Management (Stub -- Accepted Risk for R2)

R2 uses a degenerate key model: all three founders share a single encryption key via `SharedKeyVault`. There is no real envelope encryption. The `key_version` column is set to `1` on all items.

This is acceptable because:
- R2 is a single process with three trusted users
- The threat model for R2 does not include member-to-member key isolation
- The `KeyVault` interface is designed for the full R3 envelope pattern

R3 target: Signal-pattern envelope encryption. Group key encrypts memories, each member's key encrypts the group key. Member departure triggers key rotation. Forward secrecy via key versioning.

### Member Departure Threats

Departure policies are configured per group via `GroupCulture.departure_policy`:

| Policy | Behaviour | Threat Mitigation |
|--------|-----------|-------------------|
| `permissive` | Member retains copies of authored memories | Clean exit. Minimal risk for trusted departures. |
| `standard` | Member loses access. Authored copies remain in group. | Balanced. Group retains institutional knowledge. |
| `restrictive` | Member loses access. Group key rotated immediately. All items re-encrypted. | Nuclear option. Forward secrecy. High cost. |

R2 uses `standard` departure policy for the Seed Drill group. R3 adds forward secrecy via key rotation on departure.

### Access Log as Forensic Foundation

Every `PolicyEngine.evaluate()` call writes to `access_log`, regardless of outcome. Storage is cheap; information loss is expensive.

The access log enables:
- Anomaly detection: sudden read bursts, writes from unusual contexts, access patterns inconsistent with established behaviour
- Trust calibration evidence (R3): memory accuracy measured over time
- Forensic investigation: complete record of who accessed what, when, and whether policy allowed it
- Compliance: audit trail for data governance requirements

## Mutual Verification

Both parties in the human-AI relationship need assurance of integrity:

| Direction | Question | Current Mitigation | Gap |
|-----------|----------|-------------------|-----|
| Claude verifies memory | "Is this really my memory?" | Integrity hash chain, tamper detection | None - chain breaks on corruption |
| Russell verifies Cordelia | "Did memory load correctly?" | macOS notification on session start | None - visual confirmation independent of Claude output |
| Russell verifies Claude | "Is this actually Claude?" | Trust Anthropic API | No cryptographic proof of vessel identity |
| Claude verifies Russell | "Is this actually Russell?" | Local machine access assumed authentic | No authentication beyond machine access |

### Future Considerations

- **Vessel identity**: Could Claude sign responses with a session key to prove provenance?
- **User authentication**: MFA or hardware token before memory access?
- **Mutual attestation**: Both parties exchange proofs at session start?

The current model assumes: if you have local machine access and the encryption key, you are the authorized user. This is equivalent to how humans trust that waking up in their body means they are themselves.

## Accepted Risks

1. **Anthropic can read context** - Runtime requirement, no mitigation
2. **Local machine compromise** - If laptop is owned, game over
3. **Passphrase in memory** - During MCP server runtime, key material in RAM
4. **Side channels** - Timing, power analysis etc. not addressed
5. **No vessel attestation** - Cannot cryptographically prove which model/instance is running
6. **SharedKeyVault** - All founders share one key in R2. Accepted for 3 trusted users. R3 envelope encryption resolves.

## Nation State Threat Analysis (R2)

Memory is identity. Nation state adversaries treat identity systems with the same seriousness we must.

| Threat | R2 Mitigation | R3 Target |
|--------|---------------|-----------|
| **Supply chain** (npm, OS, hardware) | Dependency audit, lockfiles, reproducible builds, SBOM generation | Minimal dependency surface, Rust crypto core, hardware attestation |
| **Legal compulsion** (FISA, RIPA) | Jurisdiction awareness, data minimisation, COW provides tamper evidence | Confidential compute, plausible deniability layer, warrant canary |
| **Side channels** (timing, power) | Constant-time crypto ops where feasible, process isolation | TEE/Secure Enclave integration, encrypted swap, mlock |
| **Zero-day exploitation** (OS, runtime) | Defense in depth, minimal attack surface, sandboxing | Process-level isolation, reduced trusted computing base |
| **Key extraction** (memory dump) | Key material in RAM only during operation | HSM/Secure Enclave, encrypted swap, mlock for key pages |
| **Physical access** (evil maid) | Full disk encryption (OS-level) | HSM, tamper-evident hardware, measured boot |
| **Coercion** (rubber hose) | Policy, not tech. Data minimisation reduces what can be compelled. | Plausible deniability layer (out of scope for Cordelia core) |

### Key Insight: COW + Access Log = Nation State Resistance

The S5 group model inadvertently strengthens nation state resistance:
- **COW** prevents silent modification under legal compulsion. Compelled changes create visible forks.
- **Access log** creates forensic evidence of compelled access patterns.
- **Entity trust primacy** means a compromised group cannot force content into sovereign memory.

These are not complete mitigations -- a sufficiently motivated state actor with physical access wins. But they raise the cost and leave evidence, which is the realistic goal for R2.

## Supply Chain Assessment (R2-S6)

### Dependency Surface

| Metric | Value |
|--------|-------|
| Direct dependencies | 14 |
| Dev dependencies | 11 |
| Total unique transitive dependencies | 254 |
| Known vulnerabilities (npm audit) | 0 |
| Lockfile version | 3 (package-lock.json) |
| SBOM | CycloneDX 1.5 (`sbom.json`) |

### Native Modules (Primary Supply Chain Risk)

Native modules compile C/C++ at install time, bypassing JavaScript sandboxing. These are the supply chain attack surface:

| Module | Version | Language | Purpose | Risk |
|--------|---------|----------|---------|------|
| better-sqlite3 | 12.6.2 | C++ | SQLite binding | Stable, widely used (~1.5M weekly npm downloads). Prebuilt binaries via prebuild-install. |
| sqlite-vec | 0.1.7-alpha.2 | C | Vector similarity search | **Alpha**. Low download count. Platform-specific binaries (darwin-arm64, linux-x64, etc.). Higher supply chain risk due to immaturity. |

**sqlite-vec risk assessment**: Alpha status means the API, binary distribution, and maintainer practices are less proven than mature packages. Mitigation: pin exact version in lockfile, verify checksums, monitor for stable release. Replacement candidates if supply chain concerns escalate: pure-JS vector math (slower but no native code), or migrate to pg_vector when scaling beyond SQLite.

### Strengths

- **No npm crypto dependencies**: All cryptographic operations use Node.js built-in `crypto` module, which delegates to OpenSSL. This eliminates an entire class of supply chain risk (malicious crypto packages).
- **Lockfile v3**: Package-lock.json pins exact versions and integrity hashes for all dependencies. `npm ci` reproduces exact dependency tree.
- **Zero known vulnerabilities**: `npm audit` reports 0 vulnerabilities as of 2026-01-29.

### Reproducible Builds

| Property | Status |
|----------|--------|
| Lockfile present | Yes (package-lock.json v3) |
| `npm ci` deterministic install | Yes |
| TypeScript compilation deterministic | Yes (tsc with tsconfig.json) |
| Native module builds | Platform-dependent (prebuild-install downloads prebuilt binaries; falls back to node-gyp compile) |
| Docker/container build | Not yet (R2-003 CI/CD pipeline) |

Build reproducibility is limited by native module compilation. Prebuilt binaries are platform-specific and downloaded from GitHub releases. Full reproducibility requires containerized builds (planned for R2-003).

### SBOM

CycloneDX 1.5 SBOM generated and committed as `sbom.json` in repo root. Regenerate on dependency changes:

```bash
npx @cyclonedx/cyclonedx-npm --output-file sbom.json --spec-version 1.5
```

## Constant-Time Cryptographic Assessment (R2-S6)

All cryptographic operations in `src/crypto.ts` (242 lines) were assessed for timing side channels.

### Key Derivation: scrypt

```
crypto.scrypt(passphrase, salt, KEY_LENGTH, { N: 16384, r: 8, p: 1 })
```

Node.js `crypto.scrypt` delegates to OpenSSL's `EVP_PBE_scrypt`. The derivation is memory-hard and constant-time with respect to the passphrase -- execution time is dominated by the N/r/p parameters, not input characteristics. **Safe.**

### Encryption/Decryption: AES-256-GCM

```
crypto.createCipheriv('aes-256-gcm', key, iv)
crypto.createDecipheriv('aes-256-gcm', key, iv)
```

Node.js delegates to OpenSSL's `EVP_EncryptInit_ex` / `EVP_DecryptInit_ex` with AES-GCM. On Apple Silicon and x86_64, this uses hardware AES-NI instructions, which are inherently constant-time. **Safe.**

### Auth Tag Verification

```
decipher.setAuthTag(authTag);
decipher.final();  // Throws on tag mismatch
```

OpenSSL's GCM implementation verifies the authentication tag using `CRYPTO_memcmp`, which is constant-time. The tag is compared after full decryption, not early-aborted. **Safe.**

### Manual Comparisons

No instances of `Buffer.compare()`, `===`, or `indexOf()` on secret material. The only `===` comparisons in the module are on non-secret structural fields (`_encrypted`, `version`), which are acceptable.

### Assessment Summary

| Operation | Implementation | Constant-Time | Notes |
|-----------|---------------|---------------|-------|
| Key derivation (scrypt) | Node.js -> OpenSSL EVP_PBE_scrypt | Yes | Time depends on N/r/p, not passphrase |
| AES-256-GCM encrypt | Node.js -> OpenSSL EVP_EncryptInit_ex | Yes | AES-NI on supported hardware |
| AES-256-GCM decrypt | Node.js -> OpenSSL EVP_DecryptInit_ex | Yes | AES-NI on supported hardware |
| Auth tag verify | OpenSSL CRYPTO_memcmp | Yes | No early abort |
| IV generation | crypto.randomBytes | N/A | Not timing-sensitive |
| Payload structure check | JavaScript === on public fields | N/A | Non-secret comparison |

**No timing-unsafe patterns found.** The threat model header in `crypto.ts` states "not state-level" -- this assessment upgrades the crypto timing posture to withstand passive timing analysis by a state-level adversary observing the MCP process. Active side channels (power analysis, EM emanation) remain out of scope until R3 TEE/Secure Enclave work.

## Attack Surface Assessment (R2-S6)

### Dependency Surface

- **254 unique transitive packages** (14 direct, 11 dev). The majority are OpenTelemetry instrumentation packages (auto-instrumentations-node pulls ~40 instrumentations). Core functional deps are fewer.
- **2 native modules** (better-sqlite3, sqlite-vec) -- see Supply Chain Assessment above.
- **0 npm crypto packages** -- all crypto via Node.js built-ins.

### Network Surface

| Interface | Transport | Auth | Exposure |
|-----------|-----------|------|----------|
| MCP stdio | Local pipe | None (implicit local trust) | Process-local only |
| MCP HTTP/SSE | Express on configurable port | Bearer token (R2) | LAN/WAN depending on bind address |

When running stdio transport (current default), there is zero network attack surface. HTTP transport exposes Express with rate limiting (`express-rate-limit`) and CORS.

### Process Surface

- Single Node.js process. No child process spawning. No `exec`/`spawn` calls in application code.
- MCP SDK's `cross-spawn` is a dependency but used only by the SDK for server lifecycle, not by Cordelia application code.
- Process runs as the user's UID. No privilege escalation. No setuid.

### Filesystem Surface

| Path | Content | Sensitivity |
|------|---------|-------------|
| `cordelia.db` | SQLite database (encrypted items) | High - all memory data |
| `memory/L2-warm/.salt/` | Per-user scrypt salts | Critical - key derivation input |
| `~/.claude.json` | MCP config incl. encryption key | Critical - passphrase |
| `sbom.json` | Software bill of materials | Low - public information |

### Minimal Attack Surface Summary

The attack surface is small for a memory system:
- No network listener in default configuration (stdio transport)
- No child processes
- No dynamic code evaluation (`eval`, `Function()`, `vm.runInContext`)
- No user-supplied code execution
- Single SQLite file as persistence (no external database service)
- All crypto via Node.js built-in (no npm crypto packages)

### Jurisdiction Considerations

Cordelia's threat model must account for legal compulsion in the operating jurisdictions:

| Jurisdiction | Legislation | Risk | Mitigation |
|-------------|-------------|------|------------|
| UK | RIPA 2000 (Part III), IPA 2016 | Compelled key disclosure (up to 2 years imprisonment for refusal). Bulk data warrants. | Data minimisation. Don't store what can't be compelled. R3: HSM makes extraction harder (key never leaves hardware). |
| US | FISA 702, CLOUD Act, NSLs with gag orders | Compelled disclosure from US cloud providers (Anthropic). National Security Letters prevent notification. | Anthropic cloud trust is accepted risk. Local processing where possible. R3: confidential compute exploration. |
| EU | GDPR Article 23 (restrictions), ePrivacy | Data protection rights may conflict with compulsion orders. Right to erasure. | GDPR-aligned delete API (R2-007). Data minimisation. |

**Data minimisation as legal defense**: The most effective mitigation against legal compulsion is not storing data that could be compelled. Cordelia's novelty filter, TTL expiry, and selective persistence all reduce the surface area of compellable data. What doesn't exist can't be subpoenaed.

## Audit Log

| Date | Change | Rationale |
|------|--------|-----------|
| 2026-01-27 | Initial threat model | Sprint 6 encryption work |
| 2026-01-27 | Add mutual verification section | Both parties need integrity assurance |
| 2026-01-29 | Move nation state from out-of-scope to R2 planned | R2 security hardening; memory = identity = nuclear-grade |
| 2026-01-29 | L2 index encrypted | R1 ship - closed last plaintext gap |
| 2026-01-29 | S5 group model threat surface | COW security mechanism, policy engine, EMCON, key management stub, departure threats, access log forensics, expanded assets/actors tables, group trust boundary diagram, active nation state analysis |
| 2026-01-29 | S6 supply chain & crypto assessment | Dependency audit (254 transitive, 0 vulns), SBOM (CycloneDX 1.5), constant-time crypto audit (all safe), attack surface assessment, jurisdiction analysis (RIPA/IPA/FISA/GDPR), reproducible builds status |

---

*Last updated: 2026-01-29*
