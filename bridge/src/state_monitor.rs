use crate::{
    error::Result,
    protocol::Notification,
    state::{State, SubscriptionHandle},
    transport::NotificationSender,
};
use ouisync_lib::{MonitorId, StateMonitor};
use std::time::Duration;
use tokio::time;

/// Retrieve a state monitor corresponding to the `path`.
pub(crate) fn get(state: &State, path: Vec<MonitorId>) -> Result<StateMonitor> {
    Ok(state
        .root_monitor
        .locate(path)
        .ok_or(ouisync_lib::Error::EntryNotFound)?)
}

/// Subscribe to "on change" events happening inside a monitor corresponding to the `path`.
pub(crate) fn subscribe(
    state: &State,
    notification_tx: &NotificationSender,
    path: Vec<MonitorId>,
) -> Result<SubscriptionHandle> {
    let monitor = state
        .root_monitor
        .locate(path)
        .ok_or(ouisync_lib::Error::EntryNotFound)?;
    let mut rx = monitor.subscribe();

    let notification_tx = notification_tx.clone();

    let entry = state.tasks.vacant_entry();
    let subscription_id = entry.handle().id();

    let handle = scoped_task::spawn(async move {
        loop {
            match rx.changed().await {
                Ok(()) => {
                    notification_tx
                        .send((subscription_id, Notification::StateMonitor))
                        .await
                        .ok();
                }
                Err(_) => return,
            }

            // Prevent flooding the app with too many "on change" notifications.
            time::sleep(Duration::from_millis(200)).await;
        }
    });

    Ok(entry.insert(handle))
}
