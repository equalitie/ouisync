use metrics_ext::WatchRecorderSubscriber;
use ouisync::Repository;
use state_monitor::StateMonitor;
use tokio::{select, sync::watch};

use super::actor;

/// Utility to wait for all network requests to be handled.
///
/// This is useful for tests where we assert a non-existence (e.g., of a file). We can't wait for a
/// notification event in that case (because nothing changes) so instead we wait for the traffic to
/// settle to ensure nothing can change anymore and then assert the non-existence.
pub(crate) struct TrafficMonitor {
    requests_pending_rx: watch::Receiver<f64>,
}

impl TrafficMonitor {
    pub fn new(subscriber: WatchRecorderSubscriber) -> Self {
        let requests_pending_rx = subscriber.gauge("requests pending".into());

        Self {
            requests_pending_rx,
        }
    }

    pub async fn wait(&mut self) {
        self.requests_pending_rx
            .wait_for(|value| *value > 0.0)
            .await
            .unwrap();

        self.requests_pending_rx
            .wait_for(|value| *value == 0.0)
            .await
            .unwrap();
    }
}
