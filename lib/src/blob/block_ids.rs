use super::{block_count, read_len};
use crate::{
    blob_id::BlobId,
    branch::Branch,
    error::{Error, Result},
    locator::Locator,
    protocol::{BlockId, RootNode, SingleBlockPresence},
    store,
};

/// Stream-like object that yields the block ids of the given blob in their sequential order.
pub(crate) struct BlockIds {
    root_node: RootNode,
    branch: Branch,
    locator: Locator,
    upper_bound: Option<u32>,
}

impl BlockIds {
    pub async fn open(branch: Branch, blob_id: BlobId) -> Result<Self> {
        let mut tx = branch.store().begin_read().await?;
        let root_node = tx.load_root_node(branch.id()).await?;

        // If the first block of the blob is available, we read the blob length from it and use it
        // to know how far to iterate. If it's not, we iterate until we hit `EntryNotFound`.
        // It might seem that iterating until `EntryNotFound` should be always sufficient and there
        // should be no reason to read the length, however this is not always the case. Consider
        // the situation where a blob is truncated, or replaced with a shorter one with the same
        // blob id. Then without reading the current blob length, we would not know that we should
        // stop iterating before we hit `EntryNotFound` and we would end up processing also the
        // blocks that are past the end of the blob. This means that e.g., the garbage collector
        // would consider those blocks still reachable and would never remove them.
        let upper_bound = match read_len(&mut tx, &root_node, blob_id, branch.keys().read()).await {
            Ok(len) => Some(block_count(len)),
            Err(Error::Store(store::Error::BlockNotFound)) => None,
            Err(error) => return Err(error),
        };

        Ok(Self {
            root_node,
            branch,
            locator: Locator::head(blob_id),
            upper_bound,
        })
    }

    pub async fn try_next(&mut self) -> Result<Option<(BlockId, SingleBlockPresence)>> {
        if let Some(upper_bound) = self.upper_bound {
            if self.locator.number() >= upper_bound {
                return Ok(None);
            }
        }

        let encoded = self.locator.encode(self.branch.keys().read());
        let mut tx = self.branch.store().begin_read().await?;

        match tx.find_block_in(&self.root_node, &encoded).await {
            Ok(block) => {
                self.locator = self.locator.next();
                Ok(Some(block))
            }
            Err(error @ store::Error::LocatorNotFound) => {
                // There are two reasons why this error can be returned here:
                //
                //     1. we reached  the end of the blob, or
                //     2. the snapshot has been deleted in the meantime.
                //
                // Only in the first case can we return `Ok(None)`. In the second case we must
                // propagate the error otherwise we might end up incorrectly marking some blocks
                // as unreachable when in reality they might still be reachable just through a
                // different (newer) snapshot.
                if self.upper_bound.is_none() && tx.root_node_exists(&self.root_node).await? {
                    Ok(None)
                } else {
                    Err(error.into())
                }
            }
            Err(error) => Err(error.into()),
        }
    }
}
