# E2E Testing Guide

End-to-end testing for cordelia-node P2P replication, group propagation, and cluster health.

## Architecture

Tests run inside a Docker network using an **orchestrator pattern**:

```
[orchestrator] ---> [boot1] <---> [boot2]
                      |              |
                [edge-alpha-1]  [edge-bravo-1]
                      |              |
              [keeper-alpha-1]  [keeper-bravo-1]
              [agent-alpha-1]
```

- **Orchestrator**: Debian container with curl/jq, sits on all Docker networks, runs test scripts
- **Nodes**: `cordelia-node:test` containers with fixed bearer token, each with a role and zone
- **Networks**: Zoned Docker networks enforce topology (backbone, org-private per org)

The `gen-compose-zoned.sh` script reads `topology.env` and generates `docker-compose.generated.yml` with all nodes, networks, and configuration.

## Topology Configurations

| Config | Nodes | Use Case |
|--------|-------|----------|
| `topology-ci.env` | 7 | CI pipeline (fast, low memory) |
| `topology.env` (default) | 219 | Scale testing, full multi-org |
| Custom | Variable | Stress testing |

### CI Topology (7 nodes)

2 backbone boots, 2 orgs (alpha: 1 edge, 1 keeper, 1 agent; bravo: 1 edge, 1 keeper). Under 2GB RAM. Sync intervals: 5s moderate, 15s taciturn.

### Default Topology (219 nodes)

5 backbone boots, 12 orgs with varying sizes, 5 backbone personals. Requires ARP cache tuning for >100 nodes.

## Node Types

| Type | Hostname Pattern | Role | Network Zone |
|------|-----------------|------|-------------|
| Boot | `boot{N}` | relay | Backbone |
| Edge | `edge-{org}-{N}` | relay | Backbone + org private |
| Keeper | `keeper-{org}-{N}` | keeper | Org private only |
| Agent | `agent-{org}-{N}` | personal | Org private only |
| Backbone Personal | `agent-bb-{N}` | personal | Backbone |

Edges bridge org-private to backbone. Keepers and agents only see their own org's edge relays.

## Running Tests

### CI smoke tests (7-node topology)

```bash
cd tests/e2e
cp topology-ci.env topology.env
bash gen-compose-zoned.sh
docker compose -f docker-compose.generated.yml up -d --wait
sleep 20  # wait for mesh convergence
docker exec cordelia-e2e-orchestrator bash /tests/ci-smoke-test.sh

# With JSON report:
docker exec -e REPORT=1 cordelia-e2e-orchestrator bash /tests/ci-smoke-test.sh

# Teardown:
docker compose -f docker-compose.generated.yml down -v --remove-orphans
```

### Full smoke tests (219-node topology)

```bash
cd tests/e2e

# IMPORTANT: Bump ARP cache for >100 nodes
sudo sysctl -w net.ipv4.neigh.default.gc_thresh1=4096
sudo sysctl -w net.ipv4.neigh.default.gc_thresh2=8192
sudo sysctl -w net.ipv4.neigh.default.gc_thresh3=16384

bash gen-compose-zoned.sh  # uses topology.env
docker compose -f docker-compose.generated.yml up -d --wait
docker exec cordelia-e2e-orchestrator bash /tests/smoke-test.sh
```

### Fly-based replication tests (live nodes)

```bash
# Requires: local cordelia-node running, Fly app deployed, both in P2P mesh
./scripts/test-replication-e2e.sh
./scripts/test-replication-e2e.sh --report          # JSON output
./scripts/test-replication-e2e.sh --timeout 180      # longer timeout
./scripts/test-replication-e2e.sh --no-cleanup       # keep test data
```

## Test Scenarios

### CI Smoke Tests (`ci-smoke-test.sh`)

| # | Test | Timeout | Description |
|---|------|---------|-------------|
| 0 | Pre-flight | - | All 7 nodes reachable |
| 1 | Cross-org replication | 30s | Write on agent-alpha-1, verify on keeper-bravo-1 |
| 2 | Reverse replication | 30s | Write on keeper-bravo-1, verify on keeper-alpha-1 |
| 3 | Group isolation | 30s+10s | alpha-internal item reaches alpha keeper, absent from bravo |
| 4 | Group API + descriptor propagation | 90s | Create group, add member locally, verify descriptor reaches keeper via GroupExchange |
| 5 | Cluster health | - | All nodes have hot peers, zero sync errors |

### Full Smoke Tests (`smoke-test.sh`)

| # | Test | Description |
|---|------|-------------|
| 0 | Pre-flight | All backbone and edge nodes reachable |
| 1 | Cross-org replication | shared-xorg item reaches all orgs via backbone |
| 2 | Group isolation | org-internal item stays within org |
| 3 | Reverse replication | item flows in reverse direction |
| 4 | Cluster health | 80%+ nodes have hot peers |
| 5 | Personal group convergence | personal group replication within org |

