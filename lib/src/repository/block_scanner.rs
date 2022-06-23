use super::{utils, Shared};
use crate::{
    blob::BlockIds,
    blob_id::BlobId,
    block::{self, BlockId},
    db,
    error::{Error, Result},
    joint_directory::{JointDirectory, JointEntryRef, MissingVersionStrategy},
};
use async_recursion::async_recursion;
use std::sync::Arc;
use tokio::{
    select,
    sync::{mpsc, oneshot},
};

/// Utility that traverses the repository scanning for missing and unreachable blocks. It then
/// requests the missing blocks (from `BlockTracker`) and deletes the unreachable blocks.
pub(super) struct BlockScanner {
    shared: Arc<Shared>,
    command_rx: mpsc::Receiver<Command>,
}

impl BlockScanner {
    pub fn new(shared: Arc<Shared>) -> (Self, BlockScannerHandle) {
        let (command_tx, command_rx) = mpsc::channel(1);
        (
            Self { shared, command_rx },
            BlockScannerHandle { command_tx },
        )
    }

    pub async fn run(mut self) {
        let mut notify_rx = self.shared.store.index.subscribe();

        loop {
            select! {
                result = notify_rx.recv() => {
                    if result.is_ok() {
                        match self.process(Mode::RequestAndCollect).await {
                            Ok(()) => (),
                            Err(error) => {
                                log::error!("BlockScanner failed: {:?}", error);
                            }
                        }
                    } else {
                        break;
                    }
                }
                command = self.command_rx.recv() => {
                    match command {
                        Some(Command::Collect(result_tx)) => {
                            let result = self.process(Mode::Collect).await;
                            result_tx.send(result).unwrap_or(());
                        }
                        None => break,
                    }
                }
            }
        }

        log::trace!("BlockScanner terminated");
    }

    async fn process(&self, mode: Mode) -> Result<()> {
        // FIXME: this should run in all access modes but currently it does only in read and write:
        self.remove_outdated_branches().await?;

        self.prepare_reachable_blocks().await?;
        self.traverse_root(mode).await?;
        self.remove_unreachable_blocks().await?;

        Ok(())
    }

    async fn traverse_root(&self, mode: Mode) -> Result<()> {
        let branches = self.shared.collect_branches().await?;
        let mut versions = Vec::with_capacity(branches.len());
        let mut entries = Vec::new();
        let mut result = Ok(());

        for branch in branches {
            // We already removed outdated branches at this point, so every remaining root
            // directory version is up to date.
            entries.push(BlockIds::new(branch.clone(), BlobId::ROOT));

            if result.is_ok() {
                let mut conn = self.shared.store.db().acquire().await?;

                // Open the directory in read-only mode to bypass the cache (see `directory::Mode` for
                // more details) to make sure we obtain the most up-to-date version of the directory so
                // that we can find all the missing blocks.
                match branch.open_root_read_only(&mut conn).await {
                    Ok(dir) => versions.push(dir),
                    Err(Error::EntryNotFound) => {
                        // `EntryNotFound` here just means this is a newly created branch with no
                        // content yet. It is safe to ignore it.
                        continue;
                    }
                    Err(error) => {
                        // Remember the error but keep processing the remaining branches so that we
                        // find all the missing blocks.
                        result = Err(error);
                    }
                }
            }
        }

        for entry in entries {
            self.process_blocks(mode, entry).await?;
        }

        // If there was en error opening any version of the root directory we can't proceed because
        // we might not have access to all the entries and we could fail to identify some missing
        // blocks and/or incorrectly mark some as unreachable.
        result?;

        self.traverse(mode, JointDirectory::new(None, versions))
            .await
    }

    #[async_recursion]
    async fn traverse(&self, mode: Mode, dir: JointDirectory) -> Result<()> {
        let mut entries = Vec::new();
        let mut subdirs = Vec::new();
        let mut result = Ok(());

        let mut conn = self.shared.store.db().acquire().await?;

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

                    if result.is_ok() {
                        match entry.open(&mut conn, MissingVersionStrategy::Fail).await {
                            Ok(dir) => subdirs.push(dir),
                            Err(error) => {
                                // Remember the error but keep processing the remaining entries so
                                // that we find all the missing blocks.
                                result = Err(error);
                            }
                        }
                    }
                }
            }
        }

        drop(conn);

        for entry in entries {
            self.process_blocks(mode, entry).await?;
        }

        // If there was en error opening any of the subdirectories we can't proceed further because
        // we might not have access to all the entries.
        result?;

        for dir in subdirs {
            self.traverse(mode, dir).await?;
        }

        Ok(())
    }

    async fn remove_outdated_branches(&self) -> Result<()> {
        let mut conn = self.shared.store.db().acquire().await?;
        let local_id = self.shared.local_branch().await.map(|branch| *branch.id());
        let outdated_branches = utils::outdated_branches(
            &mut conn,
            self.shared.collect_branches().await?,
            local_id.as_ref(),
        )
        .await?;
        drop(conn);

        for branch in outdated_branches {
            self.shared.remove_branch(branch.id()).await?;
        }

        Ok(())
    }

    async fn prepare_reachable_blocks(&self) -> Result<()> {
        let mut conn = self.shared.store.db().acquire().await?;
        block::clear_reachable(&mut conn).await
    }

    async fn remove_unreachable_blocks(&self) -> Result<()> {
        let mut conn = self.shared.store.db().acquire().await?;
        let count = block::remove_unreachable(&mut conn).await?;

        if count > 0 {
            log::debug!("unreachable blocks removed: {}", count);
        }

        Ok(())
    }

    async fn process_blocks(&self, mode: Mode, mut block_ids: BlockIds) -> Result<()> {
        let mut conn = self.shared.store.db().acquire().await?;

        while let Some(block_id) = block_ids.next(&mut conn).await? {
            block::mark_reachable(&mut conn, &block_id).await?;

            if mode.should_request() {
                self.require_missing_block(&mut conn, block_id).await?;
            }
        }

        Ok(())
    }

    async fn require_missing_block(
        &self,
        conn: &mut db::Connection,
        block_id: BlockId,
    ) -> Result<()> {
        // TODO: check whether the block is already required to avoid the potentially expensive db
        // lookup.
        if !block::exists(conn, &block_id).await? {
            self.shared.store.block_tracker.require(block_id);
        }

        Ok(())
    }
}

pub(super) struct BlockScannerHandle {
    command_tx: mpsc::Sender<Command>,
}

impl BlockScannerHandle {
    /// Trigger garbage collection and wait for it to complete.
    pub async fn collect(&self) -> Result<()> {
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

#[derive(Copy, Clone)]
enum Mode {
    // request missing blocks and collect unreachable blocks
    RequestAndCollect,
    // only collect unreachable blocks
    Collect,
}

impl Mode {
    fn should_request(&self) -> bool {
        matches!(self, Self::RequestAndCollect)
    }
}
