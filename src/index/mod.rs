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
    crypto::{Hash, Hashable},
    db,
    error::{Error, Result},
    version_vector::VersionVector,
    ReplicaId,
};
use futures_util::future;
use sqlx::Row;
use std::{
    cmp::Ordering,
    collections::{hash_map::Entry, HashMap, HashSet},
    iter,
    sync::Arc,
};
use tokio::{
    select,
    sync::{watch, RwLock, RwLockReadGuard},
};

type SnapshotId = u32;

#[derive(Clone)]
pub struct Index {
    pub pool: db::Pool,
    pub this_replica_id: ReplicaId,
    shared: Arc<Shared>,
}

impl Index {
    pub async fn load(pool: db::Pool, this_replica_id: ReplicaId) -> Result<Self> {
        let branches = Branches::load(&pool, this_replica_id).await?;
        let (branch_created_tx, branch_created_rx) = watch::channel(());

        Ok(Self {
            pool,
            this_replica_id,
            shared: Arc::new(Shared {
                branches: RwLock::new(branches),
                branch_created_tx,
                branch_created_rx,
            }),
        })
    }

    pub(crate) fn this_replica_id(&self) -> &ReplicaId {
        &self.this_replica_id
    }

    pub(crate) async fn branches(&self) -> RwLockReadGuard<'_, Branches> {
        self.shared.branches.read().await
    }

    /// Subscribe to change notification from all current and future branches.
    pub(crate) fn subscribe(&self) -> Subscription {
        Subscription {
            shared: self.shared.clone(),
            branch_changed_rxs: vec![],
            branch_created_rx: self.shared.branch_created_rx.clone(),
            version: 0,
        }
    }

    /// Notify all tasks waiting for changes on the specified branches.
    /// See also [`BranchData::subscribe`].
    pub(crate) async fn notify_branches_changed(&self, replica_ids: &HashSet<ReplicaId>) {
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
        self.update_summaries(hash, 0).await?;
        self.update_remote_branch(*replica_id, node).await;

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
        self.update_summaries(parent_hash, inner_layer).await?;

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
        self.update_summaries(parent_hash, INNER_LAYER_COUNT)
            .await?;

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

    async fn update_summaries(&self, hash: Hash, layer: usize) -> Result<()> {
        // Find the replicas whose current snapshots became complete by this update.
        let replica_ids = node::update_summaries(&self.pool, hash, layer)
            .await?
            .into_iter()
            .filter(|(_, complete)| *complete)
            .map(|(id, _)| id)
            .collect();

        // Then notify them.
        self.notify_branches_changed(&replica_ids).await;

        Ok(())
    }

    /// Update the root node of the remote branch.
    pub(crate) async fn update_remote_branch(&self, replica_id: ReplicaId, node: RootNode) {
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

                self.shared.branch_created_tx.send(()).unwrap_or(());
            }
            Entry::Occupied(entry) => entry.get().branch.update_root(node).await,
        }
    }
}

struct Shared {
    branches: RwLock<Branches>,
    branch_created_tx: watch::Sender<()>,
    // TODO: remove this when [this PR](https://github.com/tokio-rs/tokio/pull/3800) gets merged
    // and published.
    branch_created_rx: watch::Receiver<()>,
}

/// Container for all known branches (local and remote)
pub(crate) struct Branches {
    local: Arc<BranchData>,
    remote: HashMap<ReplicaId, BranchHolder>,
    // Number that gets incremented every time a new branch is created.
    version: u64,
}

struct BranchHolder {
    branch: Arc<BranchData>,
    version: u64,
}

impl Branches {
    async fn load(pool: &db::Pool, this_replica_id: ReplicaId) -> Result<Self> {
        let local = Arc::new(BranchData::new(pool, this_replica_id).await?);
        let remote = load_remote_branches(pool, &this_replica_id).await?;

        Ok(Self {
            local,
            remote,
            version: 0,
        })
    }

    /// Returns a branch with the given id, if it exists.
    pub fn get(&self, replica_id: &ReplicaId) -> Option<&Arc<BranchData>> {
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
pub struct Subscription {
    shared: Arc<Shared>,
    branch_changed_rxs: Vec<(ReplicaId, watch::Receiver<()>)>,
    branch_created_rx: watch::Receiver<()>,
    version: u64,
}

impl Subscription {
    /// Receives the next change notification. Returns the id of the changed branch. Returns `None`
    /// If `Index` was dropped.
    pub async fn recv(&mut self) -> Option<ReplicaId> {
        loop {
            select! {
                id = select_branch_changed(&mut self.branch_changed_rxs), if !self.branch_changed_rxs.is_empty() => {
                    if let Some(id) = id {
                        return Some(id);
                    }
                },
                result = self.branch_created_rx.changed() => {
                    if result.is_err() {
                        return None;
                    }

                    for (branch, version) in self.shared.branches.read().await.recent(self.version) {
                        self.branch_changed_rxs.push((*branch.id(), branch.subscribe()));
                        self.version = self.version.max(version.saturating_add(1));
                    }
                }
            }
        }
    }
}

/// Waits for a change notification from any of the given watch receivers.
/// Returns the id of the branch that triggered the watch, or `None` if a watch disconnected. The
/// disconnected watch is then automatically removed from the vector.
async fn select_branch_changed(
    watches: &mut Vec<(ReplicaId, watch::Receiver<()>)>,
) -> Option<ReplicaId> {
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
) -> Result<HashMap<ReplicaId, BranchHolder>> {
    let ids = load_other_replica_ids(pool, this_replica_id).await?;
    let mut map = HashMap::new();

    for id in ids {
        let branch = Arc::new(BranchData::new(pool, id).await?);
        map.insert(id, BranchHolder { branch, version: 0 });
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
