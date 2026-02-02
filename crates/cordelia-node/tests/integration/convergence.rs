//! Governor mesh convergence tests.

use std::time::Duration;

use cordelia_governor::GovernorTargets;
use cordelia_node::config::NodeRole;

use crate::harness::{build_test_runtime, scaled_timeout, test_node_count, TestMesh, TestNodeBuilder};

/// Two nodes reach hot=1.
#[tokio::test]
async fn test_two_node_convergence() {
    let mesh = TestMesh::new(2, vec!["g1".into()]).await.unwrap();
    mesh.wait_full_mesh(Duration::from_secs(90)).await.unwrap();
    mesh.shutdown_all().await;
}

/// N nodes all connected (warm or hot). Default 3, override with TEST_NODE_COUNT.
/// Worker threads from TEST_WORKER_THREADS (default 4).
#[test]
fn test_n_node_convergence() {
    build_test_runtime(4).block_on(async {
        let n = test_node_count(3);
        let timeout = scaled_timeout(n, 90);
        let mesh = TestMesh::new(n, vec!["g1".into()]).await.unwrap();
        mesh.wait_full_mesh(timeout).await.unwrap();
        mesh.shutdown_all().await;
    });
}

/// Start node A, wait 5s, start node B pointing at A.
#[tokio::test]
async fn test_staggered_startup() {
    let groups = vec!["g1".into()];
    let targets = GovernorTargets {
        hot_min: 1,
        hot_max: 5,
        warm_min: 1,
        warm_max: 5,
        cold_max: 10,
        churn_interval_secs: 3600,
        churn_fraction: 0.0,
    };

    let node_a = TestNodeBuilder::new("node-a")
        .role(NodeRole::Relay)
        .groups(groups.clone())
        .governor_targets(targets.clone())
        .build()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;

    let node_b = TestNodeBuilder::new("node-b")
        .role(NodeRole::Relay)
        .groups(groups)
        .governor_targets(targets)
        .bootnode(node_a.listen_addr.clone())
        .build()
        .await
        .unwrap();

    node_a
        .wait_hot_peers(1, Duration::from_secs(60))
        .await
        .unwrap();
    node_b
        .wait_hot_peers(1, Duration::from_secs(60))
        .await
        .unwrap();

    node_a.shutdown().await;
    node_b.shutdown().await;
}

/// Stable N-node mesh, then add one more node. All connected.
/// Base mesh size from TEST_NODE_COUNT (default 2).
#[test]
fn test_late_joiner() {
    build_test_runtime(4).block_on(async {
        let n = test_node_count(2);
        let total = n + 1;
        let timeout = scaled_timeout(total, 90);
        let groups = vec!["g1".into()];
        let mesh = TestMesh::new(n, groups.clone()).await.unwrap();
        mesh.wait_full_mesh(timeout).await.unwrap();

        // Add a late joiner pointing at all existing nodes
        let mut builder = TestNodeBuilder::new("node-late")
            .role(NodeRole::Relay)
            .groups(groups)
            .governor_targets(GovernorTargets {
                hot_min: 1,
                hot_max: total.max(2),
                warm_min: 1,
                warm_max: total.max(2),
                cold_max: total * 2,
                churn_interval_secs: 3600,
                churn_fraction: 0.0,
            });
        for node in &mesh.nodes {
            builder = builder.bootnode(node.listen_addr.clone());
        }
        let late_node = builder.build().await.unwrap();

        // All nodes (original + late joiner) should see N peers
        let expected_peers = n; // each node sees all others
        for (i, node) in mesh.nodes.iter().enumerate() {
            node.wait_connected_peers(expected_peers, timeout)
                .await
                .unwrap_or_else(|e| panic!("node-{} failed: {}", i, e));
        }
        late_node
            .wait_connected_peers(expected_peers, timeout)
            .await
            .unwrap();

        late_node.shutdown().await;
        mesh.shutdown_all().await;
    });
}

