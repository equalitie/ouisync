use super::{block, error::Error, patch::Patch, WriteTransaction};
use crate::{
    crypto::{
        sign::{Keypair, PublicKey},
        Hash,
    },
    protocol::{Block, BlockId, Bump, SingleBlockPresence},
};

/// Recorded changes to be applied to the store as a single unit.
#[derive(Default)]
pub(crate) struct Changeset {
    links: Vec<(Hash, BlockId, SingleBlockPresence)>,
    unlinks: Vec<(Hash, Option<BlockId>)>,
    blocks: Vec<Block>,
    bump: Bump,
    bump_force: bool,
}

impl Changeset {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies this changeset to the transaction.
    /// Returns `true` if any change was performed, or `false` if the changeset is a no-op.
    pub async fn apply(
        self,
        tx: &mut WriteTransaction,
        branch_id: &PublicKey,
        write_keys: &Keypair,
    ) -> Result<bool, Error> {
        let mut patch = Patch::new(tx, *branch_id).await?;
        let mut changed = false;

        for (encoded_locator, block_id, block_presence) in self.links {
            if patch
                .insert(tx, encoded_locator, block_id, block_presence)
                .await?
            {
                changed = true;
            }
        }

        for (encoded_locator, expected_block_id) in self.unlinks {
            if patch
                .remove(tx, &encoded_locator, expected_block_id.as_ref())
                .await?
            {
                changed = true;
            }
        }

        if self.bump_force && self.bump.changes(patch.version_vector()) {
            changed = true;
        }

        if changed {
            patch.save(tx, self.bump, write_keys).await?;
        }

        for block in self.blocks {
            block::write(tx.db(), &block).await?;

            if let Some(tracker) = &tx.block_expiration_tracker {
                tracker.handle_block_update(&block.id, false);
            }

            changed = true;
        }

        Ok(changed)
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

    /// Update the root version vector. By default the vv is bumped only if this changeset actually
    /// changes anything (i.e., links and/or unlinks blocks). To force the bump even if there are
    /// no changes, call also `force_bump(true)`.
    pub fn bump(&mut self, bump: Bump) {
        self.bump = bump;
    }

    /// Set whether to update the root version vector even if there are no changes (i.e., no blocks
    /// added or removed). Default is `false`.
    pub fn force_bump(&mut self, force: bool) {
        self.bump_force = force;
    }
}
