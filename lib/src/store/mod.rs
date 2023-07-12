mod block;
mod block_ids;
mod error;
mod index;
mod inner_node;
mod leaf_node;
mod path;
mod quota;
mod receive_filter;
mod root_node;

#[cfg(test)]
mod tests;

pub use error::Error;

pub(crate) use {
    block::ReceiveStatus as BlockReceiveStatus, block_ids::BlockIdsPage,
    inner_node::ReceiveStatus as InnerNodeReceiveStatus,
    leaf_node::ReceiveStatus as LeafNodeReceiveStatus, receive_filter::ReceiveFilter,
    root_node::ReceiveStatus as RootNodeReceiveStatus,
};

use crate::{
    block::{BlockData, BlockId, BlockNonce},
    crypto::{
        sign::{Keypair, PublicKey},
        CacheHash, Hash, Hashable,
    },
    db,
    debug::DebugPrinter,
    index::{MultiBlockPresence, Proof, SingleBlockPresence, Summary, VersionVectorOp},
    progress::Progress,
    storage_size::StorageSize,
};
use futures_util::{Stream, TryStreamExt};
use index::UpdateSummaryReason;
use std::{
    borrow::Cow,
    ops::{Deref, DerefMut},
};
use tracing::Instrument;

// TODO: these items should mostly be internal to this module
#[cfg(test)]
pub(crate) use self::{
    inner_node::{get_bucket, InnerNode, EMPTY_INNER_HASH},
    leaf_node::LeafNode,
};
pub(crate) use self::{
    inner_node::{InnerNodeMap, INNER_LAYER_COUNT},
    leaf_node::LeafNodeSet,
    path::Path,
    root_node::RootNode,
};

/// Data store
#[derive(Clone)]
pub(crate) struct Store {
    db: db::Pool,
}

impl Store {
    pub fn new(db: db::Pool) -> Self {
        Self { db }
    }

    /// Acquires a `Reader`
    pub async fn acquire_read(&self) -> Result<Reader, Error> {
        Ok(Reader {
            inner: Handle::Connection(self.db.acquire().await?),
        })
    }

    /// Begins a `ReadTransaction`
    pub async fn begin_read(&self) -> Result<ReadTransaction, Error> {
        Ok(ReadTransaction {
            inner: Reader {
                inner: Handle::ReadTransaction(self.db.begin_read().await?),
            },
        })
    }

    /// Begins a `WriteTransaction`
    pub async fn begin_write(&self) -> Result<WriteTransaction, Error> {
        Ok(WriteTransaction {
            inner: ReadTransaction {
                inner: Reader {
                    inner: Handle::WriteTransaction(self.db.begin_write().await?),
                },
            },
        })
    }

    pub async fn count_blocks(&self) -> Result<u64, Error> {
        self.acquire_read().await?.count_blocks().await
    }

    /// Retrieve the syncing progress of this repository (number of downloaded blocks / number of
    /// all blocks)
    // TODO: Move this to Store
    pub async fn sync_progress(&self) -> Result<Progress, Error> {
        let mut reader = self.acquire_read().await?;

        let total = reader.count_leaf_nodes().await?;
        let present = reader.count_blocks().await?;

        Ok(Progress {
            value: present,
            total,
        })
    }

    /// Remove outdated older snapshots. Note this preserves older snapshots that can be used as
    /// fallback for the latest snapshot and only removes those that can't.
    pub async fn remove_outdated_snapshots(&self, root_node: &RootNode) -> Result<(), Error> {
        // First remove all incomplete snapshots as they can never serve as fallback.
        let mut tx = self.begin_write().await?;
        root_node::remove_older_incomplete(tx.db(), root_node).await?;
        tx.commit().await?;

        let mut reader = self.acquire_read().await?;

        // Then remove those snapshots that can't serve as fallback for the current one.
        let mut new = Cow::Borrowed(root_node);

        while let Some(old) = reader.load_prev_root_node(&new).await? {
            if root_node::check_fallback(reader.db(), &old, &new).await? {
                // `old` can serve as fallback for `self` and so we can't prune it yet. Try the
                // previous snapshot.
                tracing::trace!(
                    branch_id = ?old.proof.writer_id,
                    hash = ?old.proof.hash,
                    vv = ?old.proof.version_vector,
                    "outdated snapshot not removed - possible fallback"
                );

                new = Cow::Owned(old);
            } else {
                // `old` can't serve as fallback for `self` and so we can safely remove it
                let mut tx = self.begin_write().await?;
                root_node::remove(tx.db(), &old).await?;
                tx.commit().await?;

                tracing::trace!(
                    branch_id = ?old.proof.writer_id,
                    hash = ?old.proof.hash,
                    vv = ?old.proof.version_vector,
                    "outdated snapshot removed"
                );
            }
        }

        Ok(())
    }

