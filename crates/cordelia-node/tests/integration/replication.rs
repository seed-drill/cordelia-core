//! Item replication tests.

use std::collections::HashSet;
use std::time::Duration;

use cordelia_governor::GovernorTargets;
use cordelia_node::config::{NodeRole, RelayPosture};

use crate::harness::{build_test_runtime, scaled_timeout, test_node_count, TestMesh, TestNodeBuilder};

/// Write on node 0 with chatty culture group, verify push replication to all N nodes.
/// Node count from TEST_NODE_COUNT (default 2). Worker threads from TEST_WORKER_THREADS (default 4).
#[test]
fn test_item_replication_chatty() {
    build_test_runtime(4).block_on(async {
        let n = test_node_count(2);
        let groups = vec!["chatty-group".into()];
        let timeout = scaled_timeout(n, 90);
        let mesh = TestMesh::new(n, groups).await.unwrap();
        mesh.wait_full_mesh(timeout).await.unwrap();

        // Write item on node 0
        mesh.nodes[0]
            .api_write_item("item-chatty-001", "entity", b"test-blob-chatty", "chatty-group")
            .await
            .unwrap();

        // Verify item appears on all other nodes (chatty = eager push)
        let item_timeout = scaled_timeout(n, 30);
        for i in 1..n {
            mesh.nodes[i]
                .wait_item("item-chatty-001", item_timeout)
                .await
                .unwrap_or_else(|e| panic!("node-{} did not receive chatty item: {}", i, e));
        }

        mesh.shutdown_all().await;
    });
}

/// Write on node A, verify sync replication to node B (moderate culture uses anti-entropy).
#[tokio::test]
async fn test_item_replication_moderate() {
    let groups = vec!["mod-group".into()];

    // Build 2 nodes with moderate sync interval = 5s for faster testing
    let node_a = TestNodeBuilder::new("mod-a")
        .role(NodeRole::Relay)
        .groups(groups.clone())
        .governor_targets(GovernorTargets {
            hot_min: 1, hot_max: 5, warm_min: 1, warm_max: 5,
            cold_max: 10, churn_interval_secs: 3600, churn_fraction: 0.0,
        })
        .build()
        .await
        .unwrap();

    let node_b = TestNodeBuilder::new("mod-b")
        .role(NodeRole::Relay)
        .groups(groups)
        .governor_targets(GovernorTargets {
            hot_min: 1, hot_max: 5, warm_min: 1, warm_max: 5,
            cold_max: 10, churn_interval_secs: 3600, churn_fraction: 0.0,
        })
        .bootnode(node_a.listen_addr.clone())
        .build()
        .await
        .unwrap();

    node_a.wait_hot_peers(1, Duration::from_secs(60)).await.unwrap();

    // Write item on A
    node_a
        .api_write_item("item-mod-001", "entity", b"test-blob-moderate", "mod-group")
        .await
        .unwrap();

    // Wait for anti-entropy sync (moderate interval = 5s in test config)
    node_b
        .wait_item("item-mod-001", Duration::from_secs(60))
        .await
        .unwrap();

    node_a.shutdown().await;
    node_b.shutdown().await;
}

