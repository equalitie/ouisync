use crate::config::{ConfigKey, ConfigStore};
use ouisync_lib::network::{peer_addr::PeerAddr, Network};
use serde::{Deserialize, Serialize};
use std::{io, net::SocketAddr, num::ParseIntError};
use tokio::net;

const BIND_KEY: ConfigKey<Vec<PeerAddr>> =
    ConfigKey::new("bind", "Addresses to bind the network listeners to");

const PORT_FORWARDING_ENABLED_KEY: ConfigKey<bool> =
    ConfigKey::new("port_forwarding_enabled", "Enable port forwarding / UPnP");

const LOCAL_DISCOVERY_ENABLED_KEY: ConfigKey<bool> =
    ConfigKey::new("local_discovery_enabled", "Enable local discovery");

const PEERS_KEY: ConfigKey<Vec<PeerAddr>> = ConfigKey::new(
    "peers",
    "List of peers to connect to in addition to the ones found by various discovery mechanisms\n\
     (e.g. DHT)",
);

const LAST_USED_TCP_V4_PORT_KEY: ConfigKey<u16> =
    ConfigKey::new("last_used_tcp_v4_port", LAST_USED_TCP_PORT_COMMENT);

const LAST_USED_TCP_V6_PORT_KEY: ConfigKey<u16> =
    ConfigKey::new("last_used_tcp_v6_port", LAST_USED_TCP_PORT_COMMENT);

const LAST_USED_UDP_V4_PORT_KEY: ConfigKey<u16> =
    ConfigKey::new("last_used_udp_port_v4", LAST_USED_UDP_PORT_COMMENT);

const LAST_USED_UDP_V6_PORT_KEY: ConfigKey<u16> =
    ConfigKey::new("last_used_udp_port_v6", LAST_USED_UDP_PORT_COMMENT);

const LAST_USED_TCP_PORT_COMMENT: &str =
    "The value stored in this file is the last used TCP port for listening on incoming\n\
     connections. It is used to avoid binding to a random port every time the application starts.\n\
     This, in turn, is mainly useful for users who can't or don't want to use UPnP and have to\n\
     default to manually setting up port forwarding on their routers.";

// Intentionally not being explicity about DHT as eventually this port shall be shared with QUIC.
const LAST_USED_UDP_PORT_COMMENT: &str =
    "The value stored in this file is the last used UDP port for listening on incoming\n\
     connections. It is used to avoid binding to a random port every time the application starts.\n\
     This, in turn, is mainly useful for users who can't or don't want to use UPnP and have to\n\
     default to manually setting up port forwarding on their routers.";

pub const DEFAULT_STORAGE_SERVER_PORT: u16 = 20209;

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct NetworkDefaults {
    pub port_forwarding_enabled: bool,
    pub local_discovery_enabled: bool,
}

/// Initialize the network according to the config.
pub async fn init(network: &Network, config: &ConfigStore, defaults: NetworkDefaults) {
    let bind_addrs = config.entry(BIND_KEY).get().await.unwrap_or_default();
    bind_with_reuse_ports(network, config, &bind_addrs).await;

    let enabled = config
        .entry(PORT_FORWARDING_ENABLED_KEY)
        .get()
        .await
        .unwrap_or(defaults.port_forwarding_enabled);
    network.set_port_forwarding_enabled(enabled);

    let enabled = config
        .entry(LOCAL_DISCOVERY_ENABLED_KEY)
        .get()
        .await
        .unwrap_or(defaults.local_discovery_enabled);
    network.set_local_discovery_enabled(enabled);

    let peers = config.entry(PEERS_KEY).get().await.unwrap_or_default();
    for peer in peers {
        network.add_user_provided_peer(&peer);
    }
}

/// Binds the network to the specified addresses.
/// Rebinds all stacks (TCP/QUIC, IPv4/IPv6) if any of them needs rebinding (address or port
/// changes). If any of the addresses are missing, that particular protocol/family combination is
/// not bound. If all are missing the network is disabled.
pub async fn bind(network: &Network, config: &ConfigStore, addrs: &[PeerAddr]) {
    config.entry(BIND_KEY).set(addrs).await.ok();
    bind_with_reuse_ports(network, config, addrs).await;
}

async fn bind_with_reuse_ports(network: &Network, config: &ConfigStore, addrs: &[PeerAddr]) {
    let mut last_used_ports = LastUsedPorts::load(config).await;
    let addrs: Vec<_> = addrs
        .iter()
        .map(|addr| last_used_ports.apply(*addr))
        .collect();

    network.bind(&addrs).await;

    // Write the actually used ports to the config
    last_used_ports.extract(&network.listener_local_addrs());
    last_used_ports.save(config).await;
}