    pub fn receive_filter(&self) -> ReceiveFilter {
        ReceiveFilter::new(self.db.clone())
    }

    /// Returns all block ids referenced from complete snapshots. The result is paginated (with
    /// `page_size` entries per page) to avoid loading too many items into memory.
    pub fn block_ids(&self, page_size: u32) -> BlockIdsPage {
        BlockIdsPage::new(self.db.clone(), page_size)
    }

    pub async fn debug_print_root_node(&self, printer: DebugPrinter) {
        match self.acquire_read().await {
            Ok(mut reader) => root_node::debug_print(reader.db(), printer).await,
            Err(error) => printer.display(&format!("Failed to acquire reader {:?}", error)),
        }
    }

    /// Closes the store. Waits until all `Reader`s and `{Read|Write}Transactions` obtained from
    /// this store are dropped.
    pub async fn close(&self) -> Result<(), Error> {
        Ok(self.db.close().await?)
    }

    /// Access the underlying database pool.
    /// TODO: make this non-public when the store extraction is complete.
    pub fn db(&self) -> &db::Pool {
        &self.db
    }
}

/// Read-only operations. This is an up-to-date view of the data.
pub(crate) struct Reader {
    inner: Handle,
}

impl Reader {
    /// Reads a block from the store into a buffer.
    ///
    /// # Panics
    ///
    /// Panics if `buffer` length is less than [`BLOCK_SIZE`].
    pub async fn read_block(
        &mut self,
        id: &BlockId,
        buffer: &mut [u8],
    ) -> Result<BlockNonce, Error> {
        block::read(self.db(), id, buffer).await
    }

    /// Checks whether the block exists in the store.
    #[cfg(test)]
    pub async fn block_exists(&mut self, id: &BlockId) -> Result<bool, Error> {
        block::exists(self.db(), id).await
    }

    /// Returns the total number of blocks in the store.
    pub async fn count_blocks(&mut self) -> Result<u64, Error> {
        block::count(self.db()).await
    }

    pub async fn count_leaf_nodes(&mut self) -> Result<u64, Error> {
        leaf_node::count(self.db()).await
    }

    #[cfg(test)]
    pub async fn count_leaf_nodes_in_branch(
        &mut self,
        branch_id: &PublicKey,
    ) -> Result<usize, Error> {
        let root_hash = self.load_root_node(branch_id).await?.proof.hash;
        leaf_node::count_in(self.db(), 0, &root_hash).await
    }

    /// Load the latest approved root node of the given branch.
    pub async fn load_root_node(&mut self, branch_id: &PublicKey) -> Result<RootNode, Error> {
        root_node::load(self.db(), branch_id).await
    }

    pub async fn load_prev_root_node(
        &mut self,
        node: &RootNode,
    ) -> Result<Option<RootNode>, Error> {
        root_node::load_prev(self.db(), node).await
    }

    pub fn load_root_nodes(&mut self) -> impl Stream<Item = Result<RootNode, Error>> + '_ {
        root_node::load_all(self.db())
    }

    #[cfg(test)]
    pub fn load_root_nodes_by_writer_in_any_state<'a>(
        &'a mut self,
        writer_id: &'a PublicKey,
    ) -> impl Stream<Item = Result<RootNode, Error>> + 'a {
        root_node::load_all_by_writer_in_any_state(self.db(), writer_id)
    }

    pub async fn root_node_exists(&mut self, node: &RootNode) -> Result<bool, Error> {
        root_node::exists(self.db(), node).await
    }

    pub async fn load_inner_nodes(&mut self, parent_hash: &Hash) -> Result<InnerNodeMap, Error> {
        inner_node::load_children(self.db(), parent_hash).await
    }

    pub async fn load_leaf_nodes(&mut self, parent_hash: &Hash) -> Result<LeafNodeSet, Error> {
        leaf_node::load_children(self.db(), parent_hash).await
    }

    pub fn load_locators<'a>(
        &'a mut self,
        block_id: &'a BlockId,
    ) -> impl Stream<Item = Result<Hash, Error>> + 'a {
        leaf_node::load_locators(self.db(), block_id)
    }

    pub(super) fn missing_block_ids_in_branch<'a>(
        &'a mut self,
        branch_id: &'a PublicKey,
    ) -> impl Stream<Item = Result<BlockId, Error>> + 'a {
        block_ids::missing_block_ids_in_branch(self.db(), branch_id)
    }

    // Access the underlying database connection.
    fn db(&mut self) -> &mut db::Connection {
        &mut self.inner
    }
}

