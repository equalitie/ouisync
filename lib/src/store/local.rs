use super::{
    block, error::Error, inner_node, leaf_node, path::Path, root_node, ReadTransaction,
    WriteTransaction,
};
use crate::{
    crypto::{
        sign::{Keypair, PublicKey},
        Hash,
    },
    protocol::{self, Block, BlockId, Proof, RootNode, SingleBlockPresence, Summary},
    version_vector::VersionVector,
};
use std::{
    mem,
    ops::{Deref, DerefMut},
};
use tracing::Instrument;

/// Write transaction specialized for local operations by a writer replica.
pub(crate) struct LocalWriteTransaction {
    inner: WriteTransaction,
    changeset: Changeset,
}

impl LocalWriteTransaction {
    /// Links the given block id into the given branch under the given locator.
    pub fn link_block(
        &mut self,
        encoded_locator: Hash,
        block_id: BlockId,
        block_presence: SingleBlockPresence,
    ) {
        self.changeset
            .links
            .push((encoded_locator, block_id, block_presence));
    }

    pub fn unlink_block(&mut self, encoded_locator: Hash, expected_block_id: Option<BlockId>) {
        self.changeset
            .unlinks
            .push((encoded_locator, expected_block_id));
    }

    /// Writes a block into the store.
    pub fn write_block(&mut self, block: Block) {
        self.changeset.blocks.push(block);
    }

    /// Update the root version vector.
    pub fn bump(&mut self, merge: &VersionVector) {
        self.changeset.bump.merge(merge);
    }

    /// Applies the current changeset to the underlying transaction.
    pub async fn apply(
        &mut self,
        branch_id: &PublicKey,
        write_keys: &Keypair,
    ) -> Result<(), Error> {
        for (encoded_locator, block_id, block_presence) in self.changeset.links.drain(..) {
            apply_link(
                &mut self.inner,
                encoded_locator,
                block_id,
                block_presence,
                branch_id,
                write_keys,
            )
            .await?;
        }

        for (encoded_locator, expected_block_id) in self.changeset.unlinks.drain(..) {
            apply_unlink(
                &mut self.inner,
                encoded_locator,
                expected_block_id,
                branch_id,
                write_keys,
            )
            .await?;
        }

        for block in self.changeset.blocks.drain(..) {
            if let Some(tracker) = &self.inner.block_expiration_tracker {
                tracker.handle_block_update(&block.id);
            }

            block::write(self.inner.db(), &block).await?;
        }

        apply_bump(
            &mut self.inner,
            mem::take(&mut self.changeset.bump),
            branch_id,
            write_keys,
        )
        .await?;

        Ok(())
    }

    /// Calls `apply` and returns the underlying transaction.
    pub async fn finish(
        mut self,
        branch_id: &PublicKey,
        write_keys: &Keypair,
    ) -> Result<WriteTransaction, Error> {
        self.apply(branch_id, write_keys).await?;
        Ok(self.inner)
    }
}

