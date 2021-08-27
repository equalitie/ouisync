use super::operations::{load_block, Operations};

use super::{Cursor, OpenBlock};

use crate::{
    blob_id::BlobId, branch::Branch, crypto::NonceSequence, error::Result, locator::Locator,
};

pub(crate) struct Core {
    branch: Branch,
    locator: Locator,
    nonce_sequence: NonceSequence,
    len: u64,
    len_dirty: bool,
}

impl Core {
    /// Creates a new blob.
    pub fn create(branch: Branch, locator: Locator) -> (Self, OpenBlock) {
        let nonce_sequence = NonceSequence::new(rand::random());
        let current_block = OpenBlock::new_head(locator, &nonce_sequence);

        (
            Self {
                branch,
                locator,
                nonce_sequence,
                len: 0,
                len_dirty: false,
            },
            current_block,
        )
    }

    /// Opens an existing blob.
    pub(crate) async fn open(branch: Branch, locator: Locator) -> Result<(Self, OpenBlock)> {
        // NOTE: no need to commit this transaction because we are only reading here.
        let mut tx = branch.db_pool().begin().await?;

        let (id, buffer, auth_tag) =
            load_block(&mut tx, branch.data(), branch.cryptor(), &locator).await?;

        let mut content = Cursor::new(buffer);

        let nonce_sequence = NonceSequence::new(content.read_array());
        let nonce = nonce_sequence.get(0);

        branch
            .cryptor()
            .decrypt(&nonce, id.as_ref(), &mut content, &auth_tag)?;

        let len = content.read_u64();

        Ok((
            Self {
                branch,
                locator,
                nonce_sequence,
                len,
                len_dirty: false,
            },
            OpenBlock {
                locator,
                id,
                content,
                dirty: false,
            },
        ))
    }

    pub(crate) async fn open_first_block(&self) -> Result<OpenBlock> {
        if self.len == 0 {
            return Ok(OpenBlock::new_head(self.locator, &self.nonce_sequence));
        }

        // NOTE: no need to commit this transaction because we are only reading here.
        let mut tx = self.branch.db_pool().begin().await?;

        let (id, buffer, auth_tag) =
            load_block(&mut tx, self.branch.data(), self.branch.cryptor(), &self.locator).await?;

        let mut content = Cursor::new(buffer);
        let nonce = self.nonce_sequence.get(0);

        self.branch
            .cryptor()
            .decrypt(&nonce, id.as_ref(), &mut content, &auth_tag)?;

        Ok(OpenBlock {
            locator: self.locator,
            id,
            content,
            dirty: false,
        })
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
        &self.locator
    }

    pub fn blob_id(&self) -> &BlobId {
        match &self.locator {
            Locator::Head(blob_id) => blob_id,
            _ => unreachable!(),
        }
    }

    pub fn operations<'a>(&'a mut self, current_block: &'a mut OpenBlock) -> Operations<'a> {
        Operations::new(
            &mut self.branch,
            &mut self.locator,
            &mut self.nonce_sequence,
            &mut self.len,
            &mut self.len_dirty,
            current_block,
        )
    }
}