/// Read-only transaction. This is a snapshot of the data at the time the transaction was
/// acquired.
pub(crate) struct ReadTransaction {
    inner: Reader,
}

impl ReadTransaction {
    /// Finds the block id corresponding to the given locator in the given branch.
    pub async fn find_block(
        &mut self,
        branch_id: &PublicKey,
        encoded_locator: &Hash,
    ) -> Result<(BlockId, SingleBlockPresence), Error> {
        let root_node = self.load_root_node(branch_id).await?;
        self.find_block_in(&root_node, encoded_locator).await
    }

    pub async fn find_block_in(
        &mut self,
        root_node: &RootNode,
        encoded_locator: &Hash,
    ) -> Result<(BlockId, SingleBlockPresence), Error> {
        let path = self.load_path(root_node, encoded_locator).await?;
        path.get_leaf().ok_or(Error::LocatorNotFound)
    }

    async fn load_path(
        &mut self,
        root_node: &RootNode,
        encoded_locator: &Hash,
    ) -> Result<Path, Error> {
        let mut path = Path::new(root_node.proof.hash, root_node.summary, *encoded_locator);
        let mut parent = path.root_hash;

        for level in 0..INNER_LAYER_COUNT {
            path.inner[level] = inner_node::load_children(self.db(), &parent).await?;

            if let Some(node) = path.inner[level].get(path.get_bucket(level)) {
                parent = node.hash
            } else {
                return Ok(path);
            };
        }

        path.leaves = leaf_node::load_children(self.db(), &parent).await?;

        Ok(path)
    }

    // Access the underlying database transaction.
    fn db(&mut self) -> &mut db::ReadTransaction {
        match &mut self.inner.inner {
            Handle::ReadTransaction(tx) => tx,
            Handle::WriteTransaction(tx) => tx,
            Handle::Connection(_) => unreachable!(),
        }
    }
}

impl Deref for ReadTransaction {
    type Target = Reader;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for ReadTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

pub(crate) struct WriteTransaction {
    inner: ReadTransaction,
}

impl WriteTransaction {
    /// Links the given block id into the given branch under the given locator.
    pub async fn link_block(
        &mut self,
        branch_id: &PublicKey,
        encoded_locator: &Hash,
        block_id: &BlockId,
        block_presence: SingleBlockPresence,
        write_keys: &Keypair,
    ) -> Result<bool, Error> {
        let root_node = root_node::load_or_create(self.db(), branch_id, write_keys).await?;
        let mut path = self.load_path(&root_node, encoded_locator).await?;

        if path.has_leaf(block_id) {
            return Ok(false);
        }

        path.set_leaf(block_id, block_presence);

        self.save_path(&path, &root_node, write_keys).await?;

        Ok(true)
    }

    /// Unlinks (removes) the given block id from the given branch and locator. If
    /// `expected_block_id` is `Some`, then the block is unlinked only if its id matches it,
    /// otherwise it's removed unconditionally.
    pub async fn unlink_block(
        &mut self,
        branch_id: &PublicKey,
        encoded_locator: &Hash,
        expected_block_id: Option<&BlockId>,
        write_keys: &Keypair,
    ) -> Result<(), Error> {
        let root_node = root_node::load(self.db(), branch_id).await?;
        let mut path = self.load_path(&root_node, encoded_locator).await?;

        let block_id = path
            .remove_leaf(encoded_locator)
            .ok_or(Error::LocatorNotFound)?;

        if let Some(expected_block_id) = expected_block_id {
            if &block_id != expected_block_id {
                return Ok(());
            }
        }

        self.save_path(&path, &root_node, write_keys).await?;

        Ok(())
    }

    /// Writes a block into the store.
    ///
    /// If a block with the same id already exists, this is a no-op.
    ///
    /// # Panics
    ///
    /// Panics if buffer length is not equal to [`BLOCK_SIZE`].
    ///
    pub async fn write_block(
        &mut self,
        id: &BlockId,
        buffer: &[u8],
        nonce: &BlockNonce,
    ) -> Result<(), Error> {
        block::write(self.db(), id, buffer, nonce).await
    }

