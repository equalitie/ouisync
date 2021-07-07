mod branch;
mod node;
mod path;

#[cfg(test)]
pub use self::node::test_utils as node_test_utils;
pub use self::{
    branch::Branch,
    node::{
        receive_block, update_summaries, InnerNode, InnerNodeMap, LeafNode, LeafNodeSet, RootNode,
        Summary, INNER_LAYER_COUNT,
    },
};

use crate::{
    block::BlockId,
    crypto::Hash,
    db,
    error::{Error, Result},
    version_vector::VersionVector,
    ReplicaId,
};
use sqlx::Row;
use std::{
    cmp::Ordering,
    collections::{hash_map::Entry, HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::Mutex;

type SnapshotId = u32;

#[derive(Clone)]
pub struct Index {
    pub pool: db::Pool,
    pub this_replica_id: ReplicaId,
    branches: Arc<Mutex<Branches>>,
}

impl Index {
    pub async fn load(pool: db::Pool, this_replica_id: ReplicaId) -> Result<Self> {
        let local = Branch::new(&pool, this_replica_id).await?;
        let remote = load_remote_branches(&pool, &this_replica_id).await?;

        let branches = Branches { local, remote };

        Ok(Self {
            pool,
            this_replica_id,
            branches: Arc::new(Mutex::new(branches)),
        })
    }

    pub(crate) async fn local_branch(&self) -> Branch {
        self.branches.lock().await.local.clone()
    }

    /// Notify all tasks waiting for changes on the specified branches.
    /// See also [`Branch::subscribe`].
    pub(crate) async fn notify_branches_changed(&self, replica_ids: &HashSet<ReplicaId>) {
        let branches = self.branches.lock().await;
        for replica_id in replica_ids {
            if let Some(branch) = branches.get(replica_id) {
                branch.notify_changed()
            }
        }
    }

    /// Receive `RootNode` from other replica and store it into the db. Returns whether the
    /// received node was more up-to-date than the corresponding branch stored by this replica.
    ///
    /// # Panics
    ///
    /// Panics if `replica_id` identifies this replica instead of a remote one.
    pub(crate) async fn receive_root_node(
        &self,
        replica_id: &ReplicaId,
        versions: VersionVector,
        hash: Hash,
        summary: Summary,
    ) -> Result<bool> {
        assert_ne!(replica_id, &self.this_replica_id);

        // Only accept it if newer or concurrent with our versions.
        let this_versions = RootNode::load_latest(&self.pool, &self.this_replica_id)
            .await?
            .map(|node| node.versions)
            .unwrap_or_default();

        if versions
            .partial_cmp(&this_versions)
            .map(Ordering::is_le)
            .unwrap_or(false)
        {
            return Ok(false);
        }

        // Write the root node to the db.
        let updated = self
            .has_root_node_new_blocks(replica_id, &hash, &summary)
            .await?;
        let node =
            RootNode::create(&self.pool, replica_id, versions, hash, Summary::INCOMPLETE).await?;
        node::update_summaries(&self.pool, hash, 0).await?;

        // Update the remote branch with the new root.
        let mut branches = self.branches.lock().await;
        match branches.remote.entry(*replica_id) {
            Entry::Vacant(entry) => {
                entry.insert(Branch::with_root_node(*replica_id, node));
            }
            Entry::Occupied(entry) => entry.get().update_root(node).await,
        }

        Ok(updated)
    }

    /// Receive inner nodes from other replica and store them into the db.
    /// Returns hashes of those nodes that were more up to date than the locally stored ones.
    pub(crate) async fn receive_inner_nodes(
        &self,
        parent_hash: Hash,
        inner_layer: usize,
        nodes: InnerNodeMap,
    ) -> Result<Vec<Hash>> {
        let updated: Vec<_> = self
            .find_inner_nodes_with_new_blocks(&parent_hash, &nodes)
            .await?
            .map(|node| node.hash)
            .collect();

        nodes
            .into_incomplete()
            .save(&self.pool, &parent_hash)
            .await?;
        node::update_summaries(&self.pool, parent_hash, inner_layer).await?;

        Ok(updated)
    }

    /// Receive leaf nodes from other replica and store them into the db.
    /// Returns the ids of the blocks that the remote replica has but the local one has not.
    pub(crate) async fn receive_leaf_nodes(
        &self,
        parent_hash: Hash,
        nodes: LeafNodeSet,
    ) -> Result<Vec<BlockId>> {
        let updated: Vec<_> = self
            .find_leaf_nodes_with_new_blocks(&parent_hash, &nodes)
            .await?
            .map(|node| node.block_id)
            .collect();

        nodes.into_missing().save(&self.pool, &parent_hash).await?;
        node::update_summaries(&self.pool, parent_hash, INNER_LAYER_COUNT).await?;

        Ok(updated)
    }

    // Check whether the remote replica has some blocks under the specified root node that the
    // local one is missing.
    async fn has_root_node_new_blocks(
        &self,
        replica_id: &ReplicaId,
        hash: &Hash,
        remote_summary: &Summary,
    ) -> Result<bool> {
        if let Some(local_node) = RootNode::load(&self.pool, replica_id, hash).await? {
            Ok(!local_node
                .summary
                .is_up_to_date_with(&remote_summary)
                .unwrap_or(true))
        } else {
            // TODO: if hash is of empty nodes collection, return false
            Ok(true)
        }
    }

    // Filter inner nodes that the remote replica has some blocks in that the local one is missing.
    //
    // Assumes (but does not enforce) that `parent_hash` is the parent hash of all nodes in
    // `remote_nodes`.
    async fn find_inner_nodes_with_new_blocks<'a, 'b, 'c>(
        &'a self,
        parent_hash: &'b Hash,
        remote_nodes: &'c InnerNodeMap,
    ) -> Result<impl Iterator<Item = &'c InnerNode>> {
        let local_nodes = InnerNode::load_children(&self.pool, parent_hash).await?;

        Ok(remote_nodes
            .iter()
            .filter(move |(bucket, remote_node)| {
                let local_node = if let Some(node) = local_nodes.get(*bucket) {
                    node
                } else {
                    // node not present locally - we implicitly treat this as if the local replica
                    // had zero blocks under this node.
                    return true;
                };

                !local_node
                    .summary
                    .is_up_to_date_with(&remote_node.summary)
                    .unwrap_or(true)
            })
            .map(|(_, node)| node))
    }

    // Filter leaf nodes that the remote replica has a block for but the local one is missing it.
    //
    // Assumes (but does not enforce) that `parent_hash` is the parent hash of all nodes in
    // `remote_nodes`.
    async fn find_leaf_nodes_with_new_blocks<'a, 'b, 'c>(
        &'a self,
        parent_hash: &'b Hash,
        remote_nodes: &'c LeafNodeSet,
    ) -> Result<impl Iterator<Item = &'c LeafNode>> {
        let local_nodes = LeafNode::load_children(&self.pool, parent_hash).await?;

        Ok(remote_nodes
            .present()
            .filter(move |node| local_nodes.is_missing(node.locator())))
    }
}

