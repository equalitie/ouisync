#[cfg(test)]
pub mod test_utils;

mod inner;
mod leaf;
mod root;
mod summary;
#[cfg(test)]
mod tests;

pub(crate) use self::{
    inner::{InnerNode, InnerNodeMap, INNER_LAYER_COUNT},
    leaf::{LeafNode, LeafNodeSet, ModifyStatus},
    root::RootNode,
    summary::{Summary, SummaryUpdateStatus},
};

use crate::{
    block::BlockId,
    crypto::{sign::PublicKey, Hash},
    db,
    error::Result,
};
use futures_util::{future, TryStreamExt};
use sqlx::Acquire;

/// Get the bucket for `locator` at the specified `inner_layer`.
pub(super) fn get_bucket(locator: &Hash, inner_layer: usize) -> u8 {
    locator.as_ref()[inner_layer]
}

/// Update summary of the nodes with the specified hash and layer and all their ancestor nodes.
/// Returns a map `PublicKey -> SummaryUpdateStatus` indicating which branches were affected
/// and whether they became complete by this update.
pub(super) async fn update_summaries(
    conn: &mut db::Connection,
    hash: Hash,
    layer: usize,
) -> Result<Vec<(PublicKey, SummaryUpdateStatus)>> {
    let mut tx = conn.begin().await?;
    let statuses = update_summaries_in_transaction(&mut tx, vec![(hash, layer)]).await?;
    tx.commit().await?;

    Ok(statuses)
}

/// Receive a block from other replica. This marks the block as not missing by the local replica.
/// Returns the replica ids whose branches reference the received block (if any).
pub(crate) async fn receive_block(
    conn: &mut db::Connection,
    id: &BlockId,
) -> Result<Vec<PublicKey>> {
    let mut tx = conn.begin().await?;

    if !LeafNode::set_present(&mut tx, id).await? {
        return Ok(Vec::new());
    }

    let nodes = LeafNode::load_parent_hashes(&mut tx, id)
        .map_ok(|hash| (hash, INNER_LAYER_COUNT))
        .try_collect()
        .await?;

    let ids = update_summaries_in_transaction(&mut tx, nodes)
        .await?
        .into_iter()
        .map(|(writer_id, _)| writer_id)
        .collect();

    tx.commit().await?;

    Ok(ids)
}

async fn update_summaries_in_transaction(
    tx: &mut db::Transaction<'_>,
    mut nodes: Vec<(Hash, usize)>,
) -> Result<Vec<(PublicKey, SummaryUpdateStatus)>> {
    let mut statuses = Vec::new();

    while let Some((hash, layer)) = nodes.pop() {
        if layer > 0 {
            InnerNode::update_summaries(tx, &hash, layer - 1).await?;
            InnerNode::load_parent_hashes(&mut *tx, &hash)
                .try_for_each(|parent_hash| {
                    nodes.push((parent_hash, layer - 1));
                    future::ready(Ok(()))
                })
                .await?;
        } else {
            let status = RootNode::update_summaries(tx, &hash).await?;
            RootNode::load_writer_ids(tx, &hash)
                .try_for_each(|writer_id| {
                    statuses.push((writer_id, status));
                    future::ready(Ok(()))
                })
                .await?;
        }
    }

    Ok(statuses)
}
