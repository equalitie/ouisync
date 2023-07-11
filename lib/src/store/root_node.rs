use super::{
    error::Error,
    inner_node::{InnerNode, EMPTY_INNER_HASH},
};
use crate::{
    crypto::{
        sign::{Keypair, PublicKey},
        Hash,
    },
    db,
    debug::DebugPrinter,
    index::{MultiBlockPresence, NodeState, Proof, SingleBlockPresence, SnapshotId, Summary},
    version_vector::VersionVector,
    versioned::{BranchItem, Versioned},
};
use futures_util::{Stream, StreamExt, TryStreamExt};
use sqlx::Row;
use std::cmp::Ordering;

const EMPTY_SNAPSHOT_ID: SnapshotId = 0;

#[derive(Clone, Eq, PartialEq, Debug)]
pub(crate) struct RootNode {
    pub snapshot_id: SnapshotId,
    pub proof: Proof,
    pub summary: Summary,
}

impl Versioned for RootNode {
    fn version_vector(&self) -> &VersionVector {
        &self.proof.version_vector
    }
}

impl BranchItem for RootNode {
    fn branch_id(&self) -> &PublicKey {
        &self.proof.writer_id
    }
}

/// Creates a root node with the specified proof.
///
/// The version vector must be greater than the version vector of any currently existing root
/// node in the same branch, otherwise no node is created and an error is returned.
pub(super) async fn create(
    tx: &mut db::WriteTransaction,
    proof: Proof,
    summary: Summary,
) -> Result<RootNode, Error> {
    // Check that the root node to be created is newer than the latest existing root node in
    // the same branch.
    let old_vv: VersionVector = sqlx::query(
        "SELECT versions
         FROM snapshot_root_nodes
         WHERE snapshot_id = (
             SELECT MAX(snapshot_id)
             FROM snapshot_root_nodes
             WHERE writer_id = ?
         )",
    )
    .bind(&proof.writer_id)
    .map(|row| row.get(0))
    .fetch_optional(&mut *tx)
    .await?
    .unwrap_or_else(VersionVector::new);

    match proof.version_vector.partial_cmp(&old_vv) {
        Some(Ordering::Greater) => (),
        Some(Ordering::Equal | Ordering::Less) => {
            return Err(Error::OutdatedRootNode);
        }
        None => {
            return Err(Error::ConcurrentRootNode);
        }
    }

    let snapshot_id = sqlx::query(
        "INSERT INTO snapshot_root_nodes (
                 writer_id,
                 versions,
                 hash,
                 signature,
                 state,
                 block_presence
             )
             VALUES (?, ?, ?, ?, ?, ?)
             RETURNING snapshot_id",
    )
    .bind(&proof.writer_id)
    .bind(&proof.version_vector)
    .bind(&proof.hash)
    .bind(&proof.signature)
    .bind(summary.state)
    .bind(&summary.block_presence)
    .map(|row| row.get(0))
    .fetch_one(tx)
    .await?;

    Ok(RootNode {
        snapshot_id,
        proof,
        summary,
    })
}

pub(super) async fn load_or_create(
    conn: &mut db::Connection,
    branch_id: &PublicKey,
    write_keys: &Keypair,
) -> Result<RootNode, Error> {
    match load(conn, branch_id).await {
        Ok(root_node) => Ok(root_node),
        Err(Error::BranchNotFound) => Ok(RootNode::empty(*branch_id, write_keys)),
        Err(error) => Err(error),
    }
}

/// Returns the latest approved root node of the specified branch.
pub(super) async fn load(
    conn: &mut db::Connection,
    branch_id: &PublicKey,
) -> Result<RootNode, Error> {
    sqlx::query(
        "SELECT
                 snapshot_id,
                 versions,
                 hash,
                 signature,
                 block_presence
             FROM
                 snapshot_root_nodes
             WHERE
                 snapshot_id = (
                     SELECT MAX(snapshot_id)
                     FROM snapshot_root_nodes
                     WHERE writer_id = ? AND state = ?
                 )
            ",
    )
    .bind(branch_id)
    .bind(NodeState::Approved)
    .fetch_optional(conn)
    .await?
    .map(|row| RootNode {
        snapshot_id: row.get(0),
        proof: Proof::new_unchecked(*branch_id, row.get(1), row.get(2), row.get(3)),
        summary: Summary {
            state: NodeState::Approved,
            block_presence: row.get(4),
        },
    })
    .ok_or(Error::BranchNotFound)
}

