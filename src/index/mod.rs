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
    crypto::Hash,
    db,
    error::{Error, Result},
    ReplicaId,
};
use sqlx::Row;
use std::{
    collections::{HashMap, HashSet},
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

    /// Check whether the remote replica has some blocks under the specified root node that the
    /// local one is missing.
    pub(crate) async fn has_root_node_new_blocks(
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
            Ok(true)
        }
    }

    /// Filter inner nodes that the remote replica has some blocks in that the local one is missing.
    ///
    /// Assumes (but does not enforce) that `parent_hash` is the parent hash of all nodes in
    /// `remote_nodes`.
    pub(crate) async fn find_inner_nodes_with_new_blocks<'a, 'b, 'c>(
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

    /// Filter leaf nodes that the remote replica has a block for but the local one is missing it.
    ///
    /// Assumes (but does not enforce) that `parent_hash` is the parent hash of all nodes in
    /// `remote_nodes`.
    pub(crate) async fn find_leaf_nodes_with_new_blocks<'a, 'b, 'c>(
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
