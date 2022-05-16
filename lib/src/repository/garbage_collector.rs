use super::{utils, Shared};
use crate::{
    blob::BlockIds,
    blob_id::BlobId,
    block,
    error::{Error, Result},
    joint_directory::{JointDirectory, JointEntryRef, MissingVersionStrategy},
};
use async_recursion::async_recursion;
use std::sync::Arc;
use tokio::{
    select,
    sync::{mpsc, oneshot},
};

/// Removes unneeded blocks
pub(super) struct GarbageCollector {
    shared: Arc<Shared>,
    command_rx: mpsc::Receiver<Command>,
}

impl GarbageCollector {
    pub fn new(shared: Arc<Shared>) -> (Self, GarbageCollectorHandle) {
        let (command_tx, command_rx) = mpsc::channel(1);
        (
            Self { shared, command_rx },
            GarbageCollectorHandle { command_tx },
        )
    }

    pub async fn run(mut self) {
        let mut notify_rx = self.shared.store.index.subscribe();

        loop {
            select! {
                result = notify_rx.recv() => {
                    if result.is_ok() {
                        match self.process().await {
                            Ok(()) => (),
                            Err(error) => {
                                log::error!("garbage collector failed: {:?}", error);
                            }
                        }
                    } else {
                        break;
                    }
                }
                command = self.command_rx.recv() => {
                    match command {
                        Some(Command::Collect(result_tx)) => {
                            let result = self.process().await;
                            result_tx.send(result).unwrap_or(());
                        }
                        None => break,
                    }
                }
            }
        }

        log::debug!("garbage collector terminated");
    }

    async fn process(&self) -> Result<()> {
        self.remove_outdated_branches().await?;

        // HACK: Currently we use greedy block downloading which means it can happen that blocks
        // belonging to a file/directory are downloaded before blocks of its parent which would
        // make those blocks seem unreachable. To avoid collecting them prematurely, we proceed
        // with the collection only if there are no missing blocks. Once we switch to lazy block
        // downloading, we should remove this check.
        //
        // FIXME: this might be racy. It's possible (although not very likely) that a new snapshot
        // including some of its blocks is received after this call but before the
        // `remove_unrechable blocks` call. If that happens, those blocks might get prematurely
        // collected. Switching to lazy blocks should fix this problem as well.
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
            entries.push(BlockIds::new(branch.clone(), BlobId::ROOT));

            match branch.open_root().await {
                Ok(dir) => versions.push(dir),
                Err(Error::BlockNotFound(block_id)) => {
                    let mut conn = self.shared.store.db_pool().acquire().await?;
                    block::mark_reachable(&mut conn, &block_id).await?;
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
        let mut conn = self.shared.store.db_pool().acquire().await?;

        while let Some(block_id) = block_ids.next(&mut conn).await? {
            block::mark_reachable(&mut conn, &block_id).await?;
        }

        Ok(())
    }

    async fn remove_outdated_branches(&self) -> Result<()> {
        let outdated_branches = utils::outdated_branches(self.shared.branches().await?).await;

        for branch in outdated_branches {
            // Avoid deleting empty local branch before any content is added to it.
            if branch.data().root().await.proof.version_vector.is_empty() {
                continue;
            }

            self.shared.remove_branch(branch.id()).await?;
        }

        Ok(())
    }

    async fn prepare_reachable_blocks(&self) -> Result<()> {
        let mut conn = self.shared.store.db_pool().acquire().await?;
        block::clear_reachable(&mut conn).await
    }

    async fn remove_unreachable_blocks(&self) -> Result<()> {
        let mut conn = self.shared.store.db_pool().acquire().await?;
        let count = block::remove_unreachable(&mut conn).await?;

        if count > 0 {
            log::debug!("unreachable blocks removed: {}", count);
        }

        Ok(())
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

pub(super) struct GarbageCollectorHandle {
    command_tx: mpsc::Sender<Command>,
}

impl GarbageCollectorHandle {
    /// Trigger garbage collection and wait for it to complete.
    pub async fn collect_garbage(&self) -> Result<()> {
        let (result_tx, result_rx) = oneshot::channel();

        self.command_tx
            .send(Command::Collect(result_tx))
            .await
            .unwrap_or(());

        // When this returns error it means the task has been terminated which can only happen when
        // the repository itself was dropped. We treat it as if the gc completed successfully.
        result_rx.await.unwrap_or(Ok(()))
    }
}

enum Command {
    Collect(oneshot::Sender<Result<()>>),
}
