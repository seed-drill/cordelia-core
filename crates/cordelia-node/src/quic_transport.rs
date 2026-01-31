//! QUIC transport -- endpoint management, accept/dial, stream multiplexing.
//!
//! Each QUIC bidirectional stream is identified by a protocol byte:
//!   0x01 = Handshake
//!   0x02 = Keep-alive
//!   0x03 = Peer-sharing
//!   0x04 = Memory-sync
//!   0x05 = Memory-fetch

use std::net::SocketAddr;
use std::sync::Arc;

use cordelia_governor::Governor;
use cordelia_protocol::tls;
use tokio::sync::{broadcast, Mutex};

use crate::mini_protocols;
use crate::peer_pool::PeerPool;

/// Protocol stream identifiers.
pub const PROTO_HANDSHAKE: u8 = 0x01;
pub const PROTO_KEEPALIVE: u8 = 0x02;
pub const PROTO_PEER_SHARE: u8 = 0x03;
pub const PROTO_MEMORY_SYNC: u8 = 0x04;
pub const PROTO_MEMORY_FETCH: u8 = 0x05;
pub const PROTO_MEMORY_PUSH: u8 = 0x06;
pub const PROTO_GROUP_EXCHANGE: u8 = 0x07;

/// QUIC transport layer.
pub struct QuicTransport {
    pub endpoint: quinn::Endpoint,
    client_config: quinn::ClientConfig,
}

impl QuicTransport {
    /// Create a new QUIC transport.
    ///
    /// Binds to `listen_addr` and configures both server (accept) and client (dial).
    pub fn new(
        listen_addr: SocketAddr,
        pkcs8_der: &[u8],
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (cert_der, key_der) = tls::generate_self_signed_cert(pkcs8_der)?;
        let server_config = tls::build_server_config(cert_der, key_der)?;
        let client_config = tls::build_client_config()?;

        let endpoint = quinn::Endpoint::server(server_config, listen_addr)?;

        Ok(Self {
            endpoint,
            client_config,
        })
    }

    /// Dial a remote peer.
    pub async fn dial(
        &self,
        addr: SocketAddr,
    ) -> Result<quinn::Connection, Box<dyn std::error::Error + Send + Sync>> {
        let conn = self
            .endpoint
            .connect_with(self.client_config.clone(), addr, "cordelia-node.local")?
            .await?;
        Ok(conn)
    }

