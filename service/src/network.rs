use std::{net::SocketAddr, num::NonZeroU16};

use ouisync::{BindAddr, BindPort, Network, PeerAddr};
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
    let addrs: Vec<BindAddr> = addrs
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

    /// Converts `addr` to `BindAddr`. If `addr`'s port is zero (random), prefers the
    /// corresponding last used port (if any). Otherwise binds strictly to `addr`'s port.
    fn apply(&self, addr: PeerAddr) -> BindAddr {
        let mut bind_addr = BindAddr::from(addr);

        if bind_addr.port == BindPort::Random {
            let last_used_port = match addr {
                PeerAddr::Quic(SocketAddr::V4(_)) => self.quic_v4,
                PeerAddr::Quic(SocketAddr::V6(_)) => self.quic_v6,
                PeerAddr::Tcp(SocketAddr::V4(_)) => self.tcp_v4,
                PeerAddr::Tcp(SocketAddr::V6(_)) => self.tcp_v6,
            };

            if let Some(port) = NonZeroU16::new(last_used_port) {
                bind_addr.port = BindPort::Preferred(port);
            }
        }

        bind_addr
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn last_used_ports_apply() {
        let ports = LastUsedPorts {
            quic_v4: 1000,
            quic_v6: 0,
            tcp_v4: 2000,
            tcp_v6: 0,
        };

        let port = |port| NonZeroU16::new(port).unwrap();

        // Random port requested and last used port known => prefer the last used port.
        assert_eq!(
            ports
                .apply(PeerAddr::Quic((Ipv4Addr::UNSPECIFIED, 0).into()))
                .port,
            BindPort::Preferred(port(1000))
        );

        // Random port requested and no last used port => random.
        assert_eq!(
            ports
                .apply(PeerAddr::Quic((Ipv6Addr::UNSPECIFIED, 0).into()))
                .port,
            BindPort::Random
        );

        // Specific port requested => strict, regardless of the last used port.
        assert_eq!(
            ports
                .apply(PeerAddr::Tcp((Ipv4Addr::UNSPECIFIED, 3000).into()))
                .port,
            BindPort::Strict(port(3000))
        );
    }
}
