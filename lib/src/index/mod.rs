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
    branch_data::{BranchData, SnapshotData},
    node::{
        receive_block, update_summaries, InnerNode, InnerNodeMap, LeafNode, LeafNodeSet,
        MultiBlockPresence, RootNode, SingleBlockPresence, Summary,
    },
    proof::{Proof, UntrustedProof},
    receive_filter::ReceiveFilter,
};

use self::proof::ProofError;
use crate::{
    block::BlockId,
    collections::HashSet,
    crypto::{sign::PublicKey, CacheHash, Hash, Hashable},
    db,
    debug::DebugPrinter,
    error::{Error, Result},
    event::Event,
    repository::RepositoryId,
    version_vector::VersionVector,
};
use futures_util::TryStreamExt;
use std::cmp::Ordering;
use thiserror::Error;
use tokio::sync::broadcast;
use tracing::Level;

pub(crate) type SnapshotId = u32;

#[derive(Clone)]
pub(crate) struct Index {
    pub pool: db::Pool,
    repository_id: RepositoryId,
    notify_tx: broadcast::Sender<Event>,
}

impl Index {
    pub fn new(
        pool: db::Pool,
        repository_id: RepositoryId,
        notify_tx: broadcast::Sender<Event>,
    ) -> Self {
        Self {
            pool,
            repository_id,
            notify_tx,
        }
    }

    pub fn repository_id(&self) -> &RepositoryId {
        &self.repository_id
    }

    pub fn get_branch(&self, writer_id: PublicKey) -> BranchData {
        BranchData::new(writer_id, self.notify_tx.clone())
    }

    pub async fn load_branches(&self) -> Result<Vec<BranchData>> {
        let mut conn = self.pool.acquire().await?;
        BranchData::load_all(&mut conn, self.notify_tx.clone())
            .try_collect()
            .await
    }

    /// Load latest snapshots of all branches.
    pub async fn load_snapshots(&self) -> Result<Vec<SnapshotData>> {
        let mut conn = self.pool.acquire().await?;
        SnapshotData::load_all(&mut conn, self.notify_tx.clone())
            .try_collect()
            .await
    }

    /// Load ids of all branches.
    pub async fn load_branch_ids(&self) -> Result<HashSet<PublicKey>> {
        // TODO: load only the ids directly, avoid constructing the snapshots.
        Ok(self
            .load_snapshots()
            .await?
            .into_iter()
            .map(|snapshot| *snapshot.branch_id())
            .collect())
    }

