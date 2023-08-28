use super::{
    block, error::Error, inner_node, leaf_node, path::Path, root_node, ReadTransaction,
    WriteTransaction,
};
use crate::{
    crypto::{
        sign::{Keypair, PublicKey},
        Hash,
    },
    protocol::{
        self, Block, BlockId, Proof, RootNode, SingleBlockPresence, Summary, VersionVectorOp,
    },
};
use std::ops::{Deref, DerefMut};
use tracing::Instrument;

/// Write transaction specialized for local operations by a writer replica.
pub(crate) struct LocalWriteTransaction {
    inner: WriteTransaction,
}

impl LocalWriteTransaction {
    pub(super) fn new(inner: WriteTransaction) -> Self {
        Self { inner }
    }

    /// Links the given block id into the given branch under the given locator.
    pub async fn link_block(
        &mut self,
        branch_id: &PublicKey,
        encoded_locator: &Hash,
        block_id: &BlockId,
        block_presence: SingleBlockPresence,
        write_keys: &Keypair,
    ) -> Result<bool, Error> {
        let root_node = self.load_or_create_root_node(branch_id, write_keys).await?;
        let mut path = self.inner.load_path(&root_node, encoded_locator).await?;

        if path.has_leaf(block_id) {
            return Ok(false);
        }

        path.set_leaf(block_id, block_presence);

        self.save_path(path, &root_node, write_keys).await?;

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
        let root_node = self.inner.load_root_node(branch_id).await?;
        let mut path = self.inner.load_path(&root_node, encoded_locator).await?;

        let block_id = path
            .remove_leaf(encoded_locator)
            .ok_or(Error::LocatorNotFound)?;

        if let Some(expected_block_id) = expected_block_id {
            if &block_id != expected_block_id {
                return Ok(());
            }
        }

        self.save_path(path, &root_node, write_keys).await?;

        Ok(())
    }

    /// Writes a block into the store.
    ///
    /// If a block with the same id already exists, this is a no-op.
    pub async fn write_block(&mut self, block: &Block) -> Result<(), Error> {
        if let Some(tracker) = &self.inner.block_expiration_tracker {
            tracker.handle_block_update(&block.id);
        }

        block::write(self.inner.db(), block).await
    }

    /// Update the root version vector of the given branch.
    pub async fn bump(
        &mut self,
        branch_id: &PublicKey,
        op: VersionVectorOp<'_>,
        write_keys: &Keypair,
    ) -> Result<(), Error> {
        let root_node = self.load_or_create_root_node(branch_id, write_keys).await?;

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

    pub async fn apply(self) -> Result<WriteTransaction, Error> {
        Ok(self.inner)
    }

    pub async fn commit(self) -> Result<(), Error> {
        self.apply().await?.commit().await
    }

    pub async fn commit_and_then<F, R>(self, f: F) -> Result<R, Error>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.apply().await?.commit_and_then(f).await
    }

    async fn save_path(
        &mut self,
        path: Path,
        old_root_node: &RootNode,
        write_keys: &Keypair,
    ) -> Result<(), Error> {
        let mut parent_hash = Some(path.root_hash);
        for (i, nodes) in path.inner.into_iter().enumerate() {
            let bucket = protocol::get_bucket(&path.locator, i);
            let new_parent_hash = nodes.get(bucket).map(|node| node.hash);

            if let Some(parent_hash) = parent_hash {
                inner_node::save_all(self.inner.db(), &nodes, &parent_hash).await?;
                self.inner.inner.inner.cache.put_inners(parent_hash, nodes);
            }

            parent_hash = new_parent_hash;
        }

        if let Some(parent_hash) = parent_hash {
            leaf_node::save_all(self.inner.db(), &path.leaves, &parent_hash).await?;
            self.inner
                .inner
                .inner
                .cache
                .put_leaves(parent_hash, path.leaves);
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
        let root_node = root_node::create(self.inner.db(), new_proof, new_summary).await?;
        root_node::remove_older(self.inner.db(), &root_node).await?;

        tracing::trace!(
            vv = ?root_node.proof.version_vector,
            hash = ?root_node.proof.hash,
            branch_id = ?root_node.proof.writer_id,
            block_presence = ?root_node.summary.block_presence,
            "Local snapshot created"
        );

        self.inner.inner.inner.cache.put_root(root_node);

        Ok(())
    }

    async fn load_or_create_root_node(
        &mut self,
        branch_id: &PublicKey,
        write_keys: &Keypair,
    ) -> Result<RootNode, Error> {
        if let Some(node) = self.inner.inner.inner.cache.get_root(branch_id) {
            return Ok(node);
        }

        root_node::load_or_create(self.inner.db(), branch_id, write_keys).await
    }
}

impl Deref for LocalWriteTransaction {
    type Target = ReadTransaction;

    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

impl DerefMut for LocalWriteTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.deref_mut()
    }
}
