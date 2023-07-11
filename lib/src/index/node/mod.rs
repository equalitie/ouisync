#[cfg(test)]
pub mod test_utils;

mod summary;
#[cfg(test)]
mod tests;

pub(crate) use self::summary::{MultiBlockPresence, NodeState, SingleBlockPresence, Summary};

use super::{receive_filter::ReceiveFilter, try_collect_into};
use crate::{
    block::BlockId,
    collections::{HashMap, HashSet},
    crypto::{sign::PublicKey, Hash},
    db,
    error::Result,
    store::{InnerNode, LeafNode, RootNode},
};
use futures_util::TryStreamExt;

/// Reason for updating the summary
pub(crate) enum UpdateSummaryReason {
    /// Updating summary because a block was removed
    BlockRemoved,
    /// Updating summary for some other reason
    Other,
}

/// Update summary of the nodes with the specified hashes and all their ancestor nodes.
/// Returns the affected snapshots and their states.
pub(crate) async fn update_summaries(
    tx: &mut db::WriteTransaction,
    mut nodes: Vec<Hash>,
    reason: UpdateSummaryReason,
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

            match reason {
                // If block was removed we need to remove the corresponding receive filter entries
                // so if the block becomes needed again we can request it again.
                UpdateSummaryReason::BlockRemoved => ReceiveFilter::remove(tx, &hash).await?,
                UpdateSummaryReason::Other => (),
            }
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

    for (hash, state) in update_summaries(tx, nodes, UpdateSummaryReason::Other).await? {
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