    /// Run the accept loop -- spawns a task per inbound connection.
    pub async fn listen(
        &self,
        pool: PeerPool,
        storage: Arc<dyn cordelia_storage::Storage>,
        our_node_id: [u8; 32],
        our_groups: Vec<String>,
        governor: Option<Arc<Mutex<Governor>>>,
        shutdown: broadcast::Receiver<()>,
    ) {
        let mut shutdown = shutdown;

        loop {
            tokio::select! {
                incoming = self.endpoint.accept() => {
                    match incoming {
                        Some(incoming) => {
                            let pool = pool.clone();
                            let storage = storage.clone();
                            let our_groups = our_groups.clone();
                            let governor = governor.clone();
                            tokio::spawn(async move {
                                match incoming.await {
                                    Ok(conn) => {
                                        tracing::info!(
                                            remote = %conn.remote_address(),
                                            "accepted inbound connection"
                                        );
                                        run_connection(conn, [0u8; 32], pool, storage, our_node_id, our_groups, governor, true).await;
                                    }
                                    Err(e) => {
                                        tracing::warn!("failed to accept connection: {e}");
                                    }
                                }
                            });
                        }
                        None => {
                            tracing::info!("endpoint closed, stopping accept loop");
                            break;
                        }
                    }
                }
                _ = shutdown.recv() => {
                    tracing::info!("shutdown signal, stopping accept loop");
                    break;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
/// Run a single connection -- accept bidirectional streams and dispatch by protocol ID.
/// `peer_node_id` is known for outbound connections; for inbound, pass [0u8; 32] and
/// the handshake will resolve it. On exit, removes the peer from the pool.
pub async fn run_connection(
    conn: quinn::Connection,
    peer_node_id: [u8; 32],
    pool: PeerPool,
    storage: Arc<dyn cordelia_storage::Storage>,
    our_node_id: [u8; 32],
    our_groups: Vec<String>,
    governor: Option<Arc<Mutex<Governor>>>,
    inbound: bool,
) {
    let remote = conn.remote_address();
    let mut resolved_peer_id = peer_node_id;

    // If inbound, wait for handshake stream first
    if inbound {
        match conn.accept_bi().await {
            Ok((send, recv)) => {
                match mini_protocols::handle_inbound_handshake(
                    send,
                    recv,
                    &conn,
                    &pool,
                    our_node_id,
                    &our_groups,
                )
                .await
                {
                    Ok(peer_id) => {
                        resolved_peer_id = peer_id;
                    }
                    Err(e) => {
                        tracing::warn!(%remote, "handshake failed: {e}");
                        conn.close(quinn::VarInt::from_u32(1), b"handshake failed");
                        return;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(%remote, "failed to accept handshake stream: {e}");
                return;
            }
        }
    }

    // Accept subsequent streams
    loop {
        match conn.accept_bi().await {
            Ok((send, mut recv)) => {
                let pool = pool.clone();
                let storage = storage.clone();
                let our_groups = our_groups.clone();
                let peer_id = resolved_peer_id;
                tokio::spawn(async move {
                    // Read protocol byte
                    let mut proto_buf = [0u8; 1];
                    match recv.read_exact(&mut proto_buf).await {
                        Ok(()) => {}
                        Err(e) => {
                            tracing::debug!("failed to read protocol byte: {e}");
                            return;
                        }
                    }

                    match proto_buf[0] {
                        PROTO_KEEPALIVE => {
                            if let Err(e) = mini_protocols::handle_keepalive(send, recv).await {
                                tracing::debug!("keepalive error: {e}");
                            }
                        }
                        PROTO_PEER_SHARE => {
                            if let Err(e) =
                                mini_protocols::handle_peer_share(send, recv, &pool).await
                            {
                                tracing::debug!("peer-share error: {e}");
                            }
                        }
                        PROTO_MEMORY_SYNC => {
                            if let Err(e) =
                                mini_protocols::handle_memory_sync(send, recv, &storage).await
                            {
                                tracing::debug!("memory-sync error: {e}");
                            }
                        }
                        PROTO_MEMORY_FETCH => {
                            if let Err(e) =
                                mini_protocols::handle_memory_fetch(send, recv, &storage).await
                            {
                                tracing::debug!("memory-fetch error: {e}");
                            }
                        }
                        PROTO_MEMORY_PUSH => {
                            if let Err(e) = mini_protocols::handle_memory_push(
                                send,
                                recv,
                                &storage,
                                &our_groups,
                            )
                            .await
                            {
                                tracing::debug!("memory-push error: {e}");
                            }
                        }
                        PROTO_GROUP_EXCHANGE => {
                            if let Err(e) = mini_protocols::handle_group_exchange(
                                send,
                                recv,
                                &pool,
                                &our_groups,
                                &peer_id,
                            )
                            .await
                            {
                                tracing::debug!("group-exchange error: {e}");
                            }
                        }
                        _other => {
                            tracing::warn!("unknown protocol byte: {_other:#04x}");
                        }
                    }
                });
            }
            Err(quinn::ConnectionError::ApplicationClosed(_)) => {
                tracing::info!(%remote, "connection closed by peer");
                break;
            }
            Err(e) => {
                tracing::warn!(%remote, "connection error: {e}");
                break;
            }
        }
    }

    // Clean up pool on disconnect -- only if this connection is still the active one
    // (avoids race where a stale run_connection removes a freshly-dialled replacement)
    if resolved_peer_id != [0u8; 32] {
        let should_remove = pool
            .get(&resolved_peer_id)
            .await
            .map(|h| h.connection.stable_id() == conn.stable_id())
            .unwrap_or(false);

        if should_remove {
            pool.remove(&resolved_peer_id).await;
            if let Some(ref gov) = governor {
                gov.lock().await.mark_disconnected(&resolved_peer_id);
            }
            tracing::info!(
                peer = hex::encode(resolved_peer_id),
                %remote,
                "connection closed, removed from pool"
            );
        } else {
            tracing::debug!(
                peer = hex::encode(resolved_peer_id),
                %remote,
                "connection closed, pool has newer connection"
            );
        }
    }
}

/// Open a bidirectional stream with a protocol byte prefix.
pub async fn open_protocol_stream(
    conn: &quinn::Connection,
    protocol: u8,
) -> Result<(quinn::SendStream, quinn::RecvStream), Box<dyn std::error::Error + Send + Sync>> {
    let (mut send, recv) = conn.open_bi().await?;
    send.write_all(&[protocol]).await?;
    Ok((send, recv))
}

/// Perform outbound handshake on a newly dialled connection.
pub async fn outbound_handshake(
    conn: &quinn::Connection,
    pool: &PeerPool,
    our_node_id: [u8; 32],
    our_groups: &[String],
) -> Result<[u8; 32], Box<dyn std::error::Error + Send + Sync>> {
    let (send, recv) = conn.open_bi().await?;
    let peer_node_id =
        mini_protocols::handle_outbound_handshake(send, recv, conn, pool, our_node_id, our_groups)
            .await?;
    Ok(peer_node_id)
}