### Fly Replication Tests (`test-replication-e2e.sh`)

| # | Test | Default Timeout | Description |
|---|------|----------------|-------------|
| 1 | Local -> Fly | 120s | L2 item written locally appears on Fly node |
| 2 | Fly -> Local | 120s | L2 item written on Fly appears locally |
| 3 | Group propagation | 120s | Group created locally appears on Fly |
| 4 | Group list verify | - | Group visible in Fly's /groups/list |

## API Helpers Reference

### `lib/api.sh`

| Function | Signature | Description |
|----------|-----------|-------------|
| `api_post` | HOST PATH BODY | Raw POST request |
| `api_status` | HOST | Node status |
| `api_peers` | HOST | Peer list |
| `api_diag` | HOST | Diagnostics |
| `api_write_item` | HOST ID TYPE DATA GROUP | Write L2 item |
| `api_read_item` | HOST ID | Read L2 item |
| `api_create_group` | HOST ID NAME [CULTURE] | Create group |
| `api_add_group_member` | HOST GROUP ENTITY [ROLE] | Add group member |
| `api_read_group` | HOST GROUP_ID | Read group details |
| `api_list_groups` | HOST | List all groups |
| `api_delete_group` | HOST GROUP_ID | Delete group |
| `hot_peer_count` | HOST | Count of hot peers |

### `lib/wait.sh`

| Function | Signature | Description |
|----------|-----------|-------------|
| `wait_all_healthy` | COUNT [TIMEOUT] | All N nodes respond to /status |
| `wait_hot_peers` | HOST MIN [TIMEOUT] | Node has >= N hot peers |
| `wait_all_hot` | COUNT MIN [TIMEOUT] | All nodes have >= N hot peers |
| `wait_item_replicated` | HOST ID [TIMEOUT] | Item readable on host |
| `assert_item_absent` | HOST ID [WAIT] | Item NOT present after wait |
| `wait_group_has_member` | HOST GROUP ENTITY [TIMEOUT] | Member in group on host |
| `check_zero_sync_errors` | COUNT | Zero sync errors across all nodes |

## Configuration Reference (`topology.env`)

| Variable | Default | Description |
|----------|---------|-------------|
| `BACKBONE_COUNT` | 5 | Number of backbone boot nodes |
| `BACKBONE_PERSONAL` | 5 | Personal nodes on backbone |
| `ORG_SPEC` | (12 orgs) | `name:edges:keepers:personals,...` |
| `IMAGE` | cordelia-node:test | Docker image for nodes |
| `BEARER_TOKEN` | test-token-fixed | API bearer token |
| `RUST_LOG_LEVEL` | warn | Log level |
| `GOV_HOT_MIN/MAX` | 3/30 | Governor hot peer targets |
| `GOV_WARM_MIN/MAX` | 5/80 | Governor warm peer targets |
| `GOV_COLD_MAX` | 200 | Governor cold peer limit |
| `SYNC_MODERATE/TACITURN` | 10/30 | Replication intervals (seconds) |
| `PROXY_ENABLED` | 1 | Include proxy container |
| `PORTAL_ENABLED` | 0 | Include portal container |

## Troubleshooting

### ARP cache exhaustion (>100 nodes)

Symptom: nodes can't resolve peers, `Temporary failure in name resolution` in logs.

Fix: Bump kernel ARP table limits before starting:
```bash
sudo sysctl -w net.ipv4.neigh.default.gc_thresh1=4096
sudo sysctl -w net.ipv4.neigh.default.gc_thresh2=8192
sudo sysctl -w net.ipv4.neigh.default.gc_thresh3=16384
```

### High memory usage

The 219-node topology uses ~8-12GB. For CI or limited environments, use `topology-ci.env` (7 nodes, <2GB).

### DNS resolution failures

Inside the orchestrator container, all nodes are reachable by hostname (e.g., `boot1`, `edge-alpha-1`). If resolution fails, check that the orchestrator is connected to all networks in the generated compose file.

### Slow convergence

Reduce `SYNC_MODERATE` and `SYNC_TACITURN` for faster replication. CI topology uses 5s/15s. Default uses 10s/30s.

## Adding New Test Scenarios

1. Add API helpers to `lib/api.sh` if needed
2. Add wait/assert helpers to `lib/wait.sh` if needed
3. Add test section to the appropriate script (`ci-smoke-test.sh` or `smoke-test.sh`)
4. Follow the pattern: write data, poll with timeout, pass/fail, record timing
5. Update this document with the new scenario
