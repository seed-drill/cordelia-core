# Cordelia End-to-End Test Infrastructure

> **WARNING: ARP TABLE LIMITS**
>
> For topologies exceeding 100 nodes, you **must** increase the Linux ARP
> neighbour table limits **before** starting containers, or the host will
> lose network connectivity:
>
> ```bash
> sudo sysctl -w net.ipv4.neigh.default.gc_thresh1=4096
> sudo sysctl -w net.ipv4.neigh.default.gc_thresh2=8192
> sudo sysctl -w net.ipv4.neigh.default.gc_thresh3=16384
> ```
>
> To persist across reboots:
> ```bash
> sudo tee /etc/sysctl.d/99-cordelia-arp.conf <<'EOF'
> net.ipv4.neigh.default.gc_thresh1 = 4096
> net.ipv4.neigh.default.gc_thresh2 = 8192
> net.ipv4.neigh.default.gc_thresh3 = 16384
> EOF
> sudo sysctl -p /etc/sysctl.d/99-cordelia-arp.conf
> ```
>
> **Why:** Each container gets an ARP entry per bridge network it touches.
> 219 containers across 13 bridges exhaust the default limits (128/512/1024),
> causing the host kernel to drop ARP entries and lose connectivity to
> running containers. The VM will appear to hang.

## Overview

Zoned topology test environment for cordelia-core. Generates a multi-org
Docker Compose deployment with backbone relays, edge relays, keepers, and
personal/agent nodes -- each org isolated on its own bridge network, connected
to the backbone via edge relays.

The default topology is 219 nodes across 12 orgs (see `topology.env`).

## Quick Start

```bash
# On the test VM (cordelia-test @ 192.168.3.206)

# 1. Build the node image
docker build -t cordelia-node:test -f tests/e2e/Dockerfile.test .

# 2. Build the orchestrator image
docker build -t cordelia-orchestrator -f tests/e2e/Dockerfile.orchestrator tests/e2e/

# 3. Generate the compose file
cd tests/e2e
bash gen-compose-zoned.sh

# 4. Start the topology (~1 minute for 219 nodes with staggered startup)
docker compose -f docker-compose.generated.yml up -d

# 5. Run smoke tests
docker exec cordelia-e2e-orchestrator ./smoke-test.sh
```

## Files

| File | Description |
|------|-------------|
| `topology.env` | Default topology parameters (node counts, org definitions, governor targets) |
| `gen-compose-zoned.sh` | Generates `docker-compose.generated.yml` from topology.env or env var overrides |
| `Dockerfile.test` | Node container image (cordelia-node binary + config) |
| `Dockerfile.orchestrator` | Test orchestrator container (curl, jq, dnsutils, test scripts) |
| `smoke-test.sh` | Regression test suite (22 tests: connectivity, replication, isolation, health) |
| `monitor.sh` | Live network monitoring (node status, peer counts, replication stats) |
| `test-zoned-replication.sh` | Detailed replication test script (cross-org, isolation, posture verification) |
| `config-template.toml` | Base node configuration template |
| `gen-compose.sh` | Simple (non-zoned) compose generator |
| `runner.sh` | Test runner helper |

## Topology

The default 219-node topology (`topology.env`):

```
Backbone:  5 boot relays (transparent posture)
           5 personal nodes (directly connected)

12 orgs:   27 edge relays (dynamic posture)
           20 keepers
          162 personal/agent nodes
```

Each org gets its own Docker bridge network. Edge relays bridge the org
network to the backbone network. Keepers and personal nodes are on the
org network only.

### Relay Postures

| Posture | Default for | Behaviour |
|---------|-------------|-----------|
| `transparent` | Backbone relays | Forward items for any group |
| `dynamic` | Edge relays | Learn groups from connected org peers; forward only those |
| `explicit` | (manual config) | Forward only groups in `allowed_groups` list |

All postures respect `blocked_groups` deny-list.

### Customising the Topology

Override via environment variables:

```bash
# Smaller topology for quick iteration
BACKBONE_COUNT=3 BACKBONE_PERSONAL=2 \
  ORG_SPEC="alpha:2:1:5,bravo:2:1:5" \
  bash gen-compose-zoned.sh

# Large-scale stress test
BACKBONE_COUNT=10 BACKBONE_PERSONAL=20 \
  ORG_SPEC="alpha:5:5:50,bravo:5:5:50,charlie:5:5:50" \
  bash gen-compose-zoned.sh
```

## Orchestrator Container

The orchestrator sits inside all Docker networks and can reach every node
by hostname. It provides:

- `./smoke-test.sh` -- automated regression suite (22 tests)
- `./monitor.sh --watch` -- live cluster monitoring
- Interactive shell for ad-hoc testing

```bash
# Interactive shell
docker exec -it cordelia-e2e-orchestrator bash

# Run smoke tests
docker exec cordelia-e2e-orchestrator ./smoke-test.sh

# Watch cluster health
docker exec cordelia-e2e-orchestrator ./monitor.sh --watch
```

## Smoke Test Suite

22 tests covering:

1. **Pre-flight** -- backbone and edge node connectivity
2. **Cross-org replication** -- shared-xorg items propagate alpha -> bravo -> charlie via backbone
3. **Group isolation** -- org-internal items do not leak to other orgs
4. **Reverse replication** -- items flow in both directions through the backbone
5. **Cluster health** -- >=80% of key nodes have active hot peers

## Resource Expectations (219-node topology)

| Metric | Value |
|--------|-------|
| Startup time | ~1 minute (staggered) |
| Steady-state load | ~2.2 (on 4-core VM) |
| Memory | ~9 GB |
| ARP entries | ~164 (with sysctl fix) |
| Startup peak load | ~3.5-4.5 |
