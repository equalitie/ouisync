use super::{
    session,
    utils::{self, Bytes, Port, UniqueHandle},
};
use ouisync_lib::network::peer_addr::PeerAddr;
use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    os::raw::c_char,
    ptr,
    str::FromStr,
};
use tokio::{select, task::JoinHandle};

pub const NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH: u8 = 0;
pub const NETWORK_EVENT_PEER_SET_CHANGE: u8 = 1;

/// Subscribe to network event notifications.
#[no_mangle]
pub unsafe extern "C" fn network_subscribe(port: Port<u8>) -> UniqueHandle<JoinHandle<()>> {
    let session = session::get();
    let sender = session.sender();

    let mut on_protocol_mismatch = session.network().on_protocol_mismatch();
    let mut on_peer_set_change = session.network().on_peer_set_change();

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

    UniqueHandle::new(Box::new(handle))
}

/// Return the local TCP network endpoint as a string. The format is "<IPv4>:<PORT>". The
/// returned pointer may be null if we did not bind to a TCP IPv4 address.
///
/// Example: "192.168.1.1:65522"
///
/// IMPORTANT: the caller is responsible for deallocating the returned pointer.
#[no_mangle]
pub unsafe extern "C" fn network_tcp_listener_local_addr_v4() -> *mut c_char {
    session::get()
        .network()
        .tcp_listener_local_addr_v4()
        .map(|local_addr| utils::str_to_ptr(&format!("{}", local_addr)))
        .unwrap_or(ptr::null_mut())
}

/// Return the local TCP network endpoint as a string. The format is "<[IPv6]>:<PORT>". The
/// returned pointer pointer may be null if we did bind to a TCP IPv6 address.
///
/// Example: "[2001:db8::1]:65522"
///
/// IMPORTANT: the caller is responsible for deallocating the returned pointer.
#[no_mangle]
pub unsafe extern "C" fn network_tcp_listener_local_addr_v6() -> *mut c_char {
    session::get()
        .network()
        .tcp_listener_local_addr_v6()
        .map(|local_addr| utils::str_to_ptr(&format!("{}", local_addr)))
        .unwrap_or(ptr::null_mut())
}

/// Return the local QUIC/UDP network endpoint as a string. The format is "<IPv4>:<PORT>". The
/// returned pointer may be null if we did not bind to a QUIC/UDP IPv4 address.
///
/// Example: "192.168.1.1:65522"
///
/// IMPORTANT: the caller is responsible for deallocating the returned pointer.
#[no_mangle]
pub unsafe extern "C" fn network_quic_listener_local_addr_v4() -> *mut c_char {
    session::get()
        .network()
        .quic_listener_local_addr_v4()
        .map(|local_addr| utils::str_to_ptr(&format!("{}", local_addr)))
        .unwrap_or(ptr::null_mut())
}

