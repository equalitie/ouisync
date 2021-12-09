use super::{
    super::SnapshotId,
    inner::{InnerNode, InnerNodeMap},
    summary::{Summary, SummaryUpdateStatus},
};
use crate::{
    crypto::{sign::PublicKey, Hash, Hashable},
    db,
    error::Result,
    version_vector::VersionVector,
};
use futures_util::{Stream, TryStreamExt};
use sqlx::Row;

#[derive(Clone, Eq, PartialEq, Debug)]
pub(crate) struct RootNode {
    pub snapshot_id: SnapshotId,
    pub versions: VersionVector,
    pub hash: Hash,
    pub summary: Summary,
}

impl RootNode {
    /// Returns the root node of the specified replica with the specified hash if it exists.
    pub async fn load(
        pool: &db::Pool,
        writer_id: &PublicKey,
        hash: &Hash,
    ) -> Result<Option<Self>> {
        sqlx::query(
            "SELECT
                 snapshot_id,
                 versions,
                 hash,
                 is_complete,
                 missing_blocks_count,
                 missing_blocks_checksum
             FROM snapshot_root_nodes
             WHERE replica_id = ? AND hash = ?",
        )
        .bind(writer_id)
        .bind(hash)
        .map(|row| Self {
            snapshot_id: row.get(0),
            versions: row.get(1),
            hash: row.get(2),
            summary: Summary {
                is_complete: row.get(3),
                missing_blocks_count: db::decode_u64(row.get(4)),
                missing_blocks_checksum: db::decode_u64(row.get(5)),
            },
        })
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
    }

    /// Returns the latest root node of the specified replica. If no such node exists yet, creates
    /// it first.
    pub async fn load_latest_or_create(pool: &db::Pool, writer_id: &PublicKey) -> Result<Self> {
        let node = Self::load_latest(pool, writer_id).await?;

        if let Some(node) = node {
            Ok(node)
        } else {
            Ok(Self::create(
                pool,
                writer_id,
                VersionVector::new(),
                InnerNodeMap::default().hash(),
                Summary::FULL,
            )
            .await?)
        }
    }

    /// Returns the latest root node of the specified replica or `None` if no snapshot of that
    /// replica exists.
    pub async fn load_latest(pool: &db::Pool, writer_id: &PublicKey) -> Result<Option<Self>> {
        Self::load_all(pool, writer_id, 1).try_next().await
    }

