#[cfg(test)]
pub mod test_utils;

mod inner;
mod leaf;
mod link;
mod missing_blocks;
mod root;
#[cfg(test)]
mod tests;

pub use self::{
    inner::{InnerNode, InnerNodeMap, INNER_LAYER_COUNT},
    leaf::{LeafNode, LeafNodeSet, ModifyStatus},
    missing_blocks::MissingBlocksSummary,
    root::RootNode,
};

use crate::{block::BlockId, crypto::Hash, db, error::Result};
use futures_util::{future, TryStreamExt};

/// Get the bucket for `locator` at the specified `inner_layer`.
pub fn get_bucket(locator: &Hash, inner_layer: usize) -> u8 {
    locator.as_ref()[inner_layer]
}

/// Detect snapshots that have been completely downloaded. Start the detection from the node(s)
/// with the specified hash at the specified layer and walk the tree(s) towards the root(s).
pub async fn detect_complete_snapshots(pool: &db::Pool, hash: Hash, layer: usize) -> Result<()> {
    let stack = vec![(hash, layer)];
    let mut tx = pool.begin().await?;
    update_statuses(&mut tx, stack).await?;
    tx.commit().await?;

    Ok(())
}

/// Receive a block from other replica. This marks the block as not missing by the local replica.
// TODO: return replica_ids of affected branches
pub async fn receive_block(tx: &mut db::Transaction, id: &BlockId) -> Result<()> {
    LeafNode::set_present(tx, id).await?;

    let stack = LeafNode::load_parent_hashes(tx, id)
        .map_ok(|hash| (hash, INNER_LAYER_COUNT))
        .try_collect()
        .await?;

    update_statuses(tx, stack).await
}

async fn update_statuses(tx: &mut db::Transaction, mut stack: Vec<(Hash, usize)>) -> Result<()> {
    while let Some((hash, layer)) = stack.pop() {
        if layer > 0 {
            InnerNode::update_statuses(tx, &hash, layer - 1).await?;
            InnerNode::load_parent_hashes(&mut *tx, &hash)
                .try_for_each(|parent_hash| {
                    stack.push((parent_hash, layer - 1));
                    future::ready(Ok(()))
                })
                .await?;
        } else {
            RootNode::update_statuses(tx, &hash).await?
        }
    }

    Ok(())
}