/// A in [g1,g2], B in [g1], C in [g2]. Item in g1 reaches B not C.
#[tokio::test]
async fn test_group_scoped_replication() {
    let targets = GovernorTargets {
        hot_min: 1, hot_max: 5, warm_min: 1, warm_max: 5,
        cold_max: 10, churn_interval_secs: 3600, churn_fraction: 0.0,
    };

    let node_a = TestNodeBuilder::new("scope-a")
        .role(NodeRole::Relay)
        .groups(vec!["g1".into(), "g2".into()])
        .governor_targets(targets.clone())
        .build()
        .await
        .unwrap();

    let node_b = TestNodeBuilder::new("scope-b")
        .role(NodeRole::Relay)
        .groups(vec!["g1".into()])
        .governor_targets(targets.clone())
        .bootnode(node_a.listen_addr.clone())
        .build()
        .await
        .unwrap();

    let node_c = TestNodeBuilder::new("scope-c")
        .role(NodeRole::Relay)
        .groups(vec!["g2".into()])
        .governor_targets(targets)
        .bootnode(node_a.listen_addr.clone())
        .build()
        .await
        .unwrap();

    // Wait for mesh
    node_a.wait_connected_peers(2, Duration::from_secs(90)).await.unwrap();

    // Write item in g1 on node A
    node_a
        .api_write_item("item-g1-only", "entity", b"g1-data", "g1")
        .await
        .unwrap();

    // B should get it (shares g1)
    node_b
        .wait_item("item-g1-only", Duration::from_secs(60))
        .await
        .unwrap();

    // C should NOT have it (only in g2). Wait a bit then check.
    tokio::time::sleep(Duration::from_secs(15)).await;
    let result = node_c.api_read_item("item-g1-only").await.unwrap();
    assert!(
        result.get("data").is_none() && result.get("type").is_none(),
        "node C should NOT have item from g1 (only in g2), got: {}",
        result
    );

    node_a.shutdown().await;
    node_b.shutdown().await;
    node_c.shutdown().await;
}

// ============================================================================
// Relay forwarding tests
// ============================================================================

/// Transparent relay forwards items for any group, even without membership.
/// Topology: personal-A [g1] -> relay (transparent, no groups) -> personal-B [g1]
/// Item written on A should reach B via the relay.
#[tokio::test]
async fn test_blind_relay_forwarding_transparent() {
    let targets = GovernorTargets {
        hot_min: 1, hot_max: 5, warm_min: 1, warm_max: 5,
        cold_max: 10, churn_interval_secs: 3600, churn_fraction: 0.0,
    };

    // Relay node: no group memberships, transparent posture
    let relay = TestNodeBuilder::new("relay-t")
        .role(NodeRole::Relay)
        .groups(vec![]) // no groups
        .relay_posture(RelayPosture::Transparent)
        .governor_targets(targets.clone())
        .build()
        .await
        .unwrap();

    // Node A: member of g1, boots to relay
    let node_a = TestNodeBuilder::new("fwd-a")
        .role(NodeRole::Relay)
        .groups(vec!["g1".into()])
        .governor_targets(targets.clone())
        .bootnode(relay.listen_addr.clone())
        .build()
        .await
        .unwrap();

    // Node B: member of g1, boots to relay
    let node_b = TestNodeBuilder::new("fwd-b")
        .role(NodeRole::Relay)
        .groups(vec!["g1".into()])
        .governor_targets(targets)
        .bootnode(relay.listen_addr.clone())
        .build()
        .await
        .unwrap();

    // Wait for connectivity (each node connects to relay, relay connects to both)
    relay.wait_connected_peers(2, Duration::from_secs(60)).await.unwrap();
    node_a.wait_connected_peers(1, Duration::from_secs(60)).await.unwrap();
    node_b.wait_connected_peers(1, Duration::from_secs(60)).await.unwrap();

    // Write item on A
    node_a
        .api_write_item("relay-fwd-001", "entity", b"{\"test\":\"transparent\"}", "g1")
        .await
        .unwrap();

    // B should receive via relay forwarding
    node_b
        .wait_item("relay-fwd-001", Duration::from_secs(60))
        .await
        .unwrap();

    // Relay should also have stored a copy (transparent accepts all)
    let relay_item = relay.api_read_item("relay-fwd-001").await.unwrap();
    assert!(
        relay_item.get("data").is_some() || relay_item.get("type").is_some(),
        "transparent relay should store the forwarded item"
    );

    relay.shutdown().await;
    node_a.shutdown().await;
    node_b.shutdown().await;
}