/// Load the previous approved root node of the same writer.
pub(super) async fn load_prev(
    conn: &mut db::Connection,
    node: &RootNode,
) -> Result<Option<RootNode>, Error> {
    sqlx::query(
        "SELECT
                snapshot_id,
                versions,
                hash,
                signature,
                block_presence
             FROM snapshot_root_nodes
             WHERE writer_id = ? AND state = ? AND snapshot_id < ?
             ORDER BY snapshot_id DESC
             LIMIT 1",
    )
    .bind(&node.proof.writer_id)
    .bind(NodeState::Approved)
    .bind(node.snapshot_id)
    .fetch(conn)
    .map_ok(|row| RootNode {
        snapshot_id: row.get(0),
        proof: Proof::new_unchecked(node.proof.writer_id, row.get(1), row.get(2), row.get(3)),
        summary: Summary {
            state: NodeState::Approved,
            block_presence: row.get(4),
        },
    })
    .err_into()
    .try_next()
    .await
}

/// Return the latest approved root nodes of all known writers.
pub(super) fn load_all(
    conn: &mut db::Connection,
) -> impl Stream<Item = Result<RootNode, Error>> + '_ {
    sqlx::query(
        "SELECT
             snapshot_id,
             writer_id,
             versions,
             hash,
             signature,
             block_presence
         FROM
             snapshot_root_nodes
         WHERE
             snapshot_id IN (
                 SELECT MAX(snapshot_id)
                 FROM snapshot_root_nodes
                 WHERE state = ?
                 GROUP BY writer_id
             )",
    )
    .bind(NodeState::Approved)
    .fetch(conn)
    .map_ok(|row| RootNode {
        snapshot_id: row.get(0),
        proof: Proof::new_unchecked(row.get(1), row.get(2), row.get(3), row.get(4)),
        summary: Summary {
            state: NodeState::Approved,
            block_presence: row.get(5),
        },
    })
    .err_into()
}

/// Return the latest root nodes of all known writers in any state.
pub(super) fn load_all_in_any_state(
    conn: &mut db::Connection,
) -> impl Stream<Item = Result<RootNode, Error>> + '_ {
    sqlx::query(
        "SELECT
                 snapshot_id,
                 writer_id,
                 versions,
                 hash,
                 signature,
                 state,
                 block_presence
             FROM
                 snapshot_root_nodes
             WHERE
                 snapshot_id IN (
                     SELECT MAX(snapshot_id)
                     FROM snapshot_root_nodes
                     GROUP BY writer_id
                 )",
    )
    .fetch(conn)
    .map_ok(|row| RootNode {
        snapshot_id: row.get(0),
        proof: Proof::new_unchecked(row.get(1), row.get(2), row.get(3), row.get(4)),
        summary: Summary {
            state: row.get(5),
            block_presence: row.get(6),
        },
    })
    .err_into()
}

/// Does this node exist in the db?
pub(super) async fn exists(conn: &mut db::Connection, node: &RootNode) -> Result<bool, Error> {
    Ok(
        sqlx::query("SELECT 0 FROM snapshot_root_nodes WHERE snapshot_id = ?")
            .bind(node.snapshot_id)
            .fetch_optional(conn)
            .await?
            .is_some(),
    )
}

/// Removes the given root node including all its descendants that are not referenced from any
/// other root nodes.
pub(super) async fn remove(tx: &mut db::WriteTransaction, node: &RootNode) -> Result<(), Error> {
    // This uses db triggers to delete the whole snapshot.
    sqlx::query("DELETE FROM snapshot_root_nodes WHERE snapshot_id = ?")
        .bind(node.snapshot_id)
        .execute(tx)
        .await?;

    Ok(())
}

