//! Test harness for in-process cordelia-node integration tests.
//!
//! Provides TestNode (single node), TestNodeBuilder (config), and TestMesh
//! (N-node orchestrator) for running real libp2p swarms in the same tokio runtime.

use std::sync::Arc;
use std::time::Duration;

use cordelia_api::{AppState, ReplicationStats};
use cordelia_crypto::NodeIdentity;
use cordelia_governor::{DialPolicy, Governor, GovernorTargets};
use cordelia_node::config::{BootnodeEntry, NodeRole};
use cordelia_node::{governor_task, peer_pool, replication_task, swarm_task, StorageClone};
use cordelia_replication::{ReplicationConfig, ReplicationEngine};
use cordelia_storage::SqliteStorage;
use libp2p::{Multiaddr, PeerId};
use tokio::sync::{broadcast, RwLock};

/// Read TEST_NODE_COUNT from environment, falling back to `default`.
pub fn test_node_count(default: usize) -> usize {
    std::env::var("TEST_NODE_COUNT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// Scale a base timeout by node count. Larger meshes need more time for
/// gossip propagation and governor promotion cycles.
/// Formula: base_secs * ceil(n / 3), minimum = base_secs.
pub fn scaled_timeout(n: usize, base_secs: u64) -> Duration {
    let factor = ((n as f64) / 3.0).ceil().max(1.0) as u64;
    Duration::from_secs(base_secs * factor)
}

/// A running in-process node with all four tasks (swarm, governor, replication, API).
pub struct TestNode {
    pub peer_id: PeerId,
    pub api_addr: String,
    pub listen_addr: Multiaddr,
    pub bearer_token: String,
    pub storage: Arc<dyn cordelia_storage::Storage>,
    pub shared_groups: Arc<RwLock<Vec<String>>>,
    cmd_tx: tokio::sync::mpsc::Sender<swarm_task::SwarmCommand>,
    shutdown_tx: broadcast::Sender<()>,
    _tempdir: tempfile::TempDir,
    _handles: Vec<tokio::task::JoinHandle<()>>,
}

#[allow(dead_code)]
impl TestNode {
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    /// Forcibly disconnect a specific peer via SwarmCommand::Disconnect.
    pub async fn disconnect_peer(&self, peer_id: PeerId) {
        let _ = self
            .cmd_tx
            .send(swarm_task::SwarmCommand::Disconnect(peer_id))
            .await;
    }

    /// Poll /api/v1/peers until hot peer count >= n, or timeout.
    pub async fn wait_hot_peers(&self, n: usize, timeout: Duration) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if tokio::time::Instant::now() > deadline {
                let resp = self.api_peers().await?;
                let hot = resp["hot"].as_u64().unwrap_or(0) as usize;
                anyhow::bail!(
                    "timeout waiting for {} hot peers (have {}). response: {}",
                    n,
                    hot,
                    resp,
                );
            }
            if let Ok(resp) = self.api_peers().await {
                let hot = resp["hot"].as_u64().unwrap_or(0) as usize;
                if hot >= n {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Poll /api/v1/peers until total connected peers >= n (warm + hot), or timeout.
    pub async fn wait_connected_peers(&self, n: usize, timeout: Duration) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if tokio::time::Instant::now() > deadline {
                let resp = self.api_peers().await?;
                let total = resp["total"].as_u64().unwrap_or(0) as usize;
                anyhow::bail!(
                    "timeout waiting for {} connected peers (have {}). response: {}",
                    n,
                    total,
                    resp,
                );
            }
            if let Ok(resp) = self.api_peers().await {
                let total = resp["total"].as_u64().unwrap_or(0) as usize;
                if total >= n {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Poll /api/v1/l2/read until item is found, or timeout.
    pub async fn wait_item(
        &self,
        item_id: &str,
        timeout: Duration,
    ) -> anyhow::Result<serde_json::Value> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if tokio::time::Instant::now() > deadline {
                anyhow::bail!("timeout waiting for item {}", item_id);
            }
            if let Ok(resp) = self.api_read_item(item_id).await {
                // l2/read returns {data, type, meta} on success, empty/error otherwise
                if resp.get("data").is_some() || resp.get("type").is_some() {
                    return Ok(resp);
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// GET /api/v1/status
    pub async fn api_status(&self) -> anyhow::Result<serde_json::Value> {
        self.api_post("/api/v1/status", serde_json::json!({})).await
    }

    /// GET /api/v1/peers
    pub async fn api_peers(&self) -> anyhow::Result<serde_json::Value> {
        self.api_post("/api/v1/peers", serde_json::json!({})).await
    }

    /// GET /api/v1/diagnostics
    pub async fn api_diagnostics(&self) -> anyhow::Result<serde_json::Value> {
        self.api_post("/api/v1/diagnostics", serde_json::json!({}))
            .await
    }

    /// Write an L2 item via the API.
    pub async fn api_write_item(
        &self,
        id: &str,
        item_type: &str,
        data: &[u8],
        group_id: &str,
    ) -> anyhow::Result<serde_json::Value> {
        // API expects: item_id, type, data (as JSON value), meta (optional object)
        let data_str = String::from_utf8_lossy(data);
        let data_json: serde_json::Value = serde_json::from_str(&data_str)
            .unwrap_or_else(|_| serde_json::json!({"raw": data_str.to_string()}));
        let body = serde_json::json!({
            "item_id": id,
            "type": item_type,
            "data": data_json,
            "meta": {
                "visibility": "group",
                "group_id": group_id,
                "owner_id": "test",
                "author_id": "test",
                "key_version": 1,
            }
        });
        self.api_post("/api/v1/l2/write", body).await
    }

    /// Read an L2 item by ID.
    pub async fn api_read_item(&self, item_id: &str) -> anyhow::Result<serde_json::Value> {
        let body = serde_json::json!({ "item_id": item_id });
        self.api_post("/api/v1/l2/read", body).await
    }

    /// Create a group via the API.
    pub async fn api_create_group(
        &self,
        id: &str,
        name: &str,
        culture: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let body = serde_json::json!({
            "group_id": id,
            "name": name,
            "culture": culture,
            "security_policy": "standard",
        });
        self.api_post("/api/v1/groups/create", body).await
    }

    /// Raw POST returning (status_code, body_json).
    pub async fn api_post_raw(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> anyhow::Result<(u16, serde_json::Value)> {
        let url = format!("http://{}{}", self.api_addr, path);
        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header(
                "Authorization",
                format!("Bearer {}", self.bearer_token),
            )
            .json(&body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        let text = resp.text().await?;
        let val: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or(serde_json::json!({"_raw": text}));
        Ok((status, val))
    }

    async fn api_post(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let (_, val) = self.api_post_raw(path, body).await?;
        Ok(val)
    }
}

/// Builder for configuring and spawning a TestNode.
pub struct TestNodeBuilder {
    name: String,
    role: NodeRole,
    groups: Vec<String>,
    bootnodes: Vec<Multiaddr>,
    governor_targets: GovernorTargets,
    replication_config: ReplicationConfig,
}

#[allow(dead_code)]
impl TestNodeBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            role: NodeRole::Relay,
            groups: vec![],
            bootnodes: vec![],
            governor_targets: GovernorTargets {
                hot_min: 1,
                hot_max: 10,
                warm_min: 1,
                warm_max: 10,
                cold_max: 20,
                churn_interval_secs: 3600,
                churn_fraction: 0.0,
            },
            replication_config: ReplicationConfig {
                sync_interval_moderate_secs: 5,
                sync_interval_taciturn_secs: 15,
                tombstone_retention_days: 7,
                max_batch_size: 100,
            },
        }
    }

    pub fn role(mut self, role: NodeRole) -> Self {
        self.role = role;
        self
    }

    pub fn groups(mut self, groups: Vec<String>) -> Self {
        self.groups = groups;
        self
    }

    pub fn bootnode(mut self, addr: Multiaddr) -> Self {
        self.bootnodes.push(addr);
        self
    }

    pub fn bootnodes(mut self, addrs: Vec<Multiaddr>) -> Self {
        self.bootnodes = addrs;
        self
    }

    pub fn governor_targets(mut self, targets: GovernorTargets) -> Self {
        self.governor_targets = targets;
        self
    }

    pub fn replication_config(mut self, config: ReplicationConfig) -> Self {
        self.replication_config = config;
        self
    }

    pub async fn build(self) -> anyhow::Result<TestNode> {
        // Generate identity
        let identity = NodeIdentity::generate()?;
        let peer_id = *identity.peer_id();
        let keypair = identity.to_libp2p_keypair().map_err(|e| anyhow::anyhow!("{e}"))?;

        // Create temp storage
        let tempdir = tempfile::tempdir()?;
        let db_path = tempdir.path().join(format!("{}.db", self.name));
        let storage: Arc<dyn cordelia_storage::Storage> =
            Arc::new(SqliteStorage::create_new(&db_path)?);

        // Create groups in storage
        for group_id in &self.groups {
            storage.write_group(group_id, group_id, "chatty", "standard")?;
        }
        let shared_groups = Arc::new(RwLock::new(self.groups.clone()));

        // Bearer token
        let bearer_token = format!("test-token-{}", self.name);

        // Shutdown
        let (shutdown_tx, _) = broadcast::channel::<()>(1);

        // Write notification channel
        let (write_tx, write_rx) =
            broadcast::channel::<cordelia_api::WriteNotification>(256);

        // Replication stats
        let repl_stats = Arc::new(ReplicationStats::new());

        // Build swarm on ephemeral port
        let listen_addr: Multiaddr = "/ip4/127.0.0.1/tcp/0".parse()?;
        let mut swarm = swarm_task::build_swarm(keypair, listen_addr)
            .map_err(|e| anyhow::anyhow!("swarm build failed: {e}"))?;

        // Wait for NewListenAddr to get actual port
        use libp2p::futures::StreamExt;
        let actual_addr = loop {
            if let Some(event) = swarm.next().await {
                if let libp2p::swarm::SwarmEvent::NewListenAddr { address, .. } = event {
                    break address;
                }
            }
        };

        // Build peer pool
        let pool = peer_pool::PeerPool::new(shared_groups.clone());

        // Bind API on ephemeral port
        let api_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let api_addr = api_listener.local_addr()?.to_string();

        // Build API state
        let pool_for_count = pool.clone();
        let pool_for_list = pool.clone();
        let state = Arc::new(AppState {
            storage: Box::new(StorageClone(storage.clone())),
            node_id: identity.node_id_hex(),
            entity_id: self.name.clone(),
            bearer_token: bearer_token.clone(),
            start_time: std::time::Instant::now(),
            write_notify: Some(write_tx),
            shared_groups: Some(shared_groups.clone()),
            peer_count_fn: Some(Box::new(move || {
                let pool = pool_for_count.clone();
                Box::pin(async move { pool.peer_count_by_state().await })
            })),
            peer_list_fn: Some(Box::new(move || {
                let pool = pool_for_list.clone();
                Box::pin(async move { pool.peer_details().await })
            })),
            replication_stats: Some(repl_stats.clone()),
        });

        // Governor
        let governor = Arc::new(tokio::sync::Mutex::new(Governor::with_dial_policy(
            self.governor_targets.clone(),
            self.groups.clone(),
            DialPolicy::All,
        )));

        // Replication engine
        let repl_engine = ReplicationEngine::new(self.replication_config.clone(), self.name.clone());

        // Channels
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<swarm_task::SwarmCommand>(256);
        let (event_tx, _) = broadcast::channel::<swarm_task::SwarmEvent2>(256);

        let mut handles = Vec::new();

        // Spawn swarm task
        {
            let storage = storage.clone();
            let shared_groups = shared_groups.clone();
            let event_tx = event_tx.clone();
            let pool = pool.clone();
            let shutdown = shutdown_tx.subscribe();
            let role = self.role;
            handles.push(tokio::spawn(async move {
                swarm_task::run_swarm_loop(
                    swarm,
                    cmd_rx,
                    event_tx,
                    storage,
                    shared_groups,
                    pool,
                    role,
                    shutdown,
                )
                .await;
            }));
        }

        // Spawn governor loop
        {
            let governor = governor.clone();
            let pool = pool.clone();
            let cmd_tx = cmd_tx.clone();
            let event_rx = event_tx.subscribe();
            let bootnodes: Vec<BootnodeEntry> = self
                .bootnodes
                .iter()
                .map(|addr| BootnodeEntry {
                    addr: addr.to_string(),
                })
                .collect();
            let shared_groups = shared_groups.clone();
            let shutdown = shutdown_tx.subscribe();
            handles.push(tokio::spawn(async move {
                governor_task::run_governor_loop(
                    governor,
                    pool,
                    cmd_tx,
                    event_rx,
                    bootnodes,
                    shared_groups,
                    peer_id,
                    shutdown,
                )
                .await;
            }));
        }

        // Spawn replication loop
        {
            let pool = pool.clone();
            let storage = storage.clone();
            let shared_groups = shared_groups.clone();
            let cmd_tx = cmd_tx.clone();
            let shutdown = shutdown_tx.subscribe();
            let stats = repl_stats.clone();
            handles.push(tokio::spawn(async move {
                replication_task::run_replication_loop(
                    repl_engine,
                    pool,
                    storage,
                    shared_groups,
                    cmd_tx,
                    write_rx,
                    shutdown,
                    stats,
                )
                .await;
            }));
        }

        // Spawn API server
        {
            let router = cordelia_api::router(state);
            let shutdown = shutdown_tx.subscribe();
            handles.push(tokio::spawn(async move {
                axum::serve(api_listener, router)
                    .with_graceful_shutdown(async move {
                        let mut shutdown = shutdown;
                        let _ = shutdown.recv().await;
                    })
                    .await
                    .ok();
            }));
        }

        Ok(TestNode {
            peer_id,
            api_addr,
            listen_addr: actual_addr,
            bearer_token,
            storage,
            shared_groups,
            cmd_tx,
            shutdown_tx,
            _tempdir: tempdir,
            _handles: handles,
        })
    }
}

/// Orchestrates N nodes into a mesh for testing.
pub struct TestMesh {
    pub nodes: Vec<TestNode>,
}

impl TestMesh {
    /// Create N nodes, all sharing the same groups. Node 0 is the seed (no bootnodes),
    /// subsequent nodes bootnode to all prior nodes.
    pub async fn new(n: usize, groups: Vec<String>) -> anyhow::Result<Self> {
        let mut nodes = Vec::new();

        for i in 0..n {
            let mut builder = TestNodeBuilder::new(&format!("node-{}", i))
                .role(NodeRole::Relay)
                .groups(groups.clone())
                .governor_targets(GovernorTargets {
                    hot_min: 1,
                    hot_max: n.max(2),
                    warm_min: 1,
                    warm_max: n.max(2),
                    cold_max: n * 2,
                    churn_interval_secs: 3600,
                    churn_fraction: 0.0,
                });

            // Each node bootnodes to all prior nodes
            for prev in &nodes {
                let prev: &TestNode = prev;
                builder = builder.bootnode(prev.listen_addr.clone());
            }

            let node = builder.build().await?;
            nodes.push(node);
        }

        Ok(Self { nodes })
    }

    /// Create N nodes with per-node group assignments. Each inner Vec is the
    /// groups for that node. Node 0 is seed, subsequent nodes bootnode to all prior.
    pub async fn with_group_assignments(group_assignments: Vec<Vec<String>>) -> anyhow::Result<Self> {
        let n = group_assignments.len();
        let mut nodes = Vec::new();

        for (i, groups) in group_assignments.into_iter().enumerate() {
            let mut builder = TestNodeBuilder::new(&format!("node-{}", i))
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

            for prev in &nodes {
                let prev: &TestNode = prev;
                builder = builder.bootnode(prev.listen_addr.clone());
            }

            let node = builder.build().await?;
            nodes.push(node);
        }

        Ok(Self { nodes })
    }

    /// Wait until all nodes are connected to all other nodes (warm or hot).
    /// For 2-node mesh, also waits for hot (since there's only one peer).
    pub async fn wait_full_mesh(&self, timeout: Duration) -> anyhow::Result<()> {
        let expected = self.nodes.len() - 1;
        for (i, node) in self.nodes.iter().enumerate() {
            if self.nodes.len() == 2 {
                // 2-node: single peer should reach hot
                node.wait_hot_peers(expected, timeout).await.map_err(|e| {
                    anyhow::anyhow!("node-{} failed to reach full mesh: {}", i, e)
                })?;
            } else {
                // N-node: wait for all peers connected (warm or hot)
                node.wait_connected_peers(expected, timeout).await.map_err(|e| {
                    anyhow::anyhow!("node-{} failed to reach full mesh: {}", i, e)
                })?;
            }
        }
        Ok(())
    }

    /// Shutdown all nodes.
    pub async fn shutdown_all(self) {
        for node in self.nodes {
            node.shutdown().await;
        }
    }
}
