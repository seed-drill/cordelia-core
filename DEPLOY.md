# Cordelia Node -- Deployment Guide

Step-by-step instructions for deploying a `cordelia-node` instance.

## Prerequisites

- Linux host (Debian/Ubuntu tested) with KVM or bare metal
- Rust toolchain (`rustup`, stable channel)
- Git access to `git@github.com:seed-drill/cordelia-core.git`
- DNS record pointing to the host (e.g. `boot2.cordelia.seeddrill.io`)
- UDP port 9474 open inbound (QUIC)

## 1. SSH Access

Current topology uses a jump host:

```bash
# From your machine -> jump host -> target
ssh rezi@dooku
ssh cordelia@<target-host>
```

Note: `-J` (ProxyJump) does not work with the current dooku config. Use two-hop manually.

## 2. Install Rust (if not present)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustup default stable
```

## 3. Clone and Build

```bash
cd ~
git clone git@github.com:seed-drill/cordelia-core.git
cd cordelia-core
cargo build --release
```

Binary: `target/release/cordelia-node`

## 4. Node Identity

The node auto-generates an Ed25519 PKCS#8 keypair at `~/.cordelia/node.key` on first run. No manual key generation needed.

```bash
mkdir -p ~/.cordelia
```

## 5. Create Config

```bash
cat > ~/.cordelia/config.toml << 'EOF'
[node]
identity_key = "~/.cordelia/node.key"
api_transport = "http"
api_addr = "127.0.0.1:9473"
database = "~/.cordelia/cordelia.db"
entity_id = "martin"  # Change to your entity ID

[network]
listen_addr = "0.0.0.0:9474"

[[network.bootnodes]]
addr = "boot1.cordelia.seeddrill.io:9474"

# Add more bootnodes as they come online:
# [[network.bootnodes]]
# addr = "boot2.cordelia.seeddrill.io:9474"

[governor]
hot_min = 2
hot_max = 20
warm_min = 10
warm_max = 50

[replication]
sync_interval_moderate_secs = 300
tombstone_retention_days = 7
max_batch_size = 100
EOF
```

## 6. Create systemd Service

```bash
sudo tee /etc/systemd/system/cordelia-node.service << 'EOF'
[Unit]
Description=Cordelia P2P Node
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=cordelia
Group=cordelia
ExecStart=/home/cordelia/cordelia/cordelia-node/target/release/cordelia-node --config /home/cordelia/.cordelia/config.toml
WorkingDirectory=/home/cordelia
Restart=on-failure
RestartSec=5

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/home/cordelia/.cordelia
PrivateTmp=true

# Environment
Environment=RUST_LOG=info,cordelia_node=debug

[Install]
WantedBy=multi-user.target
EOF
```

Grant passwordless service management:

```bash
# Add to /etc/sudoers.d/cordelia
cordelia ALL=(ALL) NOPASSWD: /usr/bin/systemctl start cordelia-node, \
    /usr/bin/systemctl stop cordelia-node, \
    /usr/bin/systemctl restart cordelia-node, \
    /usr/bin/systemctl status cordelia-node, \
    /usr/bin/journalctl -u cordelia-node*
```

## 7. Start the Node

```bash
sudo systemctl daemon-reload
sudo systemctl enable cordelia-node
sudo systemctl start cordelia-node
sudo journalctl -u cordelia-node -f
```

You should see:
```
cordelia-node listening on 0.0.0.0:9474
API server on 127.0.0.1:9473
```

The SQLite database auto-initialises on first run (schema v4).

## 8. Create Groups

After the node starts, create shared groups via the API. A bearer token is auto-generated at `~/.cordelia/node-token` on first run.

```bash
TOKEN=$(cat ~/.cordelia/node-token)
curl -s http://127.0.0.1:9473/api/v1/groups/create \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"group_id":"seed-drill","name":"Seed Drill","culture":"moderate","security_policy":"{}"}' | jq .
```

Both nodes must have the same group IDs for replication to work.

## 9. Verify Peer Connection

Check logs for handshake:

```bash
sudo journalctl -u cordelia-node --since "5 min ago" | grep -E "(handshake|connected|peer)"
```

Check peer status via API or CLI:

```bash
# CLI (recommended)
cordelia-node --config ~/.cordelia/config.toml status
cordelia-node --config ~/.cordelia/config.toml peers
cordelia-node --config ~/.cordelia/config.toml groups

