#[cfg(test)]
pub mod test_utils;

mod inner;
mod leaf;
mod root;
mod summary;
#[cfg(test)]
mod tests;

use std::iter;

pub(crate) use self::{
    inner::{InnerNode, InnerNodeMap, EMPTY_INNER_HASH, INNER_LAYER_COUNT},
    leaf::{LeafNode, LeafNodeSet, ModifyStatus},
    root::RootNode,
    summary::{MultiBlockPresence, SingleBlockPresence, Summary},
};

pub(super) use self::root::Completion;

use crate::{
    block::BlockId,
    collections::{HashMap, HashSet},
    crypto::{sign::PublicKey, Hash},
    db,
    error::Result,
};
use futures_util::{future, Stream, TryStreamExt};

/// Get the bucket for `locator` at the specified `inner_layer`.
pub(super) fn get_bucket(locator: &Hash, inner_layer: usize) -> u8 {
    locator.as_ref()[inner_layer]
}

/// Update summary of the nodes with the specified hashes and all their ancestor nodes.
/// Returns the `Completion`s of the affected complete snapshots.
pub(crate) async fn update_summaries(
    tx: &mut db::WriteTransaction,
    mut nodes: Vec<Hash>,
) -> Result<HashMap<Hash, Completion>> {
    let mut statuses = HashMap::default();

    while let Some(hash) = nodes.pop() {
        match parent_kind(tx, &hash).await? {
            Some(ParentNodeKind::Root) => {
                let Some(new) = RootNode::update_summaries(tx, &hash).await? else {
                    continue;
                };

                statuses.entry(hash).or_insert(Completion::Done).update(new);
            }
            Some(ParentNodeKind::Inner) => {
                InnerNode::update_summaries(tx, &hash).await?;
                try_collect_into(InnerNode::load_parent_hashes(tx, &hash), &mut nodes).await?;
            }
            None => (),
        }
    }

    Ok(statuses)
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

    for (hash, _) in update_summaries(tx, nodes).await? {
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

// TODO: move this to some generic utils module.
pub(super) async fn try_collect_into<S, D, T, E>(src: S, dst: &mut D) -> Result<(), E>
where
    S: Stream<Item = Result<T, E>>,
    D: Extend<T>,
{
    src.try_for_each(|item| {
        dst.extend(iter::once(item));
        future::ready(Ok(()))
    })
    .await
}

/// Check whether the snapshot with the given root hash, together with all the already complete
/// snapshots, is withink the given storage quota.
pub(super) async fn check_quota(
    _conn: &mut db::Connection,
    _root_hash: &Hash,
    _quota: u64,
) -> Result<bool> {
    Ok(true)
}