/// Dynamic relay learns groups from connected peers and only forwards those.
/// Topology: node-A [g1,g2] -> relay (dynamic) -> node-B [g1]
/// Item in g1 should reach B (relay learns g1 from both peers).
/// Item in g2 should reach relay (learns g2 from A) but B rejects (not in g2).
#[tokio::test]
async fn test_dynamic_relay_scoping() {
    let targets = GovernorTargets {
        hot_min: 1, hot_max: 5, warm_min: 1, warm_max: 5,
        cold_max: 10, churn_interval_secs: 3600, churn_fraction: 0.0,
    };

    // Dynamic relay: learns groups from connected peers
    let relay = TestNodeBuilder::new("relay-d")
        .role(NodeRole::Relay)
        .groups(vec![]) // no own groups
        .relay_posture(RelayPosture::Dynamic)
        .governor_targets(targets.clone())
        .build()
        .await
        .unwrap();

    // Node A: member of g1 and g2
    let node_a = TestNodeBuilder::new("dyn-a")
        .role(NodeRole::Relay)
        .groups(vec!["g1".into(), "g2".into()])
        .governor_targets(targets.clone())
        .bootnode(relay.listen_addr.clone())
        .build()
        .await
        .unwrap();

    // Node B: member of g1 only
    let node_b = TestNodeBuilder::new("dyn-b")
        .role(NodeRole::Relay)
        .groups(vec!["g1".into()])
        .governor_targets(targets)
        .bootnode(relay.listen_addr.clone())
        .build()
        .await
        .unwrap();

    // Wait for connectivity and group exchange to complete
    relay.wait_connected_peers(2, Duration::from_secs(60)).await.unwrap();
    node_a.wait_connected_peers(1, Duration::from_secs(60)).await.unwrap();
    node_b.wait_connected_peers(1, Duration::from_secs(60)).await.unwrap();

    // Allow time for GroupExchange to propagate learned groups to relay
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Write g1 item on A -- should reach B via relay
    node_a
        .api_write_item("dyn-g1-001", "entity", b"{\"test\":\"dynamic-g1\"}", "g1")
        .await
        .unwrap();

    node_b
        .wait_item("dyn-g1-001", Duration::from_secs(60))
        .await
        .unwrap();

    // Write g2 item on A -- relay should store (learned g2 from A),
    // but B should NOT have it (B not in g2)
    node_a
        .api_write_item("dyn-g2-001", "entity", b"{\"test\":\"dynamic-g2\"}", "g2")
        .await
        .unwrap();

    // Give time for propagation attempt
    tokio::time::sleep(Duration::from_secs(15)).await;

    // Relay should have the g2 item (learned g2 from A)
    let relay_g2 = relay.api_read_item("dyn-g2-001").await.unwrap();
    assert!(
        relay_g2.get("data").is_some() || relay_g2.get("type").is_some(),
        "dynamic relay should store g2 item (learned from peer A)"
    );

    // B should NOT have the g2 item (not a member of g2)
    let b_g2 = node_b.api_read_item("dyn-g2-001").await.unwrap();
    assert!(
        b_g2.get("data").is_none() && b_g2.get("type").is_none(),
        "node B (g1 only) should NOT have g2 item, got: {}",
        b_g2
    );

    relay.shutdown().await;
    node_a.shutdown().await;
    node_b.shutdown().await;
}

