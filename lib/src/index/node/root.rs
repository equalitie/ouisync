use super::{
    super::{proof::Proof, SnapshotId},
    inner::InnerNode,
    summary::{Summary, SummaryUpdateStatus},
};
use crate::{
    crypto::{sign::PublicKey, Hash},
    db,
    error::{Error, Result},
};
use futures_util::{Stream, TryStreamExt};
use sqlx::Row;

#[derive(Clone, Eq, PartialEq, Debug)]
pub(crate) struct RootNode {
    pub snapshot_id: SnapshotId,
    pub proof: Proof,
    pub summary: Summary,
}

impl RootNode {
    /// Returns the latest root node of the specified replica or `None` if no snapshot of that
    /// replica exists.
    pub async fn load_latest(
        conn: &mut db::Connection,
        writer_id: PublicKey,
    ) -> Result<Option<Self>> {
        Self::load_all(conn, writer_id, 1).try_next().await
    }

    /// Creates a root node of the specified replica unless it already exists.
    pub async fn create(conn: &mut db::Connection, proof: Proof, summary: Summary) -> Result<Self> {
        let snapshot_id = sqlx::query(
            "INSERT INTO snapshot_root_nodes (
                 writer_id,
                 versions,
                 hash,
                 signature,
                 is_complete,
                 missing_blocks_count,
                 missing_blocks_checksum
             )
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (writer_id, hash) DO NOTHING
             RETURNING snapshot_id",
        )
        .bind(&proof.writer_id)
        .bind(&proof.version_vector)
        .bind(&proof.hash)
        .bind(&proof.signature)
        .bind(summary.is_complete)
        .bind(db::encode_u64(summary.missing_blocks_count))
        .bind(db::encode_u64(summary.missing_blocks_checksum))
        .fetch_optional(conn)
        .await?
        .ok_or(Error::EntryExists)?
        .get(0);

        Ok(Self {
            snapshot_id,
            proof,
            summary,
        })
    }

    /// Returns a stream of all (but at most `limit`) root nodes corresponding to the specified
    /// replica ordered from the most recent to the least recent.
    pub fn load_all(
        conn: &mut db::Connection,
        writer_id: PublicKey,
        limit: u32,
    ) -> impl Stream<Item = Result<Self>> + '_ {
        sqlx::query(
            "SELECT
                 snapshot_id,
                 versions,
                 hash,
                 signature,
                 is_complete,
                 missing_blocks_count,
                 missing_blocks_checksum
             FROM snapshot_root_nodes
             WHERE writer_id = ?
             ORDER BY snapshot_id DESC
             LIMIT ?",
        )
        .bind(writer_id.as_ref().to_owned()) // needed to satisfy the borrow checker.
        .bind(limit)
        .fetch(conn)
        .map_ok(move |row| Self {
            snapshot_id: row.get(0),
            proof: Proof::new_unchecked(writer_id, row.get(1), row.get(2), row.get(3)),
            summary: Summary {
                is_complete: row.get(4),
                missing_blocks_count: db::decode_u64(row.get(5)),
                missing_blocks_checksum: db::decode_u64(row.get(6)),
            },
        })
        .err_into()
    }

    /// Returns the replica ids of the nodes with the specified hash.
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

    /// Updates the proof of this node.
    ///
    /// Only the version vector can be updated. To update any other field of the proof, a new root
    /// node needs to be created.
    ///
    /// NOTE: this function take the transaction by value to make sure the `proof` member
    /// variable is updated only when the db UPDATE is finalized which happens when the
    /// transaction is committed which consumes the transaction.
    ///
    /// # Panics
    ///
    /// Panics if the writer_id or the hash of the new_proof differ from the ones in the current
    /// proof.
    pub async fn update_proof(
        &mut self,
        mut tx: db::Transaction<'_>,
        new_proof: Proof,
    ) -> Result<()> {
        assert_eq!(new_proof.writer_id, self.proof.writer_id);
        assert_eq!(new_proof.hash, self.proof.hash);

        sqlx::query(
            "UPDATE snapshot_root_nodes SET versions = ?, signature = ? WHERE snapshot_id = ?",
        )
        .bind(&new_proof.version_vector)
        .bind(&new_proof.signature)
        .bind(&self.snapshot_id)
        .execute(&mut tx)
        .await?;

        tx.commit().await?;

        self.proof = new_proof;

        Ok(())
    }

    /// Reload this root node from the db.
    pub async fn reload(&mut self, conn: &mut db::Connection) -> Result<()> {
        let row = sqlx::query(
            "SELECT is_complete, missing_blocks_count, missing_blocks_checksum
             FROM snapshot_root_nodes
             WHERE snapshot_id = ?",
        )
        .bind(self.snapshot_id)
        .fetch_one(conn)
        .await?;

        self.summary.is_complete = row.get(0);
        self.summary.missing_blocks_count = db::decode_u64(row.get(1));
        self.summary.missing_blocks_checksum = db::decode_u64(row.get(2));

        Ok(())
    }

    /// Updates the summaries of all nodes with the specified hash.
    pub async fn update_summaries(
        conn: &mut db::Connection,
        hash: &Hash,
    ) -> Result<SummaryUpdateStatus> {
        let summary = InnerNode::compute_summary(conn, hash).await?;

        let was_complete =
            sqlx::query("SELECT 0 FROM snapshot_root_nodes WHERE hash = ? AND is_complete = 1")
                .bind(hash)
                .fetch_optional(&mut *conn)
                .await?
                .is_some();

        sqlx::query(
            "UPDATE snapshot_root_nodes
             SET
                 is_complete = ?,
                 missing_blocks_count = ?,
                 missing_blocks_checksum = ?
             WHERE hash = ?",
        )
        .bind(summary.is_complete)
        .bind(db::encode_u64(summary.missing_blocks_count))
        .bind(db::encode_u64(summary.missing_blocks_checksum))
        .bind(hash)
        .execute(conn)
        .await?;

        Ok(SummaryUpdateStatus {
            is_complete: summary.is_complete,
            was_complete,
        })
    }

    pub async fn remove_recursive(&self, conn: &mut db::Connection) -> Result<()> {
        // This uses db triggers to delete the whole snapshot.
        sqlx::query(
            "PRAGMA recursive_triggers = ON;
             DELETE FROM snapshot_root_nodes WHERE snapshot_id = ?;
             PRAGMA recursive_triggers = OFF;",
        )
        .bind(self.snapshot_id)
        .execute(conn)
        .await?;

        Ok(())
    }
}