/// Return the local QUIC/UDP network endpoint as a string. The format is "<[IPv6]>:<PORT>". The
/// returned pointer may be null if we did bind to a QUIC/UDP IPv6 address.
///
/// Example: "[2001:db8::1]:65522"
///
/// IMPORTANT: the caller is responsible for deallocating the returned pointer.
#[no_mangle]
pub unsafe extern "C" fn network_quic_listener_local_addr_v6() -> *mut c_char {
    session::get()
        .network()
        .quic_listener_local_addr_v6()
        .map(|local_addr| utils::str_to_ptr(&format!("{}", local_addr)))
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
pub unsafe extern "C" fn network_add_user_provided_quic_peer(addr: *const c_char) -> bool {
    let addr = match utils::ptr_to_str(addr) {
        Ok(addr) => addr,
        Err(_) => return false,
    };

    let addr = match SocketAddr::from_str(addr) {
        Ok(addr) => addr,
        Err(_) => return false,
    };

    // The `Network::add_user_provided_peer` function internally calls `task::spawn` so needs the
    // current Tokio context (thus the `_runtime_guard`).
    let _runtime_guard = session::get().runtime().enter();

    session::get()
        .network()
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
pub unsafe extern "C" fn network_remove_user_provided_quic_peer(addr: *const c_char) -> bool {
    let addr = match utils::ptr_to_str(addr) {
        Ok(addr) => addr,
        Err(_) => return false,
    };

    let addr = match SocketAddr::from_str(addr) {
        Ok(addr) => addr,
        Err(_) => return false,
    };

    session::get()
        .network()
        .remove_user_provided_peer(&PeerAddr::Quic(addr));

    true
}

/// Return the list of peers with which we're connected, serialized with msgpack.
#[no_mangle]
pub unsafe extern "C" fn network_connected_peers() -> Bytes {
    let peer_info = session::get().network().collect_peer_info();
    let bytes = rmp_serde::to_vec(&peer_info).unwrap();
    Bytes::from_vec(bytes)
}

/// Return our currently used protocol version number.
#[no_mangle]
pub unsafe extern "C" fn network_current_protocol_version() -> u32 {
    session::get().network().current_protocol_version()
}

/// Return the highest seen protocol version number. The value returned is always higher
/// or equal to the value returned from network_current_protocol_version() fn.
#[no_mangle]
pub unsafe extern "C" fn network_highest_seen_protocol_version() -> u32 {
    session::get().network().highest_seen_protocol_version()
}

/// Returns the local dht address for ipv4, if available.
/// See [`network_local_addr`] for the format details.
///
/// IMPORTANT: the caller is responsible for deallocating the returned pointer unless it is `null`.
// FIXME: remove this function
#[deprecated = "DHT address is the same as QUIC address. Use network_quic_listener_local_addr_v4 instead"]
#[no_mangle]
pub unsafe extern "C" fn network_dht_local_addr_v4() -> *mut c_char {
    session::get()
        .network()
        .quic_listener_local_addr_v4()
        .map(|addr| utils::str_to_ptr(&format!("UDP:{}", addr)))
        .unwrap_or(ptr::null_mut())
}

/// Returns the local dht address for ipv6, if available.
/// See [`network_local_addr`] for the format details.
///
/// IMPORTANT: the caller is responsible for deallocating the returned pointer unless it is `null`.
// FIXME: remove this function
#[deprecated = "DHT address is the same as QUIC address. Use network_quic_listener_local_addr_v6 instead"]
#[no_mangle]
pub unsafe extern "C" fn network_dht_local_addr_v6() -> *mut c_char {
    session::get()
        .network()
        .quic_listener_local_addr_v6()
        .map(|addr| utils::str_to_ptr(&format!("UDP:{}", addr)))
        .unwrap_or(ptr::null_mut())
}

/// Enables the entire network
#[no_mangle]
pub unsafe extern "C" fn network_enable() {
    session::get().bind_network(vec![
        PeerAddr::Quic((Ipv4Addr::UNSPECIFIED, 0).into()),
        PeerAddr::Quic((Ipv6Addr::UNSPECIFIED, 0).into()),
    ]);
}

/// Disables the entire network
#[no_mangle]
pub unsafe extern "C" fn network_disable() {
    session::get().bind_network(vec![]);
}

/// Enables port forwarding (UPnP)
#[no_mangle]
pub unsafe extern "C" fn network_enable_port_forwarding() {
    let session = session::get();
    let _enter = session.runtime().enter();
    session.network().enable_port_forwarding()
}

/// Disables port forwarding (UPnP)
#[no_mangle]
pub unsafe extern "C" fn network_disable_port_forwarding() {
    session::get().network().disable_port_forwarding()
}

/// Checks whether port forwarding (UPnP) is enabled
#[no_mangle]
pub unsafe extern "C" fn network_is_port_forwarding_enabled() -> bool {
    session::get().network().is_port_forwarding_enabled()
}

/// Enables local discovery
#[no_mangle]
pub unsafe extern "C" fn network_enable_local_discovery() {
    let session = session::get();
    let _enter = session.runtime().enter();
    session.network().enable_local_discovery()
}

/// Disables local discovery
#[no_mangle]
pub unsafe extern "C" fn network_disable_local_discovery() {
    session::get().network().disable_local_discovery()
}

/// Checks whether local discovery is enabled
#[no_mangle]
pub unsafe extern "C" fn network_is_local_discovery_enabled() -> bool {
    session::get().network().is_local_discovery_enabled()
}
