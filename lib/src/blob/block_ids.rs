use super::{block_count, read_len};
use crate::{
    blob_id::BlobId,
    block::BlockId,
    branch::Branch,
    error::{Error, Result},
    index::SnapshotData,
    locator::Locator,
};

/// Stream-like object that yields the block ids of the given blob in their sequential order.
pub(crate) struct BlockIds {
    branch: Branch,
    snapshot: SnapshotData,
    locator: Locator,
    end: Option<u32>,
}

impl BlockIds {
    pub async fn open(branch: Branch, blob_id: BlobId) -> Result<Self> {
        let mut tx = branch.db().begin_read().await?;
        let snapshot = branch.data().load_snapshot(&mut tx).await?;

        // If the first block of the blob is available, we read the blob length from it and use it
        // to know how far to iterate. If it's not, we iterate until we hit `EntryNotFound`.
        // It might seem that iterating until `EntryNotFound` should be always sufficient and there
        // should be no reason to read the length, however this is not always the case. Consider
        // the situation where a blob is truncated, or replaced with a shorter one with the same
        // blob id. Then without reading the current blob length, we would not know that we should
        // stop iterating before we hit `EntryNotFound` and we would end up processing also the
        // blocks that are past the end of the blob. This means that e.g., the garbage collector
        // would consider those blocks still reachable and would never remove them.
        let end = match read_len(&mut tx, &snapshot, branch.keys().read(), blob_id).await {
            Ok(len) => Some(block_count(len)),
            Err(Error::BlockNotFound(_)) => None,
            Err(error) => return Err(error),
        };

        Ok(Self {
            branch,
            snapshot,
            locator: Locator::head(blob_id),
            end,
        })
    }

    pub async fn try_next(&mut self) -> Result<Option<BlockId>> {
        if let Some(end) = self.end {
            if self.locator.number() >= end {
                tracing::trace!(
                    vv = ?self.snapshot.version_vector(),
                    locator = ?self.locator,
                    "last block (upper bound)"
                );

                return Ok(None);
            }
        }

        let encoded = self.locator.encode(self.branch.keys().read());
        let mut tx = self.branch.db().begin_read().await?;

        match self.snapshot.get_block(&mut tx, &encoded).await {
            Ok((block_id, _)) => {
                self.locator = self.locator.next();
                Ok(Some(block_id))
            }
            Err(Error::EntryNotFound) => {
                tracing::trace!(
                    vv = ?self.snapshot.version_vector(),
                    locator = ?self.locator,
                    "last block (not found)"
                );

                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}
