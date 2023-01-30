use crate::{
    interface::{ClientState, Notification},
    session::{ServerState, SessionHandle, SubscriptionHandle},
    utils::Bytes,
};
use ouisync_lib::{Error, MonitorId, Result};
use std::time::Duration;
use tokio::time;

/// Retrieve a serialized state monitor corresponding to the `path`.
#[no_mangle]
pub unsafe extern "C" fn session_get_state_monitor(
    session: SessionHandle,
    path: *const u8,
    path_len: u64,
) -> Bytes {
    let path = std::slice::from_raw_parts(path, path_len as usize);
    let path: Vec<(String, u64)> = match rmp_serde::from_slice(path) {
        Ok(path) => path,
        Err(e) => {
            tracing::error!(
                "Failed to parse input in session_get_state_monitor as MessagePack: {:?}",
                e
            );
            return Bytes::NULL;
        }
    };
    let path = path
        .into_iter()
        .map(|(name, disambiguator)| MonitorId::new(name, disambiguator));

    if let Some(monitor) = session.get().state.root_monitor.locate(path) {
        let bytes = rmp_serde::to_vec(&monitor).unwrap();
        Bytes::from_vec(bytes)
    } else {
        Bytes::NULL
    }
}

/// Subscribe to "on change" events happening inside a monitor corresponding to the `path`.
pub(crate) fn subscribe(
    server_state: &ServerState,
    client_state: &ClientState,
    path: String,
) -> Result<SubscriptionHandle> {
    let path: Vec<MonitorId> = match path.split('/').map(|s| s.parse()).collect() {
        Ok(path) => path,
        Err(error) => {
            tracing::error!("Failed to parse state monitor path: {:?}", error);
            return Err(Error::MalformedData);
        }
    };

    let monitor = if let Some(monitor) = server_state.root_monitor.locate(path) {
        monitor
    } else {
        return Err(Error::EntryNotFound);
    };

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