/// blocked_groups deny-list prevents forwarding even on transparent relay.
/// Topology: node-A [g1] -> relay (transparent, blocked=[g1]) -> node-B [g1]
/// Item in g1 should NOT pass through relay.
#[tokio::test]
async fn test_blocked_groups() {
    let targets = GovernorTargets {
        hot_min: 1, hot_max: 5, warm_min: 1, warm_max: 5,
        cold_max: 10, churn_interval_secs: 3600, churn_fraction: 0.0,
    };

    // Transparent relay with g1 blocked
    let relay = TestNodeBuilder::new("relay-b")
        .role(NodeRole::Relay)
        .groups(vec![])
        .relay_posture(RelayPosture::Transparent)
        .relay_blocked_groups(HashSet::from(["g1".to_string()]))
        .governor_targets(targets.clone())
        .build()
        .await
        .unwrap();

    let node_a = TestNodeBuilder::new("blk-a")
        .role(NodeRole::Relay)
        .groups(vec!["g1".into()])
        .governor_targets(targets.clone())
        .bootnode(relay.listen_addr.clone())
        .build()
        .await
        .unwrap();

    let node_b = TestNodeBuilder::new("blk-b")
        .role(NodeRole::Relay)
        .groups(vec!["g1".into()])
        .governor_targets(targets)
        .bootnode(relay.listen_addr.clone())
        .build()
        .await
        .unwrap();

    relay.wait_connected_peers(2, Duration::from_secs(60)).await.unwrap();
    node_a.wait_connected_peers(1, Duration::from_secs(60)).await.unwrap();
    node_b.wait_connected_peers(1, Duration::from_secs(60)).await.unwrap();

    // Write g1 item on A
    node_a
        .api_write_item("blocked-001", "entity", b"{\"test\":\"blocked\"}", "g1")
        .await
        .unwrap();

    // Wait and verify relay did NOT store it (g1 is blocked)
    tokio::time::sleep(Duration::from_secs(15)).await;

    let relay_item = relay.api_read_item("blocked-001").await.unwrap();
    assert!(
        relay_item.get("data").is_none() && relay_item.get("type").is_none(),
        "relay should NOT store blocked group item, got: {}",
        relay_item
    );

    // B should NOT have it either (no direct path, relay blocked it)
    let b_item = node_b.api_read_item("blocked-001").await.unwrap();
    assert!(
        b_item.get("data").is_none() && b_item.get("type").is_none(),
        "node B should NOT receive item via blocked relay, got: {}",
        b_item
    );

    relay.shutdown().await;
    node_a.shutdown().await;
    node_b.shutdown().await;
}

// ============================================================================
// Scale stress tests
// ============================================================================

