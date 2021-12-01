use super::{
    operations::{load_block, Operations},
    Cursor, Nonce, OpenBlock, NONCE_SIZE,
};
use crate::{
    block::BlockId, branch::Branch, crypto::Cryptor, error::Result, locator::Locator, Error,
};
use std::{fmt, mem};

pub(crate) struct Core {
    pub branch: Branch,
    pub head_locator: Locator,
    pub blob_key: Cryptor,
    pub len: u64,
    pub len_dirty: bool,
}

impl Core {
    pub async fn open_first_block(&mut self) -> Result<OpenBlock> {
        // No need to commit this as we are only reading here.
        let mut tx = self.branch.db_pool().begin().await?;

        match load_block(
            &mut tx,
            self.branch.data(),
            self.branch.cryptor(),
            &self.head_locator,
        )
        .await
        {
            Ok((id, buffer, auth_tag)) => {
                let mut content = Cursor::new(buffer);
                content.pos = self.header_size();

                self.blob_key
                    .decrypt(0, id.as_ref(), &mut content, &auth_tag)?;

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

                let nonce: Nonce = rand::random();
                self.blob_key = self.branch.cryptor().derive_subkey(&nonce);

                Ok(OpenBlock::new_head(self.head_locator, &nonce))
            }
            Err(error) => Err(error),
        }
    }

    pub async fn first_block_id(branch: &Branch, head_locator: Locator) -> Result<BlockId> {
        // NOTE: no need to commit this transaction because we are only reading here.
        let mut tx = branch.db_pool().begin().await?;
        branch
            .data()
            .get(&mut tx, &head_locator.encode(branch.cryptor()))
            .await
    }

    /// Length of this blob in bytes.
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn operations<'a>(&'a mut self, current_block: &'a mut OpenBlock) -> Operations<'a> {
        Operations::new(self, current_block)
    }

    pub fn header_size(&self) -> usize {
        NONCE_SIZE + mem::size_of_val(&self.len)
    }
}

impl fmt::Debug for Core {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("blob::Core")
            .field("head_locator", &self.head_locator)
            .finish()
    }
}
