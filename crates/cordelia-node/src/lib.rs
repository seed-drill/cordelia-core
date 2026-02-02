//! Cordelia Node -- library crate for P2P memory replication.
//!
//! Re-exports all internal modules so integration tests and other crates
//! can access swarm, governor, replication, config, and peer pool types.

pub mod config;
pub mod governor_task;
pub mod peer_pool;
pub mod replication_task;
pub mod swarm_task;

// Re-export helper types used by tests and main.rs
use std::path::PathBuf;
use std::sync::Arc;

/// Wrapper to implement Storage on Arc<dyn Storage> for Box<dyn Storage>.
pub struct StorageClone(pub Arc<dyn cordelia_storage::Storage>);

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

pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs_or_home() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

pub fn dirs_or_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn load_or_create_token(path: &PathBuf) -> anyhow::Result<String> {
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

/// Parse listen_addr from config (supports both "host:port" and Multiaddr format).
pub fn parse_listen_addr(addr: &str) -> anyhow::Result<libp2p::Multiaddr> {
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
