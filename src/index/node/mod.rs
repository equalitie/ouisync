#[cfg(test)]
pub mod test_utils;

mod inner;
mod leaf;
mod link;
mod root;
mod summary;
#[cfg(test)]
mod tests;

pub use self::{
    inner::{InnerNode, InnerNodeMap, INNER_LAYER_COUNT},
    leaf::{LeafNode, LeafNodeSet, ModifyStatus},
    root::RootNode,
    summary::Summary,
};

use crate::{block::BlockId, crypto::Hash, db, error::Result, replica_id::ReplicaId};
use futures_util::{future, TryStreamExt};
use sqlx::Sqlite;
use std::collections::HashSet;

/// Get the bucket for `locator` at the specified `inner_layer`.
pub fn get_bucket(locator: &Hash, inner_layer: usize) -> u8 {
    locator.as_ref()[inner_layer]
}

/// Update summary of the nodes with the specified hash and layer and all their ancestor nodes.
/// Returns the affected replica ids.
pub async fn update_summaries<'a, T>(db: T, hash: Hash, layer: usize) -> Result<HashSet<ReplicaId>>
where
    T: sqlx::Acquire<'a, Database = Sqlite>,
{
    let mut tx = db.begin().await?;
    let replica_ids = update_summaries_in_transaction(&mut tx, vec![(hash, layer)]).await?;
    tx.commit().await?;

    Ok(replica_ids)
}

/// Receive a block from other replica. This marks the block as not missing by the local replica.
/// Returns the replica ids whose branches reference the received block (if any).
pub async fn receive_block(
    tx: &mut db::Transaction<'_>,
    id: &BlockId,
) -> Result<HashSet<ReplicaId>> {
    // TODO: return error when no node referencing the block exists.
    // TODO: return whether anything changed, bail early if not.
    LeafNode::set_present(tx, id).await?;

    let nodes = LeafNode::load_parent_hashes(tx, id)
        .map_ok(|hash| (hash, INNER_LAYER_COUNT))
        .try_collect()
        .await?;

    update_summaries_in_transaction(tx, nodes).await
}

async fn update_summaries_in_transaction(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    mut nodes: Vec<(Hash, usize)>,
) -> Result<HashSet<ReplicaId>> {
    let mut replica_ids = HashSet::new();

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
            RootNode::update_summaries(tx, &hash).await?;
            RootNode::load_replica_ids(tx, &hash)
                .try_for_each(|replica_id| {
                    replica_ids.insert(replica_id);
                    future::ready(Ok(()))
                })
                .await?;
        }
    }

    Ok(replica_ids)
}
