mod branch_data;
mod node;
mod path;

#[cfg(test)]
pub(crate) use self::node::test_utils as node_test_utils;
pub(crate) use self::{
    branch_data::BranchData,
    node::{
        receive_block, InnerNode, InnerNodeMap, LeafNode, LeafNodeSet, RootNode, Summary,
        INNER_LAYER_COUNT,
    },
};

use crate::{
    block::BlockId,
    crypto::{Hash, Hashable, sign::PublicKey},
    db,
    error::{Error, Result},
    version_vector::VersionVector,
};
use futures_util::future;
use sqlx::Row;
use std::{
    cmp::Ordering,
    collections::{hash_map::Entry, HashMap},
    iter,
    sync::Arc,
};
use tokio::{
    select,
    sync::{watch, RwLock, RwLockReadGuard},
};

type SnapshotId = u32;

#[derive(Clone)]
pub(crate) struct Index {
    pub pool: db::Pool,
    pub this_replica_id: PublicKey,
    shared: Arc<Shared>,
}

impl Index {
    pub async fn load(pool: db::Pool, this_replica_id: PublicKey) -> Result<Self> {
        let branches = Branches::load(&pool, this_replica_id).await?;
        let (index_tx, _) = watch::channel(true);

        Ok(Self {
            pool,
            this_replica_id,
            shared: Arc::new(Shared {
                branches: RwLock::new(branches),
                index_tx,
            }),
        })
    }

    pub fn this_replica_id(&self) -> &PublicKey {
        &self.this_replica_id
    }

