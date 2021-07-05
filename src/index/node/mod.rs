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

use crate::{block::BlockId, crypto::Hash, db, error::Result};
use futures_util::{future, TryStreamExt};
use sqlx::Sqlite;

/// Get the bucket for `locator` at the specified `inner_layer`.
pub fn get_bucket(locator: &Hash, inner_layer: usize) -> u8 {
    locator.as_ref()[inner_layer]
}

/// Update summaries of the specified nodes (layer + hash) and all their ancestor nodes.
pub async fn update_summaries<'a, E, I>(db: E, nodes: I) -> Result<()>
where
    E: sqlx::Acquire<'a, Database = Sqlite>,
    I: IntoIterator<Item = (Hash, usize)>,
{
    let mut tx = db.begin().await?;
    update_summaries_in_transaction(&mut tx, nodes.into_iter().collect()).await?;
    tx.commit().await?;
    Ok(())
}

async fn update_summaries_in_transaction(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    mut nodes: Vec<(Hash, usize)>,
) -> Result<()> {
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
            RootNode::update_summaries(tx, &hash).await?
        }
    }

    Ok(())
}

/// Receive a block from other replica. This marks the block as not missing by the local replica.
pub async fn receive_block(tx: &mut db::Transaction<'_>, id: &BlockId) -> Result<()> {
    // TODO: notify affected branches
    LeafNode::set_present(tx, id).await?;

    let nodes = LeafNode::load_parent_hashes(tx, id)
        .map_ok(|hash| (hash, INNER_LAYER_COUNT))
        .try_collect()
        .await?;

    update_summaries_in_transaction(tx, nodes).await
}
