use super::{
    super::{proof::Proof, SnapshotId},
    inner::InnerNode,
    summary::{Summary, SummaryUpdateStatus},
};
use crate::{
    crypto::{sign::PublicKey, Hash},
    db,
    error::Result,
    version_vector::VersionVector,
};
use futures_util::{Stream, TryStreamExt};
use sqlx::Row;

#[derive(Clone, Eq, PartialEq, Debug)]
pub(crate) struct RootNode {
    pub snapshot_id: SnapshotId,
    pub proof: Proof,
    pub versions: VersionVector,
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

    /// Creates a root node of the specified replica unless it already exists. Returns the newly
    /// created or the existing node.
    pub async fn create(
        conn: &mut db::Connection,
        proof: Proof,
        mut versions: VersionVector,
        summary: Summary,
    ) -> Result<Self> {
        // TODO: shouldn't we start with empty vv?
        versions.insert(proof.writer_id, 1);

        sqlx::query(
            "INSERT INTO snapshot_root_nodes (
                 writer_id,
                 hash,
                 versions,
                 is_complete,
                 missing_blocks_count,
                 missing_blocks_checksum
             )
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (writer_id, hash) DO NOTHING;
             SELECT
                 snapshot_id,
                 versions,
                 is_complete,
                 missing_blocks_count,
                 missing_blocks_checksum
             FROM snapshot_root_nodes
             WHERE writer_id = ? AND hash = ?",
        )
        .bind(&proof.writer_id)
        .bind(&proof.hash)
        .bind(&versions)
        .bind(summary.is_complete)
        .bind(db::encode_u64(summary.missing_blocks_count))
        .bind(db::encode_u64(summary.missing_blocks_checksum))
        .bind(&proof.writer_id)
        .bind(&proof.hash)
        .map(|row| Self {
            snapshot_id: row.get(0),
            versions: row.get(1),
            proof,
            summary: Summary {
                is_complete: row.get(2),
                missing_blocks_count: db::decode_u64(row.get(3)),
                missing_blocks_checksum: db::decode_u64(row.get(4)),
            },
        })
        .fetch_one(conn)
        .await
        .map_err(Into::into)
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
                 hash,
                 versions,
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
            proof: Proof::new(writer_id, row.get(1)),
            versions: row.get(2),
            summary: Summary {
                is_complete: row.get(3),
                missing_blocks_count: db::decode_u64(row.get(4)),
                missing_blocks_checksum: db::decode_u64(row.get(5)),
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

    /// Creates the next version of this root node with the specified proof.
    ///
    /// # Panics
    ///
    /// Panics if the writer_id of the current proof differs from the writer_id of the new proof.
    pub async fn next_version(&self, tx: &mut db::Transaction<'_>, proof: Proof) -> Result<Self> {
        assert_eq!(proof.writer_id, self.proof.writer_id);

        let versions = self.versions.clone().incremented(self.proof.writer_id);

        let snapshot_id = sqlx::query(
            "INSERT INTO snapshot_root_nodes (
                 writer_id,
                 hash,
                 versions,
                 is_complete,
                 missing_blocks_count,
                 missing_blocks_checksum
             )
             VALUES (?, ?, ?, 1, 0, 0)
             RETURNING snapshot_id",
        )
        .bind(&proof.writer_id)
        .bind(&proof.hash)
        .bind(&versions)
        .fetch_one(tx)
        .await?
        .get(0);

        Ok(Self {
            snapshot_id,
            proof,
            versions,
            summary: Summary::FULL,
        })
    }

    /// Updates the version vector of this node.
    /// If `version_vector_override` is `None`, the local counter of the current version vector is
    /// incremented by one. If it is `Some`, the current version vector is merged with the
    /// specified one.
    ///
    /// NOTE: this function take the transaction by value to make sure the version vector member
    /// variable is updated only when the db query succeeds, to keep things in sync.
    pub async fn update_version_vector(
        &mut self,
        mut tx: db::Transaction<'_>,
        version_vector_override: Option<&VersionVector>,
    ) -> Result<()> {
        let mut new_version_vector = self.versions.clone();

        if let Some(version_vector_override) = version_vector_override {
            new_version_vector.merge(version_vector_override);
        } else {
            new_version_vector.increment(self.proof.writer_id);
        }

        sqlx::query("UPDATE snapshot_root_nodes SET versions = ? WHERE snapshot_id = ?")
            .bind(&new_version_vector)
            .bind(&self.snapshot_id)
            .execute(&mut tx)
            .await?;

        tx.commit().await?;

        self.versions = new_version_vector;

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
        let summary = InnerNode::compute_summary(conn, hash, 0).await?;

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