# Or via curl (requires bearer token)
TOKEN=$(cat ~/.cordelia/node-token)
curl -s -X POST http://127.0.0.1:9473/api/v1/status \
  -H "Authorization: Bearer $TOKEN" | jq .
curl -s -X POST http://127.0.0.1:9473/api/v1/peers \
  -H "Authorization: Bearer $TOKEN" | jq .
```

## 10. Verify Replication

Write a test memory on one node:

```bash
TOKEN=$(cat ~/.cordelia/node-token)
curl -s -X POST http://127.0.0.1:9473/api/v1/l2/write \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "item_id": "test-martin-001",
    "type": "learning",
    "data": {"content": "test replication from boot2"},
    "meta": {"group_id": "seed-drill", "visibility": "group"}
  }' | jq .
```

Then check the other node:

```bash
cordelia-node --config ~/.cordelia/config.toml query test-martin-001
```

## Updating

```bash
cd ~/cordelia
git pull
cd cordelia-node
cargo build --release
sudo systemctl restart cordelia-node
```

## Troubleshooting

| Symptom | Check |
|---------|-------|
| No peer connections | UDP 9474 open? DNS resolves? Bootnode running? |
| Peers stay Cold/Warm | Check governor tick logs. Groups must overlap. |
| Replication not working | Both nodes have same group? Peers reached Hot state? |
| DB errors | Check `~/.cordelia/cordelia.db` permissions. Schema auto-inits on empty DB. |
| Build fails | `rustup update stable`. Check `Cargo.lock` is committed. |

## Local Development (macOS)

Run a local node on your laptop alongside the MCP server. Uses a separate database to avoid SQLite contention -- merge when TS/Rust integration is deliberate.

### Setup

```bash
mkdir -p ~/.cordelia

cat > ~/.cordelia/config-local.toml << 'EOF'
[node]
identity_key = "~/.cordelia/node.key"
api_transport = "http"
api_addr = "127.0.0.1:9473"
database = "~/.cordelia/p2p-node.db"
entity_id = "russell"  # Change to your entity ID

[network]
listen_addr = "0.0.0.0:9474"

[[network.bootnodes]]
addr = "boot1.cordelia.seeddrill.io:9474"

[governor]
hot_min = 2
hot_max = 20
warm_min = 10
warm_max = 50

[replication]
sync_interval_moderate_secs = 300
tombstone_retention_days = 7
max_batch_size = 100
EOF
```

### Build and Run

```bash
cd ~/cordelia/cordelia-node
cargo build --release

# Run in foreground (or use tmux)
RUST_LOG=info,cordelia_node=debug \
  target/release/cordelia-node \
  --config ~/.cordelia/config-local.toml
```

Identity key and bearer token auto-generate on first run.

### Create Groups

```bash
TOKEN=$(cat ~/.cordelia/node-token)
curl -s http://127.0.0.1:9473/api/v1/groups/create \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"group_id":"seed-drill","name":"Seed Drill","culture":"moderate","security_policy":"{}"}' | jq .
```

### Expected Output

```
INFO  starting cordelia-node node_id=dfd773b0...
INFO  storage opened db=/Users/you/.cordelia/p2p-node.db
INFO  seeded bootnode bootnode="boot1.cordelia.seeddrill.io:9474" resolved=82.69.29.148:9474
INFO  outbound handshake complete peer="3d2238..." remote=82.69.29.148:9474
INFO  connected to peer peer="3d2238..." addr=82.69.29.148:9474
DEBUG peer state transition peer="3d2238..." from="warm" to="hot"
DEBUG governor tick complete warm=0 hot=1
```

### Notes

- **Separate DB**: `p2p-node.db` keeps P2P replication isolated from the MCP server's `cordelia.db`
- **No systemd on macOS**: Use tmux, or create a launchd plist if you want auto-start
- **Port 9474/UDP**: Only needed inbound if other nodes will dial you. Outbound to boot1 works through NAT.
- **Bootnode DNS**: Hostnames resolve automatically (e.g. `boot1.cordelia.seeddrill.io:9474`)

## Current Live Nodes

| Node | Host | DNS | Status |
|------|------|-----|--------|
| boot1 | vducdl50 (KVM on pdukvm15) | boot1.cordelia.seeddrill.io:9474 | Running |
| russell-local | MacBook (GSV-Heavy-Lifting) | localhost:9474 | Running (dev) |
| boot2 | TBD (Martin) | boot2.cordelia.seeddrill.io:9474 | Planned |
| bill-local | MacBook (Bill) | localhost:9474 | Planned (after 1 week soak) |

---

*Last updated: 2026-01-29*
