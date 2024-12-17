use std::net::SocketAddr;

use ouisync::{Network, PeerAddr};
use serde::{Deserialize, Serialize};

use crate::{
    config_keys::{
        LAST_USED_TCP_V4_PORT_KEY, LAST_USED_TCP_V6_PORT_KEY, LAST_USED_UDP_V4_PORT_KEY,
        LAST_USED_UDP_V6_PORT_KEY,
    },
    config_store::ConfigStore,
};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PexConfig {
    // Is sending contacts over PEX enabled?
    pub send: bool,
    // Is receiving contacts over PEX enabled?
    pub recv: bool,
}

impl Default for PexConfig {
    fn default() -> Self {
        Self {
            send: true,
            recv: true,
        }
    }
}

pub(crate) async fn bind_with_reuse_ports(
    network: &Network,
    config: &ConfigStore,
    addrs: &[PeerAddr],
) {
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