/// Removes all root nodes that are older than the given node and are on the same branch.
pub(super) async fn remove_older(
    tx: &mut db::WriteTransaction,
    node: &RootNode,
) -> Result<(), Error> {
    // This uses db triggers to delete the whole snapshot.
    sqlx::query("DELETE FROM snapshot_root_nodes WHERE snapshot_id < ? AND writer_id = ?")
        .bind(node.snapshot_id)
        .bind(&node.proof.writer_id)
        .execute(tx)
        .await?;

    Ok(())
}

/// Removes all root nodes that are older than the given node and are on the same branch and are
/// not complete.
pub(super) async fn remove_older_incomplete(
    tx: &mut db::WriteTransaction,
    node: &RootNode,
) -> Result<(), Error> {
    // This uses db triggers to delete the whole snapshot.
    sqlx::query(
        "DELETE FROM snapshot_root_nodes
         WHERE snapshot_id < ? AND writer_id = ? AND state IN (?, ?)",
    )
    .bind(node.snapshot_id)
    .bind(&node.proof.writer_id)
    .bind(NodeState::Incomplete)
    .bind(NodeState::Rejected)
    .execute(tx)
    .await?;

    Ok(())
}

/// Check whether the `old` snapshot can serve as a fallback for the `new` snapshot.
/// A snapshot can serve as a fallback if there is at least one locator that points to a missing
/// block in `new` but present block in `old`.
pub(super) async fn check_fallback(
    conn: &mut db::Connection,
    old: &RootNode,
    new: &RootNode,
) -> Result<bool, Error> {
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

/// Approve the nodes with the specified hash.
pub(super) async fn approve(tx: &mut db::WriteTransaction, hash: &Hash) -> Result<(), Error> {
    set_state(tx, hash, NodeState::Approved).await
}

/// Reject the nodes with the specified hash.
pub(super) async fn reject(tx: &mut db::WriteTransaction, hash: &Hash) -> Result<(), Error> {
    set_state(tx, hash, NodeState::Rejected).await
}

async fn set_state(
    tx: &mut db::WriteTransaction,
    hash: &Hash,
    state: NodeState,
) -> Result<(), Error> {
    sqlx::query("UPDATE snapshot_root_nodes SET state = ? WHERE hash = ?")
        .bind(state)
        .bind(hash)
        .execute(tx)
        .await?;
    Ok(())
}

/// Returns the writer ids of the nodes with the specified hash.
pub(super) fn load_writer_ids<'a>(
    conn: &'a mut db::Connection,
    hash: &'a Hash,
) -> impl Stream<Item = Result<PublicKey, Error>> + 'a {
    sqlx::query("SELECT DISTINCT writer_id FROM snapshot_root_nodes WHERE hash = ?")
        .bind(hash)
        .fetch(conn)
        .map_ok(|row| row.get(0))
        .err_into()
}

/// Returns a stream of all root nodes corresponding to the specified writer ordered from the
/// most recent to the least recent.
#[cfg(test)]
pub(super) fn load_all_by_writer_in_any_state(
    conn: &mut db::Connection,
    writer_id: PublicKey,
) -> impl Stream<Item = Result<RootNode, Error>> + '_ {
    sqlx::query(
        "SELECT
             snapshot_id,
             versions,
             hash,
             signature,
             state,
             block_presence
         FROM snapshot_root_nodes
         WHERE writer_id = ?
         ORDER BY snapshot_id DESC",
    )
    .bind(writer_id.as_ref().to_owned()) // needed to satisfy the borrow checker.
    .fetch(conn)
    .map_ok(move |row| RootNode {
        snapshot_id: row.get(0),
        proof: Proof::new_unchecked(writer_id, row.get(1), row.get(2), row.get(3)),
        summary: Summary {
            state: row.get(4),
            block_presence: row.get(5),
        },
    })
    .err_into()
}