    /// Subscribe to change notification from all current and future branches.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.notify_tx.subscribe()
    }

    pub(crate) fn notify(&self, event: Event) {
        self.notify_tx.send(event).unwrap_or(0);
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

        // Make sure the loading of the existing nodes and the potential creation of the new node
        // happens atomically. Otherwise we could conclude the incoming node is up-to-date but
        // just before we start inserting it another snapshot might get created locally and the
        // incoming node might become outdated. But because we already concluded it's up-to-date,
        // we end up inserting it anyway which breaks the invariant that a node inserted later must
        // be happens-after any node inserted earlier in the same branch.
        let mut tx = self.pool.begin_write().await?;

        // Determine further actions by comparing the incoming node against the existing nodes:
        let action = decide_root_node_action(&mut tx, &proof, &summary).await?;

        if action.insert {
            let node = RootNode::create(&mut tx, proof, Summary::INCOMPLETE).await?;
            tracing::debug!(vv = ?node.proof.version_vector, ?node.proof.hash, "snapshot started");
            self.update_summaries(tx, node.proof.hash).await?;
        }

        Ok(action.request_children)
    }

    /// Receive inner nodes from other replica and store them into the db.
    /// Returns hashes of those nodes that were more up to date than the locally stored ones.
    /// Also returns the ids of the branches that became complete.
    pub async fn receive_inner_nodes(
        &self,
        nodes: CacheHash<InnerNodeMap>,
        receive_filter: &mut ReceiveFilter,
    ) -> Result<(Vec<InnerNode>, Vec<PublicKey>), ReceiveError> {
        let mut tx = self.pool.begin_write().await?;
        let parent_hash = nodes.hash();

        self.check_parent_node_exists(&mut tx, &parent_hash).await?;

        let updated_nodes = self
            .find_inner_nodes_with_new_blocks(&mut tx, &nodes, receive_filter)
            .await?;

        let mut nodes = nodes.into_inner().into_incomplete();
        nodes.inherit_summaries(&mut tx).await?;
        nodes.save(&mut tx, &parent_hash).await?;

        let completed_branches = self.update_summaries(tx, parent_hash).await?;

        Ok((updated_nodes, completed_branches))
    }

    /// Receive leaf nodes from other replica and store them into the db.
    /// Returns the ids of the blocks that the remote replica has but the local one has not.
    /// Also returns the ids of the branches that became complete.
    pub async fn receive_leaf_nodes(
        &self,
        nodes: CacheHash<LeafNodeSet>,
    ) -> Result<(Vec<BlockId>, Vec<PublicKey>), ReceiveError> {
        let mut tx = self.pool.begin_write().await?;
        let parent_hash = nodes.hash();

        self.check_parent_node_exists(&mut tx, &parent_hash).await?;

        let updated_blocks = self
            .find_leaf_nodes_with_new_blocks(&mut tx, &nodes)
            .await?;

        nodes
            .into_inner()
            .into_missing()
            .save(&mut tx, &parent_hash)
            .await?;

        let completed_branches = self.update_summaries(tx, parent_hash).await?;

        Ok((updated_blocks, completed_branches))
    }

    // Filter inner nodes that the remote replica has some blocks in that the local one is missing.
    //
    // Assumes (but does not enforce) that `parent_hash` is the parent hash of all nodes in
    // `remote_nodes`.
    async fn find_inner_nodes_with_new_blocks(
        &self,
        tx: &mut db::WriteTransaction,
        remote_nodes: &InnerNodeMap,
        receive_filter: &mut ReceiveFilter,
    ) -> Result<Vec<InnerNode>> {
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
                output.push(*remote_node);
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
    // after it). Also returns the completed branches.
    async fn update_summaries(
        &self,
        mut tx: db::WriteTransaction,
        hash: Hash,
    ) -> Result<Vec<PublicKey>> {
        let statuses = node::update_summaries(&mut tx, hash).await?;
        tx.commit().await?;

        let completed: Vec<_> = statuses
            .into_iter()
            .filter(|(_, complete)| *complete)
            .map(|(writer_id, _)| writer_id)
            .collect();

        if !completed.is_empty() {
            for branch_id in &completed {
                let branch = self.get_branch(*branch_id);

                if tracing::enabled!(Level::DEBUG) {
                    let mut conn = self.pool.acquire().await?;
                    let snapshot = branch.load_snapshot(&mut conn).await?;
                    tracing::debug!(
                        vv = ?snapshot.version_vector(),
                        hash = ?snapshot.root_hash(),
                        branch_id = ?snapshot.branch_id(),
                        "snapshot complete"
                    );
                }

                branch.notify();
            }
        }

        Ok(completed)
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
#[derive(Clone, Copy, Debug)]
pub(crate) enum VersionVectorOp<'a> {
    IncrementLocal,
    Merge(&'a VersionVector),
}

impl VersionVectorOp<'_> {
    pub fn apply(self, local_id: &PublicKey, target: &mut VersionVector) {
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

// Decide what to do with an incoming root node.
struct RootNodeAction {
    // Should we insert the incoming node to the db?
    insert: bool,
    // Should we request the children of the incoming node?
    request_children: bool,
}

async fn decide_root_node_action(
    tx: &mut db::WriteTransaction,
    new_proof: &Proof,
    new_summary: &Summary,
) -> Result<RootNodeAction> {
    let mut action = RootNodeAction {
        insert: true,
        request_children: true,
    };

    let mut old_nodes = RootNode::load_all_latest(tx);
    while let Some(old_node) = old_nodes.try_next().await? {
        match new_proof
            .version_vector
            .partial_cmp(&old_node.proof.version_vector)
        {
            Some(Ordering::Less) => {
                // The incoming node is outdated compared to at least one existing node - discard
                // it.
                action.insert = false;
                action.request_children = false;
            }
            Some(Ordering::Equal) => {
                // The incoming node has the same version vector as one of the existing nodes.
                // If the hashes are also equal, there is no point inserting it but if the incoming
                // summary is more up-to-date than the exising one, we still want to potentially
                // request the children. Otherwise we discard it.

                if new_proof.hash == old_node.proof.hash {
                    action.insert = false;
                }

                // NOTE: `is_outdated` is not antisymmetric, so we can't replace this condition
                // with `new_summary.is_outdated(&old_node.summary)`.
                if !old_node.summary.is_outdated(new_summary) {
                    action.request_children = false;
                }
            }
            Some(Ordering::Greater) => (),
            None => {
                if new_proof.writer_id == old_node.proof.writer_id {
                    tracing::warn!(
                        old_vv = ?old_node.proof.version_vector,
                        new_vv = ?new_proof.version_vector,
                        writer_id = ?new_proof.writer_id,
                        "received root node invalid: broken invariant - concurrency within branch is not allowed"
                    );

                    action.insert = false;
                    action.request_children = false;
                }
            }
        }

        if !action.insert && !action.request_children {
            break;
        }
    }

    Ok(action)
}
