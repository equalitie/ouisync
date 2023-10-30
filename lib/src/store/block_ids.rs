use super::error::Error;
use crate::{
    crypto::sign::PublicKey,
    db,
    protocol::{BlockId, NodeState, SingleBlockPresence},
};
use futures_util::{Stream, TryStreamExt};
use sqlx::Row;
use std::collections::BTreeSet;

pub(crate) struct BlockIdsPage {
    db: db::Pool,
    lower_bound: Option<BlockId>,
    page_size: u32,
}

impl BlockIdsPage {
    pub(super) fn new(db: db::Pool, page_size: u32) -> Self {
        Self {
            db,
            lower_bound: None,
            page_size,
        }
    }

    /// Returns the next page of the results. If the returned collection is empty it means the end
    /// of the results was reached. Calling `next` afterwards resets the page back to zero.
    pub async fn next(&mut self) -> Result<BTreeSet<BlockId>, Error> {
        let mut conn = self.db.acquire().await?;

        let ids: Result<BTreeSet<_>, Error> = sqlx::query(
            "WITH RECURSIVE
                 inner_nodes(hash) AS (
                     SELECT i.hash
                        FROM snapshot_inner_nodes AS i
                        INNER JOIN snapshot_root_nodes AS r ON r.hash = i.parent
                        WHERE r.state = ?
                     UNION ALL
                     SELECT c.hash
                        FROM snapshot_inner_nodes AS c
                        INNER JOIN inner_nodes AS p ON p.hash = c.parent
                 )
             SELECT DISTINCT block_id
                 FROM snapshot_leaf_nodes
                 WHERE
                     parent IN inner_nodes
                     AND block_presence = ?
                     AND block_id > COALESCE(?, x'')
                 ORDER BY block_id
                 LIMIT ?",
        )
        .bind(NodeState::Approved)
        .bind(SingleBlockPresence::Present)
        .bind(self.lower_bound.as_ref())
        .bind(self.page_size)
        .fetch(&mut *conn)
        .map_ok(|row| row.get::<BlockId, _>(0))
        .err_into()
        .try_collect()
        .await;

        let ids = ids?;

        // TODO: use `last` when we bump to rust 1.66
        self.lower_bound = ids.iter().next_back().copied();

        Ok(ids)
    }
}

/// Yields all missing block ids referenced from the latest complete snapshot of the given branch.
pub(super) fn missing_block_ids_in_branch<'a>(
    conn: &'a mut db::Connection,
    branch_id: &'a PublicKey,
) -> impl Stream<Item = Result<BlockId, Error>> + 'a {
    sqlx::query(
        "WITH RECURSIVE
             inner_nodes(hash) AS (
                 SELECT i.hash
                     FROM snapshot_inner_nodes AS i
                     INNER JOIN snapshot_root_nodes AS r ON r.hash = i.parent
                     WHERE r.snapshot_id = (
                         SELECT MAX(snapshot_id)
                         FROM snapshot_root_nodes
                         WHERE writer_id = ? AND state = ?
                     )
                 UNION ALL
                 SELECT c.hash
                     FROM snapshot_inner_nodes AS c
                     INNER JOIN inner_nodes AS p ON p.hash = c.parent
             )
         SELECT DISTINCT block_id
             FROM snapshot_leaf_nodes
             WHERE parent IN inner_nodes AND block_presence = ?
         ",
    )
    .bind(branch_id)
    .bind(NodeState::Approved)
    .bind(SingleBlockPresence::Missing)
    .fetch(conn)
    .map_ok(|row| row.get(0))
    .err_into()
}
