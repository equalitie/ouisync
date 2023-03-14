use crate::state::{State, SubscriptionHandle};
use ouisync_bridge::{
    protocol::{NetworkEvent, Notification},
    transport::NotificationSender,
};
use tokio::select;

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

/// Returns our runtime id formatted as a hex string.
pub(crate) fn this_runtime_id(state: &State) -> String {
    hex::encode(state.network.this_runtime_id().as_ref())
}