    /// Removes the specified block from the store and marks it as missing in the index.
    pub async fn remove_block(&mut self, id: &BlockId) -> Result<(), Error> {
        block::remove(self.db(), id).await?;
        leaf_node::set_missing(self.db(), id).await?;

        let parent_hashes: Vec<_> = leaf_node::load_parent_hashes(self.db(), id)
            .try_collect()
            .await?;

        index::update_summaries(self.db(), parent_hashes, UpdateSummaryReason::BlockRemoved)
            .await?;

        Ok(())
    }

    /// Update the root version vector of the given branch.
    pub async fn bump(
        &mut self,
        branch_id: &PublicKey,
        op: VersionVectorOp<'_>,
        write_keys: &Keypair,
    ) -> Result<(), Error> {
        let root_node = root_node::load_or_create(self.db(), branch_id, write_keys).await?;

        let mut new_vv = root_node.proof.version_vector.clone();
        op.apply(branch_id, &mut new_vv);

        // Sometimes `op` is a no-op. This is not an error.
        if new_vv == root_node.proof.version_vector {
            return Ok(());
        }

        let new_proof = Proof::new(
            root_node.proof.writer_id,
            new_vv,
            root_node.proof.hash,
            write_keys,
        );

        self.create_root_node(new_proof, root_node.summary)
            .instrument(tracing::info_span!("bump"))
            .await
    }

    pub async fn remove_branch(&mut self, root_node: &RootNode) -> Result<(), Error> {
        root_node::remove_older(self.db(), root_node).await?;
        root_node::remove(self.db(), root_node).await?;

        Ok(())
    }

    /// Write a root node received from a remote replica.
    pub async fn receive_root_node(
        &mut self,
        proof: Proof,
        block_presence: MultiBlockPresence,
    ) -> Result<RootNodeReceiveStatus, Error> {
        let hash = proof.hash;

        // Make sure the loading of the existing nodes and the potential creation of the new node
        // happens atomically. Otherwise we could conclude the incoming node is up-to-date but
        // just before we start inserting it another snapshot might get created locally and the
        // incoming node might become outdated. But because we already concluded it's up-to-date,
        // we end up inserting it anyway which breaks the invariant that a node inserted later must
        // be happens-after any node inserted earlier in the same branch.

        // Determine further actions by comparing the incoming node against the existing nodes:
        let action = root_node::decide_action(self.db(), &proof, &block_presence).await?;

        if action.insert {
            let node = root_node::create(self.db(), proof, Summary::INCOMPLETE).await?;

            // Ignoring quota here because if the snapshot became complete by receiving this root
            // node it means that we already have all the other nodes and so the quota validation
            // already took place.
            let status = index::finalize(self.db(), hash, None).await?;

            tracing::debug!(
                branch_id = ?node.proof.writer_id,
                hash = ?node.proof.hash,
                vv = ?node.proof.version_vector,
                "snapshot started"
            );

            Ok(RootNodeReceiveStatus {
                new_approved: status.new_approved,
                request_children: action.request_children,
            })
        } else {
            Ok(RootNodeReceiveStatus {
                new_approved: Vec::new(),
                request_children: action.request_children,
            })
        }
    }

    /// Write inner nodes received from a remote replica.
    pub async fn receive_inner_nodes(
        &mut self,
        nodes: CacheHash<InnerNodeMap>,
        receive_filter: &ReceiveFilter,
        quota: Option<StorageSize>,
    ) -> Result<InnerNodeReceiveStatus, Error> {
        let parent_hash = nodes.hash();

        if !index::parent_exists(self.db(), &parent_hash).await? {
            return Ok(InnerNodeReceiveStatus::default());
        }

        let request_children =
            inner_node::filter_nodes_with_new_blocks(self.db(), &nodes, receive_filter).await?;

        let mut nodes = nodes.into_inner().into_incomplete();
        inner_node::inherit_summaries(self.db(), &mut nodes).await?;
        inner_node::save_all(self.db(), &nodes, &parent_hash).await?;

        let status = index::finalize(self.db(), parent_hash, quota).await?;

        Ok(InnerNodeReceiveStatus {
            new_approved: status.new_approved,
            request_children,
        })
    }