impl From<WriteTransaction> for LocalWriteTransaction {
    fn from(inner: WriteTransaction) -> Self {
        Self {
            inner,
            changeset: Changeset::new(),
        }
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

/// Recorded changes to be applied to the store as a single unit.
#[derive(Default)]
struct Changeset {
    links: Vec<(Hash, BlockId, SingleBlockPresence)>,
    unlinks: Vec<(Hash, Option<BlockId>)>,
    blocks: Vec<Block>,
    bump: VersionVector,
}

impl Changeset {
    fn new() -> Self {
        Self::default()
    }
}

async fn apply_link(
    tx: &mut WriteTransaction,
    encoded_locator: Hash,
    block_id: BlockId,
    block_presence: SingleBlockPresence,
    branch_id: &PublicKey,
    write_keys: &Keypair,
) -> Result<(), Error> {
    let root_node = load_or_create_root_node(tx, branch_id, write_keys).await?;
    let mut path = tx.load_path(&root_node, &encoded_locator).await?;

    if path.has_leaf(&block_id) {
        return Ok(());
    }

    path.set_leaf(&block_id, block_presence);

    save_path(tx, path, &root_node, write_keys).await?;

    Ok(())
}

async fn apply_unlink(
    tx: &mut WriteTransaction,
    encoded_locator: Hash,
    expected_block_id: Option<BlockId>,
    branch_id: &PublicKey,
    write_keys: &Keypair,
) -> Result<(), Error> {
    let root_node = tx.load_root_node(branch_id).await?;
    let mut path = tx.load_path(&root_node, &encoded_locator).await?;

    let block_id = if let Some(block_id) = path.remove_leaf(&encoded_locator) {
        block_id
    } else {
        return Ok(());
    };

    if let Some(expected_block_id) = expected_block_id {
        if block_id != expected_block_id {
            return Ok(());
        }
    }

    save_path(tx, path, &root_node, write_keys).await?;

    Ok(())
}

async fn apply_bump(
    tx: &mut WriteTransaction,
    merge: VersionVector,
    branch_id: &PublicKey,
    write_keys: &Keypair,
) -> Result<(), Error> {
    let root_node = load_or_create_root_node(tx, branch_id, write_keys).await?;

    let mut new_vv = root_node.proof.version_vector.clone();

    if merge.is_empty() {
        new_vv.increment(*branch_id)
    } else {
        new_vv.merge(&merge)
    }

    // Sometimes this is a no-op. This is not an error.
    if new_vv == root_node.proof.version_vector {
        return Ok(());
    }

    let new_proof = Proof::new(
        root_node.proof.writer_id,
        new_vv,
        root_node.proof.hash,
        write_keys,
    );

    create_root_node(tx, new_proof, root_node.summary)
        .instrument(tracing::info_span!("bump"))
        .await
}

async fn save_path(
    tx: &mut WriteTransaction,
    path: Path,
    old_root_node: &RootNode,
    write_keys: &Keypair,
) -> Result<(), Error> {
    let mut parent_hash = Some(path.root_hash);
    for (i, nodes) in path.inner.into_iter().enumerate() {
        let bucket = protocol::get_bucket(&path.locator, i);
        let new_parent_hash = nodes.get(bucket).map(|node| node.hash);

        if let Some(parent_hash) = parent_hash {
            inner_node::save_all(tx.db(), &nodes, &parent_hash).await?;
            tx.inner.inner.cache.put_inners(parent_hash, nodes);
        }

        parent_hash = new_parent_hash;
    }

    if let Some(parent_hash) = parent_hash {
        leaf_node::save_all(tx.db(), &path.leaves, &parent_hash).await?;
        tx.inner.inner.cache.put_leaves(parent_hash, path.leaves);
    }

    let writer_id = old_root_node.proof.writer_id;
    let new_version_vector = old_root_node
        .proof
        .version_vector
        .clone()
        .incremented(writer_id);
    let new_proof = Proof::new(writer_id, new_version_vector, path.root_hash, write_keys);

    create_root_node(tx, new_proof, path.root_summary).await
}

async fn create_root_node(
    tx: &mut WriteTransaction,
    new_proof: Proof,
    new_summary: Summary,
) -> Result<(), Error> {
    let root_node = root_node::create(tx.db(), new_proof, new_summary).await?;
    root_node::remove_older(tx.db(), &root_node).await?;

    tracing::trace!(
        vv = ?root_node.proof.version_vector,
        hash = ?root_node.proof.hash,
        branch_id = ?root_node.proof.writer_id,
        block_presence = ?root_node.summary.block_presence,
        "Local snapshot created"
    );

    tx.inner.inner.cache.put_root(root_node);

    Ok(())
}

async fn load_or_create_root_node(
    tx: &mut WriteTransaction,
    branch_id: &PublicKey,
    write_keys: &Keypair,
) -> Result<RootNode, Error> {
    if let Some(node) = tx.inner.inner.cache.get_root(branch_id) {
        return Ok(node);
    }

    root_node::load_or_create(tx.db(), branch_id, write_keys).await
}
