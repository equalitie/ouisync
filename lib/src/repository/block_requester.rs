use async_recursion::async_recursion;

use super::{utils, Shared};
use crate::{
    blob::BlockIds,
    block::{BlockId, BlockTrackerRequester},
    error::{Error, Result},
    joint_directory::{JointDirectory, JointEntryRef, MissingVersionStrategy},
};
use std::sync::Arc;

/// Requests blocks from other replicas.
pub(super) struct BlockRequester {
    shared: Arc<Shared>,
    tracker: BlockTrackerRequester,
}

impl BlockRequester {
    pub fn new(shared: Arc<Shared>) -> Self {
        let tracker = shared.store.block_tracker.requester();
        Self { shared, tracker }
    }

    pub async fn run(self) {
        let mut notify_rx = self.shared.store.index.subscribe();

        while notify_rx.recv().await.is_ok() {
            match self.process().await {
                Ok(()) => (),
                Err(error) => {
                    log::error!("block requester failed: {:?}", error);
                }
            }
        }

        log::debug!("block requester terminated");
    }

    async fn process(&self) -> Result<()> {
        let branches = utils::current_branches(self.shared.branches().await?).await;
        let mut versions = Vec::with_capacity(branches.len());

        for branch in branches {
            match branch.open_root().await {
                Ok(dir) => versions.push(dir),
                Err(Error::BlockNotFound(block_id)) => {
                    self.request_missing_block(&block_id).await?;
                    continue;
                }
                Err(error) => return Err(error),
            }
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
            self.request_missing_blocks(entry).await?;
        }

        for dir in subdirs {
            self.traverse(dir).await?;
        }

        Ok(())
    }

    async fn request_missing_block(&self, id: &BlockId) -> Result<()> {
        let mut conn = self.shared.store.db_pool().acquire().await?;
        self.tracker.request(&mut conn, id).await?;
        Ok(())
    }

    async fn request_missing_blocks(&self, mut ids: BlockIds) -> Result<()> {
        let mut conn = self.shared.store.db_pool().acquire().await?;

        while let Some(id) = ids.next(&mut conn).await? {
            self.tracker.request(&mut conn, &id).await?;
        }

        Ok(())
    }
}
