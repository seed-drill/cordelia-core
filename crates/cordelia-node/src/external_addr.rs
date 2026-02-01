//! External address tracker for NAT hairpin avoidance.
//!
//! Two modes:
//!   1. Config override -- relay/bootnode operators set `external_addr` in TOML.
//!   2. Quorum learning -- personal nodes learn their external IP from a majority
//!      of connected peers reporting the same observed address.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

/// Tracks this node's external (public) IP address.
pub struct ExternalAddr {
    /// Config override -- if Some, all other logic is bypassed.
    config_override: Option<IpAddr>,
    /// Observations from peers: IpAddr -> count.
    observations: HashMap<IpAddr, u8>,
    /// Total observation count (for majority calculation).
    total: u8,
    /// Current resolved external address (quorum result).
    resolved: Option<IpAddr>,
}

impl ExternalAddr {
    /// Create a new tracker. If `config_override` is provided (from TOML `external_addr`),
    /// the override IP is locked and quorum learning is bypassed.
    pub fn new(config_override: Option<SocketAddr>) -> Self {
        Self {
            config_override: config_override.map(|sa| sa.ip()),
            observations: HashMap::new(),
            total: 0,
            resolved: None,
        }
    }

    /// Record one observation from a peer. Recomputes quorum.
    pub fn observe(&mut self, addr: IpAddr) {
        if self.config_override.is_some() {
            return;
        }
        // Ignore RFC1918 observations -- peers on the same LAN report private IPs
        if is_rfc1918(addr) {
            return;
        }
        self.total = self.total.saturating_add(1);
        let count = self.observations.entry(addr).or_insert(0);
        *count = count.saturating_add(1);
        self.recompute_quorum();
    }

    /// Return the current external IP (config override or quorum result).
    pub fn get(&self) -> Option<IpAddr> {
        self.config_override.or(self.resolved)
    }

    /// Returns true if `peer_ip` matches our external IP AND is not RFC1918.
    /// Used to filter hairpin peers from gossip results.
    pub fn is_same_nat(&self, peer_ip: IpAddr) -> bool {
        if is_rfc1918(peer_ip) {
            return false;
        }
        match self.get() {
            Some(ext) => ext == peer_ip,
            None => false,
        }
    }

    fn recompute_quorum(&mut self) {
        if self.total == 0 {
            self.resolved = None;
            return;
        }
        let threshold = self.total / 2;
        for (&ip, &count) in &self.observations {
            if count > threshold {
                if self.resolved != Some(ip) {
                    // Address changed -- reset observations for fresh convergence
                    let new_count = count;
                    self.observations.clear();
                    self.observations.insert(ip, new_count);
                    self.total = new_count;
                    self.resolved = Some(ip);
                    tracing::info!(%ip, "external address resolved via quorum");
                }
                return;
            }
        }
    }
}

/// Check if an IP address is in RFC1918 private space.
pub fn is_rfc1918(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            // 10.0.0.0/8
            octets[0] == 10
            // 172.16.0.0/12
            || (octets[0] == 172 && (16..=31).contains(&octets[1]))
            // 192.168.0.0/16
            || (octets[0] == 192 && octets[1] == 168)
            // 127.0.0.0/8 (loopback)
            || octets[0] == 127
        }
        IpAddr::V6(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    #[test]
    fn test_config_override_always_wins() {
        let override_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 1), 9474));
        let mut ext = ExternalAddr::new(Some(override_addr));

        // Observe different IPs -- should be ignored
        ext.observe(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)));
        ext.observe(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)));
        ext.observe(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)));

        assert_eq!(ext.get(), Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1))));
    }

    #[test]
    fn test_quorum_requires_majority() {
        let mut ext = ExternalAddr::new(None);
        let ip_a = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1));
        let ip_b = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1));
        let ip_c = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));

        // No observations -> None
        assert_eq!(ext.get(), None);

        // First observation: 1 of 1, 1 > 0 -> resolves immediately
        ext.observe(ip_a);
        assert_eq!(ext.get(), Some(ip_a));

        // Competing observations don't dislodge without majority
        ext.observe(ip_b);
        ext.observe(ip_c);
        assert_eq!(ext.get(), Some(ip_a)); // ip_a still holds

        // Fresh state: multiple IPs competing, no single majority
        ext = ExternalAddr::new(None);
        ext.observe(ip_a);
        // After first observe, ip_a resolves (1 > 0), observations reset to {ip_a:1}
        assert_eq!(ext.get(), Some(ip_a));

        // ip_b needs majority to flip: 2 votes for ip_b vs 1 for ip_a
        ext.observe(ip_b);
        // total=2, threshold=1, ip_a:1 not >1, ip_b:1 not >1
        assert_eq!(ext.get(), Some(ip_a)); // ip_a still holds

        ext.observe(ip_b);
        // total=3, threshold=1, ip_b:2 > 1 -> flips to ip_b
        assert_eq!(ext.get(), Some(ip_b));
    }

    #[test]
    fn test_rfc1918_passthrough() {
        let ext_ip = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1));
        let override_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 1), 9474));
        let ext = ExternalAddr::new(Some(override_addr));

        // RFC1918 addresses should never be treated as same NAT
        assert!(!ext.is_same_nat(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(!ext.is_same_nat(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(!ext.is_same_nat(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(!ext.is_same_nat(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));

        // But our actual external IP should match
        assert!(ext.is_same_nat(ext_ip));
    }

    #[test]
    fn test_observation_reset_on_change() {
        let mut ext = ExternalAddr::new(None);
        let ip_a = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1));
        let ip_b = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1));

        // Establish quorum for ip_a
        ext.observe(ip_a);
        ext.observe(ip_a);
        assert_eq!(ext.get(), Some(ip_a));

        // Now flood with ip_b to flip quorum
        ext.observe(ip_b);
        ext.observe(ip_b);
        ext.observe(ip_b);
        ext.observe(ip_b);
        assert_eq!(ext.get(), Some(ip_b));
    }

    #[test]
    fn test_no_observations_returns_none() {
        let ext = ExternalAddr::new(None);
        assert_eq!(ext.get(), None);
        assert!(!ext.is_same_nat(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1))));
    }

    #[test]
    fn test_rfc1918_observations_ignored() {
        let mut ext = ExternalAddr::new(None);
        // Private IP observations should be ignored
        ext.observe(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        ext.observe(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        assert_eq!(ext.get(), None);
    }

    #[test]
    fn test_is_rfc1918() {
        assert!(is_rfc1918(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(is_rfc1918(IpAddr::V4(Ipv4Addr::new(10, 255, 255, 255))));
        assert!(is_rfc1918(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(is_rfc1918(IpAddr::V4(Ipv4Addr::new(172, 31, 255, 255))));
        assert!(!is_rfc1918(IpAddr::V4(Ipv4Addr::new(172, 32, 0, 1))));
        assert!(is_rfc1918(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1))));
        assert!(is_rfc1918(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(!is_rfc1918(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1))));
        assert!(!is_rfc1918(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    }
}