impl RootNode {
    /// Creates a root node with no children without storing it in the database.
    pub fn empty(writer_id: PublicKey, write_keys: &Keypair) -> Self {
        let proof = Proof::new(
            writer_id,
            VersionVector::new(),
            *EMPTY_INNER_HASH,
            write_keys,
        );

        Self {
            snapshot_id: EMPTY_SNAPSHOT_ID,
            proof,
            summary: Summary {
                state: NodeState::Approved,
                block_presence: MultiBlockPresence::Full,
            },
        }
    }

    /// Creates a root node with the specified proof.
    ///
    /// The version vector must be greater than the version vector of any currently existing root
    /// node in the same branch, otherwise no node is created and an error is returned.
    #[deprecated = "don't use directly"]
    pub async fn create(
        tx: &mut db::WriteTransaction,
        proof: Proof,
        summary: Summary,
    ) -> Result<Self, Error> {
        create(tx, proof, summary).await
    }

    /// Returns a stream of all root nodes corresponding to the specified writer ordered from the
    /// most recent to the least recent.
    #[cfg(test)]
    #[deprecated]
    pub fn load_all_by_writer(
        conn: &mut db::Connection,
        writer_id: PublicKey,
    ) -> impl Stream<Item = Result<Self, Error>> + '_ {
        sqlx::query(
            "SELECT
                 snapshot_id,
                 versions,
                 hash,
                 signature,
                 state,
                 block_presence
             FROM snapshot_root_nodes
             WHERE writer_id = ?
             ORDER BY snapshot_id DESC",
        )
        .bind(writer_id.as_ref().to_owned()) // needed to satisfy the borrow checker.
        .fetch(conn)
        .map_ok(move |row| Self {
            snapshot_id: row.get(0),
            proof: Proof::new_unchecked(writer_id, row.get(1), row.get(2), row.get(3)),
            summary: Summary {
                state: row.get(4),
                block_presence: row.get(5),
            },
        })
        .err_into()
    }

    /// Returns the latest root node of the specified writer or `None` if no snapshot of that
    /// writer exists.
    #[cfg(test)]
    #[deprecated]
    pub async fn load_latest_by_writer(
        conn: &mut db::Connection,
        writer_id: PublicKey,
    ) -> Result<Option<Self>, Error> {
        Self::load_all_by_writer(conn, writer_id).try_next().await
    }

    /// Returns all nodes with the specified hash
    pub fn load_all_by_hash<'a>(
        conn: &'a mut db::Connection,
        hash: &'a Hash,
    ) -> impl Stream<Item = Result<Self, Error>> + 'a {
        sqlx::query(
            "SELECT
                 snapshot_id,
                 writer_id,
                 versions,
                 signature,
                 state,
                 block_presence
             FROM snapshot_root_nodes
             WHERE hash = ?",
        )
        .bind(hash)
        .fetch(conn)
        .map_ok(move |row| Self {
            snapshot_id: row.get(0),
            proof: Proof::new_unchecked(row.get(1), row.get(2), *hash, row.get(3)),
            summary: Summary {
                state: row.get(4),
                block_presence: row.get(5),
            },
        })
        .err_into()
    }

    /// Returns the writer ids of the nodes with the specified hash.
    #[deprecated]
    pub fn load_writer_ids<'a>(
        conn: &'a mut db::Connection,
        hash: &'a Hash,
    ) -> impl Stream<Item = Result<PublicKey, Error>> + 'a {
        load_writer_ids(conn, hash)
    }

    /// Reload this root node from the db.
    #[cfg(test)]
    pub async fn reload(&mut self, conn: &mut db::Connection) -> Result<(), Error> {
        let row = sqlx::query(
            "SELECT state, block_presence
             FROM snapshot_root_nodes
             WHERE snapshot_id = ?",
        )
        .bind(self.snapshot_id)
        .fetch_one(conn)
        .await?;

        self.summary.state = row.get(0);
        self.summary.block_presence = row.get(1);

        Ok(())
    }

    /// Update the summaries of all nodes with the specified hash.
    pub async fn update_summaries(
        tx: &mut db::WriteTransaction,
        hash: &Hash,
    ) -> Result<NodeState, Error> {
        let summary = InnerNode::compute_summary(tx, hash).await?;

        let state = sqlx::query(
            "UPDATE snapshot_root_nodes
             SET block_presence = ?,
                 state = CASE state WHEN ? THEN ? ELSE state END
             WHERE hash = ?
             RETURNING state
             ",
        )
        .bind(&summary.block_presence)
        .bind(NodeState::Incomplete)
        .bind(summary.state)
        .bind(hash)
        .fetch_optional(tx)
        .await?
        .map(|row| row.get(0))
        .unwrap_or(NodeState::Incomplete);

        Ok(state)
    }

    #[deprecated]
    pub async fn approve(tx: &mut db::WriteTransaction, hash: &Hash) -> Result<(), Error> {
        approve(tx, hash).await
    }

    pub async fn debug_print(conn: &mut db::Connection, printer: DebugPrinter) {
        let mut roots = sqlx::query(
            "SELECT
                 snapshot_id,
                 versions,
                 hash,
                 signature,
                 state,
                 block_presence,
                 writer_id
             FROM snapshot_root_nodes
             ORDER BY snapshot_id DESC",
        )
        .fetch(conn)
        .map_ok(move |row| Self {
            snapshot_id: row.get(0),
            proof: Proof::new_unchecked(row.get(6), row.get(1), row.get(2), row.get(3)),
            summary: Summary {
                state: row.get(4),
                block_presence: row.get(5),
            },
        });

        while let Some(root_node) = roots.next().await {
            match root_node {
                Ok(root_node) => {
                    printer.debug(&format_args!(
                        "RootNode: snapshot_id:{:?}, writer_id:{:?}, vv:{:?}, state:{:?}",
                        root_node.snapshot_id,
                        root_node.proof.writer_id,
                        root_node.proof.version_vector,
                        root_node.summary.state
                    ));
                }
                Err(err) => {
                    printer.debug(&format_args!("RootNode: error: {:?}", err));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use tempfile::TempDir;

    #[tokio::test]
    async fn create_new() {
        let (_base_dir, pool) = setup().await;

        let writer_id = PublicKey::random();
        let write_keys = Keypair::random();
        let hash = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node0 = create(
            &mut tx,
            Proof::new(
                writer_id,
                VersionVector::first(writer_id),
                hash,
                &write_keys,
            ),
            Summary::INCOMPLETE,
        )
        .await
        .unwrap();
        assert_eq!(node0.proof.hash, hash);

        let nodes: Vec<_> = load_all_by_writer_in_any_state(&mut tx, writer_id)
            .try_collect()
            .await
            .unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0], node0);
    }

    #[tokio::test]
    async fn attempt_to_create_outdated() {
        let (_base_dir, pool) = setup().await;

        let writer_id = PublicKey::random();
        let write_keys = Keypair::random();
        let hash = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let vv0 = VersionVector::first(writer_id);
        let vv1 = vv0.clone().incremented(writer_id);

        create(
            &mut tx,
            Proof::new(writer_id, vv1.clone(), hash, &write_keys),
            Summary::INCOMPLETE,
        )
        .await
        .unwrap();

        // Same vv
        assert_matches!(
            create(
                &mut tx,
                Proof::new(writer_id, vv1, hash, &write_keys),
                Summary::INCOMPLETE,
            )
            .await,
            Err(Error::OutdatedRootNode)
        );

        // Old vv
        assert_matches!(
            create(
                &mut tx,
                Proof::new(writer_id, vv0, hash, &write_keys),
                Summary::INCOMPLETE,
            )
            .await,
            Err(Error::OutdatedRootNode)
        );
    }

    async fn setup() -> (TempDir, db::Pool) {
        db::create_temp().await.unwrap()
    }
}
