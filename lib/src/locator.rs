use crate::{
    blob_id::BlobId,
    crypto::{cipher::SecretKey, Hash, Hashable},
};

use sha3::{Digest, Sha3_256};

/// A type of block identifier similar to `BlockId` but serving a different purpose. While
/// `BlockId` reflects the block content (it changes when the content change), `Locator` reflects
/// the block "location" within the filesystem. `Locator`'s purpose is to answer the question
/// "what is the n-th block of a given blob?".
/// `Locator` is unique only within a branch while `BlockId` is globally unique.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub(crate) struct Locator {
    blob: BlobId,
    block: u32,
}

impl Locator {
    /// Locator of the root block, that is, the head block of the root blob.
    pub const ROOT: Self = Self {
        blob: BlobId::ZERO,
        block: 0,
    };

    /// Locator of the head block of the given blob.
    pub fn head(blob_id: BlobId) -> Self {
        Self {
            blob: blob_id,
            block: 0,
        }
    }

    /// Id of the blob this locator points into.
    pub fn blob_id(&self) -> &BlobId {
        &self.blob
    }

    /// Block number within the containing blob. The head block's `number` is 0, the next one is 1
    /// and so on.
    pub fn number(&self) -> u32 {
        self.block
    }

    /// Secure encoding of this locator for the use in the index.
    pub fn encode(&self, secret_key: &SecretKey) -> Hash {
        let mut hasher = Sha3_256::new();

        hasher.update(self.hash().as_ref());
        hasher.update(secret_key.as_ref().hash());
        hasher.finalize().into()
    }

    /// Sequence of locators starting at `self` and continuing with the corresponding trunk
    /// locators in their sequential order.
    pub fn sequence(&self) -> impl Iterator<Item = Self> {
        let blob = self.blob;
        (self.block..).map(move |block| Self { blob, block })
    }

    pub fn next(&self) -> Self {
        self.nth(1)
    }

    pub fn nth(&self, n: u32) -> Self {
        Self {
            blob: self.blob,
            block: self
                .block
                .checked_add(n)
                .expect("locator sequence limit exceeded"),
        }
    }
}

impl Hashable for Locator {
    fn hash(&self) -> Hash {
        let mut hasher = Sha3_256::new();
        hasher.update(self.blob.as_ref());
        hasher.update(self.block.to_le_bytes());
        hasher.finalize().into()
    }
}
