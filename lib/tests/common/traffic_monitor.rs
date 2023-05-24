use ouisync::{Repository, StateMonitor};
use tokio::sync::watch;

// Utility to wait for a network traffic to start/stop
pub(crate) struct TrafficMonitor {
    monitor: StateMonitor,
    rx: watch::Receiver<()>,
    last_total: u64,
}

impl TrafficMonitor {
    pub fn new(repo: &Repository) -> Self {
        let monitor = repo.monitor().clone();
        let rx = monitor.subscribe();

        Self {
            monitor,
            rx,
            last_total: 0,
        }
    }

    pub async fn wait_start(&mut self) {
        loop {
            if self.update_total() {
                break;
            }

            self.rx.changed().await.unwrap();
        }
    }

    pub async fn wait_stop(&mut self) {
        loop {
            if self.get_pending() == 0 {
                break;
            }

            self.rx.changed().await.unwrap();
        }
    }

    fn update_total(&mut self) -> bool {
        let new = self
            .monitor
            .get_value("total requests cummulative")
            .unwrap_or_default();

        if new > self.last_total {
            self.last_total = new;
            true
        } else {
            false
        }
    }

    fn get_pending(&self) -> u64 {
        self.monitor
            .get_value("pending requests")
            .unwrap_or_default()
    }
}
