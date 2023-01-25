use super::{
    registry::Handle,
    session::SessionHandle,
    utils::{self, Bytes, Port},
};
use ouisync_lib::{network::peer_addr::PeerAddr, Result};
use scoped_task::ScopedJoinHandle;
use std::{
    net::{SocketAddr, SocketAddrV4, SocketAddrV6},
    os::raw::c_char,
    ptr,
    str::FromStr,
};
use tokio::select;

pub const NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH: u8 = 0;
pub const NETWORK_EVENT_PEER_SET_CHANGE: u8 = 1;

/// Binds the network to the specified addresses.
/// Rebinds if already bound. If any of the addresses is null, that particular protocol/family
/// combination is not bound. If all are null the network is disabled.
/// Yields `Ok` if the binding was successful, `Err` if any of the given addresses failed to
/// parse or are were of incorrect type (e.g. IPv4 instead of IpV6).
#[no_mangle]
pub unsafe extern "C" fn network_bind(
    session: SessionHandle,
    quic_v4: *const c_char,
    quic_v6: *const c_char,
    tcp_v4: *const c_char,
    tcp_v6: *const c_char,
    port: Port<Result<()>>,
) {
    session.get().with(port, |ctx| {
        let quic_v4: Option<SocketAddrV4> = utils::parse_from_ptr(quic_v4)?;
        let quic_v6: Option<SocketAddrV6> = utils::parse_from_ptr(quic_v6)?;
        let tcp_v4: Option<SocketAddrV4> = utils::parse_from_ptr(tcp_v4)?;
        let tcp_v6: Option<SocketAddrV6> = utils::parse_from_ptr(tcp_v6)?;

        let addrs: Vec<_> = [
            quic_v4.map(|a| PeerAddr::Quic(a.into())),
            quic_v6.map(|a| PeerAddr::Quic(a.into())),
            tcp_v4.map(|a| PeerAddr::Tcp(a.into())),
            tcp_v6.map(|a| PeerAddr::Tcp(a.into())),
        ]
        .into_iter()
        .flatten()
        .collect();

        let network_handle = ctx.state().network.handle();

        ctx.spawn(async move {
            network_handle.bind(&addrs).await;
            Ok(())
        })
    })
}

/// Subscribe to network event notifications.
#[no_mangle]
pub unsafe extern "C" fn network_subscribe(
    session: SessionHandle,
    port: Port<u8>,
) -> Handle<ScopedJoinHandle<()>> {
    let session = session.get();
    let sender = session.sender();

    let mut on_protocol_mismatch = session.state.network.on_protocol_mismatch();
    let mut on_peer_set_change = session.state.network.on_peer_set_change();

    let handle = session.runtime().spawn(async move {
        // TODO: This loop exits when the first of the watched channels closes. It might be less
        // error prone to keep the loop until all of the channels are closed.
        loop {
            select! {
                e = on_protocol_mismatch.changed() => {
                    match e {
                        Ok(()) => {
                            sender.send(port, NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH);
                        },
                        Err(_) => {
                            return;
                        }
                    }
                },
                e = on_peer_set_change.changed() => {
                    match e {
                        Ok(()) => {
                            sender.send(port, NETWORK_EVENT_PEER_SET_CHANGE);
                        },
                        Err(_) => {
                            return;
                        }
                    }
                }
            }
        }
    });
    let handle = ScopedJoinHandle(handle);

    session.state.tasks.insert(handle)
}

/// Gracefully disconnect from peers.
#[no_mangle]
pub unsafe extern "C" fn network_shutdown(session: SessionHandle, port: Port<Result<()>>) {
    session.get().with(port, |ctx| {
        let handle = ctx.state().network.handle();

        ctx.spawn(async move {
            handle.shutdown().await;
            Ok(())
        })
    })
}

