use std::cmp::Ordering;

use super::{
    super::{proof::Proof, SnapshotId},
    inner::InnerNode,
    summary::Summary,
    EMPTY_INNER_HASH,
};
use crate::{
    crypto::{
        sign::{Keypair, PublicKey},
        Hash,
    },
    db,
    debug::DebugPrinter,
    error::{Error, Result},
    version_vector::VersionVector,
};
use futures_util::{Stream, StreamExt, TryStreamExt};
use sqlx::Row;

const EMPTY_SNAPSHOT_ID: SnapshotId = 0;

#[derive(Clone, Eq, PartialEq, Debug)]
pub(crate) struct RootNode {
    pub snapshot_id: SnapshotId,
    pub proof: Proof,
    pub summary: Summary,
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
            summary: Summary::FULL,
        }
    }

    /// Creates a root node with the specified proof unless already exists.
    ///
    /// # Panics
    ///
    /// - If `latest` is `Some` but it's not the current latest root node in the same branch
    /// - If `latest` is `Some` and the new version vector is not happens-after or equal to that of
    ///   `latest`
    /// - If `latest` is `None` or `Some(RootNode::empty())` and there already exists a root node
    ///   in the same branch
    pub async fn create(
        tx: &mut db::WriteTransaction,
        latest: Option<&RootNode>,
        proof: Proof,
        summary: Summary,
    ) -> Result<Self> {
        if let Some(latest) = latest {
            match proof
                .version_vector
                .partial_cmp(&latest.proof.version_vector)
            {
                Some(Ordering::Greater | Ordering::Equal) => (),
                Some(Ordering::Less) | None => {
                    panic!("root node invariant violation: invalid version vector")
                }
            }
        }

        let expected_latest_snapshot_id = latest
            .map(|node| node.snapshot_id)
            .unwrap_or(EMPTY_SNAPSHOT_ID);
        let actual_latest_snapshot_id =
            sqlx::query("SELECT MAX(snapshot_id) FROM snapshot_root_nodes WHERE writer_id = ?")
                .bind(&proof.writer_id)
                .map(|row| row.get(0))
                .fetch_optional(&mut **tx)
                .await?
                .unwrap_or(EMPTY_SNAPSHOT_ID);

        assert_eq!(
            expected_latest_snapshot_id, actual_latest_snapshot_id,
            "root node invariant violation: latest mismatch"
        );

        let snapshot_id = sqlx::query(
            "INSERT INTO snapshot_root_nodes (
                 writer_id,
                 versions,
                 hash,
                 signature,
                 is_complete,
                 block_presence
             )
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (writer_id, hash) DO NOTHING
             RETURNING snapshot_id",
        )
        .bind(&proof.writer_id)
        .bind(&proof.version_vector)
        .bind(&proof.hash)
        .bind(&proof.signature)
        .bind(summary.is_complete)
        .bind(&summary.block_presence)
        .map(|row| row.get(0))
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(Error::EntryExists)?;

        Ok(Self {
            snapshot_id,
            proof,
            summary,
        })
    }

    /// Returns the latest complete root node of the specified writer. Unlike
    /// `load_latest_by_writer`, this assumes the node exists and returns `EntryNotFound` otherwise.
    pub async fn load_latest_complete_by_writer(
        conn: &mut db::Connection,
        writer_id: PublicKey,
    ) -> Result<Self> {
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
                     WHERE writer_id = ? AND is_complete = 1
                 )
            ",
        )
        .bind(&writer_id)
        .fetch_optional(conn)
        .await?
        .map(|row| Self {
            snapshot_id: row.get(0),
            proof: Proof::new_unchecked(writer_id, row.get(1), row.get(2), row.get(3)),
            summary: Summary {
                is_complete: true,
                block_presence: row.get(4),
            },
        })
        .ok_or(Error::EntryNotFound)
    }

    /// Return the latest complete root nodes of all known writers.
    pub fn load_all_latest_complete(
        conn: &mut db::Connection,
    ) -> impl Stream<Item = Result<Self>> + '_ {
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
                     WHERE is_complete = 1
                     GROUP BY writer_id
                 )",
        )
        .fetch(conn)
        .map_ok(|row| Self {
            snapshot_id: row.get(0),
            proof: Proof::new_unchecked(row.get(1), row.get(2), row.get(3), row.get(4)),
            summary: Summary {
                is_complete: true,
                block_presence: row.get(5),
            },
        })
        .err_into()
    }

    /// Return the latest root nodes of all known writers.
    pub fn load_all_latest(conn: &mut db::Connection) -> impl Stream<Item = Result<Self>> + '_ {
        sqlx::query(
            "SELECT
                 snapshot_id,
                 writer_id,
                 versions,
                 hash,
                 signature,
                 is_complete,
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
        .map_ok(|row| Self {
            snapshot_id: row.get(0),
            proof: Proof::new_unchecked(row.get(1), row.get(2), row.get(3), row.get(4)),
            summary: Summary {
                is_complete: row.get(5),
                block_presence: row.get(6),
            },
        })
        .err_into()
    }

    /// Returns a stream of all root nodes corresponding to the specified writer ordered from the
    /// most recent to the least recent.
    #[cfg(test)]
    pub fn load_all_by_writer(
        conn: &mut db::Connection,
        writer_id: PublicKey,
    ) -> impl Stream<Item = Result<Self>> + '_ {
        sqlx::query(
            "SELECT
                 snapshot_id,
                 versions,
                 hash,
                 signature,
                 is_complete,
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
                is_complete: row.get(4),
                block_presence: row.get(5),
            },
        })
        .err_into()
    }

    /// Returns the latest root node of the specified writer or `None` if no snapshot of that
    /// writer exists.
    #[cfg(test)]
    pub async fn load_latest_by_writer(
        conn: &mut db::Connection,
        writer_id: PublicKey,
    ) -> Result<Option<Self>> {
        Self::load_all_by_writer(conn, writer_id).try_next().await
    }

    /// Returns the writer ids of the nodes with the specified hash.
    pub fn load_writer_ids<'a>(
        conn: &'a mut db::Connection,
        hash: &'a Hash,
    ) -> impl Stream<Item = Result<PublicKey>> + 'a {
        sqlx::query("SELECT writer_id FROM snapshot_root_nodes WHERE hash = ?")
            .bind(hash)
            .fetch(conn)
            .map_ok(|row| row.get(0))
            .err_into()
    }

    /// Load the previous complete root node of the same writer.
    pub async fn load_prev(&self, conn: &mut db::Connection) -> Result<Option<Self>> {
        sqlx::query(
            "SELECT
                snapshot_id,
                versions,
                hash,
                signature,
                block_presence
             FROM snapshot_root_nodes
             WHERE writer_id = ? AND is_complete = 1 AND snapshot_id < ?
             ORDER BY snapshot_id DESC
             LIMIT 1",
        )
        .bind(&self.proof.writer_id)
        .bind(self.snapshot_id)
        .fetch(conn)
        .map_ok(|row| Self {
            snapshot_id: row.get(0),
            proof: Proof::new_unchecked(self.proof.writer_id, row.get(1), row.get(2), row.get(3)),
            summary: Summary {
                is_complete: true,
                block_presence: row.get(4),
            },
        })
        .err_into()
        .try_next()
        .await
    }

    /// Updates the proof of this node.
    ///
    /// Only the version vector can be updated. To update any other field of the proof, a new root
    /// node needs to be created.
    ///
    /// # Panics
    ///
    /// Panics if the writer_id or the hash of the new_proof differ from the ones in the current
    /// proof.
    ///
    /// # Cancel safety
    ///
    /// This methd consume `self` to force the caller to reload the node from the db if they still
    /// need to use it. This makes it cancel-safe because the `proof` member can never go out of
    /// sync with what's stored in the db even in case the returned future is cancelled.
    pub async fn update_proof(self, tx: &mut db::WriteTransaction, new_proof: Proof) -> Result<()> {
        assert_eq!(new_proof.writer_id, self.proof.writer_id);
        assert_eq!(new_proof.hash, self.proof.hash);

        sqlx::query(
            "UPDATE snapshot_root_nodes SET versions = ?, signature = ? WHERE snapshot_id = ?",
        )
        .bind(&new_proof.version_vector)
        .bind(&new_proof.signature)
        .bind(self.snapshot_id)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Reload this root node from the db.
    #[cfg(test)]
    pub async fn reload(&mut self, conn: &mut db::Connection) -> Result<()> {
        let row = sqlx::query(
            "SELECT is_complete, block_presence
             FROM snapshot_root_nodes
             WHERE snapshot_id = ?",
        )
        .bind(self.snapshot_id)
        .fetch_one(conn)
        .await?;

        self.summary.is_complete = row.get(0);
        self.summary.block_presence = row.get(1);

        Ok(())
    }

    /// Updates the summaries of all nodes with the specified hash. Returns whether the nodes
    /// became complete.
    pub async fn update_summaries(tx: &mut db::WriteTransaction, hash: &Hash) -> Result<bool> {
        let summary = InnerNode::compute_summary(tx, hash).await?;

        // Multiple nodes with the same hash should have the same `is_complete` which is why it's
        // enough to fetch just one.
        let was_complete: bool =
            sqlx::query("SELECT is_complete FROM snapshot_root_nodes WHERE hash = ?")
                .bind(hash)
                .fetch_optional(&mut **tx)
                .await?
                .map(|row| row.get(0))
                .unwrap_or(false);

        sqlx::query(
            "UPDATE snapshot_root_nodes
             SET is_complete = ?, block_presence = ?
             WHERE hash = ?",
        )
        .bind(summary.is_complete)
        .bind(&summary.block_presence)
        .bind(hash)
        .execute(&mut **tx)
        .await?;

        Ok(!was_complete && summary.is_complete)
    }

    /// Removes this node including its snapshot.
    pub async fn remove_recursively(&self, tx: &mut db::WriteTransaction) -> Result<()> {
        // This uses db triggers to delete the whole snapshot.
        sqlx::query("DELETE FROM snapshot_root_nodes WHERE snapshot_id = ?")
            .bind(self.snapshot_id)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    /// Removes all root nodes, including their snapshots, that are older than this node and are
    /// on the same branch.
    pub async fn remove_recursively_all_older(&self, tx: &mut db::WriteTransaction) -> Result<()> {
        // This uses db triggers to delete the whole snapshot.
        sqlx::query("DELETE FROM snapshot_root_nodes WHERE snapshot_id < ? AND writer_id = ?")
            .bind(self.snapshot_id)
            .bind(&self.proof.writer_id)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    /// Removes all root nodes, including their snapshots, that are older than this node and are
    /// on the same branch and are not complete.
    pub async fn remove_recursively_all_older_incomplete(
        &self,
        tx: &mut db::WriteTransaction,
    ) -> Result<()> {
        // This uses db triggers to delete the whole snapshot.
        sqlx::query(
            "DELETE FROM snapshot_root_nodes
             WHERE snapshot_id < ? AND writer_id = ? AND is_complete = 0",
        )
        .bind(self.snapshot_id)
        .bind(&self.proof.writer_id)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    pub async fn debug_print(conn: &mut db::Connection, printer: DebugPrinter) {
        let mut roots = sqlx::query(
            "SELECT
                 snapshot_id,
                 versions,
                 hash,
                 signature,
                 is_complete,
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
                is_complete: row.get(4),
                block_presence: row.get(5),
            },
        });

        while let Some(root_node) = roots.next().await {
            match root_node {
                Ok(root_node) => {
                    printer.debug(&format_args!(
                        "RootNode: snapshot_id:{:?}, writer_id:{:?}, vv:{:?}, is_complete:{:?}",
                        root_node.snapshot_id,
                        root_node.proof.writer_id,
                        root_node.proof.version_vector,
                        root_node.summary.is_complete
                    ));
                }
                Err(err) => {
                    printer.debug(&format_args!("RootNode: error: {:?}", err));
                }
            }
        }
    }
}
