use ouisync_lib::network::{peer_addr::PeerAddr, Network};
use std::net::{SocketAddrV4, SocketAddrV6};

/// Binds the network to the specified addresses.
/// Rebinds if already bound. If any of the addresses is null, that particular protocol/family
/// combination is not bound. If all are null the network is disabled.
/// Returns `Ok` if the binding was successful, `Err` if any of the given addresses failed to
/// parse or are were of incorrect type (e.g. IPv4 instead of IpV6).
pub async fn bind(
    network: &Network,
    quic_v4: Option<SocketAddrV4>,
    quic_v6: Option<SocketAddrV6>,
    tcp_v4: Option<SocketAddrV4>,
    tcp_v6: Option<SocketAddrV6>,
) {
    let addrs: Vec<_> = [
        quic_v4.map(|a| PeerAddr::Quic(a.into())),
        quic_v6.map(|a| PeerAddr::Quic(a.into())),
        tcp_v4.map(|a| PeerAddr::Tcp(a.into())),
        tcp_v6.map(|a| PeerAddr::Tcp(a.into())),
    ]
    .into_iter()
    .flatten()
    .collect();

    network.handle().bind(&addrs).await;
}

/// Enables/disabled port forwarding (UPnP)
pub fn set_port_forwarding_enabled(network: &Network, enabled: bool) {
    if enabled {
        network.enable_port_forwarding()
    } else {
        network.disable_port_forwarding()
    }
}

/// Enables/disabled local discovery
pub fn set_local_discovery_enabled(network: &Network, enabled: bool) {
    if enabled {
        network.enable_local_discovery()
    } else {
        network.disable_local_discovery()
    }
}
