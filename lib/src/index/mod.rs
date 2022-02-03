mod branch_data;
mod node;
mod path;
mod proof;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use self::node::test_utils as node_test_utils;
use self::proof::ProofError;
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
    debug_printer::DebugPrinter,
    error::{Error, Result},
    repository::RepositoryId,
};
use futures_util::TryStreamExt;
use std::{
    cmp::Ordering,
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};
use thiserror::Error;
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
                let root_node =
                    RootNode::create(&mut *self.pool.acquire().await?, proof, Summary::FULL)
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
        proof: UntrustedProof,
        summary: Summary,
    ) -> Result<bool, ReceiveError> {
        let proof = proof.verify(self.repository_id())?;
        let branches = self.branches().await;

        // If the received node is outdated relative to any branch we have, ignore it.
        for branch in branches.values() {
            if *branch.id() == proof.writer_id {
                // this will be checked further down.
                continue;
            }

            if proof.version_vector < branch.root().await.proof.version_vector {
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

            match proof
                .version_vector
                .partial_cmp(&old_node.proof.version_vector)
            {
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

        // Prevent deadlock.
        drop(branches);

        if create {
            let mut conn = self.pool.acquire().await?;
            let hash = proof.hash;

            match RootNode::create(&mut conn, proof, Summary::INCOMPLETE).await {
                Ok(_) => self.update_summaries(&mut conn, hash).await?,
                Err(Error::EntryExists) => (), // ignore duplicate nodes but don't fail.
                Err(error) => return Err(error.into()),
            }
        }

        Ok(updated)
    }

    pub(crate) async fn debug_print(&self, print: DebugPrinter) {
        let mut conn = self.pool.acquire().await.unwrap();
        RootNode::debug_print(&mut conn, print).await;
    }

    /// Receive inner nodes from other replica and store them into the db.
    /// Returns hashes of those nodes that were more up to date than the locally stored ones.
    pub(crate) async fn receive_inner_nodes(
        &self,
        nodes: InnerNodeMap,
    ) -> Result<Vec<Hash>, ReceiveError> {
        let mut conn = self.pool.acquire().await?;
        let parent_hash = nodes.hash();

        self.check_parent_node_exists(&mut conn, &parent_hash)
            .await?;

        let updated: Vec<_> = self
            .find_inner_nodes_with_new_blocks(&mut conn, &parent_hash, &nodes)
            .await?
            .map(|node| node.hash)
            .collect();

        nodes
            .into_incomplete()
            .save(&mut conn, &parent_hash)
            .await?;
        self.update_summaries(&mut conn, parent_hash).await?;

        Ok(updated)
    }

    /// Receive leaf nodes from other replica and store them into the db.
    /// Returns the ids of the blocks that the remote replica has but the local one has not.
    pub(crate) async fn receive_leaf_nodes(
        &self,
        nodes: LeafNodeSet,
    ) -> Result<Vec<BlockId>, ReceiveError> {
        let mut conn = self.pool.acquire().await?;
        let parent_hash = nodes.hash();

        self.check_parent_node_exists(&mut conn, &parent_hash)
            .await?;

        let updated: Vec<_> = self
            .find_leaf_nodes_with_new_blocks(&mut conn, &parent_hash, &nodes)
            .await?
            .map(|node| node.block_id)
            .collect();

        nodes.into_missing().save(&mut conn, &parent_hash).await?;
        self.update_summaries(&mut conn, parent_hash).await?;

        Ok(updated)
    }

    // Filter inner nodes that the remote replica has some blocks in that the local one is missing.
    //
    // Assumes (but does not enforce) that `parent_hash` is the parent hash of all nodes in
    // `remote_nodes`.
    async fn find_inner_nodes_with_new_blocks<'a>(
        &self,
        conn: &mut db::Connection,
        parent_hash: &Hash,
        remote_nodes: &'a InnerNodeMap,
    ) -> Result<impl Iterator<Item = &'a InnerNode>> {
        let local_nodes = InnerNode::load_children(conn, parent_hash).await?;

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
    async fn find_leaf_nodes_with_new_blocks<'a>(
        &self,
        conn: &mut db::Connection,
        parent_hash: &Hash,
        remote_nodes: &'a LeafNodeSet,
    ) -> Result<impl Iterator<Item = &'a LeafNode>> {
        let local_nodes = LeafNode::load_children(conn, parent_hash).await?;

        Ok(remote_nodes
            .present()
            .filter(move |node| local_nodes.is_missing(node.locator())))
    }

    // Updates summaries of the specified nodes and all their ancestors, notifies the affected
    // branches that became complete (wasn't before the update but became after it).
    async fn update_summaries(&self, conn: &mut db::Connection, hash: Hash) -> Result<()> {
        let statuses = node::update_summaries(conn, hash).await?;

        for (id, complete) in statuses {
            if complete {
                self.update_root_node(conn, id).await?;
            } else {
                self.reload_root_node(conn, &id).await?;
            }
        }

        Ok(())
    }

    async fn update_root_node(
        &self,
        conn: &mut db::Connection,
        writer_id: PublicKey,
    ) -> Result<()> {
        let node = RootNode::load_latest_complete_by_writer(conn, writer_id).await?;

        let created = match self.shared.branches.write().await.entry(writer_id) {
            Entry::Vacant(entry) => {
                // We could have accumulated a bunch of incomplete root nodes before this
                // particular one became complete. We want to remove those.
                node.remove_recursively_all_older(conn).await?;

                entry.insert(Arc::new(BranchData::new(
                    node,
                    self.shared.notify_tx.clone(),
                )));
                true
            }
            Entry::Occupied(entry) => {
                entry.get().update_root(conn, node).await?;
                false
            }
        };

        if created {
            broadcast(&self.shared.notify_tx, writer_id).await;
        }

        Ok(())
    }

    async fn reload_root_node(
        &self,
        conn: &mut db::Connection,
        writer_id: &PublicKey,
    ) -> Result<()> {
        if let Some(branch) = self.shared.branches.read().await.get(writer_id) {
            branch.reload_root(conn).await?;
        }

        Ok(())
    }

    async fn check_parent_node_exists(
        &self,
        conn: &mut db::Connection,
        hash: &Hash,
    ) -> Result<(), ReceiveError> {
        if node::parent_exists(conn, hash).await? {
            Ok(())
        } else {
            Err(ReceiveError::ParentNodeNotFound)
        }
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
    RootNode::load_all_latest_complete(conn)
        .map_ok(|node| {
            let writer_id = node.proof.writer_id;
            let branch = Arc::new(BranchData::new(node, notify_tx.clone()));

            (writer_id, branch)
        })
        .try_collect()
        .await
}

#[derive(Debug, Error)]
pub(crate) enum ReceiveError {
    #[error("proof is invalid")]
    InvalidProof,
    #[error("parent node not found")]
    ParentNodeNotFound,
    #[error("fatal error")]
    Fatal(#[from] Error),
}

impl From<ProofError> for ReceiveError {
    fn from(_: ProofError) -> Self {
        Self::InvalidProof
    }
}

impl From<sqlx::Error> for ReceiveError {
    fn from(error: sqlx::Error) -> Self {
        Self::from(Error::from(error))
    }
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
