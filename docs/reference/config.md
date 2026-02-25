# Configuration Reference -- config.toml

All node configuration lives in a single TOML file. Default path: `~/.cordelia/config.toml`. Override with `--config <path>`.

**Source of truth**: `crates/cordelia-node/src/config.rs`

If the config file does not exist, all defaults apply and the node starts as a personal node.

---

## File Layout

```toml
[node]          # Identity, paths, role
[network]       # P2P listen address, bootnodes, relays
[governor]      # Peer pool sizing and churn
[replication]   # Sync intervals, tombstones, batch limits
[relay]         # Relay-only: forwarding posture and group filters
```

All sections except `[node]` are optional and default to sensible values.

---

## `[node]` -- Identity and Paths

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `identity_key` | String | `~/.cordelia/node.key` | Path to Ed25519 identity keypair. Auto-generated on first run. |
| `api_transport` | String | `unix` | API transport: `unix` (socket) or `http` (TCP). |
| `api_socket` | String? | `~/.cordelia/node.sock` | Unix socket path. Used when `api_transport = "unix"`. |
| `api_addr` | String? | `127.0.0.1:9473` | HTTP listen address. Used when `api_transport = "http"`. |
| `database` | String | `~/cordelia/memory/cordelia.db` | Path to SQLite database. Tilde-expanded. |
| `entity_id` | String | `default` | Entity identity for this node. Used as `author_id` for items. |
| `role` | String | `personal` | Node role: `personal`, `relay`, or `keeper`. See [Roles](#roles). |
| `groups` | String[] | `[]` | Initial group IDs. Seeded into storage and `shared_groups` on first boot. |

**Notes:**
- `api_socket` and `api_addr` are mutually exclusive based on `api_transport`.
- Paths support `~/` tilde expansion (resolved to `$HOME`).
- `entity_id` should be unique per human/agent. Multiple nodes can share the same `entity_id` (same person, different devices).
- `groups` is for static provisioning. Groups can also be added dynamically via the `groups/create` API at runtime.

---

## `[network]` -- P2P Network

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `listen_addr` | String | `0.0.0.0:9474` | QUIC listen address for peer connections. |
| `bootnodes` | BootnodeEntry[] | `[]` | Bootstrap peers. Added to cold pool on startup. |
| `trusted_relays` | BootnodeEntry[] | `[]` | Keeper-only: explicit relay allowlist. Ignored for other roles. |
| `external_addr` | String? | _(none)_ | Fixed external address override (e.g. `"relay.example.com:9474"`). |

### `[[network.bootnodes]]`

```toml
[[network.bootnodes]]
addr = "boot1.cordelia.seeddrill.ai:9474"

[[network.bootnodes]]
addr = "boot2.cordelia.seeddrill.ai:9474"
```

Each entry has a single `addr` field (hostname or IP with port). Bootnodes have no special authority -- they're just the initial peer discovery seeds.

### `[[network.trusted_relays]]`

Same format as bootnodes. Only used by `keeper` nodes. Keepers dial ONLY these relays and reject connections from untrusted peers.

### `external_addr`

Set this on nodes with a known public IP or DNS name (relays, boot nodes). Personal nodes behind NAT should leave this unset -- the address is learned automatically via quorum from connected peers.

---

## `[governor]` -- Peer Pool Management

Controls how many peers the node maintains at each state (cold, warm, hot) and how aggressively it rotates them.

| Field | Type | Default | Valid Range | Description |
|-------|------|---------|-------------|-------------|
| `hot_min` | Integer | `2` | >= 1 | Minimum hot (active replication) peers. |
| `hot_max` | Integer | `20` | >= `hot_min` | Maximum hot peers. |
| `warm_min` | Integer | `10` | >= 1 | Minimum warm (connected, standby) peers. |
| `warm_max` | Integer | `50` | >= `warm_min` | Maximum warm peers. |
| `cold_max` | Integer | `100` | >= 1 | Maximum cold (known, not connected) peers. |
| `churn_interval_secs` | Integer | `3600` | >= 60 | Seconds between warm peer rotation cycles. |
| `churn_fraction` | Float | `0.2` | 0.0 - 1.0 | Fraction of warm peers rotated per churn cycle. |

### Role-based caps

The `effective_governor_targets()` method caps configured values by role. You can set large values in the config, but they'll be silently capped:

| Role | `hot_min` | `hot_max` | `warm_min` | `warm_max` |
|------|-----------|-----------|------------|------------|
| **personal** | <= 2 | <= 5 | <= 5 | <= 10 |
| **keeper** | <= 1 | <= 3 | <= 2 | <= 5 |
| **relay** | _(uncapped)_ | _(uncapped)_ | _(uncapped)_ | _(uncapped)_ |

Personal and keeper nodes have small peer pools by design -- they're endpoints, not infrastructure. Relay nodes need larger pools to serve many clients.

---

## `[replication]` -- Sync and Retention

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `sync_interval_moderate_secs` | Integer | `300` | **Deprecated**. Moderate maps to chatty. Retained for config compatibility; ignored at runtime. |
| `sync_interval_taciturn_secs` | Integer | `900` | Anti-entropy sync interval for taciturn groups (seconds). |
| `tombstone_retention_days` | Integer | `7` | Days to retain deletion tombstones before garbage collection. |
| `max_batch_size` | Integer | `100` | Maximum items per memory fetch request. |

**Notes:**
- Defaults are sourced from the protocol era (`ERA_0` in `cordelia-protocol`). These are network-wide agreed values.
- Chatty groups use eager push (60s anti-entropy as safety net). Taciturn groups rely solely on anti-entropy at `sync_interval_taciturn_secs`.
- `max_batch_size` is a soft limit. The 512 KB message size is the hard limit -- large items may result in fewer items per batch.

---

## `[relay]` -- Relay Forwarding Policy

Only meaningful for nodes with `role = "relay"`. Ignored by personal and keeper nodes.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `posture` | String | `dynamic` | Forwarding policy: `transparent`, `dynamic`, or `explicit`. |
| `allowed_groups` | String[] | `[]` | Group allowlist (only used with `posture = "explicit"`). |
| `blocked_groups` | String[] | `[]` | Group denylist (applied on top of any posture). |

### Postures

| Posture | Behaviour | Use case |
|---------|-----------|----------|
| `transparent` | Accept and forward items for ANY group. | Backbone/boot nodes. |
| `dynamic` | Learn groups from connected non-relay peers via GroupExchange. | Edge relays (default). |
| `explicit` | Only forward groups listed in `allowed_groups`. | Locked-down edges, compliance. |

`blocked_groups` is always applied as a final deny filter, regardless of posture.

---

## Roles

The `node.role` field determines the node's network behaviour:

| Role | Gossip visibility | Dial policy | Governor caps | Relay section |
|------|-------------------|-------------|---------------|---------------|
| `personal` | Hidden | Dials relays and bootnodes only | hot <= 5, warm <= 10 | Ignored |
| `relay` | Visible in gossip | Dials all peers | Uncapped | Active |
| `keeper` | Hidden | Dials only `trusted_relays` | hot <= 3, warm <= 5 | Ignored |

**Personal** nodes are the default. They sit behind relays and have minimal network footprint.

**Relay** nodes are infrastructure. They appear in peer sharing responses and maintain large peer pools. Sub-types are controlled by `[relay].posture`:
- Boot nodes: `transparent` posture, known DNS names
- Edge relays: `dynamic` posture, learn groups from org peers

**Keeper** nodes are high-security storage. They only connect to pre-approved relays and never appear in gossip. Used for durable group memory.

---

## Bearer Token

The API bearer token is NOT in config.toml. It lives at `~/.cordelia/node-token`.

- If the file exists, its contents (trimmed) are used as the token.
- If the file does not exist, a random 48-character alphanumeric token is generated and written to the file.
- All API requests must include `Authorization: Bearer <token>` in the header.

---

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `HOME` | Used for tilde expansion in all paths (`~/` prefix). |
| `RUST_LOG` | Standard `tracing` log level filter (e.g. `info`, `debug`, `cordelia_node=debug`). |

No config fields are overridable via environment variables. All configuration is in the TOML file.

---

## CLI

```
cordelia-node [OPTIONS] [COMMAND]

Options:
  -c, --config <PATH>    Config file path [default: ~/.cordelia/config.toml]

Commands:
  status                 Print node status and exit
```

---

## Examples

### Personal node (simplest)

```toml
[node]
entity_id = "alice"
database = "~/cordelia/memory/cordelia.db"

[[network.bootnodes]]
addr = "boot1.cordelia.seeddrill.ai:9474"

[[network.bootnodes]]
addr = "boot2.cordelia.seeddrill.ai:9474"
```

Everything else defaults. Unix socket API, personal role, small peer pool.

### Personal node with HTTP API

```toml
[node]
entity_id = "russell"
api_transport = "http"
api_addr = "127.0.0.1:9473"
database = "~/cordelia/memory/cordelia.db"
groups = ["team-alpha", "shared-xorg"]

[[network.bootnodes]]
addr = "boot1.cordelia.seeddrill.ai:9474"

[[network.bootnodes]]
addr = "boot2.cordelia.seeddrill.ai:9474"
```

### Boot node (backbone relay)

```toml
[node]
identity_key = "/home/cordelia/.cordelia/node.key"
entity_id = "boot1"
role = "relay"
api_transport = "http"
api_addr = "0.0.0.0:9473"
database = "/home/cordelia/.cordelia/cordelia.db"

[relay]
posture = "transparent"

[network]
listen_addr = "0.0.0.0:9474"

[[network.bootnodes]]
addr = "boot2.cordelia.seeddrill.ai:9474"

[governor]
hot_min = 2
hot_max = 20
warm_min = 10
warm_max = 50
cold_max = 100
churn_interval_secs = 300
churn_fraction = 0.2
```

### Edge relay (dynamic)

```toml
[node]
entity_id = "edge-alpha"
role = "relay"
api_transport = "http"
api_addr = "0.0.0.0:9473"
database = "/home/cordelia/.cordelia/cordelia.db"

[relay]
posture = "dynamic"

[network]
listen_addr = "0.0.0.0:9474"
external_addr = "edge-alpha.alpha.internal:9474"

[[network.bootnodes]]
addr = "boot1.cordelia.seeddrill.ai:9474"

[[network.bootnodes]]
addr = "boot2.cordelia.seeddrill.ai:9474"
```

### Keeper (high-security storage)

```toml
[node]
entity_id = "vault"
role = "keeper"
api_transport = "http"
api_addr = "127.0.0.1:9473"
database = "/home/cordelia/.cordelia/cordelia.db"
groups = ["team-alpha", "shared-xorg"]

[network]
listen_addr = "0.0.0.0:9474"

[[network.trusted_relays]]
addr = "boot1.cordelia.seeddrill.ai:9474"

[[network.trusted_relays]]
addr = "boot2.cordelia.seeddrill.ai:9474"

[governor]
hot_min = 1
hot_max = 3
warm_min = 2
warm_max = 5
```

### Locked-down edge relay (explicit)

```toml
[node]
entity_id = "edge-locked"
role = "relay"
api_transport = "http"
api_addr = "0.0.0.0:9473"

[relay]
posture = "explicit"
allowed_groups = ["alpha-internal", "shared-xorg"]
blocked_groups = ["blacklisted-group"]

[network]
listen_addr = "0.0.0.0:9474"

[[network.bootnodes]]
addr = "boot1.cordelia.seeddrill.ai:9474"
```

---

## References

- [Protocol Specification](protocol.md) -- Era parameters that set replication defaults
- [Network Model](../architecture/network-model.md) -- Ship classes and topology
- [Replication Routing](../design/replication-routing.md) -- Culture-driven routing and relay behaviour
- [API Reference](api.md) -- HTTP endpoints served by the node
