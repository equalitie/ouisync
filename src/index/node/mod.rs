mod inner;
mod leaf;
mod link;
mod root;
#[cfg(test)]
mod tests;

pub use self::{
    inner::{InnerNode, InnerNodeMap, INNER_LAYER_COUNT},
    leaf::{LeafNode, LeafNodeSet, ModifyStatus},
    root::RootNode,
};

use crate::{
    crypto::{Hash, Hashable},
    db,
    error::Result,
};
use futures::{future, TryStreamExt};

/// Get the bucket for `locator` at the specified `inner_layer`.
pub fn get_bucket(locator: &Hash, inner_layer: usize) -> u8 {
    locator.as_ref()[inner_layer]
}

/// Detect snapshots that have been completely downloaded. Start the detection from the node(s)
/// with the specified hash at the specified layer and walk the tree(s) towards the root(s).
pub async fn detect_complete_snapshots(
    tx: &mut db::Transaction,
    hash: Hash,
    layer: usize,
) -> Result<()> {
    let mut stack = vec![(hash, layer)];

    while let Some((hash, layer)) = stack.pop() {
        if layer < INNER_LAYER_COUNT && !inner_children_complete(tx, &hash).await? {
            continue;
        }

        if layer == INNER_LAYER_COUNT && !leaf_children_complete(tx, &hash).await? {
            continue;
        }

        if layer > INNER_LAYER_COUNT {
            continue;
        }

        if layer == 0 {
            RootNode::set_complete(tx, &hash).await?;
        } else if layer <= INNER_LAYER_COUNT {
            InnerNode::set_complete(tx, &hash).await?;
        }

        if layer > 0 {
            InnerNode::load_parent_hashes(tx, &hash)
                .try_for_each(|parent_hash| {
                    stack.push((parent_hash, layer - 1));
                    future::ready(Ok(()))
                })
                .await?;
        }
    }

    Ok(())
}

async fn inner_children_complete(tx: &mut db::Transaction, parent_hash: &Hash) -> Result<bool> {
    // If the parent hash is equal to the hash of empty node collection it means the node has no
    // children and we can cut this short.
    if *parent_hash == InnerNodeMap::default().hash() {
        return Ok(true);
    }

    // We download all children nodes of a given parent together so when we know that we have
    // at least one we also know we have them all. Thus it's enough to check that all of them are
    // complete.
    let children = InnerNode::load_children(tx, parent_hash).await?;
    Ok(!children.is_empty() && children.into_iter().all(|(_, node)| node.is_complete))
}

async fn leaf_children_complete(tx: &mut db::Transaction, parent_hash: &Hash) -> Result<bool> {
    // If the parent hash is equal to the hash of empty node collection it means the node has no
    // children and we can cut this short.
    if *parent_hash == LeafNodeSet::default().hash() {
        return Ok(true);
    }

    // Similarly as in `are_inner_children_complete`, we only need to check that we have at least
    // one leaf node child and that already tells us that we have them all.
    LeafNode::has_children(tx, &parent_hash).await
}