    pub async fn branches(&self) -> RwLockReadGuard<'_, Branches> {
        self.shared.branches.read().await
    }

    /// Subscribe to change notification from all current and future branches.
    pub fn subscribe(&self) -> Subscription {
        Subscription {
            shared: self.shared.clone(),
            branch_rxs: vec![],
            index_rx: self.shared.index_tx.subscribe(),
            version: 0,
        }
    }

    /// Notify all tasks waiting for changes on the specified branches.
    /// See also [`BranchData::subscribe`].
    pub async fn notify_branches_changed(&self, replica_ids: &[PublicKey]) {
        // Avoid the read lock
        if replica_ids.is_empty() {
            return;
        }

        let branches = self.shared.branches.read().await;
        for replica_id in replica_ids {
            if let Some(branch) = branches.get(replica_id) {
                branch.notify_changed()
            }
        }
    }

    /// Signal to all subscribers of this index that it is about to be terminated.
    pub fn close(&self) {
        self.shared.index_tx.send(false).unwrap_or(())
    }

    /// Receive `RootNode` from other replica and store it into the db. Returns whether the
    /// received node was more up-to-date than the corresponding branch stored by this replica.
    ///
    /// # Panics
    ///
    /// Panics if `replica_id` identifies this replica instead of a remote one.
    pub async fn receive_root_node(
        &self,
        replica_id: &PublicKey,
        versions: VersionVector,
        hash: Hash,
        summary: Summary,
    ) -> Result<ReceiveStatus<bool>> {
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
            return Ok(ReceiveStatus {
                updated: false,
                complete: false,
            });
        }

        // Write the root node to the db.
        let updated = self
            .has_root_node_new_blocks(replica_id, &hash, &summary)
            .await?;
        let node =
            RootNode::create(&self.pool, replica_id, versions, hash, Summary::INCOMPLETE).await?;
        let complete = self.update_summaries(hash, 0).await?;
        self.update_remote_branch(*replica_id, node).await?;

        Ok(ReceiveStatus { updated, complete })
    }

    /// Receive inner nodes from other replica and store them into the db.
    /// Returns hashes of those nodes that were more up to date than the locally stored ones.
    pub async fn receive_inner_nodes(
        &self,
        parent_hash: Hash,
        inner_layer: usize,
        nodes: InnerNodeMap,
    ) -> Result<ReceiveStatus<Vec<Hash>>> {
        let updated: Vec<_> = self
            .find_inner_nodes_with_new_blocks(&parent_hash, &nodes)
            .await?
            .map(|node| node.hash)
            .collect();

        nodes
            .into_incomplete()
            .save(&self.pool, &parent_hash)
            .await?;
        let complete = self.update_summaries(parent_hash, inner_layer).await?;

        Ok(ReceiveStatus { updated, complete })
    }

    /// Receive leaf nodes from other replica and store them into the db.
    /// Returns the ids of the blocks that the remote replica has but the local one has not.
    pub async fn receive_leaf_nodes(
        &self,
        parent_hash: Hash,
        nodes: LeafNodeSet,
    ) -> Result<ReceiveStatus<Vec<BlockId>>> {
        let updated: Vec<_> = self
            .find_leaf_nodes_with_new_blocks(&parent_hash, &nodes)
            .await?
            .map(|node| node.block_id)
            .collect();

        nodes.into_missing().save(&self.pool, &parent_hash).await?;
        let complete = self
            .update_summaries(parent_hash, INNER_LAYER_COUNT)
            .await?;

        Ok(ReceiveStatus { updated, complete })
    }

    // Check whether the remote replica has some blocks under the specified root node that the
    // local one is missing.
    async fn has_root_node_new_blocks(
        &self,
        replica_id: &PublicKey,
        hash: &Hash,
        remote_summary: &Summary,
    ) -> Result<bool> {
        if let Some(local_node) = RootNode::load(&self.pool, replica_id, hash).await? {
            Ok(!local_node
                .summary
                .is_up_to_date_with(remote_summary)
                .unwrap_or(true))
        } else {
            Ok(*hash != InnerNodeMap::default().hash())
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

    // Updates summaries of the specified nodes and all their ancestors, notifies the affected
    // branches that became complete (wasn't before the update but became after it) and returns
    // whether at least one affected branch is complete (either already was or became by
    // the update).
    async fn update_summaries(&self, hash: Hash, layer: usize) -> Result<bool> {
        let statuses = node::update_summaries(&self.pool, hash, layer).await?;

        // Find the replicas whose current snapshots became complete by this update.
        let replica_ids: Vec<_> = statuses
            .iter()
            .filter(|(_, status)| status.did_complete())
            .map(|(id, _)| *id)
            .collect();

        // Then notify them.
        self.notify_branches_changed(&replica_ids).await;

        Ok(statuses.iter().any(|(_, status)| status.is_complete))
    }

    /// Update the root node of the remote branch.
    pub(crate) async fn update_remote_branch(
        &self,
        replica_id: PublicKey,
        node: RootNode,
    ) -> Result<()> {
        let mut branches = self.shared.branches.write().await;
        let branches = &mut *branches;

        match branches.remote.entry(replica_id) {
            Entry::Vacant(entry) => {
                branches.version = branches
                    .version
                    .checked_add(1)
                    .expect("branch limit exceeded");

                entry.insert(BranchHolder {
                    branch: Arc::new(BranchData::with_root_node(replica_id, node)),
                    version: branches.version,
                });

                // If the state was set to `false` once (via `close`), it will remain `false`
                // forever.
                let state = *self.shared.index_tx.borrow();
                self.shared.index_tx.send(state).unwrap_or(());
            }
            Entry::Occupied(entry) => {
                let mut tx = self.pool.begin().await?;
                entry.get().branch.update_root(&mut tx, node).await?;
                tx.commit().await?;
            }
        }

        Ok(())
    }
}

struct Shared {
    branches: RwLock<Branches>,
    index_tx: watch::Sender<bool>,
}

/// Container for all known branches (local and remote)
pub(crate) struct Branches {
    local: Arc<BranchData>,
    remote: HashMap<PublicKey, BranchHolder>,
    // Number that gets incremented every time a new branch is created.
    version: u64,
}

struct BranchHolder {
    branch: Arc<BranchData>,
    version: u64,
}

impl Branches {
    async fn load(pool: &db::Pool, this_replica_id: PublicKey) -> Result<Self> {
        let local = Arc::new(BranchData::new(pool, this_replica_id).await?);
        let remote = load_remote_branches(pool, &this_replica_id).await?;

        Ok(Self {
            local,
            remote,
            version: 0,
        })
    }

    /// Returns a branch with the given id, if it exists.
    pub fn get(&self, replica_id: &PublicKey) -> Option<&Arc<BranchData>> {
        if self.local.id() == replica_id {
            Some(&self.local)
        } else {
            self.remote.get(replica_id).map(|holder| &holder.branch)
        }
    }

    /// Returns an iterator over all branches in this container.
    pub fn all(&self) -> impl Iterator<Item = &Arc<BranchData>> {
        iter::once(&self.local).chain(self.remote.values().map(|holder| &holder.branch))
    }

    /// Returns the local branch.
    pub fn local(&self) -> &Arc<BranchData> {
        &self.local
    }

    // Returns an iterator over the remote branches (together with their versions) that were
    // created at or after the given version.
    fn recent(&self, version: u64) -> impl Iterator<Item = (&Arc<BranchData>, u64)> {
        // TODO: this method involves linear search which can be inefficient when there is a lot of
        // branches. Consider optimizing it.

        let local = if version == 0 {
            Some((&self.local, 0))
        } else {
            None
        };

        local.into_iter().chain(
            self.remote
                .values()
                .filter(move |holder| holder.version >= version)
                .map(|holder| (&holder.branch, holder.version)),
        )
    }
}

/// Handle to receive change notification from index.
pub(crate) struct Subscription {
    shared: Arc<Shared>,
    // Receivers of change notifications from individual branches.
    branch_rxs: Vec<(PublicKey, watch::Receiver<()>)>,
    // Receiver of change notification from the whole index. The bool indicates whether `close` was
    // called on the index.
    index_rx: watch::Receiver<bool>,
    version: u64,
}

impl Subscription {
    /// Receives the next change notification. Returns the id of the changed branch. Returns `None`
    /// if the index was closed (either by calling [`Index::close`] or when the last instance of it
    /// gets dropped).
    ///
    /// If one is interested only in the close notification, it's more efficient to use
    /// [`Self::closed`].
    pub async fn recv(&mut self) -> Option<PublicKey> {
        if self.version == 0 {
            // First subscribe to the branches that already existed before this subscription was
            // created.
            self.subscribe_to_recent().await;
        }

        while *self.index_rx.borrow() {
            select! {
                id = select_branch_changed(&mut self.branch_rxs), if !self.branch_rxs.is_empty() => {
                    if let Some(id) = id {
                        return Some(id);
                    }
                },
                result = self.index_rx.changed() => {
                    if result.is_ok() {
                        self.subscribe_to_recent().await;
                    } else {
                        break;
                    }
                }
            }
        }

        None
    }

    /// Completes when the index gets closed, either by callig [`Index::close`] or when the last
    /// instance of it gets dropped. This is more efficient than using `recv` if one is interested
    /// only in the close notification.
    pub async fn closed(self) {
        let Self { mut index_rx, .. } = self;

        while *index_rx.borrow() {
            if index_rx.changed().await.is_err() {
                break;
            }
        }
    }

    async fn subscribe_to_recent(&mut self) {
        for (branch, version) in self.shared.branches.read().await.recent(self.version) {
            self.branch_rxs.push((*branch.id(), branch.subscribe()));
            self.version = self.version.max(version.saturating_add(1));

            // Emit one event for each newly created branch.
            branch.notify_changed();
        }
    }
}

/// Waits for a change notification from any of the given watch receivers.
/// Returns the id of the branch that triggered the watch, or `None` if a watch disconnected. The
/// disconnected watch is then automatically removed from the vector.
async fn select_branch_changed(
    watches: &mut Vec<(PublicKey, watch::Receiver<()>)>,
) -> Option<PublicKey> {
    assert!(!watches.is_empty());

    // TODO: `select_all` requires the futures to be `Unpin`. The easiest way to achieve that
    // is to `Box::pin` them, but perhaps there is a more efficient way that doesn't incur N
    // allocations?
    let (result, index, _) = future::select_all(
        watches
            .iter_mut()
            .map(|(id, rx)| Box::pin(async move { rx.changed().await.map(|_| id) })),
    )
    .await;

    match result {
        Ok(id) => Some(*id),
        Err(_) => {
            watches.swap_remove(index);
            None
        }
    }
}

pub(crate) struct ReceiveStatus<T> {
    // Information about the nodes that got updated.
    pub updated: T,
    // Whether at least one branch became complete by the update.
    pub complete: bool,
}

/// Returns all replica ids we know of except ours.
async fn load_other_replica_ids(
    pool: &db::Pool,
    this_replica_id: &PublicKey,
) -> Result<Vec<PublicKey>> {
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
    this_replica_id: &PublicKey,
) -> Result<HashMap<PublicKey, BranchHolder>> {
    let ids = load_other_replica_ids(pool, this_replica_id).await?;
    let mut map = HashMap::new();

    for id in ids {
        let branch = Arc::new(BranchData::new(pool, id).await?);
        map.insert(id, BranchHolder { branch, version: 0 });
    }

    Ok(map)
}

/// Initializes the index. Creates the required database schema unless already exists.
pub(crate) async fn init(pool: &db::Pool) -> Result<(), Error> {
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
    .execute(pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}
