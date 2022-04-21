use super::Shared;
use crate::{
    blob::BlockIds,
    blob_id::BlobId,
    block,
    directory::EntryRef,
    error::{Error, Result},
    joint_directory::versioned,
    JointDirectory,
};
use async_recursion::async_recursion;
use futures_util::{stream, StreamExt};
use std::sync::Arc;
use tokio::{
    select,
    sync::{mpsc, oneshot},
};

/// Takes care of requesting missing blocks and removing unneeded blocks.
pub(super) struct BlockManager {
    shared: Arc<Shared>,
    command_rx: mpsc::Receiver<Command>,
}

impl BlockManager {
    pub fn new(shared: Arc<Shared>) -> (Self, BlockManagerHandle) {
        let (command_tx, command_rx) = mpsc::channel(1);
        (
            Self { shared, command_rx },
            BlockManagerHandle { command_tx },
        )
    }

    pub async fn run(mut self) {
        let mut notify_rx = self.shared.index.subscribe();

        loop {
            select! {
                result = notify_rx.recv() => {
                    if result.is_ok() {
                        match self.process().await {
                            Ok(()) => (),
                            Err(error) => {
                                log::error!("block manager failed: {:?}", error);
                            }
                        }
                    } else {
                        break;
                    }
                }
                command = self.command_rx.recv() => {
                    match command {
                        Some(Command::CollectGarbage(result_tx)) => {
                            let result = self.process().await;
                            result_tx.send(result).unwrap_or(());
                        }
                        None => break,
                    }
                }
            }
        }

        log::debug!("block manager terminated");
    }

    async fn process(&self) -> Result<()> {
        self.remove_outdated_branches().await?;

        let root = self.joint_root().await?;
        self.traverse(root).await
    }

    async fn joint_root(&self) -> Result<JointDirectory> {
        let branches = self.shared.branches().await?;
        let mut versions = Vec::with_capacity(branches.len());

        for branch in branches {
            match branch.open_root().await {
                Ok(dir) => versions.push(dir),
                Err(Error::BlockNotFound(_)) => {
                    // TODO: request the missing block
                    continue;
                }
                Err(error) => return Err(error),
            }
        }

        Ok(JointDirectory::new(None, versions))
    }

    #[async_recursion]
    async fn traverse(&self, dir: JointDirectory) -> Result<()> {
        for (_, entry_versions) in dir.read().await.entries_all_versions() {
            // Process children first.
            self.recurse(&entry_versions).await?;

            let (current, outdated): (_, Vec<_>) = versioned::partition(entry_versions, None);

            for entry in current {
                self.request_missing_blocks(entry).await?;
            }

            for entry in outdated {
                self.remove_unneeded_blocks(entry).await?;
            }
        }

        Ok(())
    }

    async fn recurse(&self, versions: &[EntryRef<'_>]) -> Result<()> {
        let dir_entries = versions.iter().filter_map(|entry| entry.directory().ok());
        let dirs: Vec<_> = stream::iter(dir_entries)
            .filter_map(|entry| async move { entry.open().await.ok() })
            .collect()
            .await;

        self.traverse(JointDirectory::new(None, dirs)).await
    }

    async fn request_missing_blocks(&self, _entry: EntryRef<'_>) -> Result<()> {
        // TODO
        Ok(())
    }

    async fn remove_unneeded_blocks(&self, entry: EntryRef<'_>) -> Result<()> {
        let blob_id = if let Some(blob_id) = blob_id(&entry) {
            blob_id
        } else {
            return Ok(());
        };

        let mut conn = self.shared.index.pool.acquire().await?;
        let mut block_ids = BlockIds::new(entry.branch(), *blob_id);

        while let Some(block_id) = block_ids.next(&mut conn).await? {
            block::remove(&mut conn, &block_id).await?;

            // TODO: consider releasing and re-acquiring the connection after some number of iterations,
            // to not hold it for too long.
        }

        Ok(())
    }

    async fn remove_outdated_branches(&self) -> Result<()> {
        let branches = self.shared.branches().await?;
        let branches: Vec<_> = stream::iter(branches)
            .then(|branch| async move {
                let id = *branch.id();
                let vv = branch.data().root().await.proof.version_vector.clone();

                (id, vv)
            })
            .collect()
            .await;

        let (_, outdated): (_, Vec<_>) = versioned::partition(branches, None);

        for (id, _) in outdated {
            self.shared.remove_branch(&id).await?;
        }

        Ok(())
    }
}

pub(super) struct BlockManagerHandle {
    command_tx: mpsc::Sender<Command>,
}

impl BlockManagerHandle {
    /// Trigger garbage collection and wait for it to complete.
    pub async fn collect_garbage(&self) -> Result<()> {
        let (result_tx, result_rx) = oneshot::channel();

        self.command_tx
            .send(Command::CollectGarbage(result_tx))
            .await
            .unwrap_or(());

        // When this returns error it means the task has been terminated which can only happen when
        // the repository itself was dropped. We treat it as if the gc completed successfully.
        result_rx.await.unwrap_or(Ok(()))
    }
}

enum Command {
    CollectGarbage(oneshot::Sender<Result<()>>),
}

fn blob_id<'a>(entry: &EntryRef<'a>) -> Option<&'a BlobId> {
    match entry {
        EntryRef::File(entry) => Some(entry.blob_id()),
        EntryRef::Directory(entry) => Some(entry.blob_id()),
        EntryRef::Tombstone(_) => None,
    }
}