/// Write item on node 0, measure wall-clock time until all N nodes have it.
/// Prints actual propagation latency for benchmarking.
/// Node count from TEST_NODE_COUNT (default 5). Worker threads from TEST_WORKER_THREADS (default 4).
#[test]
fn test_replication_propagation_timing() {
    build_test_runtime(4).block_on(async {
        let n = test_node_count(5);
        let timeout = scaled_timeout(n, 90);
        let mesh = TestMesh::new(n, vec!["chatty-group".into()]).await.unwrap();
        mesh.wait_full_mesh(timeout).await.unwrap();

        // Write item on node 0 and start the clock
        let start = tokio::time::Instant::now();
        mesh.nodes[0]
            .api_write_item(
                "timing-item-001",
                "entity",
                b"{\"payload\":\"propagation-timing-test\"}",
                "chatty-group",
            )
            .await
            .unwrap();

        // Track per-node arrival times
        let item_timeout = scaled_timeout(n, 60);
        let mut arrival_times = Vec::with_capacity(n - 1);
        // Poll all nodes concurrently for faster detection
        let mut handles = Vec::new();
        for i in 1..n {
            let node_api_addr = mesh.nodes[i].api_addr.clone();
            let bearer = mesh.nodes[i].bearer_token.clone();
            let timeout_dur = item_timeout;
            handles.push(tokio::spawn(async move {
                let client = reqwest::Client::new();
                let deadline = tokio::time::Instant::now() + timeout_dur;
                loop {
                    if tokio::time::Instant::now() > deadline {
                        return (i, None);
                    }
                    let url = format!("http://{}/api/v1/l2/read", node_api_addr);
                    let body = serde_json::json!({ "item_id": "timing-item-001" });
                    if let Ok(resp) = client
                        .post(&url)
                        .header("Content-Type", "application/json")
                        .header("Authorization", format!("Bearer {}", bearer))
                        .json(&body)
                        .send()
                        .await
                    {
                        if let Ok(val) = resp.json::<serde_json::Value>().await {
                            if val.get("data").is_some() || val.get("type").is_some() {
                                return (i, Some(start.elapsed()));
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }));
        }

        for handle in handles {
            let (node_idx, elapsed) = handle.await.unwrap();
            match elapsed {
                Some(dur) => arrival_times.push((node_idx, dur)),
                None => panic!("node-{} never received timing item within timeout", node_idx),
            }
        }

        // Sort by arrival time and print results
        arrival_times.sort_by_key(|(_, dur)| *dur);
        eprintln!("\n=== PROPAGATION TIMING ({} nodes) ===", n);
        for (node_idx, dur) in &arrival_times {
            eprintln!("  node-{}: {:.1}ms", node_idx, dur.as_secs_f64() * 1000.0);
        }
        let first = arrival_times.first().unwrap().1;
        let last = arrival_times.last().unwrap().1;
        eprintln!("  FIRST: {:.1}ms  LAST: {:.1}ms  SPREAD: {:.1}ms",
            first.as_secs_f64() * 1000.0,
            last.as_secs_f64() * 1000.0,
            (last - first).as_secs_f64() * 1000.0,
        );
        eprintln!("=========================================\n");

        // Sanity: all nodes got the item
        assert_eq!(arrival_times.len(), n - 1);

        mesh.shutdown_all().await;
    });
}

/// Split N nodes across overlapping groups, verify items only reach correct members.
/// Topology: nodes 0..N/3 in [g1,g2], nodes N/3..2N/3 in [g1 only], nodes 2N/3..N in [g2 only].
/// Write to g1 -> all g1 members get it, g2-only members do NOT.
/// Node count from TEST_NODE_COUNT (default 6). Worker threads from TEST_WORKER_THREADS (default 4).
#[test]
fn test_group_isolation_at_scale() {
    build_test_runtime(4).block_on(async {
        let n = test_node_count(6);
        assert!(n >= 6, "group isolation test needs at least 6 nodes");
        let timeout = scaled_timeout(n, 90);

        // Build group assignments: 3 tiers
        let tier1_end = n / 3;          // [g1, g2] - bridge nodes
        let tier2_end = 2 * n / 3;      // [g1 only]
        let tier3_end = n;               // [g2 only]

        let mut assignments = Vec::new();
        for i in 0..n {
            if i < tier1_end {
                assignments.push(vec!["g1".into(), "g2".into()]);
            } else if i < tier2_end {
                assignments.push(vec!["g1".into()]);
            } else {
                assignments.push(vec!["g2".into()]);
            }
        }

        let g1_members: Vec<usize> = (0..tier2_end).collect();
        let g2_only: Vec<usize> = (tier2_end..tier3_end).collect();

        eprintln!(
            "\n=== GROUP ISOLATION ({} nodes) ===\n  tier1 [g1,g2]: 0..{}\n  tier2 [g1]:    {}..{}\n  tier3 [g2]:    {}..{}",
            n, tier1_end, tier1_end, tier2_end, tier2_end, tier3_end,
        );

        let mesh = TestMesh::with_group_assignments(assignments).await.unwrap();

        // Wait for all nodes to be connected to at least some peers
        // (not full mesh -- different groups won't fully interconnect)
        let min_peers = (n / 3).max(1);
        for (i, node) in mesh.nodes.iter().enumerate() {
            node.wait_connected_peers(min_peers, timeout)
                .await
                .unwrap_or_else(|e| panic!("node-{} failed to connect: {}", i, e));
        }

        // Write item in g1 on node 0 (bridge node)
        mesh.nodes[0]
            .api_write_item("g1-scale-item", "entity", b"{\"scope\":\"g1\"}", "g1")
            .await
            .unwrap();

        // All g1 members should receive it
        let item_timeout = scaled_timeout(n, 60);
        for &i in &g1_members {
            if i == 0 { continue; } // writer
            mesh.nodes[i]
                .wait_item("g1-scale-item", item_timeout)
                .await
                .unwrap_or_else(|e| panic!("g1 node-{} did not receive g1 item: {}", i, e));
        }
        eprintln!("  g1 propagation: OK ({} nodes received item)", g1_members.len() - 1);

        // g2-only members should NOT have it -- wait then verify absence
        tokio::time::sleep(Duration::from_secs(15)).await;
        for &i in &g2_only {
            let result = mesh.nodes[i].api_read_item("g1-scale-item").await.unwrap();
            assert!(
                result.get("data").is_none() && result.get("type").is_none(),
                "node-{} (g2-only) should NOT have g1 item, got: {}",
                i,
                result
            );
        }
        eprintln!("  g2 isolation: OK ({} nodes correctly excluded)", g2_only.len());
        eprintln!("=========================================\n");

        mesh.shutdown_all().await;
    });
}

/// Every node writes a unique item simultaneously. Verify all N items reach all N nodes.
/// This is the N^2 stress test -- at 20 nodes that's 380 replication events.
/// Node count from TEST_NODE_COUNT (default 5). Worker threads from TEST_WORKER_THREADS (default 4).
#[test]
fn test_concurrent_write_convergence() {
    build_test_runtime(4).block_on(async {
        let n = test_node_count(5);
        let timeout = scaled_timeout(n, 90);
        let mesh = TestMesh::new(n, vec!["chatty-group".into()]).await.unwrap();
        mesh.wait_full_mesh(timeout).await.unwrap();

        let start = tokio::time::Instant::now();

        // Every node writes a unique item simultaneously
        let mut write_handles = Vec::new();
        for i in 0..n {
            let api_addr = mesh.nodes[i].api_addr.clone();
            let bearer = mesh.nodes[i].bearer_token.clone();
            let item_id = format!("concurrent-{:03}", i);
            write_handles.push(tokio::spawn(async move {
                let client = reqwest::Client::new();
                let url = format!("http://{}/api/v1/l2/write", api_addr);
                let body = serde_json::json!({
                    "item_id": item_id,
                    "type": "entity",
                    "data": {"source_node": i, "test": "concurrent-write"},
                    "meta": {
                        "visibility": "group",
                        "group_id": "chatty-group",
                        "owner_id": "test",
                        "author_id": format!("node-{}", i),
                        "key_version": 1,
                    }
                });
                let resp = client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .header("Authorization", format!("Bearer {}", bearer))
                    .json(&body)
                    .send()
                    .await;
                assert!(resp.is_ok(), "node-{} write failed: {:?}", i, resp.err());
                (i, item_id)
            }));
        }

        // Collect all item IDs
        let mut items: Vec<(usize, String)> = Vec::new();
        for handle in write_handles {
            items.push(handle.await.unwrap());
        }
        let write_elapsed = start.elapsed();

        eprintln!("\n=== CONCURRENT WRITE CONVERGENCE ({} nodes, {} items, {}x{} = {} replication events) ===",
            n, n, n, n - 1, n * (n - 1));
        eprintln!("  All writes completed in {:.1}ms", write_elapsed.as_secs_f64() * 1000.0);

        // Now verify every node has every item
        let item_timeout = scaled_timeout(n, 60);
        let mut missing = 0usize;
        for (writer_idx, item_id) in &items {
            for i in 0..n {
                if i == *writer_idx { continue; } // skip the writer
                match mesh.nodes[i].wait_item(item_id, item_timeout).await {
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("  MISSING: node-{} does not have {} (from node-{}): {}",
                            i, item_id, writer_idx, e);
                        missing += 1;
                    }
                }
            }
        }

        let total_elapsed = start.elapsed();
        let expected = n * (n - 1);
        let received = expected - missing;
        eprintln!("  Convergence: {}/{} items replicated ({:.1}%)",
            received, expected, (received as f64 / expected as f64) * 100.0);
        eprintln!("  Total time: {:.1}s", total_elapsed.as_secs_f64());
        eprintln!("=========================================\n");

        assert_eq!(missing, 0,
            "{} of {} replication events failed -- items did not converge",
            missing, expected);

        mesh.shutdown_all().await;
    });
}
