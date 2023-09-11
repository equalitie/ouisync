use super::{block, error::Error, patch::Patch, WriteTransaction};
use crate::{
    crypto::{
        sign::{Keypair, PublicKey},
        Hash,
    },
    protocol::{Block, BlockId, SingleBlockPresence},
    version_vector::VersionVector,
};

/// Recorded changes to be applied to the store as a single unit.
#[derive(Default)]
pub(crate) struct Changeset {
    links: Vec<(Hash, BlockId, SingleBlockPresence)>,
    unlinks: Vec<(Hash, Option<BlockId>)>,
    blocks: Vec<Block>,
    bump: VersionVector,
}

impl Changeset {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies this changeset to the transaction.
    pub async fn apply(
        self,
        tx: &mut WriteTransaction,
        branch_id: &PublicKey,
        write_keys: &Keypair,
    ) -> Result<(), Error> {
        let mut patch = Patch::new(tx, *branch_id).await?;

        for (encoded_locator, block_id, block_presence) in self.links {
            patch
                .insert(tx, encoded_locator, block_id, block_presence)
                .await?;
        }

        for (encoded_locator, expected_block_id) in self.unlinks {
            patch
                .remove(tx, &encoded_locator, expected_block_id.as_ref())
                .await?;
        }

        patch.save(tx, &self.bump, write_keys).await?;

        for block in self.blocks {
            block::write(tx.db(), &block).await?;

            if let Some(tracker) = &tx.block_expiration_tracker {
                tracker.handle_block_update(&block.id, false);
            }
        }

        Ok(())
    }

    /// Links the given block id into the given branch under the given locator.
    pub fn link_block(
        &mut self,
        encoded_locator: Hash,
        block_id: BlockId,
        block_presence: SingleBlockPresence,
    ) {
        self.links.push((encoded_locator, block_id, block_presence));
    }

    pub fn unlink_block(&mut self, encoded_locator: Hash, expected_block_id: Option<BlockId>) {
        self.unlinks.push((encoded_locator, expected_block_id));
    }

    /// Writes a block into the store.
    pub fn write_block(&mut self, block: Block) {
        self.blocks.push(block);
    }

    /// Update the root version vector.
    pub fn bump(&mut self, merge: &VersionVector) {
        self.bump.merge(merge);
    }
}