    /// Receive leaf nodes from other replica and store them into the db.
    /// Returns the ids of the blocks that the remote replica has but the local one has not.
    /// Also returns the receive status.
    pub async fn receive_leaf_nodes(
        &mut self,
        nodes: CacheHash<LeafNodeSet>,
        quota: Option<StorageSize>,
    ) -> Result<LeafNodeReceiveStatus, Error> {
        let parent_hash = nodes.hash();

        if !index::parent_exists(self.db(), &parent_hash).await? {
            return Ok(LeafNodeReceiveStatus::default());
        }

        let request_blocks = leaf_node::filter_nodes_with_new_blocks(self.db(), &nodes).await?;

        leaf_node::save_all(self.db(), &nodes.into_inner().into_missing(), &parent_hash).await?;

        let status = index::finalize(self.db(), parent_hash, quota).await?;

        Ok(LeafNodeReceiveStatus {
            old_approved: status.old_approved,
            new_approved: status.new_approved,
            request_blocks,
        })
    }

    /// Write a block received from a remote replica and marks it as present in the index.
    /// The block must already be referenced by the index, otherwise an `BlockNotReferenced` error
    /// is returned.
    pub(crate) async fn receive_block(
        &mut self,
        data: &BlockData,
        nonce: &BlockNonce,
    ) -> Result<BlockReceiveStatus, Error> {
        block::receive(self.db(), data, nonce).await
    }

    pub async fn commit(self) -> Result<(), Error> {
        match self.inner.inner.inner {
            Handle::WriteTransaction(tx) => Ok(tx.commit().await?),
            Handle::Connection(_) | Handle::ReadTransaction(_) => unreachable!(),
        }
    }

    /// Commits the transaction and if (and only if) the commit completes successfully, runs the
    /// given closure.
    ///
    /// See `db::WriteTransaction::commit_and_then` for explanation why this is necessary.
    pub async fn commit_and_then<F, R>(self, f: F) -> Result<R, sqlx::Error>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        match self.inner.inner.inner {
            Handle::WriteTransaction(tx) => Ok(tx.commit_and_then(f).await?),
            Handle::Connection(_) | Handle::ReadTransaction(_) => unreachable!(),
        }
    }

    async fn save_path(
        &mut self,
        path: &Path,
        old_root_node: &RootNode,
        write_keys: &Keypair,
    ) -> Result<(), Error> {
        for (i, inner_layer) in path.inner.iter().enumerate() {
            if let Some(parent_hash) = path.hash_at_layer(i) {
                inner_node::save_all(self.db(), inner_layer, &parent_hash).await?;
            }
        }

        let layer = Path::total_layer_count() - 1;
        if let Some(parent_hash) = path.hash_at_layer(layer - 1) {
            leaf_node::save_all(self.db(), &path.leaves, &parent_hash).await?;
        }

        let writer_id = old_root_node.proof.writer_id;
        let new_version_vector = old_root_node
            .proof
            .version_vector
            .clone()
            .incremented(writer_id);
        let new_proof = Proof::new(writer_id, new_version_vector, path.root_hash, write_keys);

        self.create_root_node(new_proof, path.root_summary).await
    }

    async fn create_root_node(
        &mut self,
        new_proof: Proof,
        new_summary: Summary,
    ) -> Result<(), Error> {
        let root_node = root_node::create(self.db(), new_proof, new_summary).await?;
        root_node::remove_older(self.db(), &root_node).await?;

        tracing::trace!(
            vv = ?root_node.proof.version_vector,
            hash = ?root_node.proof.hash,
            branch_id = ?root_node.proof.writer_id,
            "create local snapshot"
        );

        Ok(())
    }

    // Access the underlying database transaction.
    fn db(&mut self) -> &mut db::WriteTransaction {
        match &mut self.inner.inner.inner {
            Handle::WriteTransaction(tx) => tx,
            Handle::Connection(_) | Handle::ReadTransaction(_) => unreachable!(),
        }
    }
}

impl Deref for WriteTransaction {
    type Target = ReadTransaction;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for WriteTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

enum Handle {
    Connection(db::PoolConnection),
    ReadTransaction(db::ReadTransaction),
    WriteTransaction(db::WriteTransaction),
}

impl Deref for Handle {
    type Target = db::Connection;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Connection(conn) => conn,
            Self::ReadTransaction(tx) => tx,
            Self::WriteTransaction(tx) => tx,
        }
    }
}

impl DerefMut for Handle {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Connection(conn) => conn,
            Self::ReadTransaction(tx) => &mut *tx,
            Self::WriteTransaction(tx) => &mut *tx,
        }
    }
}
