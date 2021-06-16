use super::{super::SnapshotId, inner::InnerNodeMap, link::Link};
use crate::{
    crypto::{Hash, Hashable},
    db,
    error::Result,
    replica_id::ReplicaId,
    version_vector::VersionVector,
};
use futures::{Stream, TryStreamExt};
use sqlx::Row;

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct RootNode {
    pub snapshot_id: SnapshotId,
    pub versions: VersionVector,
    pub hash: Hash,
    pub is_complete: bool,
}

impl RootNode {
    /// Returns the latest root node of the specified replica. If no such node exists yet, creates
    /// it first.
    pub async fn load_latest_or_create(
        tx: &mut db::Transaction,
        replica_id: &ReplicaId,
    ) -> Result<Self> {
        let node = Self::load_latest(&mut *tx, replica_id).await?;

        if let Some(node) = node {
            Ok(node)
        } else {
            Ok(Self::create(tx, replica_id, InnerNodeMap::default().hash())
                .await?
                .0)
        }
    }

    /// Returns the latest root node of the specified replica or `None` no snapshot of that replica
    /// exists.
    pub async fn load_latest(
        tx: &mut db::Transaction,
        replica_id: &ReplicaId,
    ) -> Result<Option<Self>> {
        Self::load_all(tx, replica_id, 1).try_next().await
    }

    /// Creates a root node of the specified replica. Returns the node itself and a flag indicating
    /// whether a new node was created (`true`) or the node already existed (`false`).
    pub async fn create(
        tx: &mut db::Transaction,
        replica_id: &ReplicaId,
        hash: Hash,
    ) -> Result<(Self, bool)> {
        let is_complete = hash == InnerNodeMap::default().hash();

        let mut versions = VersionVector::new();
        versions.increment(*replica_id);

        let row = sqlx::query(
            "INSERT INTO snapshot_root_nodes (
                 replica_id, versions, hash, is_complete
             )
             VALUES (?, ?, ?, ?)
             ON CONFLICT (replica_id, hash) DO NOTHING;
             SELECT snapshot_id, versions, is_complete, CHANGES()
             FROM snapshot_root_nodes
             WHERE replica_id = ? AND hash = ?",
        )
        .bind(replica_id)
        .bind(&versions)
        .bind(&hash)
        .bind(is_complete)
        .bind(replica_id)
        .bind(&hash)
        .fetch_one(tx)
        .await?;

        Ok((
            RootNode {
                snapshot_id: row.get(0),
                versions: row.get(1),
                hash,
                is_complete: row.get(2),
            },
            row.get::<u32, _>(3) > 0,
        ))
    }

    /// Returns a stream of all (but at most `limit`) root nodes corresponding to the specified
    /// replica ordered from the most recent to the least recent.
    pub fn load_all<'a>(
        tx: &'a mut db::Transaction,
        replica_id: &'a ReplicaId,
        limit: u32,
    ) -> impl Stream<Item = Result<Self>> + 'a {
        sqlx::query(
            "SELECT snapshot_id, versions, hash, is_complete
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
            is_complete: row.get(3),
        })
        .fetch(tx)
        .err_into()
    }

    /// Mark all root nodes with the specified hash as complete.
    pub async fn set_complete(tx: &mut db::Transaction, hash: &Hash) -> Result<()> {
        sqlx::query("UPDATE snapshot_root_nodes SET is_complete = 1 WHERE hash = ?")
            .bind(hash)
            .execute(tx)
            .await?;

        Ok(())
    }

    /// Creates the next version of this root node with the specified hash.
    pub async fn next_version(&self, tx: &mut db::Transaction, hash: Hash) -> Result<Self> {
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
                 replica_id, versions, hash, is_complete
             )
             SELECT replica_id, ?, ?, is_complete
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
            is_complete: self.is_complete,
        })
    }

    /// Reload this root node from the db. Currently used only in tests.
    #[cfg(test)]
    pub async fn reload(&mut self, tx: &mut db::Transaction) -> Result<()> {
        let row = sqlx::query("SELECT is_complete FROM snapshot_root_nodes WHERE snapshot_id = ?")
            .bind(self.snapshot_id)
            .fetch_one(tx)
            .await?;

        self.is_complete = row.get(0);

        Ok(())
    }

    pub async fn remove_recursive(&self, tx: &mut db::Transaction) -> Result<()> {
        self.as_link().remove_recursive(0, tx).await
    }

    fn as_link(&self) -> Link {
        Link::ToRoot { node: self.clone() }
    }
}
