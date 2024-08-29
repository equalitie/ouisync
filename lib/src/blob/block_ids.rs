use super::{block_count, read_len};
use crate::{
    blob::BlobId,
    branch::Branch,
    error::{Error, Result},
    protocol::{BlockId, Locator, RootNode, RootNodeFilter, SingleBlockPresence},
    store::{self, ReadTransaction},
};

/// Stream-like object that yields the block ids of the given blob in their sequential order.
pub(crate) struct BlockIds {
    tx: ReadTransaction,
    branch: Branch,
    root_node: RootNode,
    locator: Locator,
    upper_bound: Option<u32>,
}

impl BlockIds {
    pub async fn open(branch: Branch, blob_id: BlobId) -> Result<Self> {
        let mut tx = branch.store().begin_read().await?;
        let root_node = tx
            .load_latest_approved_root_node(branch.id(), RootNodeFilter::Any)
            .await?;

        // If the first block of the blob is available, we read the blob length from it and use it
        // to know how far to iterate. If it's not, we iterate until we hit `LocatorNotFound`. It
        // might seem that iterating until `LocatorNotFound` should be always sufficient and there
        // should be no reason to read the length, however this is not always the case. Consider
        // the situation where a blob is truncated, or replaced with a shorter one with the same
        // blob id. Then without reading the current blob length, we would not know that we should
        // stop iterating before we hit `LocatorNotFound` and we would end up processing also the
        // blocks that are past the end of the blob. This means that e.g., the garbage collector
        // would consider those blocks still reachable and would never remove them.
        let upper_bound = match read_len(&mut tx, &root_node, blob_id, branch.keys().read()).await {
            Ok(len) => Some(block_count(len)),
            Err(Error::Store(store::Error::BlockNotFound)) => None,
            Err(error) => return Err(error),
        };

        Ok(Self {
            tx,
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

        match self.tx.find_block_at(&self.root_node, &encoded).await {
            Ok(block_info) => {
                self.locator = self.locator.next();
                Ok(Some(block_info))
            }
            Err(store::Error::LocatorNotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    #[cfg(test)]
    pub async fn try_collect<B>(&mut self) -> Result<B>
    where
        B: Default + Extend<BlockId>,
    {
        use std::iter;

        let mut collection = B::default();

        while let Some((block_id, _)) = self.try_next().await? {
            collection.extend(iter::once(block_id));
        }

        Ok(collection)
    }
}
