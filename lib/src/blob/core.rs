use super::{
    operations::{load_block, Operations},
    {Cursor, OpenBlock},
};
use crate::{
    blob_id::BlobId, block::BlockId, branch::Branch, crypto::NonceSequence, error::Result,
    locator::Locator,
};
use std::{fmt, mem};

pub struct Core {
    pub(super) branch: Branch,
    pub(super) head_locator: Locator,
    pub(super) nonce_sequence: NonceSequence,
    pub(super) len: u64,
    pub(super) len_dirty: bool,
}

impl Core {
    pub(crate) async fn open_first_block(&self) -> Result<OpenBlock> {
        if self.len == 0 {
            return Ok(OpenBlock::new_head(self.head_locator, &self.nonce_sequence));
        }

        // NOTE: no need to commit this transaction because we are only reading here.
        let mut tx = self.branch.db_pool().begin().await?;

        let (id, buffer, auth_tag) = load_block(
            &mut tx,
            self.branch.data(),
            self.branch.cryptor(),
            &self.head_locator,
        )
        .await?;

        let mut content = Cursor::new(buffer);
        content.pos = self.header_size();

        let nonce = self.nonce_sequence.get(0);

        self.branch
            .cryptor()
            .decrypt(&nonce, id.as_ref(), &mut content, &auth_tag)?;

        Ok(OpenBlock {
            head_locator: self.head_locator,
            locator: self.head_locator,
            id,
            content,
            dirty: false,
        })
    }

    pub(crate) async fn first_block_id(branch: &Branch, head_locator: Locator) -> Result<BlockId> {
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

    pub fn branch(&self) -> &Branch {
        &self.branch
    }

    /// Locator of this blob.
    pub fn locator(&self) -> &Locator {
        &self.head_locator
    }

    pub fn blob_id(&self) -> &BlobId {
        match &self.head_locator {
            Locator::Head(blob_id) => blob_id,
            _ => unreachable!(),
        }
    }

    pub(crate) fn operations<'a>(&'a mut self, current_block: &'a mut OpenBlock) -> Operations<'a> {
        Operations::new(self, current_block)
    }

    pub(crate) fn header_size(&self) -> usize {
        self.nonce_sequence.prefix().len() + mem::size_of_val(&self.len)
    }
}

impl fmt::Debug for Core {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("blob::Core")
            .field("head_locator", &self.head_locator)
            .finish()
    }
}