/// Return the local TCP network endpoint as a string. The format is "<IPv4>:<PORT>". The
/// returned pointer may be null if we did not bind to a TCP IPv4 address.
///
/// Example: "192.168.1.1:65522"
///
/// IMPORTANT: the caller is responsible for deallocating the returned pointer.
#[no_mangle]
pub unsafe extern "C" fn network_tcp_listener_local_addr_v4(session: SessionHandle) -> *mut c_char {
    session
        .get()
        .state
        .network
        .tcp_listener_local_addr_v4()
        .map(|local_addr| utils::str_to_ptr(&local_addr.to_string()))
        .unwrap_or(ptr::null_mut())
}

/// Return the local TCP network endpoint as a string. The format is "<[IPv6]>:<PORT>". The
/// returned pointer pointer may be null if we did bind to a TCP IPv6 address.
///
/// Example: "[2001:db8::1]:65522"
///
/// IMPORTANT: the caller is responsible for deallocating the returned pointer.
#[no_mangle]
pub unsafe extern "C" fn network_tcp_listener_local_addr_v6(session: SessionHandle) -> *mut c_char {
    session
        .get()
        .state
        .network
        .tcp_listener_local_addr_v6()
        .map(|local_addr| utils::str_to_ptr(&local_addr.to_string()))
        .unwrap_or(ptr::null_mut())
}

/// Return the local QUIC/UDP network endpoint as a string. The format is "<IPv4>:<PORT>". The
/// returned pointer may be null if we did not bind to a QUIC/UDP IPv4 address.
///
/// Example: "192.168.1.1:65522"
///
/// IMPORTANT: the caller is responsible for deallocating the returned pointer.
#[no_mangle]
pub unsafe extern "C" fn network_quic_listener_local_addr_v4(
    session: SessionHandle,
) -> *mut c_char {
    session
        .get()
        .state
        .network
        .quic_listener_local_addr_v4()
        .map(|local_addr| utils::str_to_ptr(&local_addr.to_string()))
        .unwrap_or(ptr::null_mut())
}

/// Return the local QUIC/UDP network endpoint as a string. The format is "<[IPv6]>:<PORT>". The
/// returned pointer may be null if we did bind to a QUIC/UDP IPv6 address.
///
/// Example: "[2001:db8::1]:65522"
///
/// IMPORTANT: the caller is responsible for deallocating the returned pointer.
#[no_mangle]
pub unsafe extern "C" fn network_quic_listener_local_addr_v6(
    session: SessionHandle,
) -> *mut c_char {
    session
        .get()
        .state
        .network
        .quic_listener_local_addr_v6()
        .map(|local_addr| utils::str_to_ptr(&local_addr.to_string()))
        .unwrap_or(ptr::null_mut())
}

/// Add a QUIC endpoint to which which OuiSync shall attempt to connect. Upon failure or success
/// but then disconnection, the endpoint be retried until the below
/// `network_remove_user_provided_quic_peer` function with the same endpoint is called.
///
/// The endpoint provided to this function may be an IPv4 endpoint in the format
/// "192.168.0.1:1234", or an IPv6 address in the format "[2001:db8:1]:1234".
///
/// If the format is not parsed correctly, this function returns `false`, in all other cases it
/// returns `true`. The latter includes the case when the peer has already been added.
#[no_mangle]
pub unsafe extern "C" fn network_add_user_provided_quic_peer(
    session: SessionHandle,
    addr: *const c_char,
) -> bool {
    let addr = match utils::ptr_to_str(addr) {
        Ok(addr) => addr,
        Err(_) => return false,
    };

    let addr = match SocketAddr::from_str(addr) {
        Ok(addr) => addr,
        Err(_) => return false,
    };

    let session = session.get();

    // The `Network::add_user_provided_peer` function internally calls `task::spawn` so needs the
    // current Tokio context (thus the `_runtime_guard`).
    let _runtime_guard = session.runtime().enter();

    session
        .state
        .network
        .add_user_provided_peer(&PeerAddr::Quic(addr));

    true
}

