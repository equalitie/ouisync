use super::{
    session,
    utils::{Port, UniqueHandle},
};
use crate::network::NetworkEvent;
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