/// Enable/disable port forwarding
pub async fn set_port_forwarding_enabled(network: &Network, config: &ConfigStore, enabled: bool) {
    config
        .entry(PORT_FORWARDING_ENABLED_KEY)
        .set(&enabled)
        .await
        .ok();
    network.set_port_forwarding_enabled(enabled);
}

/// Enable/disable local discovery
pub async fn set_local_discovery_enabled(network: &Network, config: &ConfigStore, enabled: bool) {
    config
        .entry(LOCAL_DISCOVERY_ENABLED_KEY)
        .set(&enabled)
        .await
        .ok();
    network.set_local_discovery_enabled(enabled);
}

/// Add peers to connect to
pub async fn add_user_provided_peers(network: &Network, config: &ConfigStore, peers: &[PeerAddr]) {
    let entry = config.entry(PEERS_KEY);
    let mut stored = entry.get().await.unwrap_or_default();

    let len = stored.len();
    stored.extend(peers.iter().copied());
    stored.sort();
    stored.dedup();

    if stored.len() > len {
        entry.set(&stored).await.ok();
    }

    for peer in peers {
        network.add_user_provided_peer(peer);
    }
}

/// Remove peers previously added with `add_user_provided_peer`
pub async fn remove_user_provided_peers(
    network: &Network,
    config: &ConfigStore,
    peers: &[PeerAddr],
) {
    let entry = config.entry(PEERS_KEY);
    let mut stored = entry.get().await.unwrap_or_default();

    let len = stored.len();
    stored.retain(|stored| !peers.contains(stored));

    if stored.len() < len {
        entry.set(&stored).await.ok();
    }

    for peer in peers {
        network.remove_user_provided_peer(peer);
    }
}

/// Gets all user provided peers
pub async fn user_provided_peers(config: &ConfigStore) -> Vec<PeerAddr> {
    config.entry(PEERS_KEY).get().await.unwrap_or_default()
}

/// Add a storage server. This adds it as a user provided peers so we can immediatelly connect to
/// it and don't have to wait for it to be discovered (e.g. on the DHT).
///
/// NOTE: Currently this is not persisted.
pub async fn add_storage_server(network: &Network, host: &str) -> Result<(), io::Error> {
    let (hostname, port) = split_port(host).map_err(|error| {
        tracing::error!(host, "invalid storage server host");
        io::Error::new(io::ErrorKind::InvalidInput, error)
    })?;
    let port = port.unwrap_or(DEFAULT_STORAGE_SERVER_PORT);

    let addrs = net::lookup_host((hostname, port))
        .await
        .map(|addrs| addrs.peekable())
        .and_then(|mut addrs| {
            if addrs.peek().is_some() {
                Ok(addrs)
            } else {
                Err(io::Error::new(io::ErrorKind::Other, "no DNS records found"))
            }
        })
        .map_err(|error| {
            tracing::error!(host, ?error, "failed to lookup storage server host");
            error
        })?;

    for addr in addrs {
        network.add_user_provided_peer(&PeerAddr::Quic(addr));
        tracing::info!(host, %addr, "storage server added");
    }

    Ok(())
}

/// Utility to help reuse bind ports across network restarts.
struct LastUsedPorts {
    quic_v4: u16,
    quic_v6: u16,
    tcp_v4: u16,
    tcp_v6: u16,
}

impl LastUsedPorts {
    async fn load(config: &ConfigStore) -> Self {
        Self {
            quic_v4: config
                .entry(LAST_USED_UDP_V4_PORT_KEY)
                .get()
                .await
                .unwrap_or(0),
            quic_v6: config
                .entry(LAST_USED_UDP_V6_PORT_KEY)
                .get()
                .await
                .unwrap_or(0),
            tcp_v4: config
                .entry(LAST_USED_TCP_V4_PORT_KEY)
                .get()
                .await
                .unwrap_or(0),
            tcp_v6: config
                .entry(LAST_USED_TCP_V6_PORT_KEY)
                .get()
                .await
                .unwrap_or(0),
        }
    }

    async fn save(&self, config: &ConfigStore) {
        for (key, value) in [
            (LAST_USED_UDP_V4_PORT_KEY, self.quic_v4),
            (LAST_USED_UDP_V6_PORT_KEY, self.quic_v6),
            (LAST_USED_TCP_V4_PORT_KEY, self.tcp_v4),
            (LAST_USED_TCP_V6_PORT_KEY, self.tcp_v6),
        ] {
            if value != 0 {
                config.entry(key).set(&value).await.ok();
            }
        }
    }

