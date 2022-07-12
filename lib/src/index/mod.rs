mod branch_data;
mod node;
mod path;
mod proof;
mod receive_filter;
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
    receive_filter::ReceiveFilter,
};

use self::proof::ProofError;
use crate::{
    block::BlockId,
    crypto::{sign::PublicKey, CacheHash, Hash, Hashable},
    db,
    debug::DebugPrinter,
    error::{Error, Result},
    event::Event,
    repository::RepositoryId,
};
use futures_util::TryStreamExt;
use std::{
    cmp::Ordering,
    collections::{hash_map::Entry, HashMap},
    sync::{Arc, Mutex},
};
use thiserror::Error;
use tokio::sync::broadcast;

type SnapshotId = u32;

#[derive(Clone)]
pub(crate) struct Index {
    pub pool: db::Pool,
    shared: Arc<Shared>,
}

impl Index {
    pub async fn load(
        pool: db::Pool,
        repository_id: RepositoryId,
        notify_tx: broadcast::Sender<Event>,
    ) -> Result<Self> {
        let branches = load_branches(&mut *pool.acquire().await?, notify_tx.clone()).await?;

        Ok(Self {
            pool,
            shared: Arc::new(Shared {
                repository_id,
                branches: Mutex::new(branches),
                notify_tx,
            }),
        })
    }

    pub fn repository_id(&self) -> &RepositoryId {
        &self.shared.repository_id
    }

    pub fn get_branch(&self, id: &PublicKey) -> Option<Arc<BranchData>> {
        self.shared.branches.lock().unwrap().get(id).cloned()
    }

    pub async fn create_branch(&self, proof: Proof) -> Result<Arc<BranchData>> {
        let mut conn = self.pool.acquire().await?;
        let root_node = RootNode::create(&mut conn, proof, Summary::FULL).await?;

        match self
            .shared
            .branches
            .lock()
            .unwrap()
            .entry(root_node.proof.writer_id)
        {
            Entry::Occupied(_) => Err(Error::EntryExists),
            Entry::Vacant(entry) => {
                let branch =
                    BranchData::new(root_node.proof.writer_id, self.shared.notify_tx.clone());
                let branch = Arc::new(branch);

                entry.insert(branch.clone());

                Ok(branch)
            }
        }
    }

    pub fn collect_branches(&self) -> Vec<Arc<BranchData>> {
        self.shared
            .branches
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect()
    }

    /// Removes the branch.
    pub fn remove_branch(&self, id: &PublicKey) -> Option<Arc<BranchData>> {
        self.shared.branches.lock().unwrap().remove(id)
    }

