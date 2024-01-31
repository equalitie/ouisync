use crate::state::{State, SubscriptionHandle};
use ouisync_bridge::{protocol::Notification, transport::NotificationSender};
use state_monitor::{MonitorId, StateMonitor};
use std::time::Duration;
use tokio::time;

/// Retrieve a state monitor corresponding to the `path`.
pub(crate) fn get(state: &State, path: Vec<MonitorId>) -> Result<StateMonitor, ouisync_lib::Error> {
    state
        .root_monitor
        .locate(path)
        .ok_or(ouisync_lib::Error::EntryNotFound)
}

/// Subscribe to "on change" events happening inside a monitor corresponding to the `path`.
pub(crate) fn subscribe(
    state: &State,
    notification_tx: &NotificationSender,
    path: Vec<MonitorId>,
) -> Result<SubscriptionHandle, ouisync_lib::Error> {
    let monitor = state
        .root_monitor
        .locate(path)
        .ok_or(ouisync_lib::Error::EntryNotFound)?;
    let mut rx = monitor.subscribe();
    let notification_tx = notification_tx.clone();

    let handle = state.spawn_task(|id| async move {
        loop {
            match rx.changed().await {
                Ok(()) => {
                    notification_tx
                        .send((id, Notification::StateMonitor))
                        .await
                        .ok();
                }
                Err(_) => return,
            }

            // Prevent flooding the app with too many "on change" notifications.
            time::sleep(Duration::from_millis(200)).await;
        }
    });

    Ok(handle)
}
