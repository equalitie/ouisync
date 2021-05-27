use std::iter;

use crate::crypto::{Cryptor, Hash, Hashable};
use sha3::{Digest, Sha3_256};

/// A type of block identifier similar to `BlockId` but serving a different purpose. While
/// `BlockId` reflects the block content (it changes when the content change), `Locator` reflects
/// the block "location" within the filesystem. `Locator`'s purpose is to answer the question
/// "what is the n-th block of a given blob?".
/// `Locator` is unique only within a branch while `BlockId` is globally unique.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum Locator {
    /// Locator of the root block, that is, the head block of the root blob.
    Root,
    /// Locator of the head (first) block of a blob. The first element is the hash of the locator
    /// of the directory which contain the blob (more precisely, it is the locator of the head block
    /// of that directory) and the second element is the sequence number (position) of the blob
    /// within that directory.
    Head(Hash, u32),
    /// Locator of a trunk (other than first) block of a blob. The first element is the hash of the
    /// locator of the blob (the locator of the head block of the blob) and the second element is
    /// the sequence number (position) of the block within its blob.
    Trunk(Hash, u32),
}

impl Locator {
    /// Block number within the containing blob. The head block's `number` is 0, the next one is 1
    /// and so on.
    pub fn number(&self) -> u32 {
        match self {
            Self::Root | Self::Head(..) => 0,
            Self::Trunk(_, seq) => *seq,
        }
    }

    /// Secure encoding of this locator for the use in the index.
    pub fn encode(&self, cryptor: &Cryptor) -> Hash {
        let mut hasher = Sha3_256::new();

        hasher.update(self.hash().as_ref());

        match cryptor {
            Cryptor::ChaCha20Poly1305(key) => {
                hasher.update(key.as_array().as_slice().hash());
            }
            Cryptor::Null => {}
        }

        hasher.finalize().into()
    }

    /// Sequence of locators starting at `self` and continuing with the corresponding trunk
    /// locators in their sequential order.
    pub fn sequence(&self) -> impl Iterator<Item = Self> {
        let (parent_hash, seq) = self.parent_hash_and_number();
        iter::once(*self).chain((seq + 1..).map(move |seq| Self::Trunk(parent_hash, seq)))
    }

    pub fn next(&self) -> Self {
        let (parent_hash, seq) = self.parent_hash_and_number();
        Self::Trunk(
            parent_hash,
            seq.checked_add(1).expect("locator sequence limit exceeded"),
        )
    }

    fn parent_hash_and_number(&self) -> (Hash, u32) {
        match self {
            Self::Root | Self::Head(..) => (self.hash(), 0),
            Self::Trunk(parent_hash, seq) => (*parent_hash, *seq),
        }
    }
}

impl Hashable for Locator {
    fn hash(&self) -> Hash {
        let mut hasher = Sha3_256::new();

        match self {
            Self::Root => (),
            Self::Head(parent, seq) => {
                hasher.update(parent.as_ref());
                hasher.update(seq.to_le_bytes());
                hasher.update(&[0]);
            }
            Self::Trunk(parent, seq) => {
                hasher.update(parent.as_ref());
                hasher.update(seq.to_le_bytes());
                hasher.update(&[1]);
            }
        }

        hasher.finalize().into()
    }
}
