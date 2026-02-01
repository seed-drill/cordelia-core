//! Cordelia Node -- single binary P2P memory replication.
//!
//! Usage:
//!   cordelia-node                      # Run with default config
//!   cordelia-node --config path.toml   # Run with custom config
//!   cordelia-node identity             # Show node identity
//!   cordelia-node identity generate    # Generate new identity

mod config;
mod external_addr;
mod governor_task;
mod mini_protocols;
mod peer_pool;
mod quic_transport;
mod replication_task;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;

use cordelia_api::AppState;
use cordelia_crypto::NodeIdentity;
use cordelia_governor::{DialPolicy, Governor, GovernorTargets};
use cordelia_replication::{ReplicationConfig, ReplicationEngine};
use cordelia_storage::SqliteStorage;

#[derive(Parser)]
#[command(name = "cordelia-node", about = "Cordelia P2P memory replication node")]
struct Cli {
    /// Path to config file
    #[arg(short, long, default_value = "~/.cordelia/config.toml")]
    config: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Show or generate node identity
    Identity {
        #[command(subcommand)]
        action: Option<IdentityAction>,
    },
    /// Run the node (default)
    Run,
    /// Show node status (queries local API)
    Status,
    /// List connected peers
    Peers,
    /// List groups
    Groups,
    /// Read an L2 item by ID
    Query {
        /// Item ID to read
        item_id: String,
    },
}

#[derive(Subcommand)]
enum IdentityAction {
    /// Generate a new identity keypair
    Generate,
    /// Show current node identity
    Show,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cordelia_node=info,cordelia_api=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let config_path = expand_tilde(&cli.config);
    let cfg = config::NodeConfig::load_or_default(&config_path)?;

    match cli.command {
        Some(Commands::Identity { action }) => {
            let key_path = expand_tilde(&cfg.node.identity_key);
            match action {
                Some(IdentityAction::Generate) | None => {
                    let identity = NodeIdentity::load_or_create(&key_path)?;
                    println!("Node ID: {}", identity.node_id_hex());
                    println!("Key file: {}", key_path.display());
                }
                Some(IdentityAction::Show) => {
                    if key_path.exists() {
                        let identity = NodeIdentity::from_file(&key_path)?;
                        println!("Node ID: {}", identity.node_id_hex());
                    } else {
                        eprintln!("No identity found at {}", key_path.display());
                        std::process::exit(1);
                    }
                }
            }
        }
        Some(Commands::Run) | None => {
            run_node(cfg).await?;
        }
        Some(Commands::Status) => {
            cli_api_call(&cfg, "/api/v1/status", "{}").await?;
        }
        Some(Commands::Peers) => {
            cli_api_call(&cfg, "/api/v1/peers", "{}").await?;
        }
        Some(Commands::Groups) => {
            cli_api_call(&cfg, "/api/v1/groups/list", "{}").await?;
        }
        Some(Commands::Query { item_id }) => {
            let body = serde_json::json!({ "item_id": item_id }).to_string();
            cli_api_call(&cfg, "/api/v1/l2/read", &body).await?;
        }
    }

    Ok(())
}

