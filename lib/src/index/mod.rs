mod branch_data;
mod node;
mod path;
mod proof;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use self::node::test_utils as node_test_utils;
pub(crate) use self::{
    branch_data::BranchData,
    node::{
        receive_block, InnerNode, InnerNodeMap, LeafNode, LeafNodeSet, RootNode, Summary,
        EMPTY_INNER_HASH,
    },
    proof::{Proof, UntrustedProof},
};

use crate::{
    block::BlockId,
    crypto::{sign::PublicKey, Hash, Hashable},
    db,
    error::{Error, Result},
    repository::RepositoryId,
    version_vector::VersionVector,
};
use sqlx::Row;
use std::{
    cmp::Ordering,
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};
use tokio::sync::{RwLock, RwLockReadGuard};

type SnapshotId = u32;

#[derive(Clone)]
pub struct Index {
    pub(crate) pool: db::Pool,
    shared: Arc<Shared>,
}

impl Index {
    pub(crate) async fn load(pool: db::Pool, repository_id: RepositoryId) -> Result<Self> {
        let (notify_tx, notify_rx) = async_broadcast::broadcast(32);
        let branches = load_branches(&mut *pool.acquire().await?, notify_tx.clone()).await?;

        Ok(Self {
            pool,
            shared: Arc::new(Shared {
                repository_id,
                branches: RwLock::new(branches),
                notify_tx,
                notify_rx: notify_rx.deactivate(),
            }),
        })
    }

    pub(crate) fn repository_id(&self) -> &RepositoryId {
        &self.shared.repository_id
    }

