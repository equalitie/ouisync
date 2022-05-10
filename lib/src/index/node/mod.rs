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
    summary::Summary,
};

use crate::{
    block::BlockId,
    crypto::{sign::PublicKey, Hash},
    db,
    error::{Error, Result},
};
use futures_util::{future, TryStreamExt};
use sqlx::Acquire;

pub(super) async fn init(conn: &mut db::Connection) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS snapshot_root_nodes (
             snapshot_id             INTEGER PRIMARY KEY,
             writer_id               BLOB NOT NULL,
             versions                BLOB NOT NULL,

             -- Hash of the children
             hash                    BLOB NOT NULL,

             -- Signature proving the creator has write access
             signature               BLOB NOT NULL,

             -- Is this snapshot completely downloaded?
             is_complete             INTEGER NOT NULL,

             -- Summary of the missing blocks in this subree
             missing_blocks_count    INTEGER NOT NULL,
             missing_blocks_checksum INTEGER NOT NULL,

             UNIQUE(writer_id, hash)
         );

         CREATE INDEX IF NOT EXISTS index_snapshot_root_nodes_on_hash
             ON snapshot_root_nodes (hash);

         CREATE TABLE IF NOT EXISTS snapshot_inner_nodes (
             -- Parent's `hash`
             parent                  BLOB NOT NULL,

             -- Index of this node within its siblings
             bucket                  INTEGER NOT NULL,

             -- Hash of the children
             hash                    BLOB NOT NULL,

             -- Is this subree completely downloaded?
             is_complete             INTEGER NOT NULL,

             -- Summary of the missing blocks in this subree
             missing_blocks_count    INTEGER NOT NULL,
             missing_blocks_checksum INTEGER NOT NULL,

             UNIQUE(parent, bucket)
         );

         CREATE INDEX IF NOT EXISTS index_snapshot_inner_nodes_on_hash
             ON snapshot_inner_nodes (hash);

         CREATE TABLE IF NOT EXISTS snapshot_leaf_nodes (
             -- Parent's `hash`
             parent      BLOB NOT NULL,
             locator     BLOB NOT NULL,
             block_id    BLOB NOT NULL,

             -- Is the block pointed to by this node missing?
             is_missing  INTEGER NOT NULL,

             UNIQUE(parent, locator, block_id)
         );

         CREATE INDEX IF NOT EXISTS index_snapshot_leaf_nodes_on_block_id
             ON snapshot_leaf_nodes (block_id);

         -- Prevents creating multiple inner nodes with the same parent and bucket but different
         -- hash.
         CREATE TRIGGER IF NOT EXISTS snapshot_inner_nodes_conflict_check
         BEFORE INSERT ON snapshot_inner_nodes
         WHEN EXISTS (
             SELECT 0
             FROM snapshot_inner_nodes
             WHERE parent = new.parent
               AND bucket = new.bucket
               AND hash <> new.hash
         )
         BEGIN
             SELECT RAISE (ABORT, 'inner node conflict');
         END;

         -- Delete whole subtree if a node is deleted and there are no more nodes at the same layer
         -- with the same hash.
         -- Note this needs `PRAGMA recursive_triggers = ON` to work.
         CREATE TRIGGER IF NOT EXISTS snapshot_inner_nodes_delete_on_root_deleted
         AFTER DELETE ON snapshot_root_nodes
         WHEN NOT EXISTS (SELECT 0 FROM snapshot_root_nodes WHERE hash = old.hash)
         BEGIN
             DELETE FROM snapshot_inner_nodes WHERE parent = old.hash;
         END;

         CREATE TRIGGER IF NOT EXISTS snapshot_inner_nodes_delete_on_parent_deleted
         AFTER DELETE ON snapshot_inner_nodes
         WHEN NOT EXISTS (SELECT 0 FROM snapshot_inner_nodes WHERE hash = old.hash)
         BEGIN
             DELETE FROM snapshot_inner_nodes WHERE parent = old.hash;
         END;

         CREATE TRIGGER IF NOT EXISTS snapshot_leaf_nodes_delete_on_parent_deleted
         AFTER DELETE ON snapshot_inner_nodes
         WHEN NOT EXISTS (SELECT 0 FROM snapshot_inner_nodes WHERE hash = old.hash)
         BEGIN
             DELETE FROM snapshot_leaf_nodes WHERE parent = old.hash;
         END;

         ",
    )
    .execute(conn)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}

/// Get the bucket for `locator` at the specified `inner_layer`.
pub(super) fn get_bucket(locator: &Hash, inner_layer: usize) -> u8 {
    locator.as_ref()[inner_layer]
}

/// Update summary of the nodes with the specified hash and all their ancestor nodes.
/// Returns a map `PublicKey -> bool` indicating which branches were affected and whether they are
/// complete.
pub(super) async fn update_summaries(
    conn: &mut db::Connection,
    hash: Hash,
) -> Result<Vec<(PublicKey, bool)>> {
    let mut tx = conn.begin().await?;
    let statuses = update_summaries_with_stack(&mut tx, vec![hash]).await?;
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
        .try_collect()
        .await?;

    let ids = update_summaries_with_stack(&mut tx, nodes)
        .await?
        .into_iter()
        .map(|(writer_id, _)| writer_id)
        .collect();

    tx.commit().await?;

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
    conn: &mut db::Connection,
    mut nodes: Vec<Hash>,
) -> Result<Vec<(PublicKey, bool)>> {
    let mut statuses = Vec::new();

    while let Some(hash) = nodes.pop() {
        match parent_kind(conn, &hash).await? {
            Some(ParentNodeKind::Root) => {
                let complete = RootNode::update_summaries(conn, &hash).await?;
                RootNode::load_writer_ids(conn, &hash)
                    .try_for_each(|writer_id| {
                        statuses.push((writer_id, complete));
                        future::ready(Ok(()))
                    })
                    .await?;
            }
            Some(ParentNodeKind::Inner) => {
                InnerNode::update_summaries(conn, &hash).await?;
                InnerNode::load_parent_hashes(conn, &hash)
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
