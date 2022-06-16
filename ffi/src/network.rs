use super::{
    session,
    utils::{self, Bytes, Port, UniqueHandle},
};
use std::{os::raw::c_char, ptr};
use tokio::{select, task::JoinHandle};

pub const NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH: u8 = 0;
pub const NETWORK_EVENT_PEER_SET_CHANGE: u8 = 1;

/// Subscribe to network event notifications.
#[no_mangle]
pub unsafe extern "C" fn network_subscribe(port: Port<u8>) -> UniqueHandle<JoinHandle<()>> {
    let session = session::get();
    let sender = session.sender();

    let mut on_protocol_mismatch = session.network().handle().on_protocol_mismatch();
    let mut on_peer_set_change = session.network().handle().on_peer_set_change();

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
#[no_mangle]
pub unsafe extern "C" fn network_dht_local_addr_v4() -> *mut c_char {
    session::get()
        .network()
        .dht_local_addr_v4()
        .map(|addr| utils::str_to_ptr(&format!("UDP:{}", addr)))
        .unwrap_or(ptr::null_mut())
}

/// Returns the local dht address for ipv6, if available.
/// See [`network_local_addr`] for the format details.
///
/// IMPORTANT: the caller is responsible for deallocating the returned pointer unless it is `null`.
#[no_mangle]
pub unsafe extern "C" fn network_dht_local_addr_v6() -> *mut c_char {
    session::get()
        .network()
        .dht_local_addr_v6()
        .map(|addr| utils::str_to_ptr(&format!("UDP:{}", addr)))
        .unwrap_or(ptr::null_mut())
}
