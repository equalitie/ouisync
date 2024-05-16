//! Wait until replica(s) fully synchronize with another replica.

use super::EVENT_TIMEOUT;
use ouisync::{Repository, VersionVector};
use tokio::{
    select,
    sync::{broadcast::error::RecvError, watch},
    time,
};

pub(crate) struct Sender(watch::Sender<VersionVector>);

impl Sender {
    pub fn new() -> Self {
        Self(watch::Sender::new(VersionVector::new()))
    }

    pub fn subscribe(&self) -> Receiver {
        Receiver(self.0.subscribe())
    }

    /// Notifies the `Receiver`s about changes in `repo`. Returns when there are no more receivers.
    pub async fn run(&self, repo: &Repository) {
        let mut rx = repo.subscribe();
        let branch = repo.local_branch().unwrap();

        loop {
            let vv = branch.version_vector().await.unwrap();
            self.0.send(vv).ok();

            select! {
                event = rx.recv() => {
                    match event {
                        Ok(_) | Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => panic!("notification channel unexpectedly closed"),
                    }
                }
                _ = self.0.closed() => {
                    break;
                }
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct Receiver(watch::Receiver<VersionVector>);

impl Receiver {
    /// Waits until `repo` fully syncs with the one passed to the corresponding `Sender::run`.
    pub async fn run(mut self, repo: &Repository) {
        let mut rx = repo.subscribe();
        let branch = repo.local_branch().unwrap();

        loop {
            let progress = repo.sync_progress().await.unwrap();
            // debug!(progress = %progress.percent());

            if progress.total > 0 && progress.value == progress.total {
                let this_vv = branch.version_vector().await.unwrap();
                let that_vv = self.0.borrow();

                debug!(?this_vv, that_vv = ?*that_vv);

                if this_vv == *that_vv {
                    break;
                }
            }

            select! {
                event = rx.recv() => {
                    match event {
                        Ok(_) | Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => panic!("notification channel unexpectedly closed"),
                    }
                }
                result = self.0.changed() => {
                    if result.is_err() {
                        panic!("sender unexpectedly closed")
                    }

                    continue;
                }
                _ = time::sleep(*EVENT_TIMEOUT) => panic!("timeout waiting for notification"),
            }
        }
    }
}

pub(crate) fn channel() -> (Sender, Receiver) {
    let tx = Sender::new();
    let rx = tx.subscribe();

    (tx, rx)
}
