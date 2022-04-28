use super::Shared;
use crate::{
    blob::BlockIds,
    blob_id::BlobId,
    block::{self, BlockId},
    error::{Error, Result},
    joint_directory::{versioned, JointDirectory, JointEntryRef, MissingVersionStrategy},
};
use async_recursion::async_recursion;
use futures_util::{stream, StreamExt};
use std::{collections::HashSet, sync::Arc};
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

        // HACK: Currently we use greedy block downloading which means it can happen that blocks
        // belonging to a file/directory are downloaded before blocks of its parent which would
        // make those blocks seem unreachable. To avoid collecting them prematurelly, we proceed
        // with the collection only if there are no missing blocks. Once we switch to lazy block
        // downloading, we should remove this check.
        if self.has_missing_blocks().await? {
            return Ok(());
        }

        self.prepare_reachable_blocks().await?;
        self.traverse_root().await?;
        self.remove_unreachable_blocks().await?;

        Ok(())
    }

    async fn traverse_root(&self) -> Result<()> {
        let branches = self.shared.branches().await?;
        let mut versions = Vec::with_capacity(branches.len());
        let mut entries = Vec::new();

        for branch in branches {
            // We already removed outdated branches at this point, so every remaining root
            // directory version is up to date.
            entries.push(BlockIds::new(branch.clone(), BlobId::ZERO));

            match branch.open_root().await {
                Ok(dir) => versions.push(dir),
                Err(Error::BlockNotFound(block_id)) => {
                    let mut conn = self.shared.index.pool.acquire().await?;
                    block::mark_reachable(&mut conn, &block_id).await?;
                    self.handle_missing_block(block_id);
                    continue;
                }
                Err(error) => return Err(error),
            }
        }

        for entry in entries {
            self.mark_reachable(entry).await?;
        }

        self.traverse(JointDirectory::new(None, versions)).await
    }

    #[async_recursion]
    async fn traverse(&self, dir: JointDirectory) -> Result<()> {
        let mut entries = Vec::new();
        let mut subdirs = Vec::new();

        // Collect the entries first, so we don't keep the directories locked while we are
        // processing the entries.
        for entry in dir.read().await.entries() {
            match entry {
                JointEntryRef::File(entry) => {
                    entries.push(BlockIds::new(
                        entry.inner().branch().clone(),
                        *entry.inner().blob_id(),
                    ));
                }
                JointEntryRef::Directory(entry) => {
                    for version in entry.versions() {
                        entries.push(BlockIds::new(version.branch().clone(), *version.blob_id()));
                    }

                    subdirs.push(entry.open(MissingVersionStrategy::Skip).await?);
                }
            }
        }

        for entry in entries {
            self.mark_reachable(entry).await?;
        }

        for dir in subdirs {
            self.traverse(dir).await?;
        }

        Ok(())
    }

    async fn mark_reachable(&self, mut block_ids: BlockIds) -> Result<()> {
        let mut conn = self.shared.index.pool.acquire().await?;

        while let Some(block_id) = block_ids.next(&mut conn).await? {
            block::mark_reachable(&mut conn, &block_id).await?;

            if !block::exists(&mut conn, &block_id).await? {
                self.handle_missing_block(block_id);
            }
        }

        Ok(())
    }

    async fn remove_outdated_branches(&self) -> Result<()> {
        let branches = self.shared.branches().await?;
        let branch_infos: Vec<_> = stream::iter(&branches)
            .then(|branch| async move {
                let id = *branch.id();
                let vv = branch.data().root().await.proof.version_vector.clone();

                (id, vv)
            })
            .collect()
            .await;

        let keep: HashSet<_> = versioned::keep_maximal(branch_infos, None)
            .into_iter()
            .map(|(id, _)| id)
            .collect();

        for branch in branches {
            if keep.contains(branch.id()) {
                continue;
            }

            // Avoid deleting empty local branch before any content is added to it.
            if branch.data().root().await.proof.version_vector.is_empty() {
                continue;
            }

            self.shared.remove_branch(branch.id()).await?;
        }

        Ok(())
    }

    async fn prepare_reachable_blocks(&self) -> Result<()> {
        let mut conn = self.shared.index.pool.acquire().await?;
        block::clear_reachable(&mut conn).await
    }

    async fn remove_unreachable_blocks(&self) -> Result<()> {
        let mut conn = self.shared.index.pool.acquire().await?;
        let count = block::remove_unreachable(&mut conn).await?;

        if count > 0 {
            log::debug!("unreachable blocks removed: {}", count);
        }

        Ok(())
    }

    fn handle_missing_block(&self, _block_id: BlockId) {
        // TODO
    }

    async fn has_missing_blocks(&self) -> Result<bool> {
        for branch in self.shared.branches().await? {
            if branch.data().root().await.summary.missing_blocks_count() > 0 {
                return Ok(true);
            }
        }

        Ok(false)
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
