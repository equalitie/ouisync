use super::{operations, BlobNonce, Cursor, OpenBlock, BLOB_NONCE_SIZE};
use crate::{
    block::{BlockId, BLOCK_SIZE},
    branch::Branch,
    crypto::cipher::SecretKey,
    db,
    error::Result,
    locator::Locator,
    Error,
};
use std::{fmt, mem};

pub(crate) struct Core {
    pub branch: Branch,
    pub head_locator: Locator,
    pub blob_key: SecretKey,
    pub len: u64,
    pub len_dirty: bool,
}

impl Core {
    pub async fn open_first_block(&mut self) -> Result<OpenBlock> {
        // No need to commit this as we are only reading here.
        let mut tx = self.branch.db_pool().begin().await?;

        match operations::load_block(
            &mut tx,
            self.branch.data(),
            self.branch.keys().read(),
            &self.head_locator,
        )
        .await
        {
            Ok((id, mut buffer, auth_tag)) => {
                operations::decrypt_block(
                    &self.blob_key,
                    &id,
                    0,
                    &mut buffer[BLOB_NONCE_SIZE..],
                    &auth_tag,
                )?;

                let mut content = Cursor::new(buffer);
                content.pos = self.header_size();

                Ok(OpenBlock {
                    locator: self.head_locator,
                    id,
                    content,
                    dirty: false,
                })
            }
            Err(Error::EntryNotFound) if self.len == 0 => {
                // create a new block but we need to also generate new blob key because we no longer
                // have the original nonce.

                let nonce: BlobNonce = rand::random();
                self.blob_key = self.branch.keys().read().derive_subkey(&nonce);

                Ok(OpenBlock::new_head(self.head_locator, &nonce))
            }
            Err(error) => Err(error),
        }
    }

    pub async fn first_block_id(&self) -> Result<BlockId> {
        // NOTE: no need to commit this transaction because we are only reading here.
        let mut tx = self.branch.db_pool().begin().await?;
        self.branch
            .data()
            .get(
                &mut tx,
                &self.head_locator.encode(self.branch.keys().read()),
            )
            .await
    }

    pub fn db_pool(&self) -> &db::Pool {
        self.branch.db_pool()
    }

    /// Length of this blob in bytes.
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn header_size(&self) -> usize {
        BLOB_NONCE_SIZE + mem::size_of_val(&self.len)
    }

    // Total number of blocks in this blob including the possibly partially filled final block.
    pub fn block_count(&self) -> u32 {
        // https://stackoverflow.com/questions/2745074/fast-ceiling-of-an-integer-division-in-c-c
        (1 + (self.len + self.header_size() as u64 - 1) / BLOCK_SIZE as u64)
            .try_into()
            .unwrap_or(u32::MAX)
    }

    pub fn locators(&self) -> impl Iterator<Item = Locator> {
        self.head_locator
            .sequence()
            .take(self.block_count() as usize)
    }
}

impl fmt::Debug for Core {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("blob::Core")
            .field("head_locator", &self.head_locator)
            .finish()
    }
}