/// Make a POST request to the local node API and print the JSON response.
async fn cli_api_call(cfg: &config::NodeConfig, path: &str, body: &str) -> anyhow::Result<()> {
    let addr = cfg.node.api_addr.as_deref().unwrap_or("127.0.0.1:9473");
    let url = format!("http://{}{}", addr, path);

    let token_path = expand_tilde("~/.cordelia/node-token");
    let token = if token_path.exists() {
        std::fs::read_to_string(&token_path)?.trim().to_string()
    } else {
        String::new()
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", token))
        .body(body.to_string())
        .send()
        .await?;

    let status = resp.status();
    let text = resp.text().await?;

    if status.is_success() {
        // Pretty-print JSON
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else {
            println!("{}", text);
        }
    } else {
        eprintln!("Error ({}): {}", status, text);
        std::process::exit(1);
    }
    Ok(())
}

async fn run_node(cfg: config::NodeConfig) -> anyhow::Result<()> {
    let key_path = expand_tilde(&cfg.node.identity_key);
    let identity = NodeIdentity::load_or_create(&key_path)?;
    let our_node_id = *identity.node_id();
    let our_role = cfg.role();
    tracing::info!(
        node_id = %identity.node_id_hex(),
        version = env!("CARGO_PKG_VERSION"),
        role = our_role.as_str(),
        protocol_min = cordelia_protocol::VERSION_MIN,
        protocol_max = cordelia_protocol::VERSION_MAX,
        protocol_magic = format_args!("{:#010x}", cordelia_protocol::PROTOCOL_MAGIC),
        entity_id = %cfg.node.entity_id,
        "starting cordelia-node"
    );
    tracing::info!(
        listen = %cfg.network.listen_addr,
        api_transport = %cfg.node.api_transport,
        api_addr = cfg.node.api_addr.as_deref().unwrap_or("(default)"),
        bootnodes = cfg.network.bootnodes.len(),
        "network config"
    );

    // Open storage
    let db_path = expand_tilde(&cfg.node.database);
    let storage = SqliteStorage::open(&db_path)?;
    let storage: Arc<dyn cordelia_storage::Storage> = Arc::new(storage);
    tracing::info!(db = %db_path.display(), "storage opened");

    // Load bearer token
    let token_path = expand_tilde("~/.cordelia/node-token");
    let bearer_token = load_or_create_token(&token_path)?;

    // Determine our groups from storage (shared across all tasks)
    let initial_groups: Vec<String> = storage
        .list_groups()
        .unwrap_or_default()
        .into_iter()
        .map(|g| g.id)
        .collect();
    tracing::info!(groups = ?initial_groups, "loaded groups");
    let shared_groups = Arc::new(tokio::sync::RwLock::new(initial_groups.clone()));
    let our_groups = initial_groups;

    // External address tracker (NAT hairpin avoidance)
    let external_addr = Arc::new(tokio::sync::RwLock::new(external_addr::ExternalAddr::new(
        cfg.network
            .external_addr
            .as_deref()
            .and_then(|s| s.parse::<std::net::SocketAddr>().ok()),
    )));

    // Shutdown broadcast channel
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    // Write notification channel (API -> replication task)
    let (write_tx, write_rx) =
        tokio::sync::broadcast::channel::<cordelia_api::WriteNotification>(256);

    // Build QUIC transport
    let listen_addr: std::net::SocketAddr = cfg.network.listen_addr.parse()?;
    let transport = Arc::new(
        quic_transport::QuicTransport::new(listen_addr, identity.pkcs8_der())
            .map_err(|e| anyhow::anyhow!("QUIC transport init failed: {e}"))?,
    );
    tracing::info!(addr = %listen_addr, "QUIC transport ready");

    // Build peer pool (before API state so we can wire the peer count callback)
    let pool = peer_pool::PeerPool::new(our_groups.clone());

    // Build API state
    let pool_for_count = pool.clone();
    let pool_for_list = pool.clone();
    let state = Arc::new(AppState {
        storage: Box::new(StorageClone(storage.clone())),
        node_id: identity.node_id_hex(),
        entity_id: cfg.node.entity_id.clone(),
        bearer_token,
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
    });

    // Build governor with role-based targets and dial policy
    let effective_gov = cfg.effective_governor_targets();
    let governor_targets = GovernorTargets {
        hot_min: effective_gov.hot_min,
        hot_max: effective_gov.hot_max,
        warm_min: effective_gov.warm_min,
        warm_max: effective_gov.warm_max,
        cold_max: effective_gov.cold_max,
        churn_interval_secs: effective_gov.churn_interval_secs,
        churn_fraction: effective_gov.churn_fraction,
    };
    let dial_policy = match our_role {
        config::NodeRole::Relay => DialPolicy::All,
        config::NodeRole::Personal => DialPolicy::RelaysOnly,
        config::NodeRole::Keeper => {
            // Resolve trusted relay addresses to node IDs (hash-based, same as bootnode seeding)
            let trusted_ids: Vec<[u8; 32]> = cfg
                .network
                .trusted_relays
                .iter()
                .filter_map(|entry| {
                    entry
                        .addr
                        .parse::<std::net::SocketAddr>()
                        .ok()
                        .or_else(|| {
                            use std::net::ToSocketAddrs;
                            entry.addr.to_socket_addrs().ok().and_then(|mut a| a.next())
                        })
                        .map(|addr| {
                            let hash = cordelia_crypto::sha256_hex(addr.to_string().as_bytes());
                            let hash_bytes = hex::decode(&hash).unwrap_or_default();
                            let mut id = [0u8; 32];
                            let len = id.len().min(hash_bytes.len());
                            id[..len].copy_from_slice(&hash_bytes[..len]);
                            id
                        })
                })
                .collect();
            DialPolicy::TrustedOnly(trusted_ids)
        }
    };
    tracing::info!(
        role = our_role.as_str(),
        hot_max = governor_targets.hot_max,
        warm_max = governor_targets.warm_max,
        "governor targets (role-adjusted)"
    );
    let governor = Arc::new(tokio::sync::Mutex::new(Governor::with_dial_policy(
        governor_targets,
        our_groups.clone(),
        dial_policy,
    )));

    // Build replication engine
    let repl_config = ReplicationConfig {
        sync_interval_moderate_secs: cfg.replication.sync_interval_moderate_secs,
        sync_interval_taciturn_secs: cfg.replication.sync_interval_taciturn_secs,
        tombstone_retention_days: cfg.replication.tombstone_retention_days,
        max_batch_size: cfg.replication.max_batch_size,
    };
    let repl_engine = ReplicationEngine::new(repl_config, cfg.node.entity_id.clone());

    // Spawn QUIC accept loop
    let quic_handle = {
        let transport = transport.clone();
        let pool = pool.clone();
        let storage = storage.clone();
        let groups = our_groups.clone();
        let role = our_role.as_str().to_string();
        let governor = governor.clone();
        let external_addr = external_addr.clone();
        let shutdown = shutdown_tx.subscribe();
        tokio::spawn(async move {
            transport
                .listen(
                    pool,
                    storage,
                    our_node_id,
                    groups,
                    role,
                    Some(governor),
                    external_addr,
                    shutdown,
                )
                .await;
        })
    };

    // Spawn governor loop
    let governor_handle = {
        let governor = governor.clone();
        let pool = pool.clone();
        let transport = transport.clone();
        let storage = storage.clone();
        let bootnodes = cfg.network.bootnodes.clone();
        let groups = our_groups.clone();
        let role = our_role.as_str().to_string();
        let external_addr = external_addr.clone();
        let shutdown = shutdown_tx.subscribe();
        let shutdown_tx_clone = shutdown_tx.clone();
        tokio::spawn(async move {
            governor_task::run_governor_loop(
                governor,
                pool,
                transport,
                storage,
                bootnodes,
                our_node_id,
                groups,
                role,
                external_addr,
                shutdown,
                shutdown_tx_clone,
            )
            .await;
        })
    };

    // Spawn replication loop
    let repl_handle = {
        let pool = pool.clone();
        let storage = storage.clone();
        let shared_groups = shared_groups.clone();
        let shutdown = shutdown_tx.subscribe();
        tokio::spawn(async move {
            replication_task::run_replication_loop(
                repl_engine,
                pool,
                storage,
                shared_groups,
                write_rx,
                shutdown,
            )
            .await;
        })
    };

    // Start API server
    let router = cordelia_api::router(state);

    let api_handle = match cfg.node.api_transport.as_str() {
        "http" => {
            let addr = cfg.node.api_addr.as_deref().unwrap_or("127.0.0.1:9473");
            tracing::info!(addr, "API listening (HTTP)");
            let listener = tokio::net::TcpListener::bind(addr).await?;
            let shutdown = shutdown_tx.subscribe();
            tokio::spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(async move {
                        let mut shutdown = shutdown;
                        let _ = shutdown.recv().await;
                    })
                    .await
                    .ok();
            })
        }
        "unix" => {
            let sock_path = expand_tilde(
                cfg.node
                    .api_socket
                    .as_deref()
                    .unwrap_or("~/.cordelia/node.sock"),
            );
            // Remove stale socket
            let _ = std::fs::remove_file(&sock_path);
            if let Some(parent) = sock_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            tracing::info!(path = %sock_path.display(), "API listening (Unix socket)");
            let listener = tokio::net::UnixListener::bind(&sock_path)?;

            let shutdown = shutdown_tx.subscribe();
            tokio::spawn(async move {
                use hyper_util::rt::TokioIo;
                use tower::Service;
                let mut shutdown = shutdown;

                loop {
                    tokio::select! {
                        accept = listener.accept() => {
                            match accept {
                                Ok((stream, _addr)) => {
                                    let router = router.clone();
                                    tokio::spawn(async move {
                                        let io = TokioIo::new(stream);
                                        let service = hyper::service::service_fn(move |req| {
                                            let mut router = router.clone();
                                            async move { router.call(req).await }
                                        });
                                        if let Err(e) = hyper_util::server::conn::auto::Builder::new(
                                            hyper_util::rt::TokioExecutor::new(),
                                        )
                                        .serve_connection(io, service)
                                        .await
                                        {
                                            tracing::error!("connection error: {e}");
                                        }
                                    });
                                }
                                Err(e) => {
                                    tracing::error!("accept error: {e}");
                                }
                            }
                        }
                        _ = shutdown.recv() => {
                            break;
                        }
                    }
                }
            })
        }
        other => {
            anyhow::bail!("unsupported api_transport: {other}");
        }
    };

    tracing::info!("all tasks spawned, press Ctrl-C to stop");

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down...");
    let _ = shutdown_tx.send(());

    // Wait for all tasks
    let _ = tokio::join!(quic_handle, governor_handle, repl_handle, api_handle);

    tracing::info!("shutdown complete");
    Ok(())
}

