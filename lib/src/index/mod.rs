mod branch_data;
mod node;
mod path;
mod proof;
mod receive_filter;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use self::node::{test_utils as node_test_utils, EMPTY_INNER_HASH};
pub(crate) use self::{
    branch_data::BranchData,
    node::{receive_block, InnerNode, InnerNodeMap, LeafNode, LeafNodeSet, RootNode, Summary},
    proof::UntrustedProof,
    receive_filter::ReceiveFilter,
};

use self::{branch_data::SnapshotData, proof::ProofError};
use crate::{
    block::BlockId,
    crypto::{sign::PublicKey, CacheHash, Hash, Hashable},
    db,
    debug::DebugPrinter,
    error::{Error, Result},
    event::Event,
    repository::RepositoryId,
    version_vector::VersionVector,
};
use futures_util::TryStreamExt;
use std::{cmp::Ordering, collections::HashMap, sync::Arc};
use thiserror::Error;
use tokio::sync::broadcast;

type SnapshotId = u32;

#[derive(Clone)]
pub(crate) struct Index {
    pub pool: db::Pool,
    shared: Arc<Shared>,
}

impl Index {
    pub fn new(
        pool: db::Pool,
        repository_id: RepositoryId,
        notify_tx: broadcast::Sender<Event>,
    ) -> Self {
        Self {
            pool,
            shared: Arc::new(Shared {
                repository_id,
                notify_tx,
            }),
        }
    }

    pub fn repository_id(&self) -> &RepositoryId {
        &self.shared.repository_id
    }

    pub fn get_branch(&self, writer_id: PublicKey) -> Arc<BranchData> {
        Arc::new(BranchData::new(writer_id, self.shared.notify_tx.clone()))
    }

    pub async fn load_branches(&self) -> Result<Vec<Arc<BranchData>>> {
        let mut conn = self.pool.acquire().await?;
        BranchData::load_all(&mut conn, self.shared.notify_tx.clone())
            .try_collect()
            .await
    }

    /// Load latest snapshots of all branches.
    pub async fn load_snapshots(&self, conn: &mut db::Connection) -> Result<Vec<SnapshotData>> {
        SnapshotData::load_all(conn, self.shared.notify_tx.clone())
            .try_collect()
            .await
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

        // Load latest complete root nodes of all known branches.
        let nodes: HashMap<_, _> = {
            let mut conn = self.pool.acquire().await?;
            RootNode::load_all_latest_complete(&mut conn)
                .map_ok(|node| (node.proof.writer_id, node))
                .try_collect()
                .await?
        };

        // If the received node is outdated relative to any branch we have, ignore it.
        let uptodate = nodes.values().all(|old_node| {
            match proof
                .version_vector
                .partial_cmp(&old_node.proof.version_vector)
            {
                Some(Ordering::Greater) => true,
                Some(Ordering::Equal) => old_node.summary.is_outdated(&summary),
                Some(Ordering::Less) => false,
                None => proof.writer_id != old_node.proof.writer_id,
            }
        });

        if uptodate {
            let mut tx = self.pool.begin().await?;
            let hash = proof.hash;

            match RootNode::create(&mut tx, proof, Summary::INCOMPLETE).await {
                Ok(_) => self.update_summaries(tx, hash).await?,
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
        let mut tx = self.pool.begin().await?;
        let parent_hash = nodes.hash();

        self.check_parent_node_exists(&mut tx, &parent_hash).await?;

        let updated = self
            .find_inner_nodes_with_new_blocks(&mut tx, &nodes, receive_filter)
            .await?;

        let mut nodes = nodes.into_inner().into_incomplete();
        nodes.inherit_summaries(&mut tx).await?;
        nodes.save(&mut tx, &parent_hash).await?;
        self.update_summaries(tx, parent_hash).await?;

        Ok(updated)
    }

    /// Receive leaf nodes from other replica and store them into the db.
    /// Returns the ids of the blocks that the remote replica has but the local one has not.
    pub async fn receive_leaf_nodes(
        &self,
        nodes: CacheHash<LeafNodeSet>,
    ) -> Result<Vec<BlockId>, ReceiveError> {
        let mut tx = self.pool.begin().await?;
        let parent_hash = nodes.hash();

        self.check_parent_node_exists(&mut tx, &parent_hash).await?;

        let updated = self
            .find_leaf_nodes_with_new_blocks(&mut tx, &nodes)
            .await?;

        nodes
            .into_inner()
            .into_missing()
            .save(&mut tx, &parent_hash)
            .await?;
        self.update_summaries(tx, parent_hash).await?;

        Ok(updated)
    }

    // Filter inner nodes that the remote replica has some blocks in that the local one is missing.
    //
    // Assumes (but does not enforce) that `parent_hash` is the parent hash of all nodes in
    // `remote_nodes`.
    async fn find_inner_nodes_with_new_blocks(
        &self,
        tx: &mut db::Transaction<'_>,
        remote_nodes: &InnerNodeMap,
        receive_filter: &mut ReceiveFilter,
    ) -> Result<Vec<Hash>> {
        let mut output = Vec::with_capacity(remote_nodes.len());

        for (_, remote_node) in remote_nodes {
            if !receive_filter
                .check(tx, &remote_node.hash, &remote_node.summary)
                .await?
            {
                continue;
            }

            let local_node = InnerNode::load(tx, &remote_node.hash).await?;
            let insert = if let Some(local_node) = local_node {
                local_node.summary.is_outdated(&remote_node.summary)
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
    async fn find_leaf_nodes_with_new_blocks(
        &self,
        conn: &mut db::Connection,
        remote_nodes: &LeafNodeSet,
    ) -> Result<Vec<BlockId>> {
        let mut output = Vec::new();

        for remote_node in remote_nodes.present() {
            if !LeafNode::is_present(conn, &remote_node.block_id).await? {
                output.push(remote_node.block_id);
            }
        }

        Ok(output)
    }

    // Updates summaries of the specified nodes and all their ancestors, commits the transaction
    // and notifies the affected branches that became complete (wasn't before the update but became
    // after it).
    async fn update_summaries(&self, mut tx: db::Transaction<'_>, hash: Hash) -> Result<()> {
        let statuses = node::update_summaries(&mut tx, hash).await?;
        tx.commit().await?;

        let completed: Vec<_> = statuses
            .into_iter()
            .filter(|(_, complete)| *complete)
            .map(|(writer_id, _)| self.get_branch(writer_id))
            .collect();

        for branch in completed {
            branch.notify();
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
    notify_tx: broadcast::Sender<Event>,
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

/// Operation on version vector
pub(crate) enum VersionVectorOp {
    IncrementLocal,
    Merge(VersionVector),
}

impl VersionVectorOp {
    pub fn apply(&self, local_id: &PublicKey, target: &mut VersionVector) {
        match self {
            Self::IncrementLocal => {
                target.increment(*local_id);
            }
            Self::Merge(other) => {
                target.merge(other);
            }
        }
    }
}
