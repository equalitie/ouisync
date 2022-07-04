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
    sync::{mpsc, oneshot},
};

/// Utility that merges remote branches into the local one.
pub(super) struct Merger {
    shared: Arc<Shared>,
    local_branch: Branch,
    command_rx: mpsc::Receiver<Command>,
    event_scope: EventScope,
}

impl Merger {
    pub fn new(shared: Arc<Shared>, local_branch: Branch) -> (Self, MergerHandle) {
        let (command_tx, command_rx) = mpsc::channel(1);

        (
            Self {
                shared,
                local_branch,
                command_rx,
                event_scope: EventScope::new(),
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
                event = recv(&mut notify_rx, self.event_scope) => {
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
                _ = process_and_log(&self.shared, &self.local_branch, self.event_scope) => (),
            }

            wait = true;
        }

        log::trace!("Merger terminated");
    }

    async fn handle_command(&self, command: Command) {
        match command {
            Command::Merge(result_tx) => {
                let result = process(&self.shared, &self.local_branch, self.event_scope).await;
                result_tx.send(result).unwrap_or(());
            }
        }
    }
}

// Receive next event but ignore events triggered from inside the given scope.
async fn recv(
    rx: &mut async_broadcast::Receiver<Event>,
    ignore_scope: EventScope,
) -> Result<Event, async_broadcast::RecvError> {
    loop {
        let event = rx.recv().await?;
        if event.scope != ignore_scope {
            break Ok(event);
        }
    }
}

async fn process(shared: &Shared, local_branch: &Branch, event_scope: EventScope) -> Result<()> {
    let branches = shared.collect_branches().await?;
    let mut roots = Vec::with_capacity(branches.len());

    for branch in branches {
        let mut conn = shared.store.db().acquire().await?;
        match branch.open_root(&mut conn).await {
            Ok(dir) => roots.push(dir),
            Err(Error::EntryNotFound | Error::BlockNotFound(_)) => continue,
            Err(error) => return Err(error),
        }
    }

    // Run the merge within `event_scope` so we can distinguish the events triggered by the merge
    // itself from any other events and not interrupt the merge by the event it itself triggered.
    event_scope
        .apply(JointDirectory::new(Some(local_branch.clone()), roots).merge(shared.store.db()))
        .await?;

    Ok(())
}

async fn process_and_log(shared: &Shared, local_branch: &Branch, event_scope: EventScope) {
    log::trace!("merge started");

    match process(shared, local_branch, event_scope).await {
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
