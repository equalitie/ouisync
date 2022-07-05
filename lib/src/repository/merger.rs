use super::Shared;
use crate::{
    branch::Branch,
    error::{Error, Result},
    event::{Event, EventScope},
    joint_directory::JointDirectory,
};
use log::Level;
use std::sync::Arc;
use tokio::{
    select,
    sync::{
        broadcast::{self, error::RecvError},
        mpsc, oneshot,
    },
};

/// Utility that merges remote branches into the local one.
pub(super) struct Merger {
    inner: Inner,
    command_rx: mpsc::Receiver<Command>,
}

impl Merger {
    pub fn new(shared: Arc<Shared>, local_branch: Branch) -> (Self, MergerHandle) {
        let (command_tx, command_rx) = mpsc::channel(1);

        (
            Self {
                inner: Inner {
                    shared,
                    local_branch,
                    event_scope: EventScope::new(),
                },
                command_rx,
            },
            MergerHandle { command_tx },
        )
    }

    pub async fn run(mut self) {
        // TODO: consider throttling / debouncing this stream
        let mut notify_rx = self.inner.shared.store.index.subscribe();
        let mut wait = false;

        loop {
            // Wait for notification or command.
            if wait {
                select! {
                    event = notify_rx.recv() => {
                        match event {
                            Ok(_) | Err(RecvError::Lagged(_)) => (),
                            Err(RecvError::Closed) => break,
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
                event = recv(&mut notify_rx, self.inner.event_scope) => {
                    match event {
                        Ok(_) | Err(RecvError::Lagged(_)) => {
                            wait = false;
                            continue;
                        }
                        Err(RecvError::Closed) => break,
                    }
                }
                command = self.command_rx.recv() => {
                    if let Some(command) = command {
                        self.handle_command(command).await
                    } else {
                        break;
                    }
                }
                _ = self.inner.process_and_log() => (),
            }

            wait = true;
        }

        log::trace!("Merger terminated");
    }

    async fn handle_command(&self, command: Command) {
        match command {
            Command::Merge(result_tx) => {
                let result = self.inner.process().await;
                result_tx.send(result).unwrap_or(());
            }
        }
    }
}

struct Inner {
    shared: Arc<Shared>,
    local_branch: Branch,
    event_scope: EventScope,
}

impl Inner {
    async fn process(&self) -> Result<()> {
        let branches = self.shared.collect_branches()?;
        let mut roots = Vec::with_capacity(branches.len());

        for branch in branches {
            let mut conn = self.shared.store.db().acquire().await?;
            match branch.open_root(&mut conn).await {
                Ok(dir) => roots.push(dir),
                Err(Error::EntryNotFound | Error::BlockNotFound(_)) => continue,
                Err(error) => return Err(error),
            }
        }

        // Run the merge within `event_scope` so we can distinguish the events triggered by the merge
        // itself from any other events and not interrupt the merge by the event it itself triggered.
        let mut joint = JointDirectory::new(Some(self.local_branch.clone()), roots);
        self.event_scope
            .apply(joint.merge(self.shared.store.db()))
            .await?;

        Ok(())
    }

    async fn process_and_log(&self) {
        log::trace!("merge started");

        match self.process().await {
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

// Receive next event but ignore events triggered from inside the given scope.
async fn recv(
    rx: &mut broadcast::Receiver<Event>,
    ignore_scope: EventScope,
) -> Result<Event, RecvError> {
    loop {
        let event = rx.recv().await?;
        if event.scope != ignore_scope {
            break Ok(event);
        }
    }
}
