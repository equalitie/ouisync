use crate::{
    server_message::Notification,
    session::SubscriptionHandle,
    state::{ClientState, ServerState},
};
use ouisync_lib::{Error, MonitorId, Result, StateMonitor};
use std::time::Duration;
use tokio::time;

/// Retrieve a state monitor corresponding to the `path`.
pub(crate) fn get(state: &ServerState, path: Vec<MonitorId>) -> Result<StateMonitor> {
    state.root_monitor.locate(path).ok_or(Error::EntryNotFound)
}

/// Subscribe to "on change" events happening inside a monitor corresponding to the `path`.
pub(crate) fn subscribe(
    server_state: &ServerState,
    client_state: &ClientState,
    path: Vec<MonitorId>,
) -> Result<SubscriptionHandle> {
    let monitor = server_state
        .root_monitor
        .locate(path)
        .ok_or(Error::EntryNotFound)?;
    let mut rx = monitor.subscribe();

    let notification_tx = client_state.notification_tx.clone();

    let entry = server_state.tasks.vacant_entry();
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