    /// If `addr`'s port is not zero, replace it with the corresponding last used port.
    fn apply(&self, mut addr: PeerAddr) -> PeerAddr {
        if addr.port() != 0 {
            return addr;
        }

        let new_port = match addr {
            PeerAddr::Quic(SocketAddr::V4(_)) => self.quic_v4,
            PeerAddr::Quic(SocketAddr::V6(_)) => self.quic_v6,
            PeerAddr::Tcp(SocketAddr::V4(_)) => self.tcp_v4,
            PeerAddr::Tcp(SocketAddr::V6(_)) => self.tcp_v6,
        };

        addr.set_port(new_port);
        addr
    }

    fn extract(&mut self, addrs: &[PeerAddr]) {
        if let Some(port) = addrs.iter().find_map(|addr| {
            if let PeerAddr::Quic(SocketAddr::V4(_)) = addr {
                Some(addr.port())
            } else {
                None
            }
        }) {
            self.quic_v4 = port;
        }

        if let Some(port) = addrs.iter().find_map(|addr| {
            if let PeerAddr::Quic(SocketAddr::V6(_)) = addr {
                Some(addr.port())
            } else {
                None
            }
        }) {
            self.quic_v6 = port;
        }

        if let Some(port) = addrs.iter().find_map(|addr| {
            if let PeerAddr::Tcp(SocketAddr::V4(_)) = addr {
                Some(addr.port())
            } else {
                None
            }
        }) {
            self.tcp_v4 = port;
        }

        if let Some(port) = addrs.iter().find_map(|addr| {
            if let PeerAddr::Tcp(SocketAddr::V6(_)) = addr {
                Some(addr.port())
            } else {
                None
            }
        }) {
            self.tcp_v6 = port;
        }
    }
}

fn split_port(s: &str) -> Result<(&str, Option<u16>), ParseIntError> {
    if let Some(index) = s.rfind(':') {
        let port = s[index..].parse()?;
        Ok((&s[..index], Some(port)))
    } else {
        Ok((s, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ouisync_lib::StateMonitor;
    use std::{net::Ipv4Addr, time::Duration};
    use tempfile::TempDir;
    use tokio::time;

    #[tokio::test(flavor = "multi_thread")]
    async fn network_disable_enable_idle() {
        let config_dir = TempDir::new().unwrap();
        let config = ConfigStore::new(config_dir.path());
        let network = Network::new(None, StateMonitor::make_root());

        let bind_addr = PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into());

        bind(&network, &config, &[bind_addr]).await;

        let local_addr_0 = network.listener_local_addrs()[0];

        bind(&network, &config, &[]).await;
        bind(&network, &config, &[bind_addr]).await;

        let local_addr_1 = network.listener_local_addrs()[0];

        assert_eq!(local_addr_1, local_addr_0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn network_disable_enable_pending_connection() {
        let config_dir = TempDir::new().unwrap();
        let config = ConfigStore::new(config_dir.path());
        let network = Network::new(None, StateMonitor::make_root());

        let bind_addr = PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into());

        bind(&network, &config, &[bind_addr]).await;
        let local_addr_0 = network.listener_local_addrs()[0];

        let peer_addr = {
            let mut addr = local_addr_0;
            addr.set_port(local_addr_0.port().checked_add(1).unwrap());
            addr
        };

        // Connect and wait until the connection starts begin established.
        network.add_user_provided_peer(&peer_addr);
        expect_knows(&network, peer_addr).await;

        bind(&network, &config, &[]).await;
        bind(&network, &config, &[bind_addr]).await;
        let local_addr_1 = network.listener_local_addrs()[0];

        assert_eq!(local_addr_1, local_addr_0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn network_disable_enable_addr_takeover() {
        use tokio::net::UdpSocket;

        let config_dir = TempDir::new().unwrap();
        let config = ConfigStore::new(config_dir.path());
        let network = Network::new(None, StateMonitor::make_root());

        let bind_addr = PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into());

        bind(&network, &config, &[bind_addr]).await;
        let local_addr_0 = network.listener_local_addrs()[0];

        bind(&network, &config, &[]).await;

        // Bind some other socket to the same address while the network is disabled.
        let _socket = time::timeout(TIMEOUT, async {
            loop {
                if let Ok(socket) = UdpSocket::bind(local_addr_0.socket_addr()).await {
                    break socket;
                } else {
                    time::sleep(Duration::from_millis(250)).await;
                }
            }
        })
        .await
        .unwrap();

        // Enabling the network binds it to a different port.
        bind(&network, &config, &[bind_addr]).await;
        let local_addr_1 = network.listener_local_addrs()[0];

        assert_ne!(local_addr_1, local_addr_0);
    }

    const TIMEOUT: Duration = Duration::from_secs(10);

    async fn expect_knows(network: &Network, peer_addr: PeerAddr) {
        time::timeout(TIMEOUT, async move {
            let mut rx = network.on_peer_set_change();

            loop {
                if network.peer_info(peer_addr).is_some() {
                    break;
                }

                rx.changed().await.unwrap();
            }
        })
        .await
        .unwrap()
    }
}
