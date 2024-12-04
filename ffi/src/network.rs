use crate::state::{State, TaskHandle};
use ouisync_bridge::{protocol::Notification, transport::NotificationSender};

/// Subscribe to network event notifications.
pub(crate) fn subscribe(state: &State, notification_tx: &NotificationSender) -> TaskHandle {
    let mut rx = state.network.subscribe();
    let notification_tx = notification_tx.clone();

    state.spawn_task(|id| async move {
        while let Some(event) = rx.recv().await {
            notification_tx
                .send((id, Notification::Network(event)))
                .await
                .ok();
        }
    })
}

/// Returns our runtime id formatted as a hex string.
pub(crate) fn this_runtime_id(state: &State) -> String {
    hex::encode(state.network.this_runtime_id().as_ref())
}