    /// Subscribe to change notification from all current and future branches.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.shared.notify_tx.subscribe()
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        let mut conn = self.pool.acquire().await.unwrap();
        RootNode::debug_print(&mut conn, print).await;
    }

    /// Receive `RootNode` from other replica and store it into the db. Returns whether the
    /// received node has any new information compared to all the nodes already stored locally.
    pub async fn receive_root_node(
        &self,
        proof: UntrustedProof,
        summary: Summary,
    ) -> Result<bool, ReceiveError> {
        let proof = proof.verify(self.repository_id())?;

        // Ignore branches with empty version vectors because they have no content yet.
        if proof.version_vector.is_empty() {
            return Ok(false);
        }

        let mut conn = self.pool.acquire().await?;

        // Load latest complete root nodes of all known branches.
        let nodes: HashMap<_, _> = RootNode::load_all_latest_complete(&mut conn)
            .map_ok(|node| (node.proof.writer_id, node))
            .try_collect()
            .await?;

        // If the received node is outdated relative to any branch we have, ignore it.
        let uptodate = nodes.values().all(|old_node| {
            match proof
                .version_vector
                .partial_cmp(&old_node.proof.version_vector)
            {
                Some(Ordering::Greater) => true,
                Some(Ordering::Equal) => !old_node
                    .summary
                    .is_up_to_date_with(&summary)
                    .unwrap_or(true),
                Some(Ordering::Less) => false,
                None => proof.writer_id != old_node.proof.writer_id,
            }
        });

        if uptodate {
            let hash = proof.hash;

            match RootNode::create(&mut conn, proof, Summary::INCOMPLETE).await {
                Ok(_) => self.update_summaries(&mut conn, hash).await?,
                Err(Error::EntryExists) => (), // ignore duplicate nodes but don't fail.
                Err(error) => return Err(error.into()),
            }

            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Receive inner nodes from other replica and store them into the db.
    /// Returns hashes of those nodes that were more up to date than the locally stored ones.
    pub async fn receive_inner_nodes(
        &self,
        nodes: CacheHash<InnerNodeMap>,
        receive_filter: &mut ReceiveFilter,
    ) -> Result<Vec<Hash>, ReceiveError> {
        let mut conn = self.pool.acquire().await?;
        let parent_hash = nodes.hash();

        self.check_parent_node_exists(&mut conn, &parent_hash)
            .await?;

        let updated = self
            .find_inner_nodes_with_new_blocks(&mut conn, &parent_hash, &nodes, receive_filter)
            .await?;

        let mut nodes = nodes.into_inner().into_incomplete();
        nodes.inherit_summaries(&mut conn).await?;
        nodes.save(&mut conn, &parent_hash).await?;
        self.update_summaries(&mut conn, parent_hash).await?;

        Ok(updated)
    }

    /// Receive leaf nodes from other replica and store them into the db.
    /// Returns the ids of the blocks that the remote replica has but the local one has not.
    pub async fn receive_leaf_nodes(
        &self,
        nodes: CacheHash<LeafNodeSet>,
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

        nodes
            .into_inner()
            .into_missing()
            .save(&mut conn, &parent_hash)
            .await?;
        self.update_summaries(&mut conn, parent_hash).await?;

        Ok(updated)
    }

    // Filter inner nodes that the remote replica has some blocks in that the local one is missing.
    //
    // Assumes (but does not enforce) that `parent_hash` is the parent hash of all nodes in
    // `remote_nodes`.
    async fn find_inner_nodes_with_new_blocks(
        &self,
        conn: &mut db::Connection,
        parent_hash: &Hash,
        remote_nodes: &InnerNodeMap,
        receive_filter: &mut ReceiveFilter,
    ) -> Result<Vec<Hash>> {
        let local_nodes = InnerNode::load_children(conn, parent_hash).await?;
        let mut output = Vec::with_capacity(remote_nodes.len());

        for (bucket, remote_node) in remote_nodes {
            if !receive_filter
                .check(conn, &remote_node.hash, &remote_node.summary)
                .await?
            {
                continue;
            }

            let insert = if let Some(local_node) = local_nodes.get(bucket) {
                !local_node
                    .summary
                    .is_up_to_date_with(&remote_node.summary)
                    .unwrap_or(true)
            } else {
                // node not present locally - we implicitly treat this as if the local replica
                // had zero blocks under this node unless the remote node is empty, in that
                // case we ignore it.
                !remote_node.is_empty()
            };

            if insert {
                output.push(remote_node.hash);
            }
        }

        Ok(output)
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
                self.update_root_node(id);
            }
        }

        Ok(())
    }

    fn update_root_node(&self, writer_id: PublicKey) {
        self.shared
            .branches
            .lock()
            .unwrap()
            .entry(writer_id)
            .or_insert_with(|| Arc::new(BranchData::new(writer_id, self.shared.notify_tx.clone())))
            .notify();
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
    branches: Mutex<Branches>,
    notify_tx: broadcast::Sender<Event>,
}

/// Container for all known branches (local and remote)
pub(crate) type Branches = HashMap<PublicKey, Arc<BranchData>>;

async fn load_branches(
    conn: &mut db::Connection,
    notify_tx: broadcast::Sender<Event>,
) -> Result<HashMap<PublicKey, Arc<BranchData>>> {
    RootNode::load_all_latest_complete(conn)
        .map_ok(|node| {
            let writer_id = node.proof.writer_id;
            let branch = Arc::new(BranchData::new(writer_id, notify_tx.clone()));

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
