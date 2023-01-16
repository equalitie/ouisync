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
    summary::{SingleBlockPresence, Summary},
};

use crate::{
    block::BlockId,
    collections::{HashMap, HashSet},
    crypto::{sign::PublicKey, Hash},
    db,
    error::Result,
};
use futures_util::{future, TryStreamExt};

/// Get the bucket for `locator` at the specified `inner_layer`.
pub(super) fn get_bucket(locator: &Hash, inner_layer: usize) -> u8 {
    locator.as_ref()[inner_layer]
}

/// Update summary of the nodes with the specified hash and all their ancestor nodes.
/// Returns a map `PublicKey -> bool` indicating which branches were affected and whether they
/// became complete.
pub(crate) async fn update_summaries(
    tx: &mut db::WriteTransaction,
    hash: Hash,
) -> Result<HashMap<PublicKey, bool>> {
    update_summaries_with_stack(tx, vec![hash]).await
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

    let ids = update_summaries_with_stack(tx, nodes)
        .await?
        .into_iter()
        .map(|(writer_id, _)| writer_id)
        .collect();

    Ok(ids)
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

enum ParentNodeKind {
    Root,
    Inner,
}

async fn parent_kind(conn: &mut db::Connection, hash: &Hash) -> Result<Option<ParentNodeKind>> {
    use sqlx::Row;

    let kind: u8 = sqlx::query(
        "SELECT CASE
             WHEN EXISTS(SELECT 0 FROM snapshot_root_nodes  WHERE hash = ?) THEN 1
             WHEN EXISTS(SELECT 0 FROM snapshot_inner_nodes WHERE hash = ?) THEN 2
             ELSE 0
         END",
    )
    .bind(hash)
    .bind(hash)
    .fetch_one(conn)
    .await?
    .get(0);

    match kind {
        0 => Ok(None),
        1 => Ok(Some(ParentNodeKind::Root)),
        2 => Ok(Some(ParentNodeKind::Inner)),
        _ => unreachable!(),
    }
}

async fn update_summaries_with_stack(
    tx: &mut db::WriteTransaction,
    mut nodes: Vec<Hash>,
) -> Result<HashMap<PublicKey, bool>> {
    let mut statuses = HashMap::default();

    while let Some(hash) = nodes.pop() {
        match parent_kind(tx, &hash).await? {
            Some(ParentNodeKind::Root) => {
                let complete = RootNode::update_summaries(tx, &hash).await?;
                RootNode::load_writer_ids(tx, &hash)
                    .try_for_each(|writer_id| {
                        let entry = statuses.entry(writer_id).or_insert(false);
                        *entry = *entry || complete;

                        future::ready(Ok(()))
                    })
                    .await?;
            }
            Some(ParentNodeKind::Inner) => {
                InnerNode::update_summaries(tx, &hash).await?;
                InnerNode::load_parent_hashes(tx, &hash)
                    .try_for_each(|parent_hash| {
                        nodes.push(parent_hash);
                        future::ready(Ok(()))
                    })
                    .await?;
            }
            None => (),
        }
    }

    Ok(statuses)
}
