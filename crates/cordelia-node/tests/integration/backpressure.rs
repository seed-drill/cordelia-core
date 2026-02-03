//! Backpressure tests -- oversized item rejection.

use std::time::Duration;

use crate::harness::TestMesh;

/// Item > 16KB should be rejected by the API with 413.
#[tokio::test]
async fn test_oversized_item_rejected_api() {
    let mesh = TestMesh::new(1, vec!["g1".into()]).await.unwrap();

    // Create an item > 16KB. The data bytes get JSON-wrapped as {"raw": "BBB..."}
    // which exceeds the 16KB MAX_ITEM_BYTES limit.
    let big_data = vec![0x42u8; 17_000];

    // Use raw API call to check the HTTP status code
    let data_str = String::from_utf8_lossy(&big_data);
    let data_json: serde_json::Value = serde_json::from_str(&data_str)
        .unwrap_or_else(|_| serde_json::json!({"raw": data_str.to_string()}));
    let body = serde_json::json!({
        "item_id": "big-item",
        "type": "entity",
        "data": data_json,
        "meta": {
            "visibility": "group",
            "group_id": "g1",
            "owner_id": "test",
            "author_id": "test",
            "key_version": 1,
        }
    });
    let (status, _resp) = mesh.nodes[0]
        .api_post_raw("/api/v1/l2/write", body)
        .await
        .unwrap();

    assert_eq!(
        status, 413,
        "oversized item should be rejected with 413 Payload Too Large, got {}",
        status
    );

    mesh.shutdown_all().await;
}

/// Oversized items should not be replicated to peers (rejected at API layer).
#[tokio::test]
async fn test_oversized_suppressed_replication() {
    let mesh = TestMesh::new(2, vec!["g1".into()]).await.unwrap();
    mesh.wait_full_mesh(Duration::from_secs(90)).await.unwrap();

    // Write a normal-sized item first to confirm replication works
    mesh.nodes[0]
        .api_write_item("normal-item", "entity", b"small-data", "g1")
        .await
        .unwrap();

    mesh.nodes[1]
        .wait_item("normal-item", Duration::from_secs(30))
        .await
        .unwrap();

    // Attempt to write an oversized item (should be rejected with 413)
    let big_data = vec![0x42u8; 17_000];
    let data_str = String::from_utf8_lossy(&big_data);
    let data_json: serde_json::Value = serde_json::from_str(&data_str)
        .unwrap_or_else(|_| serde_json::json!({"raw": data_str.to_string()}));
    let body = serde_json::json!({
        "item_id": "big-repl-item",
        "type": "entity",
        "data": data_json,
        "meta": {
            "visibility": "group",
            "group_id": "g1",
            "owner_id": "test",
            "author_id": "test",
            "key_version": 1,
        }
    });
    let (status, _) = mesh.nodes[0]
        .api_post_raw("/api/v1/l2/write", body)
        .await
        .unwrap();

    // API should reject it -- item never enters storage or replication
    assert_eq!(status, 413, "oversized write should be rejected");

    // Verify it does NOT appear on node 1
    tokio::time::sleep(Duration::from_secs(5)).await;
    let result = mesh.nodes[1].api_read_item("big-repl-item").await.unwrap();
    assert!(
        result.get("data").is_none() && result.get("type").is_none(),
        "oversized item should not be replicated, got: {}",
        result
    );

    mesh.shutdown_all().await;
}