    /// Creates a root node of the specified replica unless it already exists. Returns the newly
    /// created or the existing node.
    pub async fn create(
        pool: &db::Pool,
        writer_id: &PublicKey,
        mut versions: VersionVector,
        hash: Hash,
        summary: Summary,
    ) -> Result<Self> {
        let is_complete = hash == InnerNodeMap::default().hash();

        versions.insert(*writer_id, 1);

        sqlx::query(
            "INSERT INTO snapshot_root_nodes (
                 replica_id,
                 versions,
                 hash,
                 is_complete,
                 missing_blocks_count,
                 missing_blocks_checksum
             )
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (replica_id, hash) DO NOTHING;
             SELECT
                 snapshot_id,
                 versions,
                 is_complete,
                 missing_blocks_count,
                 missing_blocks_checksum
             FROM snapshot_root_nodes
             WHERE replica_id = ? AND hash = ?",
        )
        .bind(writer_id)
        .bind(&versions)
        .bind(&hash)
        .bind(is_complete)
        .bind(db::encode_u64(summary.missing_blocks_count))
        .bind(db::encode_u64(summary.missing_blocks_checksum))
        .bind(writer_id)
        .bind(&hash)
        .map(|row| RootNode {
            snapshot_id: row.get(0),
            versions: row.get(1),
            hash,
            summary: Summary {
                is_complete: row.get(2),
                missing_blocks_count: db::decode_u64(row.get(3)),
                missing_blocks_checksum: db::decode_u64(row.get(4)),
            },
        })
        .fetch_one(pool)
        .await
        .map_err(Into::into)
    }

    /// Returns a stream of all (but at most `limit`) root nodes corresponding to the specified
    /// replica ordered from the most recent to the least recent.
    pub fn load_all<'a>(
        pool: &'a db::Pool,
        writer_id: &'a PublicKey,
        limit: u32,
    ) -> impl Stream<Item = Result<Self>> + 'a {
        sqlx::query(
            "SELECT
                 snapshot_id,
                 versions,
                 hash,
                 is_complete,
                 missing_blocks_count,
                 missing_blocks_checksum
             FROM snapshot_root_nodes
             WHERE replica_id = ?
             ORDER BY snapshot_id DESC
             LIMIT ?",
        )
        .bind(writer_id)
        .bind(limit)
        .map(|row| Self {
            snapshot_id: row.get(0),
            versions: row.get(1),
            hash: row.get(2),
            summary: Summary {
                is_complete: row.get(3),
                missing_blocks_count: db::decode_u64(row.get(4)),
                missing_blocks_checksum: db::decode_u64(row.get(5)),
            },
        })
        .fetch(pool)
        .err_into()
    }

    /// Returns the replica ids of the nodes with the specified hash.
    pub fn load_writer_ids<'a>(
        tx: &'a mut db::Transaction<'_>,
        hash: &'a Hash,
    ) -> impl Stream<Item = Result<PublicKey>> + 'a {
        sqlx::query("SELECT replica_id FROM snapshot_root_nodes WHERE hash = ?")
            .bind(hash)
            .map(|row| row.get(0))
            .fetch(tx)
            .err_into()
    }

    /// Creates the next version of this root node with the specified hash.
    pub async fn next_version(&self, tx: &mut db::Transaction<'_>, hash: Hash) -> Result<Self> {
        let writer_id = self.load_writer_id(&mut *tx).await?;
        let versions = self.versions.clone().incremented(writer_id);

        let snapshot_id = sqlx::query(
            "INSERT INTO snapshot_root_nodes (
                 replica_id,
                 versions,
                 hash,
                 is_complete,
                 missing_blocks_count,
                 missing_blocks_checksum
             )
             SELECT replica_id, ?, ?, 1, 0, 0
             FROM snapshot_root_nodes
             WHERE snapshot_id = ?
             RETURNING snapshot_id",
        )
        .bind(&versions)
        .bind(&hash)
        .bind(self.snapshot_id)
        .fetch_one(tx)
        .await?
        .get(0);

        Ok(Self {
            snapshot_id,
            versions,
            hash,
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
            let writer_id = self.load_writer_id(&mut tx).await?;
            new_version_vector.increment(writer_id);
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

    /// Reload this root node from the db. Currently used only in tests.
    #[cfg(test)]
    pub async fn reload(&mut self, pool: &db::Pool) -> Result<()> {
        let row = sqlx::query(
            "SELECT is_complete, missing_blocks_count, missing_blocks_checksum
             FROM snapshot_root_nodes
             WHERE snapshot_id = ?",
        )
        .bind(self.snapshot_id)
        .fetch_one(pool)
        .await?;

        self.summary.is_complete = row.get(0);
        self.summary.missing_blocks_count = db::decode_u64(row.get(1));
        self.summary.missing_blocks_checksum = db::decode_u64(row.get(2));

        Ok(())
    }

    /// Updates the summaries of all nodes with the specified hash.
    pub async fn update_summaries(
        tx: &mut db::Transaction<'_>,
        hash: &Hash,
    ) -> Result<SummaryUpdateStatus> {
        let summary = InnerNode::compute_summary(tx, hash, 0).await?;

        let was_complete =
            sqlx::query("SELECT 0 FROM snapshot_root_nodes WHERE hash = ? AND is_complete = 1")
                .bind(hash)
                .fetch_optional(&mut *tx)
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
        .execute(tx)
        .await?;

        Ok(SummaryUpdateStatus {
            is_complete: summary.is_complete,
            was_complete,
        })
    }

    pub async fn remove_recursive(&self, tx: &mut db::Transaction<'_>) -> Result<()> {
        // This uses db triggers to delete the whole snapshot.
        sqlx::query(
            "PRAGMA recursive_triggers = ON;
             DELETE FROM snapshot_root_nodes WHERE snapshot_id = ?;
             PRAGMA recursive_triggers = OFF;",
        )
        .bind(self.snapshot_id)
        .execute(tx)
        .await?;

        Ok(())
    }

    /// Returns the replica id of this node
    async fn load_writer_id(&self, tx: &mut db::Transaction<'_>) -> Result<PublicKey> {
        let writer_id =
            sqlx::query("SELECT replica_id FROM snapshot_root_nodes WHERE snapshot_id = ?")
                .bind(&self.snapshot_id)
                .fetch_one(tx)
                .await?
                .get(0);
        Ok(writer_id)
    }
}
