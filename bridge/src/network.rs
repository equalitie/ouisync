use crate::config::{ConfigEntry, ConfigKey, ConfigStore};
use futures_util::future;
use ouisync_lib::network::{peer_addr::PeerAddr, Network};
use std::net::{SocketAddrV4, SocketAddrV6};

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

/// Binds the network to the specified addresses.
/// Rebinds if already bound. If any of the addresses is null, that particular protocol/family
/// combination is not bound. If all are null the network is disabled.
/// Returns `Ok` if the binding was successful, `Err` if any of the given addresses failed to
/// parse or are were of incorrect type (e.g. IPv4 instead of IpV6).
pub async fn bind(
    network: &Network,
    config: &ConfigStore,
    quic_v4: Option<SocketAddrV4>,
    quic_v6: Option<SocketAddrV6>,
    tcp_v4: Option<SocketAddrV4>,
    tcp_v6: Option<SocketAddrV6>,
) {
    let config_entry_quic_v4 = config.entry(LAST_USED_UDP_V4_PORT_KEY);
    let config_entry_quic_v6 = config.entry(LAST_USED_UDP_V6_PORT_KEY);
    let config_entry_tcp_v4 = config.entry(LAST_USED_TCP_V4_PORT_KEY);
    let config_entry_tcp_v6 = config.entry(LAST_USED_TCP_V6_PORT_KEY);

    // Read last used ports from the config and apply them to the addresses
    let addrs = future::join_all(
        [
            (
                quic_v4.map(From::from).map(PeerAddr::Quic),
                &config_entry_quic_v4,
            ),
            (
                quic_v6.map(From::from).map(PeerAddr::Quic),
                &config_entry_quic_v6,
            ),
            (
                tcp_v4.map(From::from).map(PeerAddr::Tcp),
                &config_entry_tcp_v4,
            ),
            (
                tcp_v6.map(From::from).map(PeerAddr::Tcp),
                &config_entry_tcp_v6,
            ),
        ]
        .into_iter()
        .filter_map(|(addr, entry)| Some((addr?, entry)))
        .map(|(addr, entry)| use_last_port(addr, entry)),
    )
    .await;

    network.handle().bind(&addrs).await;

    // Write the actually used ports to the config
    future::join_all(
        [
            (network.quic_listener_local_addr_v4(), config_entry_quic_v4),
            (network.quic_listener_local_addr_v6(), config_entry_quic_v6),
            (network.tcp_listener_local_addr_v4(), config_entry_tcp_v4),
            (network.tcp_listener_local_addr_v6(), config_entry_tcp_v6),
        ]
        .into_iter()
        .filter_map(|(addr, entry)| Some((addr?, entry)))
        .map(|(addr, entry)| async move { entry.set(&addr.port()).await.ok() }),
    )
    .await;
}

async fn use_last_port(mut addr: PeerAddr, config: &ConfigEntry<u16>) -> PeerAddr {
    if addr.port() == 0 {
        if let Ok(last_port) = config.get().await {
            addr.set_port(last_port);
        }
    }

    addr
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::Ipv4Addr, time::Duration};
    use tempfile::TempDir;
    use tokio::time;

    #[tokio::test(flavor = "multi_thread")]
    async fn network_disable_enable_idle() {
        let config_dir = TempDir::new().unwrap();
        let config = ConfigStore::new(config_dir.path());
        let network = Network::new();

        let bind_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);

        bind(&network, &config, Some(bind_addr), None, None, None).await;

        let local_addr_0 = network.quic_listener_local_addr_v4().unwrap();

        bind(&network, &config, None, None, None, None).await;
        bind(&network, &config, Some(bind_addr), None, None, None).await;

        let local_addr_1 = network.quic_listener_local_addr_v4().unwrap();

        assert_eq!(local_addr_1, local_addr_0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn network_disable_enable_pending_connection() {
        let config_dir = TempDir::new().unwrap();
        let config = ConfigStore::new(config_dir.path());
        let network = Network::new();

        let bind_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);

        bind(&network, &config, Some(bind_addr), None, None, None).await;
        let local_addr_0 = network.quic_listener_local_addr_v4().unwrap();

        let peer_addr = {
            let mut addr = local_addr_0;
            addr.set_port(local_addr_0.port().checked_add(1).unwrap());
            PeerAddr::Quic(addr)
        };

        // Connect and wait until the connection starts begin established.
        network.add_user_provided_peer(&peer_addr);
        expect_knows(&network, peer_addr).await;

        bind(&network, &config, None, None, None, None).await;
        bind(&network, &config, Some(bind_addr), None, None, None).await;
        let local_addr_1 = network.quic_listener_local_addr_v4().unwrap();

        assert_eq!(local_addr_1, local_addr_0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn network_disable_enable_addr_takeover() {
        use tokio::net::UdpSocket;

        let config_dir = TempDir::new().unwrap();
        let config = ConfigStore::new(config_dir.path());
        let network = Network::new();

        let bind_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);

        bind(&network, &config, Some(bind_addr), None, None, None).await;
        let local_addr_0 = network.quic_listener_local_addr_v4().unwrap();

        bind(&network, &config, None, None, None, None).await;

        // Bind some other socket to the same address while the network is disabled.
        let _socket = time::timeout(TIMEOUT, async {
            loop {
                if let Ok(socket) = UdpSocket::bind(local_addr_0).await {
                    break socket;
                } else {
                    time::sleep(Duration::from_millis(250)).await;
                }
            }
        })
        .await
        .unwrap();

        // Enabling the network binds it to a different port.
        bind(&network, &config, Some(bind_addr), None, None, None).await;
        let local_addr_1 = network.quic_listener_local_addr_v4().unwrap();

        assert_ne!(local_addr_1, local_addr_0);
    }

    const TIMEOUT: Duration = Duration::from_secs(10);

    async fn expect_knows(network: &Network, peer_addr: PeerAddr) {
        time::timeout(TIMEOUT, async move {
            let mut rx = network.on_peer_set_change();

            loop {
                if network.knows_peer(peer_addr) {
                    break;
                }

                rx.changed().await.unwrap();
            }
        })
        .await
        .unwrap()
    }
}