/// Remove a QUIC endpoint from the list of user provided QUIC peers (added by the above
/// `network_add_user_provided_quic_peer` function). Note that users added by other discovery
/// mechanisms are not affected by this function. Also, removing a peer will not cause
/// disconnection if the connection has already been established. But if the peers disconnected due
/// to other reasons, the connection to this `addr` shall not be reattempted after the call to this
/// function.
///
/// The endpoint provided to this function may be an IPv4 endpoint in the format
/// "192.168.0.1:1234", or an IPv6 address in the format "[2001:db8:1]:1234".
///
/// If the format is not parsed correctly, this function returns `false`, in all other cases it
/// returns `true`. The latter includes the case when the peer has not been previously added.
#[no_mangle]
pub unsafe extern "C" fn network_remove_user_provided_quic_peer(
    session: SessionHandle,
    addr: *const c_char,
) -> bool {
    let addr = match utils::ptr_to_str(addr) {
        Ok(addr) => addr,
        Err(_) => return false,
    };

    let addr = match SocketAddr::from_str(addr) {
        Ok(addr) => addr,
        Err(_) => return false,
    };

    session
        .get()
        .state
        .network
        .remove_user_provided_peer(&PeerAddr::Quic(addr));

    true
}

/// Return the list of peers with which we're connected, serialized with msgpack.
#[no_mangle]
pub unsafe extern "C" fn network_connected_peers(session: SessionHandle) -> Bytes {
    let peer_info = session.get().state.network.collect_peer_info();
    let bytes = rmp_serde::to_vec(&peer_info).unwrap();
    Bytes::from_vec(bytes)
}

/// Returns our runtime id formatted as a hex string.
/// The caller is responsible for deallocating it.
#[no_mangle]
pub unsafe extern "C" fn network_this_runtime_id(session: SessionHandle) -> *const c_char {
    utils::str_to_ptr(&hex::encode(
        session.get().state.network.this_runtime_id().as_ref(),
    ))
}

/// Return our currently used protocol version number.
#[no_mangle]
pub unsafe extern "C" fn network_current_protocol_version(session: SessionHandle) -> u32 {
    session.get().state.network.current_protocol_version()
}

/// Return the highest seen protocol version number. The value returned is always higher
/// or equal to the value returned from network_current_protocol_version() fn.
#[no_mangle]
pub unsafe extern "C" fn network_highest_seen_protocol_version(session: SessionHandle) -> u32 {
    session.get().state.network.highest_seen_protocol_version()
}

/// Enables port forwarding (UPnP)
#[no_mangle]
pub unsafe extern "C" fn network_enable_port_forwarding(session: SessionHandle) {
    let session = session.get();
    let _enter = session.runtime().enter();
    session.state.network.enable_port_forwarding()
}

/// Disables port forwarding (UPnP)
#[no_mangle]
pub unsafe extern "C" fn network_disable_port_forwarding(session: SessionHandle) {
    session.get().state.network.disable_port_forwarding()
}

/// Checks whether port forwarding (UPnP) is enabled
#[no_mangle]
pub unsafe extern "C" fn network_is_port_forwarding_enabled(session: SessionHandle) -> bool {
    session.get().state.network.is_port_forwarding_enabled()
}

/// Enables local discovery
#[no_mangle]
pub unsafe extern "C" fn network_enable_local_discovery(session: SessionHandle) {
    let session = session.get();
    let _enter = session.runtime().enter();
    session.state.network.enable_local_discovery()
}

/// Disables local discovery
#[no_mangle]
pub unsafe extern "C" fn network_disable_local_discovery(session: SessionHandle) {
    session.get().state.network.disable_local_discovery()
}

/// Checks whether local discovery is enabled
#[no_mangle]
pub unsafe extern "C" fn network_is_local_discovery_enabled(session: SessionHandle) -> bool {
    session.get().state.network.is_local_discovery_enabled()
}