    pub(crate) async fn branches(&self) -> RwLockReadGuard<'_, Branches> {
        self.shared.branches.read().await
    }

    pub(crate) async fn create_branch(&self, proof: Proof) -> Result<Arc<BranchData>> {
        let mut branches = self.shared.branches.write().await;

        match branches.entry(proof.writer_id) {
            Entry::Occupied(_) => Err(Error::EntryExists),
            Entry::Vacant(entry) => {
                let root_node = RootNode::create(
                    &mut *self.pool.acquire().await?,
                    proof,
                    VersionVector::new(),
                    Summary::FULL,
                )
                .await?;

                let branch = BranchData::new(root_node, self.shared.notify_tx.clone());
                let branch = Arc::new(branch);
                entry.insert(branch.clone());

                Ok(branch)
            }
        }
    }

    /// Subscribe to change notification from all current and future branches.
    pub(crate) fn subscribe(&self) -> async_broadcast::Receiver<PublicKey> {
        self.shared.notify_rx.activate_cloned()
    }

    /// Signal to all subscribers of this index that it is about to be terminated.
    pub(crate) fn close(&self) {
        self.shared.notify_tx.close();
    }

    /// Receive `RootNode` from other replica and store it into the db. Returns whether the
    /// received node was more up-to-date than the corresponding branch stored by this replica.
    pub(crate) async fn receive_root_node(
        &self,
        proof: Proof,
        version_vector: VersionVector,
        summary: Summary,
    ) -> Result<bool> {
        let branches = self.branches().await;

        // If the received node is outdated relative to any branch we have, ignore it.
        for branch in branches.values() {
            if *branch.id() == proof.writer_id {
                // this will be checked further down.
                continue;
            }

            if version_vector < branch.root().await.versions {
                return Ok(false);
            }
        }

        // Whether to create new node. We create only if we don't have the branch yet or if the
        // received one is strictly newer than the one we have.
        let create;
        // Whether the remote replica's branch is more up-to-date than ours.
        let updated;

        if let Some(branch) = branches.get(&proof.writer_id) {
            let old_node = branch.root().await;

            match version_vector.partial_cmp(&old_node.versions) {
                Some(Ordering::Greater) => {
                    create = true;
                    updated = true;
                }
                Some(Ordering::Equal) => {
                    create = false;
                    updated = !old_node
                        .summary
                        .is_up_to_date_with(&summary)
                        .unwrap_or(true);
                }
                Some(Ordering::Less) | None => {
                    // outdated or invalid
                    create = false;
                    updated = false;
                }
            }
        } else {
            create = true;
            updated = proof.hash != *EMPTY_INNER_HASH;
        };

        // avoid deadlock
        drop(branches);

        if create {
            let hash = proof.hash;
            let node = RootNode::create(
                &mut *self.pool.acquire().await?,
                proof,
                version_vector,
                Summary::INCOMPLETE,
            )
            .await?;
            self.update_remote_branch(node).await?;
            self.update_summaries(hash).await?;
        }

        Ok(updated)
    }

    /// Receive inner nodes from other replica and store them into the db.
    /// Returns hashes of those nodes that were more up to date than the locally stored ones.
    pub(crate) async fn receive_inner_nodes(&self, nodes: InnerNodeMap) -> Result<Vec<Hash>> {
        let parent_hash = nodes.hash();
        let updated: Vec<_> = self
            .find_inner_nodes_with_new_blocks(&parent_hash, &nodes)
            .await?
            .map(|node| node.hash)
            .collect();

        nodes
            .into_incomplete()
            .save(&mut *self.pool.acquire().await?, &parent_hash)
            .await?;
        self.update_summaries(parent_hash).await?;

        Ok(updated)
    }

    /// Receive leaf nodes from other replica and store them into the db.
    /// Returns the ids of the blocks that the remote replica has but the local one has not.
    pub(crate) async fn receive_leaf_nodes(&self, nodes: LeafNodeSet) -> Result<Vec<BlockId>> {
        let parent_hash = nodes.hash();
        let updated: Vec<_> = self
            .find_leaf_nodes_with_new_blocks(&parent_hash, &nodes)
            .await?
            .map(|node| node.block_id)
            .collect();

        nodes
            .into_missing()
            .save(&mut *self.pool.acquire().await?, &parent_hash)
            .await?;
        self.update_summaries(parent_hash).await?;

        Ok(updated)
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
        let local_nodes =
            InnerNode::load_children(&mut *self.pool.acquire().await?, parent_hash).await?;

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
        let local_nodes =
            LeafNode::load_children(&mut *self.pool.acquire().await?, parent_hash).await?;

        Ok(remote_nodes
            .present()
            .filter(move |node| local_nodes.is_missing(node.locator())))
    }

    // Updates summaries of the specified nodes and all their ancestors, notifies the affected
    // branches that became complete (wasn't before the update but became after it).
    async fn update_summaries(&self, hash: Hash) -> Result<()> {
        let mut conn = self.pool.acquire().await?;

        let statuses = node::update_summaries(&mut conn, hash).await?;
        let branches = self.branches().await;

        // Reload cached root nodes of the branches whose completion status changed.
        for (id, status) in &statuses {
            if status.did_change() {
                if let Some(branch) = branches.get(id) {
                    branch.reload_root(&mut conn).await?;
                }
            }
        }

        drop(conn);
        drop(branches);

        // Find the replicas whose current snapshots became complete by this update and notify them.
        for (writer_id, status) in statuses {
            if status.did_complete() {
                broadcast(&self.shared.notify_tx, writer_id).await;
            }
        }

        Ok(())
    }

    /// Update the root node of the remote branch.
    pub(crate) async fn update_remote_branch(&self, node: RootNode) -> Result<()> {
        let mut branches = self.shared.branches.write().await;
        let writer_id = node.proof.writer_id;

        match branches.entry(writer_id) {
            Entry::Vacant(entry) => {
                entry.insert(Arc::new(BranchData::new(
                    node,
                    self.shared.notify_tx.clone(),
                )));

                broadcast(&self.shared.notify_tx, writer_id).await;
            }
            Entry::Occupied(entry) => {
                let mut tx = self.pool.begin().await?;
                entry.get().update_root(&mut tx, node).await?;
                tx.commit().await?;
            }
        };

        Ok(())
    }
}

struct Shared {
    repository_id: RepositoryId,
    branches: RwLock<Branches>,
    notify_tx: async_broadcast::Sender<PublicKey>,
    notify_rx: async_broadcast::InactiveReceiver<PublicKey>,
}

/// Container for all known branches (local and remote)
pub(crate) type Branches = HashMap<PublicKey, Arc<BranchData>>;

