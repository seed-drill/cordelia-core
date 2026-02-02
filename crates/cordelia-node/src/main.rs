//! Cordelia Node -- single binary P2P memory replication.
//!
//! Usage:
//!   cordelia-node                      # Run with default config
//!   cordelia-node --config path.toml   # Run with custom config
//!   cordelia-node identity             # Show node identity

use cordelia_node::config::{self, RelayPosture};
use cordelia_node::governor_task;
use cordelia_node::peer_pool;
use cordelia_node::replication_task;
use cordelia_node::swarm_task;
use cordelia_node::{expand_tilde, load_or_create_token, parse_listen_addr, StorageClone};

use clap::{Parser, Subcommand};
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
    let listen_addr: libp2p::Multiaddr = parse_listen_addr(&cfg.network.listen_addr).await?;
    let mut swarm = swarm_task::build_swarm(keypair, listen_addr)
        .map_err(|e| anyhow::anyhow!("swarm build failed: {e}"))?;

    // Add external address so identify announces our public IP (critical for Docker/NAT)
    if let Some(ext) = &cfg.network.external_addr {
        let ext_addr: libp2p::Multiaddr = parse_listen_addr(ext).await?;
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

    // Build relay state (only relevant for relay nodes)
    let relay_posture_val = if our_role == config::NodeRole::Relay {
        Some(cfg.relay_posture())
    } else {
        None
    };
    let relay_blocked = Arc::new(cfg.relay_blocked_groups());

    // For dynamic/explicit relays: build the accepted groups set.
    // For dynamic: starts empty, populated by governor via GroupExchange.
    // For explicit: pre-populated from config.
    // For transparent: not used (acceptance is always true).
    let relay_accepted_groups: Option<Arc<tokio::sync::RwLock<std::collections::HashSet<String>>>> =
        match relay_posture_val {
            Some(RelayPosture::Dynamic) => {
                Some(Arc::new(tokio::sync::RwLock::new(std::collections::HashSet::new())))
            }
            Some(RelayPosture::Explicit) => {
                Some(Arc::new(tokio::sync::RwLock::new(cfg.relay_allowed_groups())))
            }
            _ => None,
        };

    // For dynamic relays, the governor needs to write to this same set.
    // We alias it as relay_learned_groups for clarity.
    let relay_learned_groups: Option<Arc<tokio::sync::RwLock<std::collections::HashSet<String>>>> =
        if relay_posture_val == Some(RelayPosture::Dynamic) {
            relay_accepted_groups.clone()
        } else {
            None
        };

    if let Some(posture) = relay_posture_val {
        tracing::info!(
            posture = %posture,
            blocked = relay_blocked.len(),
            "relay posture configured"
        );
    }

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
        let relay_accepted = relay_accepted_groups.clone();
        let relay_blocked = relay_blocked.clone();
        tokio::spawn(async move {
            swarm_task::run_swarm_loop(
                swarm,
                cmd_rx,
                event_tx,
                storage,
                shared_groups,
                pool,
                our_role,
                relay_posture_val,
                relay_accepted,
                relay_blocked,
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
        let relay_learned = relay_learned_groups.clone();
        tokio::spawn(async move {
            governor_task::run_governor_loop(
                governor,
                pool,
                cmd_tx,
                event_rx,
                bootnodes,
                shared_groups,
                relay_learned,
                our_peer_id,
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
        let is_relay = our_role == config::NodeRole::Relay;
        let relay_learned = relay_learned_groups.clone();
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
                is_relay,
                relay_learned,
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
