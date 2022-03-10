use super::{
    session,
    utils::{self, Port, UniqueHandle},
};
use crate::network::NetworkEvent;
use std::{os::raw::c_char, ptr};
use tokio::task::JoinHandle;

pub const NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH: u8 = 0;

/// Subscribe to network event notifications.
#[no_mangle]
pub unsafe extern "C" fn network_subscribe(port: Port<u8>) -> UniqueHandle<JoinHandle<()>> {
    let session = session::get();
    let sender = session.sender();
    let mut rx = session.network().handle().subscribe();

    let handle = session.runtime().spawn(async move {
        while let Ok(event) = rx.recv().await {
            sender.send(port, encode_network_event(event));
        }
    });

    UniqueHandle::new(Box::new(handle))
}

fn encode_network_event(event: NetworkEvent) -> u8 {
    match event {
        NetworkEvent::ProtocolVersionMismatch => NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH,
    }
}

/// Return the local network endpoint as string. The format is
/// "<TCP or UDP>:<IPv4 or [IPv6]>:<PORT>". Examples:
///
/// For IPv4: "TCP:192.168.1.1:65522"
/// For IPv6: "TCP:[2001:db8::1]:65522"
///
/// IMPORTANT: the caller is responsible for deallocating the returned pointer unless it is `null`.
#[no_mangle]
pub unsafe extern "C" fn network_listener_local_addr() -> *mut c_char {
    let local_addr = session::get().network().listener_local_addr();

    // TODO: Get <TCP or UDP> from the network object.
    utils::str_to_ptr(&format!("TCP:{}", local_addr))
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