async fn broadcast<T: Clone>(tx: &async_broadcast::Sender<T>, value: T) {
    // don't await if there are only inactive receivers.
    // FIXME: this is racy, because all active receivers might get deactivated after this check but
    //        before the call to `broadcast`. Is there a better way?
    if tx.receiver_count() == 0 {
        return;
    }

    tx.broadcast(value).await.map(|_| ()).unwrap_or(())
}

async fn load_branches(
    conn: &mut db::Connection,
    notify_tx: async_broadcast::Sender<PublicKey>,
) -> Result<HashMap<PublicKey, Arc<BranchData>>> {
    // TODO: load the root nodes in a single query

    let rows = sqlx::query("SELECT DISTINCT writer_id FROM snapshot_root_nodes")
        .fetch_all(&mut *conn)
        .await?;

    let mut branches = HashMap::new();

    for row in rows {
        let id = row.get(0);
        let root_node = if let Some(node) = RootNode::load_latest(conn, id).await? {
            node
        } else {
            continue;
        };
        let notify_tx = notify_tx.clone();
        let branch = Arc::new(BranchData::new(root_node, notify_tx));

        branches.insert(id, branch);
    }

    Ok(branches)
}

/// Initializes the index. Creates the required database schema unless already exists.
pub(crate) async fn init(conn: &mut db::Connection) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS snapshot_root_nodes (
             snapshot_id             INTEGER PRIMARY KEY,
             writer_id               BLOB NOT NULL,
             versions                BLOB NOT NULL,

             -- Hash of the children
             hash                    BLOB NOT NULL,

             -- Signature proving the creator has write access
             signature               BLOB NOT NULL,

             -- Is this snapshot completely downloaded?
             is_complete             INTEGER NOT NULL,

             -- Summary of the missing blocks in this subree
             missing_blocks_count    INTEGER NOT NULL,
             missing_blocks_checksum INTEGER NOT NULL,

             UNIQUE(writer_id, hash)
         );

         CREATE INDEX IF NOT EXISTS index_snapshot_root_nodes_on_hash
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

         CREATE INDEX IF NOT EXISTS index_snapshot_inner_nodes_on_hash
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

         CREATE INDEX IF NOT EXISTS index_snapshot_leaf_nodes_on_block_id
             ON snapshot_leaf_nodes (block_id);

         -- Prevents creating multiple inner nodes with the same parent and bucket but different
         -- hash.
         CREATE TRIGGER IF NOT EXISTS snapshot_inner_nodes_conflict_check
         BEFORE INSERT ON snapshot_inner_nodes
         WHEN EXISTS (
             SELECT 0
             FROM snapshot_inner_nodes
             WHERE parent = new.parent
               AND bucket = new.bucket
               AND hash <> new.hash
         )
         BEGIN
             SELECT RAISE (ABORT, 'inner node conflict');
         END;

         -- Delete whole subtree if a node is deleted and there are no more nodes at the same layer
         -- with the same hash.
         -- Note this needs `PRAGMA recursive_triggers = ON` to work.
         CREATE TRIGGER IF NOT EXISTS snapshot_inner_nodes_delete_on_root_deleted
         AFTER DELETE ON snapshot_root_nodes
         WHEN NOT EXISTS (SELECT 0 FROM snapshot_root_nodes WHERE hash = old.hash)
         BEGIN
             DELETE FROM snapshot_inner_nodes WHERE parent = old.hash;
         END;

         CREATE TRIGGER IF NOT EXISTS snapshot_inner_nodes_delete_on_parent_deleted
         AFTER DELETE ON snapshot_inner_nodes
         WHEN NOT EXISTS (SELECT 0 FROM snapshot_inner_nodes WHERE hash = old.hash)
         BEGIN
             DELETE FROM snapshot_inner_nodes WHERE parent = old.hash;
         END;

         CREATE TRIGGER IF NOT EXISTS snapshot_leaf_nodes_delete_on_parent_deleted
         AFTER DELETE ON snapshot_inner_nodes
         WHEN NOT EXISTS (SELECT 0 FROM snapshot_inner_nodes WHERE hash = old.hash)
         BEGIN
             DELETE FROM snapshot_leaf_nodes WHERE parent = old.hash;
         END;

         ",
    )
    .execute(conn)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}
