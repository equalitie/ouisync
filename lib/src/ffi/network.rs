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
#[no_mangle]
pub unsafe extern "C" fn network_local_addr() -> *const c_char {
    let local_addr = session::get().network().local_addr();

    // TODO: Get <TCP or UDP> from the network object.

    if let Ok(s) = utils::str_to_c_string(&format!("TCP:{}", local_addr)) {
        s.into_raw()
    } else {
        ptr::null()
    }
}
