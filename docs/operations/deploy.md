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
          seeddrill-proxy (lhr)
          embedded node = keeper
          groups: [seeddrill-internal, shared-xorg]
          bootnodes: [boot1, boot2]
          REST API + dashboard on :3847
```

Monthly cost: ~$12-13 (2x shared-cpu-1x 256MB boots + 1x shared-cpu-1x 512MB proxy + 2x dedicated IPv4 @ $2/mo)

## Prerequisites

- `flyctl` installed: `curl -L https://fly.io/install.sh | sh`
- Fly.io account authenticated: `flyctl auth login`
- Git access to `git@github.com:seed-drill/cordelia-core.git`
- Git access to `git@github.com:seed-drill/cordelia-proxy.git`

## Deploy

### CI/CD (automatic)

Boot nodes deploy automatically via GitHub Actions on push to `main`:

- **Workflow**: `.github/workflows/fly-deploy.yml`
- **Trigger**: push to `main` branch
- **Strategy**: matrix deploy -- boot1 and boot2 in parallel
- **Secret**: `FLY_API_TOKEN` (org-scoped, set on cordelia-core repo)

The proxy deploys separately from `cordelia-proxy` via its own `fly-deploy.yml`.

Order matters on first deploy -- boot nodes must be up before the keeper can connect. CI/CD handles boot nodes together; the proxy workflow runs independently.

### Manual deploy (if needed)

```bash
# Boot nodes
cd cordelia-core
flyctl deploy --config fly-boot1.toml --remote-only
flyctl deploy --config fly-boot2.toml --remote-only

# Proxy
cd cordelia-proxy
flyctl deploy --remote-only
```

Verify:
```bash
curl https://seeddrill-proxy.fly.dev/api/health
curl https://seeddrill-proxy.fly.dev/api/core/status
```

Expected: `ok: true`, `connected: true`, peers >= 2.

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

Same procedure for boot2 and seeddrill-proxy (proxy config at `/data/core/config.toml`).

## DNS

All DNS managed in Cloudflare. Boot nodes use **A records** pointing to dedicated IPv4 addresses (not CNAMEs) because Cloudflare's HTTP proxy doesn't handle raw TCP on custom ports. Proxy status must be **DNS only** (grey cloud).

| Record | Type | Value | Proxy |
|--------|------|-------|-------|
| boot1.cordelia.seeddrill.ai | A | 137.66.16.11 | DNS only |
| boot2.cordelia.seeddrill.ai | A | 213.188.208.49 | DNS only |

Dedicated IPv4 addresses are allocated per-app on Fly.io ($2/mo each). To check current IPs:

```bash
flyctl ips list -a cordelia-boot1
flyctl ips list -a cordelia-boot2
```

## Node Configuration

Both boot nodes use a single parameterised `Dockerfile` with `BOOT_CONFIG` build arg:

| Node | Fly App | Config | Region | Role | Dedicated IP |
|------|---------|--------|--------|------|--------------|
| boot1 | cordelia-boot1 | boot1-config.toml | lhr (London) | relay/transparent | 137.66.16.11 |
| boot2 | cordelia-boot2 | boot2-config.toml | ams (Amsterdam) | relay/transparent | 213.188.208.49 |
| proxy | seeddrill-proxy | fly-node-config.toml (in cordelia-proxy) | lhr (London) | keeper | shared |

Boot nodes are configured with `auto_stop_machines = "off"` and `min_machines_running = 1` to ensure they are always available for peer discovery.

## Verification Checklist

1. `dig boot1.cordelia.seeddrill.ai` -- resolves to `137.66.16.11`
2. `dig boot2.cordelia.seeddrill.ai` -- resolves to `213.188.208.49`
3. `nc -zv boot1.cordelia.seeddrill.ai 9474` -- P2P port open
4. `nc -zv boot2.cordelia.seeddrill.ai 9474` -- P2P port open
5. `flyctl logs -a cordelia-boot1` -- peer connections to boot2
6. `flyctl logs -a cordelia-boot2` -- peer connections to boot1
7. `curl https://seeddrill-proxy.fly.dev/api/health` -- ok: true
8. `curl https://seeddrill-proxy.fly.dev/api/core/status` -- connected: true, groups include seeddrill-internal
9. `curl https://seeddrill-proxy.fly.dev/api/docs` -- Swagger UI loads

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
| No peer connections | TCP 9474 open? DNS resolves to dedicated IP? Bootnode running? |
| Peers stay Cold/Warm | Check governor tick logs. Groups must overlap. |
| Replication not working | Both nodes have same group? Peers reached Hot state? |
| DB errors | Check DB file permissions. Schema auto-inits on empty DB. |
| Build fails | `rustup update stable`. Check `Cargo.lock` is committed. |

---

*Last updated: 2026-02-03*
