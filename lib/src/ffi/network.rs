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
                            // If it's false, than that's the initial state and there is nothing to report.
                            if *on_peer_set_change.borrow() {
                                sender.send(port, NETWORK_EVENT_PEER_SET_CHANGE);
                            }
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

/// Return the local network endpoint as string. The format is
/// "<TCP or UDP>:<IPv4 or [IPv6]>:<PORT>". Examples:
///
/// For IPv4: "TCP:192.168.1.1:65522"
/// For IPv6: "TCP:[2001:db8::1]:65522"
///
/// IMPORTANT: the caller is responsible for deallocating the returned pointer.
#[no_mangle]
pub unsafe extern "C" fn network_listener_local_addr() -> *mut c_char {
    let local_addr = session::get().network().listener_local_addr();

    // TODO: Get <TCP or UDP> from the network object.
    utils::str_to_ptr(&format!("TCP:{}", local_addr))
}

/// Return the list of peers with which we're connected, serialized with msgpack.
#[no_mangle]
pub unsafe extern "C" fn network_connected_peers() -> Bytes {
    let peer_info = session::get().network().collect_peer_info();
    let bytes = rmp_serde::to_vec(&peer_info).unwrap();
    Bytes::from_vec(bytes)
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
