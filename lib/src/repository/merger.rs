use super::Shared;
use crate::{
    branch::Branch,
    crypto::sign::PublicKey,
    error::{Error, Result},
    joint_directory::JointDirectory,
    scoped_task::{self, ScopedJoinHandle},
};
use futures_util::{stream::FuturesUnordered, StreamExt};
use log::Level;
use std::{collections::HashMap, iter, sync::Arc};
use tokio::select;

// The merge algorithm.
pub(super) struct Merger {
    shared: Arc<Shared>,
    local_branch: Branch,
    tasks: FuturesUnordered<ScopedJoinHandle<PublicKey>>,
    states: HashMap<PublicKey, MergeState>,
}

impl Merger {
    pub fn new(shared: Arc<Shared>, local_branch: Branch) -> Self {
        Self {
            shared,
            local_branch,
            tasks: FuturesUnordered::new(),
            states: HashMap::new(),
        }
    }

    pub async fn run(mut self) {
        let mut rx = self.shared.index.subscribe();

        loop {
            select! {
                branch_id = rx.recv() => {
                    if let Ok(branch_id) = branch_id {
                        self.handle_branch_changed(branch_id).await
                    } else {
                        break;
                    }
                }
                branch_id = self.tasks.next(), if !self.tasks.is_empty() => {
                    if let Some(Ok(branch_id)) = branch_id {
                        self.handle_task_finished(branch_id).await
                    }
                }
            }
        }
    }

    async fn handle_branch_changed(&mut self, branch_id: PublicKey) {
        if branch_id == *self.local_branch.id() {
            // local branch change - ignore.
            return;
        }

        if let Some(state) = self.states.get_mut(&branch_id) {
            // Merge of this branch is already ongoing - schedule to run it again after it's
            // finished.
            *state = MergeState::Pending;
            return;
        }

        self.spawn_task(branch_id).await
    }

    async fn handle_task_finished(&mut self, branch_id: PublicKey) {
        if let Some(MergeState::Pending) = self.states.remove(&branch_id) {
            self.spawn_task(branch_id).await
        }
    }

    async fn spawn_task(&mut self, remote_id: PublicKey) {
        let local = self.local_branch.clone();

        let remote = if let Ok(remote) = self.shared.branch(&remote_id).await {
            remote
        } else {
            // branch removed in the meantime - ignore.
            return;
        };

        let handle = scoped_task::spawn(async move {
            if local.data().root().await.versions > remote.data().root().await.versions {
                log::debug!(
                    "merge with branch {:?} suppressed - local branch already up to date",
                    remote_id
                );
                return remote_id;
            }

            log::debug!("merge with branch {:?} started", remote_id);

            match merge_branches(local, remote).await {
                Ok(()) => {
                    log::info!("merge with branch {:?} complete", remote_id)
                }
                Err(error) => {
                    // `EntryNotFound` most likely means the remote snapshot is not fully
                    // downloaded yet and `BlockNotFound` means that a block is not downloaded yet.
                    // Both error are harmless because the merge will be attempted again on the
                    // next change notification. We reduce the log severity for them to avoid log
                    // spam.
                    let level = if matches!(error, Error::EntryNotFound | Error::BlockNotFound(_)) {
                        Level::Trace
                    } else {
                        Level::Error
                    };

                    log::log!(
                        level,
                        "merge with branch {:?} failed: {}",
                        remote_id,
                        error.verbose()
                    )
                }
            }

            remote_id
        });

        self.tasks.push(handle);
        self.states.insert(remote_id, MergeState::Ongoing);
    }
}

enum MergeState {
    // A merge is ongoing
    Ongoing,
    // A merge is ongoing and another one was already scheduled
    Pending,
}

async fn merge_branches(local: Branch, remote: Branch) -> Result<()> {
    let remote_root = remote.open_root().await?;
    JointDirectory::new(Some(local.clone()), iter::once(remote_root))
        .merge()
        .await?;

    Ok(())
}
