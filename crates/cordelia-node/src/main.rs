//! Cordelia Node -- single binary P2P memory replication.
//!
//! Usage:
//!   cordelia-node                      # Run with default config
//!   cordelia-node --config path.toml   # Run with custom config
//!   cordelia-node identity             # Show node identity
//!   cordelia-node identity generate    # Generate new identity

mod config;
mod governor_task;
mod peer_pool;
mod replication_task;
mod swarm_task;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;

use cordelia_api::AppState;
use cordelia_crypto::NodeIdentity;
use cordelia_governor::{DialPolicy, Governor, GovernorTargets};
use cordelia_replication::{ReplicationConfig, ReplicationEngine};
use cordelia_storage::SqliteStorage;
use libp2p::PeerId;

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
    /// Show replication diagnostics
    Diagnostics,
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
        Some(Commands::Diagnostics) => {
            cli_api_call(&cfg, "/api/v1/diagnostics", "{}").await?;
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
    let our_peer_id = *identity.peer_id();
    let our_role = cfg.role();

    // Create libp2p keypair from node identity
    let keypair = identity
        .to_libp2p_keypair()
        .map_err(|e| anyhow::anyhow!("failed to create libp2p keypair: {e}"))?;

    tracing::info!(
        peer_id = %our_peer_id,
        version = env!("CARGO_PKG_VERSION"),
        role = our_role.as_str(),
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

    // Determine our groups from storage
    let initial_groups: Vec<String> = storage
        .list_groups()
        .unwrap_or_default()
        .into_iter()
        .map(|g| g.id)
        .collect();
    tracing::info!(groups = ?initial_groups, "loaded groups");
    let shared_groups = Arc::new(tokio::sync::RwLock::new(initial_groups));

    // Shutdown broadcast channel
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    // Write notification channel (API -> replication task)
    let (write_tx, write_rx) =
        tokio::sync::broadcast::channel::<cordelia_api::WriteNotification>(256);

    // Replication diagnostics counters (shared between replication task and API)
    let repl_stats = Arc::new(cordelia_api::ReplicationStats::new());

    // Build libp2p swarm
    let listen_addr: libp2p::Multiaddr = parse_listen_addr(&cfg.network.listen_addr)?;
    let mut swarm = swarm_task::build_swarm(keypair, listen_addr)
        .map_err(|e| anyhow::anyhow!("swarm build failed: {e}"))?;

    // Add external address so identify announces our public IP (critical for Docker/NAT)
    if let Some(ext) = &cfg.network.external_addr {
        let ext_addr: libp2p::Multiaddr = parse_listen_addr(ext)?;
        swarm.add_external_address(ext_addr.clone());
        tracing::info!(%ext_addr, "added external address");
    }

    // Build peer pool
    let pool = peer_pool::PeerPool::new(shared_groups.clone());

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
        replication_stats: Some(repl_stats.clone()),
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
            // For keeper nodes, resolve trusted relay PeerIds
            // (placeholder IDs, same as bootnode seeding)
            let trusted_ids: Vec<PeerId> = cfg
                .network
                .trusted_relays
                .iter()
                .filter_map(|entry| {
                    let hash = cordelia_crypto::sha256_hex(entry.addr.as_bytes());
                    let hash_bytes = hex::decode(&hash).ok()?;
                    let mut seed = [0u8; 32];
                    let len = seed.len().min(hash_bytes.len());
                    seed[..len].copy_from_slice(&hash_bytes[..len]);
                    let kp = libp2p::identity::Keypair::ed25519_from_bytes(seed).ok()?;
                    Some(PeerId::from(kp.public()))
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
        shared_groups.read().await.clone(),
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

    // Create swarm command/event channels
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<swarm_task::SwarmCommand>(256);
    let (event_tx, _) = tokio::sync::broadcast::channel::<swarm_task::SwarmEvent2>(256);

    // Spawn swarm task
    let swarm_handle = {
        let storage = storage.clone();
        let shared_groups = shared_groups.clone();
        let event_tx = event_tx.clone();
        let pool = pool.clone();
        let shutdown = shutdown_tx.subscribe();
        tokio::spawn(async move {
            swarm_task::run_swarm_loop(
                swarm,
                cmd_rx,
                event_tx,
                storage,
                shared_groups,
                pool,
                our_role,
                shutdown,
            )
            .await;
        })
    };

    // Spawn governor loop
    let governor_handle = {
        let governor = governor.clone();
        let pool = pool.clone();
        let cmd_tx = cmd_tx.clone();
        let event_rx = event_tx.subscribe();
        let bootnodes = cfg.network.bootnodes.clone();
        let shared_groups = shared_groups.clone();
        let shutdown = shutdown_tx.subscribe();
        tokio::spawn(async move {
            governor_task::run_governor_loop(
                governor,
                pool,
                cmd_tx,
                event_rx,
                bootnodes,
                shared_groups,
                shutdown,
            )
            .await;
        })
    };

    // Spawn replication loop
    let repl_handle = {
        let pool = pool.clone();
        let storage = storage.clone();
        let shared_groups = shared_groups.clone();
        let cmd_tx = cmd_tx.clone();
        let shutdown = shutdown_tx.subscribe();
        let stats = repl_stats.clone();
        tokio::spawn(async move {
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
    let _ = tokio::join!(swarm_handle, governor_handle, repl_handle, api_handle);

    tracing::info!("shutdown complete");
    Ok(())
}

/// Parse listen_addr from config (supports both "host:port" and Multiaddr format).
fn parse_listen_addr(addr: &str) -> anyhow::Result<libp2p::Multiaddr> {
    // Try Multiaddr first
    if let Ok(ma) = addr.parse::<libp2p::Multiaddr>() {
        return Ok(ma);
    }

    // Fall back to host:port -> /ip4/HOST/tcp/PORT
    let socket_addr: std::net::SocketAddr = addr.parse()?;
    let multiaddr: libp2p::Multiaddr =
        format!("/ip4/{}/tcp/{}", socket_addr.ip(), socket_addr.port()).parse()?;
    Ok(multiaddr)
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
    fn storage_stats(&self) -> cordelia_storage::Result<cordelia_storage::StorageStats> {
        self.0.storage_stats()
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

    /// Two-node integration test using in-process libp2p swarms.
    #[tokio::test]
    async fn test_two_node_replication() {
        // Generate identities for both nodes
        let id_a = NodeIdentity::generate().unwrap();
        let id_b = NodeIdentity::generate().unwrap();
        let kp_a = id_a.to_libp2p_keypair().unwrap();
        let kp_b = id_b.to_libp2p_keypair().unwrap();
        let _peer_id_a = PeerId::from(kp_a.public());
        let _peer_id_b = PeerId::from(kp_b.public());

        // Create temp databases
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let storage_a: Arc<dyn cordelia_storage::Storage> =
            Arc::new(SqliteStorage::create_new(&dir_a.path().join("a.db")).unwrap());
        let storage_b: Arc<dyn cordelia_storage::Storage> =
            Arc::new(SqliteStorage::create_new(&dir_b.path().join("b.db")).unwrap());

        let group_id = "test-group";
        let shared_groups = Arc::new(tokio::sync::RwLock::new(vec![group_id.to_string()]));

        // Build swarms on ephemeral ports
        let addr_a: libp2p::Multiaddr = "/ip4/127.0.0.1/tcp/0".parse().unwrap();
        let addr_b: libp2p::Multiaddr = "/ip4/127.0.0.1/tcp/0".parse().unwrap();

        let swarm_a = swarm_task::build_swarm(kp_a, addr_a).unwrap();
        let swarm_b = swarm_task::build_swarm(kp_b, addr_b).unwrap();

        // Get actual listening addresses
        // We need to start the swarms first to get the actual addresses
        let (_cmd_tx_a, cmd_rx_a) = tokio::sync::mpsc::channel(64);
        let (_cmd_tx_b, cmd_rx_b) = tokio::sync::mpsc::channel(64);
        let (event_tx_a, _) = tokio::sync::broadcast::channel(64);
        let (event_tx_b, _) = tokio::sync::broadcast::channel(64);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        // Spawn swarm tasks
        let sa = storage_a.clone();
        let sb = storage_b.clone();
        let ga = shared_groups.clone();
        let gb = shared_groups.clone();
        let eta = event_tx_a.clone();
        let etb = event_tx_b.clone();
        let shut_a = shutdown_tx.subscribe();
        let shut_b = shutdown_tx.subscribe();

        let pool_a = peer_pool::PeerPool::new(shared_groups.clone());
        let pool_b = peer_pool::PeerPool::new(shared_groups.clone());
        let _swarm_a_handle = tokio::spawn(async move {
            swarm_task::run_swarm_loop(
                swarm_a,
                cmd_rx_a,
                eta,
                sa,
                ga,
                pool_a,
                config::NodeRole::Personal,
                shut_a,
            )
            .await;
        });
        let _swarm_b_handle = tokio::spawn(async move {
            swarm_task::run_swarm_loop(
                swarm_b,
                cmd_rx_b,
                etb,
                sb,
                gb,
                pool_b,
                config::NodeRole::Personal,
                shut_b,
            )
            .await;
        });

        // Wait for swarms to start listening
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

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

        // Use sync request via command channel
        // (In a real test we'd need to dial first and wait for connection)
        // For now, verify the basic infrastructure works

        // Clean shutdown
        let _ = shutdown_tx.send(());
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
