use super::{
    super::SnapshotId,
    inner::{InnerNode, InnerNodeMap},
    link::Link,
    summary::Summary,
};
use crate::{
    crypto::{Hash, Hashable},
    db,
    error::Result,
    replica_id::ReplicaId,
    version_vector::VersionVector,
};
use futures_util::{Stream, TryStreamExt};
use sqlx::Row;

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct RootNode {
    pub snapshot_id: SnapshotId,
    pub versions: VersionVector,
    pub hash: Hash,
    pub summary: Summary,
}

impl RootNode {
    /// Returns the root node of the specified replica with the specified hash if it exists.
    pub async fn load(
        pool: &db::Pool,
        replica_id: &ReplicaId,
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
        .bind(replica_id)
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
    pub async fn load_latest_or_create(pool: &db::Pool, replica_id: &ReplicaId) -> Result<Self> {
        let node = Self::load_latest(pool, replica_id).await?;

        if let Some(node) = node {
            Ok(node)
        } else {
            Ok(Self::create(
                pool,
                replica_id,
                VersionVector::new(),
                InnerNodeMap::default().hash(),
                Summary::FULL,
            )
            .await?)
        }
    }

    /// Returns the latest root node of the specified replica or `None` if no snapshot of that
    /// replica exists.
    pub async fn load_latest(pool: &db::Pool, replica_id: &ReplicaId) -> Result<Option<Self>> {
        Self::load_all(pool, replica_id, 1).try_next().await
    }

    /// Creates a root node of the specified replica unless it already exists. Returns the newly
    /// created or the existing node.
    pub async fn create(
        pool: &db::Pool,
        replica_id: &ReplicaId,
        mut versions: VersionVector,
        hash: Hash,
        summary: Summary,
    ) -> Result<Self> {
        let is_complete = hash == InnerNodeMap::default().hash();

        versions.insert(*replica_id, 1);

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
        .bind(replica_id)
        .bind(&versions)
        .bind(&hash)
        .bind(is_complete)
        .bind(db::encode_u64(summary.missing_blocks_count))
        .bind(db::encode_u64(summary.missing_blocks_checksum))
        .bind(replica_id)
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
        replica_id: &'a ReplicaId,
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
        .bind(replica_id)
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
    pub fn load_replica_ids<'a>(
        tx: &'a mut db::Transaction<'_>,
        hash: &'a Hash,
    ) -> impl Stream<Item = Result<ReplicaId>> + 'a {
        sqlx::query("SELECT replica_id FROM snapshot_root_nodes WHERE hash = ?")
            .bind(hash)
            .map(|row| row.get(0))
            .fetch(tx)
            .err_into()
    }

    /// Creates the next version of this root node with the specified hash.
    pub async fn next_version(&self, tx: &mut db::Transaction<'_>, hash: Hash) -> Result<Self> {
        let replica_id =
            sqlx::query("SELECT replica_id FROM snapshot_root_nodes WHERE snapshot_id = ?")
                .bind(&self.snapshot_id)
                .fetch_one(&mut *tx)
                .await?
                .get(0);

        let mut versions = self.versions.clone();
        versions.increment(replica_id);

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
    pub async fn update_summaries(tx: &mut db::Transaction<'_>, hash: &Hash) -> Result<()> {
        let summary = InnerNode::compute_summary(tx, hash, 0).await?;

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

        Ok(())
    }

    pub async fn remove_recursive(&self, tx: &mut db::Transaction<'_>) -> Result<()> {
        self.as_link().remove_recursive(0, tx).await
    }

    fn as_link(&self) -> Link {
        Link::ToRoot { node: self.clone() }
    }
}