/// Wrapper to implement Storage on Arc<dyn Storage> for Box<dyn Storage>.
struct StorageClone(Arc<dyn cordelia_storage::Storage>);

impl cordelia_storage::Storage for StorageClone {
    fn read_l1(&self, user_id: &str) -> cordelia_storage::Result<Option<Vec<u8>>> {
        self.0.read_l1(user_id)
    }
    fn write_l1(&self, user_id: &str, data: &[u8]) -> cordelia_storage::Result<()> {
        self.0.write_l1(user_id, data)
    }
    fn read_l2_item(
        &self,
        id: &str,
    ) -> cordelia_storage::Result<Option<cordelia_storage::L2ItemRow>> {
        self.0.read_l2_item(id)
    }
    fn write_l2_item(&self, item: &cordelia_storage::L2ItemWrite) -> cordelia_storage::Result<()> {
        self.0.write_l2_item(item)
    }
    fn delete_l2_item(&self, id: &str) -> cordelia_storage::Result<bool> {
        self.0.delete_l2_item(id)
    }
    fn read_l2_item_meta(
        &self,
        id: &str,
    ) -> cordelia_storage::Result<Option<cordelia_storage::L2ItemMeta>> {
        self.0.read_l2_item_meta(id)
    }
    fn list_group_items(
        &self,
        group_id: &str,
        since: Option<&str>,
        limit: u32,
    ) -> cordelia_storage::Result<Vec<cordelia_storage::ItemHeader>> {
        self.0.list_group_items(group_id, since, limit)
    }
    fn write_group(
        &self,
        id: &str,
        name: &str,
        culture: &str,
        security_policy: &str,
    ) -> cordelia_storage::Result<()> {
        self.0.write_group(id, name, culture, security_policy)
    }
    fn read_group(&self, id: &str) -> cordelia_storage::Result<Option<cordelia_storage::GroupRow>> {
        self.0.read_group(id)
    }
    fn list_groups(&self) -> cordelia_storage::Result<Vec<cordelia_storage::GroupRow>> {
        self.0.list_groups()
    }
    fn list_members(
        &self,
        group_id: &str,
    ) -> cordelia_storage::Result<Vec<cordelia_storage::GroupMemberRow>> {
        self.0.list_members(group_id)
    }
    fn get_membership(
        &self,
        group_id: &str,
        entity_id: &str,
    ) -> cordelia_storage::Result<Option<cordelia_storage::GroupMemberRow>> {
        self.0.get_membership(group_id, entity_id)
    }
    fn add_member(
        &self,
        group_id: &str,
        entity_id: &str,
        role: &str,
    ) -> cordelia_storage::Result<()> {
        self.0.add_member(group_id, entity_id, role)
    }
    fn remove_member(&self, group_id: &str, entity_id: &str) -> cordelia_storage::Result<bool> {
        self.0.remove_member(group_id, entity_id)
    }
    fn update_member_posture(
        &self,
        group_id: &str,
        entity_id: &str,
        posture: &str,
    ) -> cordelia_storage::Result<bool> {
        self.0.update_member_posture(group_id, entity_id, posture)
    }
    fn delete_group(&self, id: &str) -> cordelia_storage::Result<bool> {
        self.0.delete_group(id)
    }
    fn log_access(&self, entry: &cordelia_storage::AccessLogEntry) -> cordelia_storage::Result<()> {
        self.0.log_access(entry)
    }
    fn read_l2_index(&self) -> cordelia_storage::Result<Option<Vec<u8>>> {
        self.0.read_l2_index()
    }
    fn write_l2_index(&self, data: &[u8]) -> cordelia_storage::Result<()> {
        self.0.write_l2_index(data)
    }
    fn fts_search(&self, query: &str, limit: u32) -> cordelia_storage::Result<Vec<String>> {
        self.0.fts_search(query, limit)
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs_or_home() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

fn dirs_or_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn load_or_create_token(path: &PathBuf) -> anyhow::Result<String> {
    if path.exists() {
        let token = std::fs::read_to_string(path)?.trim().to_string();
        return Ok(token);
    }

    // Generate random token
    use rand::Rng;
    let token: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(48)
        .map(char::from)
        .collect();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &token)?;

    // Restrict permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }

    tracing::info!(path = %path.display(), "generated bearer token");
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordelia_storage::{L2ItemWrite, SqliteStorage};
    use std::net::SocketAddr;

    fn test_external_addr() -> Arc<tokio::sync::RwLock<external_addr::ExternalAddr>> {
        Arc::new(tokio::sync::RwLock::new(external_addr::ExternalAddr::new(
            None,
        )))
    }

    /// Two-node integration test: spawn two in-process QUIC endpoints, handshake,
    /// write an item on node A, sync+fetch to node B, verify it arrives.
    #[tokio::test]
    async fn test_two_node_replication() {
        // Generate identities for both nodes
        let id_a = NodeIdentity::generate().unwrap();
        let id_b = NodeIdentity::generate().unwrap();
        let node_id_a = *id_a.node_id();
        let node_id_b = *id_b.node_id();

        // Create temp databases
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let storage_a: Arc<dyn cordelia_storage::Storage> =
            Arc::new(SqliteStorage::create_new(&dir_a.path().join("a.db")).unwrap());
        let storage_b: Arc<dyn cordelia_storage::Storage> =
            Arc::new(SqliteStorage::create_new(&dir_b.path().join("b.db")).unwrap());

        let group_id = "test-group";
        let our_groups = vec![group_id.to_string()];

        // Build QUIC transports on ephemeral ports
        let addr_a: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let addr_b: SocketAddr = "127.0.0.1:0".parse().unwrap();

        let transport_a = quic_transport::QuicTransport::new(addr_a, id_a.pkcs8_der()).unwrap();
        let transport_b = quic_transport::QuicTransport::new(addr_b, id_b.pkcs8_der()).unwrap();

        // Get the actual bound addresses
        let _real_addr_a = transport_a.endpoint.local_addr().unwrap();
        let real_addr_b = transport_b.endpoint.local_addr().unwrap();

        // Create peer pools
        let pool_a = peer_pool::PeerPool::new(our_groups.clone());
        let pool_b = peer_pool::PeerPool::new(our_groups.clone());

        // Shutdown channels
        let (shutdown_tx_b, _) = tokio::sync::broadcast::channel::<()>(1);

        // Spawn node B's accept loop
        let ext_b = test_external_addr();
        let listen_handle = {
            let pool_b = pool_b.clone();
            let storage_b = storage_b.clone();
            let groups_b = our_groups.clone();
            let ext_b = ext_b.clone();
            let shutdown = shutdown_tx_b.subscribe();
            tokio::spawn(async move {
                transport_b
                    .listen(
                        pool_b,
                        storage_b,
                        node_id_b,
                        groups_b,
                        "relay".into(),
                        None,
                        ext_b,
                        shutdown,
                    )
                    .await;
            })
        };

        // Give node B a moment to start listening
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Node A dials node B
        let conn = transport_a.dial(real_addr_b).await.unwrap();

        // Outbound handshake from A -> B
        let ext_a = test_external_addr();
        let peer_id =
            quic_transport::outbound_handshake(&conn, &pool_a, node_id_a, &our_groups, &ext_a)
                .await
                .unwrap();

        assert_eq!(peer_id, node_id_b, "handshake should return node B's ID");

        // Spawn A's stream accept loop so B can open streams back to A
        let conn_for_accept = conn.clone();
        let pool_a_accept = pool_a.clone();
        let storage_a_accept = storage_a.clone();
        let groups_a_accept = our_groups.clone();
        let ext_a2 = ext_a.clone();
        let _accept_handle = tokio::spawn(async move {
            quic_transport::run_connection(
                conn_for_accept,
                [0u8; 32],
                pool_a_accept,
                storage_a_accept,
                node_id_a,
                groups_a_accept,
                "relay".into(),
                None,
                ext_a2,
                false,
            )
            .await;
        });

        // Verify both pools registered the peer
        assert_eq!(pool_a.len().await, 1, "node A pool should have 1 peer");
        // Give node B's accept loop a moment to process
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(pool_b.len().await, 1, "node B pool should have 1 peer");

        // Write an item to node A's storage
        let test_data = b"encrypted-test-blob".to_vec();
        storage_a
            .write_l2_item(&L2ItemWrite {
                id: "item-001".into(),
                item_type: "entity".into(),
                data: test_data.clone(),
                owner_id: Some("russell".into()),
                visibility: "group".into(),
                group_id: Some(group_id.into()),
                author_id: Some("russell".into()),
                key_version: 1,
                parent_id: None,
                is_copy: false,
            })
            .unwrap();

        // Node B requests sync from node A (via the connection A opened to B)
        // We need A's connection to B -- but A dialled B, so `conn` goes A->B.
        // For B to request sync from A, B needs a connection to A.
        // In practice, the accept loop gives B a conn to A.
        // But for this test, let's use the simpler path:
        // B dials A, handshakes, then requests sync.

        // Actually, let's use the connection from B's pool (registered during handshake).
        let b_handle = pool_b.get(&node_id_a).await.unwrap();

        // Sync: B asks A for headers in the test group
        let sync_resp = mini_protocols::request_sync(&b_handle.connection, group_id, None, 100)
            .await
            .unwrap();

        assert_eq!(sync_resp.items.len(), 1, "sync should return 1 header");
        assert_eq!(sync_resp.items[0].item_id, "item-001");

        // Fetch: B asks A for the full item
        let fetched = mini_protocols::fetch_items(&b_handle.connection, vec!["item-001".into()])
            .await
            .unwrap();

        assert_eq!(fetched.len(), 1, "fetch should return 1 item");
        assert_eq!(fetched[0].item_id, "item-001");
        assert_eq!(fetched[0].encrypted_blob, test_data);
        assert_eq!(fetched[0].group_id, group_id);
        assert_eq!(fetched[0].author_id, "russell");

        // Store fetched item into B's storage
        storage_b
            .write_l2_item(&L2ItemWrite {
                id: fetched[0].item_id.clone(),
                item_type: fetched[0].item_type.clone(),
                data: fetched[0].encrypted_blob.clone(),
                owner_id: None,
                visibility: "group".into(),
                group_id: Some(fetched[0].group_id.clone()),
                author_id: Some(fetched[0].author_id.clone()),
                key_version: fetched[0].key_version as i32,
                parent_id: fetched[0].parent_id.clone(),
                is_copy: true,
            })
            .unwrap();

        // Verify item exists in B's storage
        let row_b = storage_b.read_l2_item("item-001").unwrap().unwrap();
        assert_eq!(row_b.data, test_data);
        assert_eq!(row_b.item_type, "entity");
        assert!(row_b.is_copy);

        // Clean shutdown
        let _ = shutdown_tx_b.send(());
        // Close connections
        conn.close(quinn::VarInt::from_u32(0), b"test done");
        transport_a
            .endpoint
            .close(quinn::VarInt::from_u32(0), b"done");

        // Wait for listen task to finish
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), listen_handle).await;
    }

    /// End-to-end replication test: two full nodes (governor + replication + QUIC),
    /// write item on A, verify it automatically replicates to B.
    #[tokio::test]
    async fn test_end_to_end_replication_full_stack() {
        use cordelia_replication::{ReplicationConfig, ReplicationEngine};

        let id_a = NodeIdentity::generate().unwrap();
        let id_b = NodeIdentity::generate().unwrap();
        let node_id_a = *id_a.node_id();
        let node_id_b = *id_b.node_id();

        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let storage_a: Arc<dyn cordelia_storage::Storage> =
            Arc::new(SqliteStorage::create_new(&dir_a.path().join("a.db")).unwrap());
        let storage_b: Arc<dyn cordelia_storage::Storage> =
            Arc::new(SqliteStorage::create_new(&dir_b.path().join("b.db")).unwrap());

        let group_id = "repl-test-group";

        // Create the group in both storages
        storage_a
            .write_group(
                group_id,
                "Repl Test",
                r#"{"broadcast_eagerness":"chatty"}"#,
                "{}",
            )
            .unwrap();
        storage_b
            .write_group(
                group_id,
                "Repl Test",
                r#"{"broadcast_eagerness":"chatty"}"#,
                "{}",
            )
            .unwrap();

        let groups_a = Arc::new(tokio::sync::RwLock::new(vec![group_id.to_string()]));
        let groups_b = Arc::new(tokio::sync::RwLock::new(vec![group_id.to_string()]));
        let our_groups_a = vec![group_id.to_string()];
        let our_groups_b = vec![group_id.to_string()];

        // Build transports
        let addr_a: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let addr_b: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let transport_a =
            Arc::new(quic_transport::QuicTransport::new(addr_a, id_a.pkcs8_der()).unwrap());
        let transport_b =
            Arc::new(quic_transport::QuicTransport::new(addr_b, id_b.pkcs8_der()).unwrap());
        let real_addr_b = transport_b.endpoint.local_addr().unwrap();

        // Pools
        let pool_a = peer_pool::PeerPool::new(our_groups_a.clone());
        let pool_b = peer_pool::PeerPool::new(our_groups_b.clone());

        // Shutdown channels
        let (shutdown_tx_a, _) = tokio::sync::broadcast::channel::<()>(1);
        let (shutdown_tx_b, _) = tokio::sync::broadcast::channel::<()>(1);

        // Write notification channels
        let (write_tx_a, write_rx_a) =
            tokio::sync::broadcast::channel::<cordelia_api::WriteNotification>(256);
        let (_write_tx_b, write_rx_b) =
            tokio::sync::broadcast::channel::<cordelia_api::WriteNotification>(256);

        // Governors
        let gov_targets = cordelia_governor::GovernorTargets {
            hot_min: 1,
            warm_min: 0,
            ..Default::default()
        };
        let _gov_a = Arc::new(tokio::sync::Mutex::new(cordelia_governor::Governor::new(
            gov_targets.clone(),
            our_groups_a.clone(),
        )));
        let _gov_b = Arc::new(tokio::sync::Mutex::new(cordelia_governor::Governor::new(
            gov_targets,
            our_groups_b.clone(),
        )));

        // Replication engines
        let repl_a = ReplicationEngine::new(ReplicationConfig::default(), "node-a".into());
        let repl_b = ReplicationEngine::new(ReplicationConfig::default(), "node-b".into());

        // Spawn node B: accept loop + replication
        let ext_b = test_external_addr();
        let _listen_b = {
            let transport_b = transport_b.clone();
            let pool_b = pool_b.clone();
            let storage_b = storage_b.clone();
            let groups_b = our_groups_b.clone();
            let ext_b = ext_b.clone();
            let shutdown = shutdown_tx_b.subscribe();
            tokio::spawn(async move {
                transport_b
                    .listen(
                        pool_b,
                        storage_b,
                        node_id_b,
                        groups_b,
                        "relay".into(),
                        None,
                        ext_b,
                        shutdown,
                    )
                    .await;
            })
        };

        let _repl_b = {
            let pool_b = pool_b.clone();
            let storage_b = storage_b.clone();
            let groups_b = groups_b.clone();
            let shutdown = shutdown_tx_b.subscribe();
            tokio::spawn(async move {
                replication_task::run_replication_loop(
                    repl_b, pool_b, storage_b, groups_b, write_rx_b, shutdown,
                )
                .await;
            })
        };

        // Spawn node A: replication loop (no accept needed, A dials B)
        let _repl_a = {
            let pool_a = pool_a.clone();
            let storage_a = storage_a.clone();
            let groups_a = groups_a.clone();
            let shutdown = shutdown_tx_a.subscribe();
            tokio::spawn(async move {
                replication_task::run_replication_loop(
                    repl_a, pool_a, storage_a, groups_a, write_rx_a, shutdown,
                )
                .await;
            })
        };

        // Give B time to start listening
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // A dials B, handshakes
        let ext_a = test_external_addr();
        let conn = transport_a.dial(real_addr_b).await.unwrap();
        let peer_id =
            quic_transport::outbound_handshake(&conn, &pool_a, node_id_a, &our_groups_a, &ext_a)
                .await
                .unwrap();
        assert_eq!(peer_id, node_id_b);

        // Mark peer as Hot in pool A (simulating governor promotion)
        pool_a
            .set_state(&peer_id, cordelia_governor::PeerState::Hot)
            .await;

        // Spawn A's connection handler so B can open push streams to A
        {
            let pool_a = pool_a.clone();
            let storage_a = storage_a.clone();
            let groups_a = our_groups_a.clone();
            let ext_a = ext_a.clone();
            tokio::spawn(async move {
                quic_transport::run_connection(
                    conn,
                    [0u8; 32],
                    pool_a,
                    storage_a,
                    node_id_a,
                    groups_a,
                    "relay".into(),
                    None,
                    ext_a,
                    false,
                )
                .await;
            });
        }

        // Give handshake time to complete on B's side
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Also mark the peer as Hot in B's pool (B sees A as Hot)
        pool_b
            .set_state(&node_id_a, cordelia_governor::PeerState::Hot)
            .await;

        // Write an item on node A
        let test_data = b"auto-replicated-blob".to_vec();
        storage_a
            .write_l2_item(&L2ItemWrite {
                id: "e2e-item-001".into(),
                item_type: "entity".into(),
                data: test_data.clone(),
                owner_id: Some("russell".into()),
                visibility: "group".into(),
                group_id: Some(group_id.into()),
                author_id: Some("russell".into()),
                key_version: 1,
                parent_id: None,
                is_copy: false,
            })
            .unwrap();

        // Fire write notification (simulating what the API handler does)
        let _ = write_tx_a.send(cordelia_api::WriteNotification {
            item_id: "e2e-item-001".into(),
            item_type: "entity".into(),
            group_id: Some(group_id.into()),
            data: test_data.clone(),
            key_version: 1,
        });

        // Wait for replication to occur (push via 0x06)
        let mut found = false;
        for _ in 0..30 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if let Ok(Some(row)) = storage_b.read_l2_item("e2e-item-001") {
                assert_eq!(row.data, test_data);
                assert_eq!(row.item_type, "entity");
                found = true;
                break;
            }
        }

        assert!(
            found,
            "item should have replicated from A to B automatically"
        );

        // Shutdown
        let _ = shutdown_tx_a.send(());
        let _ = shutdown_tx_b.send(());
        transport_a
            .endpoint
            .close(quinn::VarInt::from_u32(0), b"done");
        transport_b
            .endpoint
            .close(quinn::VarInt::from_u32(0), b"done");
    }

    /// Dial a remote bootnode and complete handshake.
    /// Run with: cargo test --package cordelia-node -- test_dial_bootnode --ignored --nocapture
    #[tokio::test]
    #[ignore] // requires live bootnode
    async fn test_dial_bootnode() {
        use std::net::ToSocketAddrs;

        let target = "boot1.cordelia.seeddrill.io:9474";
        let addr = target
            .to_socket_addrs()
            .expect("DNS resolution failed")
            .next()
            .expect("no addresses returned");
        println!("resolved {target} -> {addr}");

        let id = NodeIdentity::generate().unwrap();
        let transport =
            quic_transport::QuicTransport::new("0.0.0.0:0".parse().unwrap(), id.pkcs8_der())
                .unwrap();

        let pool = peer_pool::PeerPool::new(vec!["test-group".into()]);

        println!("dialling {addr}...");
        let conn = tokio::time::timeout(std::time::Duration::from_secs(5), transport.dial(addr))
            .await
            .expect("dial timed out after 5s")
            .expect("dial failed");

        println!("QUIC connected to {}", conn.remote_address());

        let ext = test_external_addr();
        let peer_id = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            quic_transport::outbound_handshake(
                &conn,
                &pool,
                *id.node_id(),
                &["test-group".to_string()],
                &ext,
            ),
        )
        .await
        .expect("handshake timed out")
        .expect("handshake failed");

        println!("HANDSHAKE OK - bootnode node_id: {}", hex::encode(peer_id));

        conn.close(quinn::VarInt::from_u32(0), b"test done");
        transport
            .endpoint
            .close(quinn::VarInt::from_u32(0), b"done");
    }

    /// Connect to live bootnode, create group, write item, verify sync.
    /// Run with: cargo test --package cordelia-node -- test_bootnode_replication --ignored --nocapture
    #[tokio::test]
    #[ignore] // requires live bootnode
    async fn test_bootnode_replication() {
        use std::net::ToSocketAddrs;

        let target = "boot1.cordelia.seeddrill.io:9474";
        let addr = target
            .to_socket_addrs()
            .expect("DNS resolution failed")
            .next()
            .expect("no addresses returned");

        let id = NodeIdentity::generate().unwrap();
        let transport =
            quic_transport::QuicTransport::new("0.0.0.0:0".parse().unwrap(), id.pkcs8_der())
                .unwrap();

        let pool = peer_pool::PeerPool::new(vec!["boot-test-group".into()]);

        let conn = tokio::time::timeout(std::time::Duration::from_secs(5), transport.dial(addr))
            .await
            .expect("dial timed out")
            .expect("dial failed");

        let ext = test_external_addr();
        let peer_id = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            quic_transport::outbound_handshake(
                &conn,
                &pool,
                *id.node_id(),
                &["boot-test-group".to_string()],
                &ext,
            ),
        )
        .await
        .expect("handshake timed out")
        .expect("handshake failed");

        println!("connected to bootnode: {}", hex::encode(peer_id));

        // Request sync for the group
        let sync_resp = mini_protocols::request_sync(&conn, "boot-test-group", None, 100)
            .await
            .expect("sync request failed");

        println!("sync response: {} items", sync_resp.items.len());

        conn.close(quinn::VarInt::from_u32(0), b"test done");
        transport
            .endpoint
            .close(quinn::VarInt::from_u32(0), b"done");
    }
}