/// Shutdown 1 of N nodes, rebuild it, verify reconvergence.
/// Node count from TEST_NODE_COUNT (default 3).
#[test]
fn test_rolling_restart() {
    build_test_runtime(4).block_on(async {
        let n = test_node_count(3);
        let timeout = scaled_timeout(n, 90);
        let groups = vec!["g1".into()];
        let mut mesh = TestMesh::new(n, groups.clone()).await.unwrap();
        mesh.wait_full_mesh(timeout).await.unwrap();

        // Shutdown the last node
        let removed = mesh.nodes.remove(n - 1);
        removed.shutdown().await;

        // Wait a bit for disconnect propagation
        tokio::time::sleep(Duration::from_secs(5)).await;

        // Rebuild pointing at remaining nodes
        let mut builder = TestNodeBuilder::new("node-rebuilt")
            .role(NodeRole::Relay)
            .groups(groups)
            .governor_targets(GovernorTargets {
                hot_min: 1,
                hot_max: n.max(2),
                warm_min: 1,
                warm_max: n.max(2),
                cold_max: n * 2,
                churn_interval_secs: 3600,
                churn_fraction: 0.0,
            });
        for node in &mesh.nodes {
            builder = builder.bootnode(node.listen_addr.clone());
        }
        let rebuilt = builder.build().await.unwrap();

        // All N nodes should be connected to N-1 peers again
        let expected = n - 1;
        for (i, node) in mesh.nodes.iter().enumerate() {
            node.wait_connected_peers(expected, timeout)
                .await
                .unwrap_or_else(|e| panic!("node-{} failed: {}", i, e));
        }
        rebuilt
            .wait_connected_peers(expected, timeout)
            .await
            .unwrap();

        rebuilt.shutdown().await;
        mesh.shutdown_all().await;
    });
}

/// Forcibly disconnect a peer mid-mesh, verify gossip rediscovery and reconnection.
/// Simulates the boot4 pattern: connection drops without node restart.
#[test]
fn test_peer_disconnect_recovery() {
    build_test_runtime(4).block_on(async {
        let n = test_node_count(3);
        assert!(n >= 2, "disconnect recovery requires at least 2 nodes");
        let timeout = scaled_timeout(n, 90);
        let mesh = TestMesh::new(n, vec!["g1".into()]).await.unwrap();
        mesh.wait_full_mesh(timeout).await.unwrap();

        // Forcibly disconnect node[0] from node[1]
        let target_peer = mesh.nodes[1].peer_id;
        mesh.nodes[0].disconnect_peer(target_peer).await;

        // Brief sleep for disconnect to propagate
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Wait for full mesh reconvergence (gossip -> cold -> warm -> hot)
        mesh.wait_full_mesh(timeout).await.unwrap();

        mesh.shutdown_all().await;
    });
}

/// Verify connection count stays stable after mesh converges.
/// Detects the boot4 bug: cold bootnode placeholder not consumed when peer
/// connects inbound before first governor tick, causing connection accumulation.
#[test]
fn test_connection_stability() {
    build_test_runtime(4).block_on(async {
        let n = test_node_count(3);
        let timeout = scaled_timeout(n, 90);
        let mesh = TestMesh::new(n, vec!["g1".into()]).await.unwrap();
        mesh.wait_full_mesh(timeout).await.unwrap();

        // Record initial connection counts
        let expected = n - 1;
        let mut initial_counts = Vec::new();
        for node in &mesh.nodes {
            let resp = node.api_peers().await.unwrap();
            let total = resp["total"].as_u64().unwrap_or(0) as usize;
            initial_counts.push(total);
            assert!(
                total >= expected,
                "node {} should have >= {} peers, has {}",
                node.peer_id,
                expected,
                total,
            );
        }

        // Let several governor ticks pass (6 ticks = 60s at 10s/tick)
        // If connections are leaking, count will grow above expected.
        tokio::time::sleep(Duration::from_secs(60)).await;

        // Verify connection counts haven't grown
        for (i, node) in mesh.nodes.iter().enumerate() {
            let resp = node.api_peers().await.unwrap();
            let total = resp["total"].as_u64().unwrap_or(0) as usize;
            let max_allowed = expected + 1; // +1 tolerance for transient churn
            assert!(
                total <= max_allowed,
                "node-{}: connection leak detected! expected <= {}, have {} (was {}). \
                 This indicates the governor is accumulating connections. response: {}",
                i,
                max_allowed,
                total,
                initial_counts[i],
                resp,
            );
        }

        mesh.shutdown_all().await;
    });
}
