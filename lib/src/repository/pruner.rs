use super::{utils, Shared};
use crate::error::Result;
use std::sync::Arc;
use tokio::{
    select,
    sync::{broadcast::error::RecvError, mpsc, oneshot},
};

/// Worker that removes outdated snapshot and branches.
pub(super) struct Pruner {
    shared: Arc<Shared>,
    command_rx: mpsc::Receiver<Command>,
}

impl Pruner {
    pub fn new(shared: Arc<Shared>) -> (Self, PrunerHandle) {
        let (command_tx, command_rx) = mpsc::channel(1);
        (Self { shared, command_rx }, PrunerHandle { command_tx })
    }

    pub async fn run(mut self) {
        let mut notify_rx = self.shared.store.index.subscribe();
        let mut blob_close_rx = self.shared.blob_cache.subscribe();

        loop {
            // Wait for either a notification about branch change or that a blob has been closed.
            let event = async {
                select! {
                    event = notify_rx.recv() => matches!(event, Ok(_) | Err(RecvError::Lagged(_))),
                    event = blob_close_rx.dropped() => event.is_ok(),
                }
            };

            select! {
                event = event => {
                    if event {
                        match self.process().await {
                            Ok(()) => (),
                            Err(error) => {
                                log::error!("Pruner failed: {:?}", error);
                            }
                        }
                    } else {
                        break
                    }
                }
                command = self.command_rx.recv() => {
                    match command {
                        Some(Command::Prune(result_tx)) => {
                            let result = self.process().await;
                            result_tx.send(result).unwrap_or(());
                        }
                        None => break,
                    }
                }
            }
        }

        log::trace!("Pruner terminated");
    }

    async fn process(&self) -> Result<()> {
        let mut conn = self.shared.store.db().acquire().await?;

        let local_id = self.shared.local_branch().map(|branch| *branch.id());
        let (uptodate, outdated) = utils::partition_branches(
            &mut conn,
            self.shared.collect_branches()?,
            local_id.as_ref(),
        )
        .await?;

        // Remove outdated branches
        for branch in outdated {
            // Never remove local branch
            if Some(branch.id()) == local_id.as_ref() {
                continue;
            }

            // Don't remove branches that are in use. We get notified when they stopped being used
            // so we can try again.
            if branch.is_any_blob_open() {
                continue;
            }

            log::trace!("removing outdated branch {:?}", branch.id());
            self.shared.store.index.remove_branch(branch.id());
            branch.data().destroy(&mut conn).await?;
        }

        // Remove outdated snapshots
        for branch in uptodate {
            branch.data().remove_old_snapshots(&mut conn).await?;
        }

        Ok(())
    }
}

pub(super) struct PrunerHandle {
    command_tx: mpsc::Sender<Command>,
}

impl PrunerHandle {
    /// Trigger branch pruning and wait for it to complete.
    pub async fn prune(&self) -> Result<()> {
        utils::oneshot(&self.command_tx, Command::Prune).await
    }
}

enum Command {
    Prune(oneshot::Sender<Result<()>>),
}
