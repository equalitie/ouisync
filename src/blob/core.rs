use super::operations::{load_block, Operations};

use super::{Cursor, OpenBlock, Blob};

use crate::{
    blob_id::BlobId, branch::Branch, crypto::NonceSequence, error::Result, locator::Locator,
};

use std::sync::Arc;
use tokio::sync::Mutex;
use std::fmt;

pub struct Core {
    branch: Branch,
    head_locator: Locator,
    nonce_sequence: NonceSequence,
    len: u64,
    len_dirty: bool,
}

impl Core {
    /// Creates a new blob.
    pub(crate) fn create_blob(branch: Branch, head_locator: Locator) -> Blob {
        let nonce_sequence = NonceSequence::new(rand::random());
        let current_block = OpenBlock::new_head(head_locator, &nonce_sequence);

        Blob::new(
            Arc::new(Mutex::new(Self {
                branch: branch.clone(),
                head_locator,
                nonce_sequence,
                len: 0,
                len_dirty: false,
            })),
            head_locator,
            branch,
            current_block,
        )
    }

    /// Opens an existing blob.
    pub(crate) async fn open_blob(branch: Branch, head_locator: Locator) -> Result<Blob> {
        // NOTE: no need to commit this transaction because we are only reading here.
        let mut tx = branch.db_pool().begin().await?;

        let (id, buffer, auth_tag) =
            load_block(&mut tx, branch.data(), branch.cryptor(), &head_locator).await?;

        let mut content = Cursor::new(buffer);

        let nonce_sequence = NonceSequence::new(content.read_array());
        let nonce = nonce_sequence.get(0);

        branch
            .cryptor()
            .decrypt(&nonce, id.as_ref(), &mut content, &auth_tag)?;

        let len = content.read_u64();

        Ok(Blob::new(
            Arc::new(Mutex::new(Self {
                branch: branch.clone(),
                head_locator,
                nonce_sequence,
                len,
                len_dirty: false,
            })),
            head_locator,
            branch,
            OpenBlock {
                locator: head_locator,
                id,
                content,
                dirty: false,
            },
        ))
    }

    pub async fn reopen(core: Arc<Mutex<Core>>) -> Result<Blob> {
        let ptr = core.clone();
        let mut guard = core.lock().await;
        let core = &mut *guard;

        let current_block = core.open_first_block().await?;

        Ok(Blob::new(
            ptr,
            core.head_locator,
            core.branch.clone(),
            current_block,
        ))
    }

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
        let nonce = self.nonce_sequence.get(0);

        self.branch
            .cryptor()
            .decrypt(&nonce, id.as_ref(), &mut content, &auth_tag)?;

        Ok(OpenBlock {
            locator: self.head_locator,
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
        &self.head_locator
    }

    pub fn blob_id(&self) -> &BlobId {
        match &self.head_locator {
            Locator::Head(blob_id) => blob_id,
            _ => unreachable!(),
        }
    }

    pub(crate) fn operations<'a>(&'a mut self, current_block: &'a mut OpenBlock) -> Operations<'a> {
        Operations::new(
            &mut self.branch,
            &mut self.head_locator,
            &mut self.nonce_sequence,
            &mut self.len,
            &mut self.len_dirty,
            current_block,
        )
    }
}

impl fmt::Debug for Core {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("blob::Core")
            .field("head_locator", &self.head_locator)
            .finish()
    }
}

