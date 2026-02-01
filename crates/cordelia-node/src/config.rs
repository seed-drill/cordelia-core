//! Configuration types for cordelia-node.
//! Parsed from ~/.cordelia/config.toml.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;
use std::str::FromStr;

/// Node role determines dial policy and gossip visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeRole {
    /// Infrastructure relay: appears in gossip, dials all peers.
    Relay,
    /// Personal node (default): hidden from gossip, dials relays/bootnodes only.
    Personal,
    /// High-security keeper: hidden from gossip, dials only trusted relays.
    Keeper,
}

impl NodeRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeRole::Relay => "relay",
            NodeRole::Personal => "personal",
            NodeRole::Keeper => "keeper",
        }
    }
}

impl fmt::Display for NodeRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for NodeRole {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "relay" => Ok(NodeRole::Relay),
            "personal" => Ok(NodeRole::Personal),
            "keeper" => Ok(NodeRole::Keeper),
            other => Err(format!("unknown node role: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub node: NodeSection,
    #[serde(default)]
    pub network: NetworkSection,
    #[serde(default)]
    pub governor: GovernorSection,
    #[serde(default)]
    pub replication: ReplicationSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSection {
    #[serde(default = "default_identity_key")]
    pub identity_key: String,
    #[serde(default = "default_api_transport")]
    pub api_transport: String,
    pub api_socket: Option<String>,
    pub api_addr: Option<String>,
    #[serde(default = "default_database")]
    pub database: String,
    #[serde(default = "default_entity_id")]
    pub entity_id: String,
    /// Node role: "relay", "personal" (default), or "keeper".
    #[serde(default = "default_role")]
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkSection {
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    #[serde(default)]
    pub bootnodes: Vec<BootnodeEntry>,
    /// Keeper-only: explicit relay allowlist. Ignored for other roles.
    #[serde(default)]
    pub trusted_relays: Vec<BootnodeEntry>,
    /// Fixed external address override. Set this on relays/bootnodes with known
    /// public IPs. Personal nodes behind NAT should leave this unset (learned via quorum).
    #[serde(default)]
    pub external_addr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootnodeEntry {
    pub addr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernorSection {
    #[serde(default = "default_2")]
    pub hot_min: usize,
    #[serde(default = "default_20")]
    pub hot_max: usize,
    #[serde(default = "default_10")]
    pub warm_min: usize,
    #[serde(default = "default_50")]
    pub warm_max: usize,
    #[serde(default = "default_100")]
    pub cold_max: usize,
    #[serde(default = "default_3600")]
    pub churn_interval_secs: u64,
    #[serde(default = "default_churn_fraction")]
    pub churn_fraction: f64,
}

impl Default for GovernorSection {
    fn default() -> Self {
        Self {
            hot_min: 2,
            hot_max: 20,
            warm_min: 10,
            warm_max: 50,
            cold_max: 100,
            churn_interval_secs: 3600,
            churn_fraction: 0.2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationSection {
    #[serde(default = "default_300")]
    pub sync_interval_moderate_secs: u64,
    #[serde(default = "default_900")]
    pub sync_interval_taciturn_secs: u64,
    #[serde(default = "default_7")]
    pub tombstone_retention_days: u32,
    #[serde(default = "default_batch")]
    pub max_batch_size: u32,
}

impl Default for ReplicationSection {
    fn default() -> Self {
        Self {
            sync_interval_moderate_secs: cordelia_protocol::SYNC_INTERVAL_MODERATE_SECS,
            sync_interval_taciturn_secs: cordelia_protocol::SYNC_INTERVAL_TACITURN_SECS,
            tombstone_retention_days: cordelia_protocol::TOMBSTONE_RETENTION_DAYS,
            max_batch_size: cordelia_protocol::MAX_BATCH_SIZE,
        }
    }
}

// Default value functions
fn default_identity_key() -> String {
    "~/.cordelia/node.key".into()
}
fn default_api_transport() -> String {
    "unix".into()
}
fn default_database() -> String {
    "~/cordelia/memory/cordelia.db".into()
}
fn default_entity_id() -> String {
    "default".into()
}
fn default_listen_addr() -> String {
    "0.0.0.0:9474".into()
}
fn default_2() -> usize {
    2
}
fn default_10() -> usize {
    10
}
fn default_20() -> usize {
    20
}
fn default_50() -> usize {
    50
}
fn default_100() -> usize {
    100
}
fn default_3600() -> u64 {
    3600
}
fn default_churn_fraction() -> f64 {
    0.2
}
fn default_300() -> u64 {
    cordelia_protocol::SYNC_INTERVAL_MODERATE_SECS
}
fn default_900() -> u64 {
    cordelia_protocol::SYNC_INTERVAL_TACITURN_SECS
}
fn default_7() -> u32 {
    cordelia_protocol::TOMBSTONE_RETENTION_DAYS
}
fn default_batch() -> u32 {
    cordelia_protocol::MAX_BATCH_SIZE
}
fn default_role() -> String {
    "personal".into()
}

impl NodeConfig {
    /// Load config from file, or create default if missing.
    pub fn load_or_default(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            let config: NodeConfig = toml::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// Parse the configured node role.
    pub fn role(&self) -> NodeRole {
        self.node.role.parse().unwrap_or(NodeRole::Personal)
    }

    /// Governor targets capped by role.
    /// Personal: hot 2-5, warm 5-10. Keeper: hot 1-3, warm 2-5.
    /// Relay: use config values as-is.
    pub fn effective_governor_targets(&self) -> GovernorSection {
        let mut g = self.governor.clone();
        match self.role() {
            NodeRole::Personal => {
                g.hot_min = g.hot_min.min(2);
                g.hot_max = g.hot_max.min(5);
                g.warm_min = g.warm_min.min(5);
                g.warm_max = g.warm_max.min(10);
            }
            NodeRole::Keeper => {
                g.hot_min = g.hot_min.min(1);
                g.hot_max = g.hot_max.min(3);
                g.warm_min = g.warm_min.min(2);
                g.warm_max = g.warm_max.min(5);
            }
            NodeRole::Relay => {} // use config values
        }
        g
    }
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            node: NodeSection {
                identity_key: default_identity_key(),
                api_transport: default_api_transport(),
                api_socket: Some("~/.cordelia/node.sock".into()),
                api_addr: None,
                database: default_database(),
                entity_id: default_entity_id(),
                role: default_role(),
            },
            network: NetworkSection::default(),
            governor: GovernorSection::default(),
            replication: ReplicationSection::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = NodeConfig::default();
        assert_eq!(cfg.governor.hot_min, 2);
        assert_eq!(cfg.governor.hot_max, 20);
        assert_eq!(cfg.replication.max_batch_size, 100);
    }

    #[test]
    fn test_parse_toml() {
        let toml_str = r#"
[node]
identity_key = "~/.cordelia/node.key"
api_transport = "http"
api_addr = "127.0.0.1:9473"
database = "~/cordelia/memory/cordelia.db"
entity_id = "russell"

[network]
listen_addr = "0.0.0.0:9474"

[[network.bootnodes]]
addr = "russell.cordelia.seeddrill.io:9474"

[[network.bootnodes]]
addr = "martin.cordelia.seeddrill.io:9474"

[governor]
hot_min = 2
hot_max = 20
warm_min = 10
warm_max = 50

[replication]
sync_interval_moderate_secs = 300
tombstone_retention_days = 7
"#;

        let cfg: NodeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.node.entity_id, "russell");
        assert_eq!(cfg.network.bootnodes.len(), 2);
        assert_eq!(
            cfg.network.bootnodes[0].addr,
            "russell.cordelia.seeddrill.io:9474"
        );
        assert_eq!(cfg.governor.hot_min, 2);
    }

    #[test]
    fn test_default_role_is_personal() {
        let cfg = NodeConfig::default();
        assert_eq!(cfg.role(), NodeRole::Personal);
        assert_eq!(cfg.node.role, "personal");
    }

    #[test]
    fn test_parse_relay_role() {
        let toml_str = r#"
[node]
identity_key = "~/.cordelia/node.key"
api_transport = "http"
api_addr = "127.0.0.1:9473"
database = "~/cordelia/memory/cordelia.db"
entity_id = "boot1"
role = "relay"

[network]
listen_addr = "0.0.0.0:9474"
"#;
        let cfg: NodeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.role(), NodeRole::Relay);
    }

    #[test]
    fn test_parse_keeper_with_trusted_relays() {
        let toml_str = r#"
[node]
identity_key = "~/.cordelia/node.key"
api_transport = "http"
api_addr = "127.0.0.1:9473"
database = "~/cordelia/memory/cordelia.db"
entity_id = "vault"
role = "keeper"

[network]
listen_addr = "0.0.0.0:9474"

[[network.trusted_relays]]
addr = "boot1.cordelia.seeddrill.io:9474"

[[network.trusted_relays]]
addr = "boot2.cordelia.seeddrill.io:9474"
"#;
        let cfg: NodeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.role(), NodeRole::Keeper);
        assert_eq!(cfg.network.trusted_relays.len(), 2);
    }

    #[test]
    fn test_effective_governor_targets_personal() {
        let cfg = NodeConfig::default(); // personal
        let eff = cfg.effective_governor_targets();
        assert!(eff.hot_max <= 5, "personal hot_max should be capped at 5");
        assert!(
            eff.warm_max <= 10,
            "personal warm_max should be capped at 10"
        );
    }

    #[test]
    fn test_effective_governor_targets_relay() {
        let toml_str = r#"
[node]
role = "relay"
"#;
        let cfg: NodeConfig = toml::from_str(toml_str).unwrap();
        let eff = cfg.effective_governor_targets();
        // Relay uses config defaults (20, 50)
        assert_eq!(eff.hot_max, 20);
        assert_eq!(eff.warm_max, 50);
    }

    #[test]
    fn test_serialise_default() {
        let cfg = NodeConfig::default();
        let toml_str = toml::to_string_pretty(&cfg).unwrap();
        assert!(toml_str.contains("[node]"));
        assert!(toml_str.contains("entity_id"));
    }
}
