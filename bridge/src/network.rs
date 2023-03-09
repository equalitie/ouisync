use crate::{
    constants::{NETWORK_EVENT_PEER_SET_CHANGE, NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH},
    protocol::Notification,
    state::{State, SubscriptionHandle},
    transport::NotificationSender,
};
use ouisync_lib::network::{peer_addr::PeerAddr, Network};
use serde::{Deserialize, Serialize};
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use thiserror::Error;
use tokio::select;

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[repr(u8)]
#[serde(into = "u8", try_from = "u8")]
pub enum NetworkEvent {
    ProtocolVersionMismatch = NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH,
    PeerSetChange = NETWORK_EVENT_PEER_SET_CHANGE,
}

impl From<NetworkEvent> for u8 {
    fn from(event: NetworkEvent) -> Self {
        event as u8
    }
}

impl TryFrom<u8> for NetworkEvent {
    type Error = NetworkEventDecodeError;

    fn try_from(input: u8) -> Result<Self, Self::Error> {
        match input {
            NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH => Ok(Self::ProtocolVersionMismatch),
            NETWORK_EVENT_PEER_SET_CHANGE => Ok(Self::PeerSetChange),
            _ => Err(NetworkEventDecodeError),
        }
    }
}

#[derive(Error, Debug)]
#[error("failed to decode network event")]
pub struct NetworkEventDecodeError;

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

/// Subscribe to network event notifications.
pub(crate) fn subscribe(state: &State, notification_tx: &NotificationSender) -> SubscriptionHandle {
    let mut on_protocol_mismatch = state.network.on_protocol_mismatch();
    let mut on_peer_set_change = state.network.on_peer_set_change();

    let notification_tx = notification_tx.clone();

    let entry = state.tasks.vacant_entry();
    let subscription_id = entry.handle().id();

    let handle = scoped_task::spawn(async move {
        // TODO: This loop exits when the first of the watched channels closes. It might be less
        // error prone to keep the loop until all of the channels are closed.
        loop {
            let event = select! {
                e = on_protocol_mismatch.changed() => {
                    match e {
                        Ok(()) => NetworkEvent::ProtocolVersionMismatch,
                        Err(_) => return,
                    }
                },
                e = on_peer_set_change.changed() => {
                    match e {
                        Ok(()) => NetworkEvent::PeerSetChange,
                        Err(_) => return,
                    }
                }
            };

            notification_tx
                .send((subscription_id, Notification::Network(event)))
                .await
                .ok();
        }
    });

    entry.insert(handle)
}

/// Gracefully disconnect from peers.
pub(crate) async fn shutdown(state: &State) {
    state.network.handle().shutdown().await;
}

/// Return the local TCP network endpoint or None if we did not bind to a TCP IPv4 address.
pub(crate) fn tcp_listener_local_addr_v4(state: &State) -> Option<SocketAddr> {
    state.network.tcp_listener_local_addr_v4()
}

/// Return the local TCP network endpoint as or None if we did not bind to a TCP IPv6 address.
pub(crate) fn tcp_listener_local_addr_v6(state: &State) -> Option<SocketAddr> {
    state.network.tcp_listener_local_addr_v6()
}

/// Return the local QUIC/UDP network endpoint or None if we did not bind to a QUIC/UDP IPv4 address.
pub(crate) fn quic_listener_local_addr_v4(state: &State) -> Option<SocketAddr> {
    state.network.quic_listener_local_addr_v4()
}

/// Return the local QUIC/UDP network endpoint or None if we did bind to a QUIC/UDP IPv6 address.
pub(crate) fn quic_listener_local_addr_v6(state: &State) -> Option<SocketAddr> {
    state.network.quic_listener_local_addr_v6()
}

/// Returns our runtime id formatted as a hex string.
pub(crate) fn this_runtime_id(state: &State) -> String {
    hex::encode(state.network.this_runtime_id().as_ref())
}

/// Return our currently used protocol version number.
pub(crate) fn current_protocol_version(state: &State) -> u32 {
    state.network.current_protocol_version()
}

/// Return the highest seen protocol version number. The value returned is always higher
/// or equal to the value returned from current_protocol_version() fn.
pub(crate) fn highest_seen_protocol_version(state: &State) -> u32 {
    state.network.highest_seen_protocol_version()
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
