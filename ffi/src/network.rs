use crate::{
    constants::{NETWORK_EVENT_PEER_SET_CHANGE, NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH},
    server_message::Notification,
    session::SubscriptionHandle,
    state::{ClientState, ServerState},
};
use ouisync_lib::{network::peer_addr::PeerAddr, PeerInfo};
use serde::Serialize;
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use tokio::select;

#[derive(Clone, Copy, Serialize)]
#[repr(u8)]
#[serde(into = "u8")]
pub(crate) enum NetworkEvent {
    ProtocolVersionMismatch = NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH,
    PeerSetChange = NETWORK_EVENT_PEER_SET_CHANGE,
}

impl From<NetworkEvent> for u8 {
    fn from(event: NetworkEvent) -> Self {
        event as u8
    }
}

/// Binds the network to the specified addresses.
/// Rebinds if already bound. If any of the addresses is null, that particular protocol/family
/// combination is not bound. If all are null the network is disabled.
/// Returns `Ok` if the binding was successful, `Err` if any of the given addresses failed to
/// parse or are were of incorrect type (e.g. IPv4 instead of IpV6).
pub(crate) async fn bind(
    state: &ServerState,
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

    state.network.handle().bind(&addrs).await;
}

/// Subscribe to network event notifications.
pub(crate) fn subscribe(
    server_state: &ServerState,
    client_state: &ClientState,
) -> SubscriptionHandle {
    let mut on_protocol_mismatch = server_state.network.on_protocol_mismatch();
    let mut on_peer_set_change = server_state.network.on_peer_set_change();

    let notification_tx = client_state.notification_tx.clone();

    let entry = server_state.tasks.vacant_entry();
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
pub(crate) async fn shutdown(state: &ServerState) {
    state.network.handle().shutdown().await;
}

/// Return the local TCP network endpoint or None if we did not bind to a TCP IPv4 address.
pub(crate) fn tcp_listener_local_addr_v4(state: &ServerState) -> Option<SocketAddr> {
    state.network.tcp_listener_local_addr_v4()
}

/// Return the local TCP network endpoint as or None if we did not bind to a TCP IPv6 address.
pub(crate) fn tcp_listener_local_addr_v6(state: &ServerState) -> Option<SocketAddr> {
    state.network.tcp_listener_local_addr_v6()
}

/// Return the local QUIC/UDP network endpoint or None if we did not bind to a QUIC/UDP IPv4 address.
pub(crate) fn quic_listener_local_addr_v4(state: &ServerState) -> Option<SocketAddr> {
    state.network.quic_listener_local_addr_v4()
}

/// Return the local QUIC/UDP network endpoint or None if we did bind to a QUIC/UDP IPv6 address.
pub(crate) fn quic_listener_local_addr_v6(state: &ServerState) -> Option<SocketAddr> {
    state.network.quic_listener_local_addr_v6()
}

/// Add a QUIC endpoint to which which OuiSync shall attempt to connect. Upon failure or success
/// but then disconnection, the endpoint be retried until the below
/// `network_remove_user_provided_quic_peer` function with the same endpoint is called.
pub(crate) fn add_user_provided_quic_peer(state: &ServerState, addr: SocketAddr) {
    state.network.add_user_provided_peer(&PeerAddr::Quic(addr));
}

/// Remove a QUIC endpoint from the list of user provided QUIC peers (added by the above
/// `network_add_user_provided_quic_peer` function). Note that users added by other discovery
/// mechanisms are not affected by this function. Also, removing a peer will not cause
/// disconnection if the connection has already been established. But if the peers disconnected due
/// to other reasons, the connection to this `addr` shall not be reattempted after the call to this
/// function.
pub(crate) fn remove_user_provided_quic_peer(state: &ServerState, addr: SocketAddr) {
    state
        .network
        .remove_user_provided_peer(&PeerAddr::Quic(addr));
}

/// Return the list of known peers.
pub(crate) fn known_peers(state: &ServerState) -> Vec<PeerInfo> {
    state.network.collect_peer_info()
}

/// Returns our runtime id formatted as a hex string.
pub(crate) fn this_runtime_id(state: &ServerState) -> String {
    hex::encode(state.network.this_runtime_id().as_ref())
}

/// Return our currently used protocol version number.
pub(crate) fn current_protocol_version(state: &ServerState) -> u32 {
    state.network.current_protocol_version()
}

/// Return the highest seen protocol version number. The value returned is always higher
/// or equal to the value returned from current_protocol_version() fn.
pub(crate) fn highest_seen_protocol_version(state: &ServerState) -> u32 {
    state.network.highest_seen_protocol_version()
}

/// Enables/disabled port forwarding (UPnP)
pub(crate) fn set_port_forwarding_enabled(state: &ServerState, enabled: bool) {
    if enabled {
        state.network.enable_port_forwarding()
    } else {
        state.network.disable_port_forwarding()
    }
}

/// Checks whether port forwarding (UPnP) is enabled
pub(crate) fn is_port_forwarding_enabled(state: &ServerState) -> bool {
    state.network.is_port_forwarding_enabled()
}

/// Enables/disabled local discovery
pub(crate) fn set_local_discovery_enabled(state: &ServerState, enabled: bool) {
    if enabled {
        state.network.enable_local_discovery()
    } else {
        state.network.disable_local_discovery()
    }
}

/// Checks whether local discovery is enabled
pub(crate) fn is_local_discovery_enabled(state: &ServerState) -> bool {
    state.network.is_local_discovery_enabled()
}
