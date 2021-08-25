#[cfg(test)]
pub mod test_utils;

mod inner;
mod leaf;
mod link;
mod root;
mod summary;
#[cfg(test)]
mod tests;

pub(crate) use self::{
    inner::{InnerNode, InnerNodeMap, INNER_LAYER_COUNT},
    leaf::{LeafNode, LeafNodeSet, ModifyStatus},
    root::RootNode,
    summary::Summary,
};

use crate::{block::BlockId, crypto::Hash, db, error::Result, replica_id::ReplicaId};
use futures_util::{future, TryStreamExt};
use sqlx::Sqlite;
use std::collections::HashMap;

/// Get the bucket for `locator` at the specified `inner_layer`.
pub(super) fn get_bucket(locator: &Hash, inner_layer: usize) -> u8 {
    locator.as_ref()[inner_layer]
}

/// Update summary of the nodes with the specified hash and layer and all their ancestor nodes.
/// Returns a map `ReplicaId -> bool` indicating which branches were affected and whether they
/// became complete by this update.
pub(super) async fn update_summaries<'a, T>(
    db: T,
    hash: Hash,
    layer: usize,
) -> Result<HashMap<ReplicaId, bool>>
where
    T: sqlx::Acquire<'a, Database = Sqlite>,
{
    let mut tx = db.begin().await?;
    let status = update_summaries_in_transaction(&mut tx, vec![(hash, layer)]).await?;
    tx.commit().await?;

    Ok(status)
}

/// Receive a block from other replica. This marks the block as not missing by the local replica.
/// Returns the replica ids whose branches reference the received block (if any).
pub(crate) async fn receive_block(
    tx: &mut db::Transaction<'_>,
    id: &BlockId,
) -> Result<HashMap<ReplicaId, bool>> {
    if !LeafNode::set_present(tx, id).await? {
        return Ok(HashMap::new());
    }

    let nodes = LeafNode::load_parent_hashes(tx, id)
        .map_ok(|hash| (hash, INNER_LAYER_COUNT))
        .try_collect()
        .await?;

    update_summaries_in_transaction(tx, nodes).await
}

async fn update_summaries_in_transaction(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    mut nodes: Vec<(Hash, usize)>,
) -> Result<HashMap<ReplicaId, bool>> {
    let mut status = HashMap::new();

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
            let complete = RootNode::update_summaries(tx, &hash).await?;
            RootNode::load_replica_ids(tx, &hash)
                .try_for_each(|replica_id| {
                    status.insert(replica_id, complete);
                    future::ready(Ok(()))
                })
                .await?;
        }
    }

    Ok(status)
}
