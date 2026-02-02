# Cordelia Node -- Deployment Guide (Fly.io)

All production infrastructure runs on Fly.io. Two relay boot nodes provide mesh connectivity; the cordelia-proxy runs an embedded keeper node for persistent storage and the REST API.

## Architecture

```
                [Cloudflare]
                CF Pages (seeddrill.ai) -- static site
                CF Proxy -- DNS/TLS termination
                    |
                [Fly.io]
                    |
    boot1 (lhr) -------- boot2 (ams)
    relay/transparent     relay/transparent
            \              /
          cordelia-proxy (lhr)
          embedded node = keeper
          groups: [seeddrill-internal, shared-xorg]
          bootnodes: [boot1, boot2]
          REST API + dashboard on :3847
```

Monthly cost: ~$8-9 (2x shared-cpu-1x 256MB boots + 1x shared-cpu-1x 512MB proxy)

## Prerequisites

- `flyctl` installed: `curl -L https://fly.io/install.sh | sh`
- Fly.io account authenticated: `flyctl auth login`
- Git access to `git@github.com:seed-drill/cordelia-core.git`
- Git access to `git@github.com:seed-drill/cordelia-proxy.git`

## Deploy

Order matters -- boot nodes must be up before the keeper can connect.

### 1. Deploy boot1 (London)

```bash
cd cordelia-core
flyctl deploy --config fly-boot1.toml --remote-only
```

Verify:
```bash
flyctl logs -a cordelia-boot1 --no-tail | tail -20
```

### 2. Deploy boot2 (Amsterdam)

```bash
flyctl deploy --config fly-boot2.toml --remote-only
```

Verify boot1 <-> boot2 peering:
```bash
flyctl logs -a cordelia-boot2 --no-tail | grep -E "(handshake|connected|peer)"
```

### 3. Deploy cordelia-proxy (London)

```bash
cd cordelia-proxy
flyctl deploy --remote-only
```

Verify:
```bash
curl https://cordelia-proxy.fly.dev/api/health
curl https://cordelia-proxy.fly.dev/api/core/status
```

Expected: `ok: true`, `connected: true`, peers >= 2.

## Updating an Existing Deployment

### Code changes

Redeploy from the relevant repo:

```bash
# Boot nodes
cd cordelia-core
flyctl deploy --config fly-boot1.toml --remote-only
flyctl deploy --config fly-boot2.toml --remote-only

# Proxy
cd cordelia-proxy
flyctl deploy --remote-only
```

### Config changes on existing volumes

The config is copied to the Fly volume on first boot only. To update config on a running node:

```bash
# SSH into the machine
flyctl ssh console -a cordelia-boot1

# Edit the on-volume config
vi /home/cordelia/.cordelia/config.toml

# Restart (the process manager will restart the app)
exit
flyctl machines restart -a cordelia-boot1
```

Same procedure for boot2 and cordelia-proxy (proxy config at `/data/core/config.toml`).

## DNS

All DNS managed in Cloudflare.

| Record | Type | Value |
|--------|------|-------|
| boot1.cordelia.seeddrill.ai | CNAME | cordelia-boot1.fly.dev |
| boot2.cordelia.seeddrill.ai | CNAME | cordelia-boot2.fly.dev |

## Node Configuration

Both boot nodes use a single parameterised `Dockerfile` with `BOOT_CONFIG` build arg:

| Node | Config | Region | Role |
|------|--------|--------|------|
| boot1 | boot1-config.toml | lhr (London) | relay/transparent |
| boot2 | boot2-config.toml | ams (Amsterdam) | relay/transparent |
| proxy | fly-node-config.toml (in cordelia-proxy) | lhr (London) | keeper |

## Verification Checklist

1. `flyctl logs -a cordelia-boot1` -- peer connections to boot2
2. `flyctl logs -a cordelia-boot2` -- peer connections to boot1
3. `curl https://cordelia-proxy.fly.dev/api/health` -- ok: true
4. `curl https://cordelia-proxy.fly.dev/api/core/status` -- connected: true, groups include seeddrill-internal
5. `curl https://cordelia-proxy.fly.dev/api/docs` -- Swagger UI loads
6. `dig boot1.cordelia.seeddrill.ai` / `dig boot2.cordelia.seeddrill.ai` -- resolve to Fly IPs

## Local Development (macOS)

Run a local node on your laptop for development. Uses a separate database to avoid SQLite contention.

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
addr = "boot1.cordelia.seeddrill.ai:9474"

[[network.bootnodes]]
addr = "boot2.cordelia.seeddrill.ai:9474"

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
cd ~/cordelia-core
cargo build --release

RUST_LOG=info,cordelia_node=debug \
  target/release/cordelia-node \
  --config ~/.cordelia/config-local.toml
```

Identity key and bearer token auto-generate on first run.

## Troubleshooting

| Symptom | Check |
|---------|-------|
| No peer connections | UDP 9474 open? DNS resolves? Bootnode running? |
| Peers stay Cold/Warm | Check governor tick logs. Groups must overlap. |
| Replication not working | Both nodes have same group? Peers reached Hot state? |
| DB errors | Check DB file permissions. Schema auto-inits on empty DB. |
| Build fails | `rustup update stable`. Check `Cargo.lock` is committed. |

---

*Last updated: 2026-02-02*
