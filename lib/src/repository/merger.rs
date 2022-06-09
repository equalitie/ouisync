use super::Shared;
use crate::{
    branch::Branch,
    error::{Error, Result},
    joint_directory::JointDirectory,
};
use log::Level;
use std::sync::Arc;
use tokio::{
    select,
    sync::{mpsc, oneshot},
};

/// Utility that merges remote branches into the local one.
pub(super) struct Merger {
    shared: Arc<Shared>,
    local_branch: Branch,
    command_rx: mpsc::Receiver<Command>,
}

impl Merger {
    pub fn new(shared: Arc<Shared>, local_branch: Branch) -> (Self, MergerHandle) {
        let (command_tx, command_rx) = mpsc::channel(1);

        (
            Self {
                shared,
                local_branch,
                command_rx,
            },
            MergerHandle { command_tx },
        )
    }

    pub async fn run(mut self) {
        // TODO: consider throttling / debouncing this stream
        let mut notify_rx = self.shared.store.index.subscribe();
        let mut wait = false;

        loop {
            // Wait for notification or command.
            if wait {
                select! {
                    event = notify_rx.recv() => {
                        if event.is_err() {
                            break;
                        }
                    }
                    command = self.command_rx.recv() => {
                        if let Some(command) = command {
                            self.handle_command(command).await;
                            continue;
                        } else {
                            break;
                        }
                    }
                }
            }

            // Run the merge process but interrupt and restart it when we receive notification or
            // command.
            select! {
                event = notify_rx.recv() => {
                    if event.is_ok() {
                        wait = false;
                        continue;
                    } else {
                        break;
                    }
                }
                command = self.command_rx.recv() => {
                    if let Some(command) = command {
                        self.handle_command(command).await
                    } else {
                        break;
                    }
                }
                _ = process_and_log(&self.shared, &self.local_branch) => (),
            }

            wait = true;
        }

        log::trace!("Merger terminated");
    }

    async fn handle_command(&self, command: Command) {
        match command {
            Command::Merge(result_tx) => {
                let result = process(&self.shared, &self.local_branch).await;
                result_tx.send(result).unwrap_or(());
            }
        }
    }
}

async fn process(shared: &Shared, local_branch: &Branch) -> Result<()> {
    let branches = shared.branches().await?;
    let mut roots = Vec::with_capacity(branches.len());

    for branch in branches {
        match branch.open_root().await {
            Ok(dir) => roots.push(dir),
            Err(Error::EntryNotFound | Error::BlockNotFound(_)) => continue,
            Err(error) => return Err(error),
        }
    }

    JointDirectory::new(Some(local_branch.clone()), roots)
        .merge()
        .await?;
    Ok(())
}

async fn process_and_log(shared: &Shared, local_branch: &Branch) {
    log::trace!("merge started");

    match process(shared, local_branch).await {
        Ok(()) => log::trace!("merge completed"),
        Err(error) => {
            // `BlockNotFound` means that a block is not downloaded yet. This error
            // is harmless because the merge will be attempted again on the next
            // change notification. We reduce the log severity for them to avoid
            // log spam.
            let level = if let Error::BlockNotFound(_) = error {
                Level::Trace
            } else {
                Level::Error
            };

            log::log!(level, "merge failed: {:?}", error)
        }
    }
}

pub(super) struct MergerHandle {
    command_tx: mpsc::Sender<Command>,
}

impl MergerHandle {
    pub async fn merge(&self) -> Result<()> {
        let (result_tx, result_rx) = oneshot::channel();

        self.command_tx
            .send(Command::Merge(result_tx))
            .await
            .unwrap_or(());

        // When this returns error it means the task has been terminated which can only happen when
        // the repository itself was dropped. We treat it as if the merge completed successfully.
        result_rx.await.unwrap_or(Ok(()))
    }
}

enum Command {
    Merge(oneshot::Sender<Result<()>>),
}
