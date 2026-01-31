# Deploy boot4 on Fly.io

Boot node 4 for the Cordelia mesh network. This runs on Martin's Fly.io account.

## Prerequisites

- `flyctl` installed: `curl -L https://fly.io/install.sh | sh`
- Fly.io account authenticated: `flyctl auth login`
- Clone of cordelia-core repo

## Steps

### 1. Create the Fly app

```bash
flyctl apps create cordelia-boot4 --org personal
```

### 2. Deploy

From the cordelia-core repo root:

```bash
flyctl deploy --config fly-boot4.toml --remote-only
```

This builds the Rust binary on Fly's remote builders (no local Rust toolchain needed) and deploys it. First build takes ~2 minutes.

### 3. Verify

Check the node is running and peering:

```bash
flyctl logs -a cordelia-boot4 --no-tail | tail -20
```

You should see:
- `starting cordelia-node ... version="0.1.1" ... entity_id=boot4`
- `seeded bootnode ... boot1.cordelia.seeddrill.io:9474`
- `seeded bootnode ... boot2.cordelia.seeddrill.io:9474`
- `seeded bootnode ... boot3.cordelia.seeddrill.io:9474`
- `outbound handshake complete` for each peer

### 4. Tell Russell

Once deployed, Russell will add the DNS record:

```
boot4.cordelia.seeddrill.io CNAME cordelia-boot4.fly.dev
```

## Network topology

All boot nodes run v0.1.1 with standardised governor/replication config.

| Node   | Host                              | Operator |
|--------|-----------------------------------|----------|
| boot1  | boot1.cordelia.seeddrill.io:9474  | Russell (vducdl50, Docker) |
| boot2  | boot2.cordelia.seeddrill.io:9474  | Russell (vducdl91, Docker) |
| boot3  | boot3.cordelia.seeddrill.io:9474  | Russell (Fly.io) |
| boot4  | boot4.cordelia.seeddrill.io:9474  | Martin (Fly.io) |

## Redeployment

To upgrade after a new commit to main:

```bash
cd cordelia-core
git pull
flyctl deploy --config fly-boot4.toml --remote-only
```

## Troubleshooting

**"failed to resolve bootnode address"** for boot4 - DNS record not yet created. Ask Russell.

**Connection errors to other boot nodes** - Check the other nodes are running. The governor will retry automatically.

**Build fails** - Ensure you're on the `main` branch with latest changes. The Dockerfile.boot4 and boot4-config.toml must both be present.