struct Branches {
    local: Branch,
    remote: HashMap<ReplicaId, Branch>,
}

impl Branches {
    fn get(&self, replica_id: &ReplicaId) -> Option<&Branch> {
        if self.local.replica_id() == replica_id {
            Some(&self.local)
        } else {
            self.remote.get(replica_id)
        }
    }
}

/// Returns all replica ids we know of except ours.
async fn load_other_replica_ids(
    pool: &db::Pool,
    this_replica_id: &ReplicaId,
) -> Result<Vec<ReplicaId>> {
    Ok(
        sqlx::query("SELECT DISTINCT replica_id FROM snapshot_root_nodes WHERE replica_id <> ?")
            .bind(this_replica_id)
            .map(|row| row.get(0))
            .fetch_all(pool)
            .await?,
    )
}

async fn load_remote_branches(
    pool: &db::Pool,
    this_replica_id: &ReplicaId,
) -> Result<HashMap<ReplicaId, Branch>> {
    let ids = load_other_replica_ids(pool, this_replica_id).await?;
    let mut map = HashMap::new();

    for id in ids {
        let branch = Branch::new(pool, id).await?;
        map.insert(id, branch);
    }

    Ok(map)
}

/// Initializes the index. Creates the required database schema unless already exists.
pub async fn init(pool: &db::Pool) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS snapshot_root_nodes (
             snapshot_id             INTEGER PRIMARY KEY,
             replica_id              BLOB NOT NULL,
             versions                BLOB NOT NULL,

             -- Hash of the children
             hash                    BLOB NOT NULL,

             -- Is this snapshot completely downloaded?
             is_complete             INTEGER NOT NULL,

             -- Summary of the missing blocks in this subree
             missing_blocks_count    INTEGER NOT NULL,
             missing_blocks_checksum INTEGER NOT NULL,

             UNIQUE(replica_id, hash)
         );

         CREATE INDEX index_snapshot_root_nodes_on_hash
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

         CREATE INDEX index_snapshot_inner_nodes_on_hash
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

         CREATE INDEX index_snapshot_leaf_nodes_on_block_id
             ON snapshot_leaf_nodes (block_id);

         -- Prevents creating multiple inner nodes with the same parent and bucket but different
         -- hash.
         CREATE TRIGGER IF NOT EXISTS snapshot_inner_nodes_conflict_check
         BEFORE INSERT ON snapshot_inner_nodes
         WHEN EXISTS(
             SELECT 0
             FROM snapshot_inner_nodes
             WHERE parent = new.parent
               AND bucket = new.bucket
               AND hash <> new.hash
         )
         BEGIN
             SELECT RAISE (ABORT, 'inner node conflict');
         END;",
    )
    .execute(pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}
