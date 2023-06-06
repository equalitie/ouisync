#[cfg(test)]
pub mod test_utils;

mod inner;
mod leaf;
mod root;
mod summary;
#[cfg(test)]
mod tests;

pub(crate) use self::{
    inner::{InnerNode, InnerNodeMap, EMPTY_INNER_HASH, INNER_LAYER_COUNT},
    leaf::{LeafNode, LeafNodeSet, ModifyStatus},
    root::RootNode,
    summary::{MultiBlockPresence, NodeState, SingleBlockPresence, Summary},
};

use super::try_collect_into;
use crate::{
    block::BlockId,
    collections::{HashMap, HashSet},
    crypto::{sign::PublicKey, Hash},
    db,
    error::Result,
};
use futures_util::TryStreamExt;

/// Get the bucket for `locator` at the specified `inner_layer`.
pub(super) fn get_bucket(locator: &Hash, inner_layer: usize) -> u8 {
    locator.as_ref()[inner_layer]
}

/// Update summary of the nodes with the specified hashes and all their ancestor nodes.
/// Returns the affected snapshots and their states.
pub(crate) async fn update_summaries(
    tx: &mut db::WriteTransaction,
    mut nodes: Vec<Hash>,
) -> Result<HashMap<Hash, NodeState>> {
    let mut states = HashMap::default();

    while let Some(hash) = nodes.pop() {
        let mut has_parent = false;

        // NOTE: There are no orphaned nodes so when `load_parent_hashes` returns nothing it can
        // only mean that the node is a root.

        try_collect_into(
            InnerNode::load_parent_hashes(tx, &hash).map_ok(|hash| {
                has_parent = true;
                hash
            }),
            &mut nodes,
        )
        .await?;

        if has_parent {
            InnerNode::update_summaries(tx, &hash).await?;
        } else {
            let state = RootNode::update_summaries(tx, &hash).await?;
            states.entry(hash).or_insert(state).update(state);
        }
    }

    Ok(states)
}

/// Receive a block from other replica. This marks the block as not missing by the local replica.
/// Returns the replica ids whose branches reference the received block (if any).
pub(crate) async fn receive_block(
    tx: &mut db::WriteTransaction,
    id: &BlockId,
) -> Result<HashSet<PublicKey>> {
    if !LeafNode::set_present(tx, id).await? {
        return Ok(HashSet::default());
    }

    let nodes = LeafNode::load_parent_hashes(tx, id).try_collect().await?;
    let mut branch_ids = HashSet::default();

    for (hash, state) in update_summaries(tx, nodes).await? {
        if !state.is_approved() {
            continue;
        }

        try_collect_into(RootNode::load_writer_ids(tx, &hash), &mut branch_ids).await?;
    }

    Ok(branch_ids)
}

/// Does a parent node (root or inner) with the given hash exist?
pub(crate) async fn parent_exists(conn: &mut db::Connection, hash: &Hash) -> Result<bool> {
    use sqlx::Row;

    Ok(sqlx::query(
        "SELECT
             EXISTS(SELECT 0 FROM snapshot_root_nodes  WHERE hash = ?) OR
             EXISTS(SELECT 0 FROM snapshot_inner_nodes WHERE hash = ?)",
    )
    .bind(hash)
    .bind(hash)
    .fetch_one(conn)
    .await?
    .get(0))
}

/// Check whether the `old` snapshot can serve as a fallback for the `new` snapshot.
/// A snapshot can serve as a fallback if there is at least one locator that points to a missing
/// block in `new` but present block in `old`.
pub(crate) async fn check_fallback(
    conn: &mut db::Connection,
    old: &RootNode,
    new: &RootNode,
) -> Result<bool> {
    // TODO: verify this query is efficient, especially on large repositories

    Ok(sqlx::query(
        "WITH RECURSIVE
             inner_nodes_old(hash) AS (
                 SELECT i.hash
                     FROM snapshot_inner_nodes AS i
                     INNER JOIN snapshot_root_nodes AS r ON r.hash = i.parent
                     WHERE r.snapshot_id = ?
                 UNION ALL
                 SELECT c.hash
                     FROM snapshot_inner_nodes AS c
                     INNER JOIN inner_nodes_old AS p ON p.hash = c.parent
             ),
             inner_nodes_new(hash) AS (
                 SELECT i.hash
                     FROM snapshot_inner_nodes AS i
                     INNER JOIN snapshot_root_nodes AS r ON r.hash = i.parent
                     WHERE r.snapshot_id = ?
                 UNION ALL
                 SELECT c.hash
                     FROM snapshot_inner_nodes AS c
                     INNER JOIN inner_nodes_new AS p ON p.hash = c.parent
             )
         SELECT locator
             FROM snapshot_leaf_nodes
             WHERE block_presence = ? AND parent IN inner_nodes_old
         INTERSECT
         SELECT locator
             FROM snapshot_leaf_nodes
             WHERE block_presence = ? AND parent IN inner_nodes_new
         LIMIT 1",
    )
    .bind(old.snapshot_id)
    .bind(new.snapshot_id)
    .bind(SingleBlockPresence::Present)
    .bind(SingleBlockPresence::Missing)
    .fetch_optional(conn)
    .await?
    .is_some())
}
