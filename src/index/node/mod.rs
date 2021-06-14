mod inner;
mod leaf;
mod link;
mod root;
#[cfg(test)]
mod tests;

pub use self::{
    inner::{InnerNode, InnerNodeMap},
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
/// with the specified hash and walk the tree(s) towards the root(s).
pub async fn detect_complete_snapshots(tx: &mut db::Transaction, hash: Hash) -> Result<()> {
    let mut stack = vec![hash];

    while let Some(hash) = stack.pop() {
        if !are_children_complete(tx, &hash).await? {
            continue;
        }

        InnerNode::set_complete(tx, &hash).await?;
        RootNode::set_complete(tx, &hash).await?;

        InnerNode::load_parent_hashes(tx, &hash)
            .try_for_each(|parent_hash| {
                stack.push(parent_hash);
                future::ready(Ok(()))
            })
            .await?;
    }

    Ok(())
}

async fn are_children_complete(tx: &mut db::Transaction, hash: &Hash) -> Result<bool> {
    if *hash == InnerNodeMap::default().hash() || *hash == LeafNodeSet::default().hash() {
        // The hash of this node is equal to the hash of empty node collection which means it has
        // no children, thus the children are "complete".
        return Ok(true);
    }

    // We download all the children of a single node together so when we know that we have at least
    // one we also know we have them all. Thus it's enough to check that all of them are complete.
    let inner_children = InnerNode::load_children(tx, hash).await?;
    if !inner_children.is_empty() && inner_children.into_iter().all(|(_, node)| node.is_complete) {
        return Ok(true);
    }

    // For the same reason, if `hash` is of a node that is at the last inner layer, we only need to
    // check that we have at least one leaf node child and that already tells us that we have them
    // all.
    if LeafNode::has_children(tx, &hash).await? {
        return Ok(true);
    }

    Ok(false)
}
