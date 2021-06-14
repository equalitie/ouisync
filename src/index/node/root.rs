use super::{
    super::{SnapshotId, INNER_LAYER_COUNT},
    inner::{InnerNode, InnerNodeMap},
    leaf::{LeafNode, LeafNodeSet},
    link::Link,
};
use crate::{
    crypto::{Hash, Hashable},
    db,
    error::Result,
    replica_id::ReplicaId,
};
use async_recursion::async_recursion;
use futures::{Stream, TryStreamExt};
use sqlx::Row;

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct RootNode {
    pub snapshot_id: SnapshotId,
    pub hash: Hash,
    is_complete: bool,
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
        let row = sqlx::query(
            "INSERT INTO snapshot_root_nodes (replica_id, hash, is_complete)
             VALUES (?, ?, 0)
             ON CONFLICT (replica_id, hash) DO NOTHING;
             SELECT snapshot_id, is_complete, CHANGES()
             FROM snapshot_root_nodes
             WHERE replica_id = ? AND hash = ?",
        )
        .bind(replica_id)
        .bind(&hash)
        .bind(replica_id)
        .bind(&hash)
        .fetch_one(tx)
        .await?;

        Ok((
            RootNode {
                snapshot_id: row.get(0),
                hash,
                is_complete: row.get(1),
            },
            row.get::<u32, _>(2) > 0,
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
            "SELECT snapshot_id, hash, is_complete
             FROM snapshot_root_nodes
             WHERE replica_id = ?
             ORDER BY snapshot_id DESC
             LIMIT ?",
        )
        .bind(replica_id)
        .bind(limit)
        .map(|row| Self {
            snapshot_id: row.get(0),
            hash: row.get(1),
            is_complete: row.get(2),
        })
        .fetch(tx)
        .err_into()
    }

    pub async fn clone_with_new_hash(&self, tx: &mut db::Transaction, hash: Hash) -> Result<Self> {
        let snapshot_id = sqlx::query(
            "INSERT INTO snapshot_root_nodes (replica_id, hash, is_complete)
             SELECT replica_id, ?, ?
             FROM snapshot_root_nodes
             WHERE snapshot_id = ?
             RETURNING snapshot_id",
        )
        .bind(&hash)
        .bind(self.is_complete)
        .bind(self.snapshot_id)
        .fetch_one(tx)
        .await?
        .get(0);

        Ok(Self {
            snapshot_id,
            hash,
            is_complete: self.is_complete,
        })
    }

    pub async fn update(&self, tx: &mut db::Transaction) -> Result<()> {
        if !self.is_complete {
            return Ok(());
        }

        sqlx::query(
            "UPDATE snapshot_root_nodes
             SET is_complete = 1
             WHERE snapshot_id = ? AND is_complete = 0",
        )
        .bind(self.snapshot_id)
        .execute(tx)
        .await?;

        Ok(())
    }

    /// Check whether all the constituent nodes of this snapshot have been downloaded and stored
    /// locally.
    pub async fn check_complete(&mut self, tx: &mut db::Transaction) -> Result<bool> {
        if self.is_complete {
            Ok(true)
        } else if check_subtree_complete(tx, &self.hash, 0).await? {
            self.is_complete = true;
            self.update(tx).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn remove_recursive(&self, tx: &mut db::Transaction) -> Result<()> {
        self.as_link().remove_recursive(0, tx).await
    }

    fn as_link(&self) -> Link {
        Link::ToRoot { node: self.clone() }
    }
}

// TODO: find a way to avoid the recursion
#[async_recursion]
async fn check_subtree_complete(
    tx: &mut db::Transaction,
    parent_hash: &Hash,
    layer: usize,
) -> Result<bool> {
    if layer < INNER_LAYER_COUNT {
        let nodes = InnerNode::load_children(tx, parent_hash).await?;
        let mut complete_children_count = 0usize;

        for (bucket, node) in &nodes {
            if node.is_complete {
                complete_children_count += 1;
            } else if check_subtree_complete(tx, &node.hash, layer + 1).await? {
                let node = InnerNode {
                    hash: node.hash,
                    is_complete: true,
                };
                node.save(tx, parent_hash, bucket).await?;

                complete_children_count += 1;
            }
        }

        if nodes.is_empty() {
            // If the child nodes are empty, there are two possibilities: either there are no
            // children or they haven't been downloaded yet. We distinguish between them by checking
            // whether the parent hash is equal to the hash of empty child node collection.
            Ok(*parent_hash == InnerNodeMap::default().hash())
            // If the child nodes are not empty, we assume we already have all of them because we
            // check their hash against the parent hash when we download them and accept them only
            // when they match. This assumptions allows us to skip the potentially expensive hash
            // calculation here.
        } else {
            Ok(complete_children_count == nodes.len())
        }
    } else {
        let nodes = LeafNode::load_children(tx, parent_hash).await?;

        if nodes.is_empty() {
            // Same as in the inner nodes case, we need to distinguish between the case of not
            // having the child nodes and them not being downloaded yet.
            Ok(*parent_hash == LeafNodeSet::default().hash())
        } else {
            // Same here - if we have at least one, we have them all.
            Ok(true)
        }
    }
}
