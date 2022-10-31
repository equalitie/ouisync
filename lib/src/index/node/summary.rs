use super::{InnerNodeMap, LeafNodeSet};
use crc::{Crc, CRC_64_XZ};
use serde::{Deserialize, Serialize};
use sqlx::{
    encode::IsNull,
    error::BoxDynError,
    sqlite::{SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef},
    Decode, Encode, Sqlite, Type,
};
use thiserror::Error;

/// Summary info of a snapshot subtree. Contains whether the subtree has been completely downloaded
/// and the number of missing blocks in the subtree.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct Summary {
    pub is_complete: bool,
    pub block_presence: BlockPresence,
    // Checksum is used to determine whether two nodes with the same `block_presence` have also the
    // same set of present blocks. Only used if `block_presence` is `Some`, otherwise it's zero.
    pub block_presence_checksum: u64,
}

impl Summary {
    /// Summary indicating the subtree hasn't been completely downloaded yet.
    pub const INCOMPLETE: Self = Self {
        is_complete: false,
        block_presence: BlockPresence::None,
        block_presence_checksum: 0,
    };

    /// Summary indicating that the whole subtree is complete and all its blocks present.
    pub const FULL: Self = Self {
        is_complete: true,
        block_presence: BlockPresence::Full,
        block_presence_checksum: 0,
    };

    pub fn from_leaves(nodes: &LeafNodeSet) -> Self {
        let crc = Crc::<u64>::new(&CRC_64_XZ);
        let mut digest = crc.digest();
        let mut present = 0;

        for node in nodes {
            if node.is_missing {
                digest.update(&[BlockPresence::None as u8])
            } else {
                digest.update(&[BlockPresence::Full as u8]);
                present += 1;
            }
        }

        let (block_presence, block_presence_checksum) = if present == 0 {
            (BlockPresence::None, 0)
        } else if present < nodes.len() {
            (BlockPresence::Some, digest.finalize())
        } else {
            (BlockPresence::Full, 0)
        };

        Self {
            is_complete: true,
            block_presence,
            block_presence_checksum,
        }
    }

    pub fn from_inners(nodes: &InnerNodeMap) -> Self {
        let crc = Crc::<u64>::new(&CRC_64_XZ);
        let mut digest = crc.digest();
        let mut block_presence = BlockPresence::None;
        let mut is_complete = true;

        for (index, (_, node)) in nodes.into_iter().enumerate() {
            // We should never store empty nodes, but in case someone sends us one anyway, ignore
            // it.
            if node.is_empty() {
                continue;
            }

            block_presence = match (index, block_presence, node.summary.block_presence) {
                (0, _, _) => node.summary.block_presence,
                (_, BlockPresence::None, BlockPresence::None) => BlockPresence::None,
                (_, BlockPresence::None, BlockPresence::Some | BlockPresence::Full) => {
                    BlockPresence::Some
                }
                (_, BlockPresence::Some, _) => BlockPresence::Some,
                (_, BlockPresence::Full, BlockPresence::None | BlockPresence::Some) => {
                    BlockPresence::Some
                }
                (_, BlockPresence::Full, BlockPresence::Full) => BlockPresence::Full,
            };

            is_complete = is_complete && node.summary.is_complete;
            digest.update(&[node.summary.block_presence as u8]);
            digest.update(&node.summary.block_presence_checksum.to_le_bytes());
        }

        let block_presence_checksum = match block_presence {
            BlockPresence::Some => digest.finalize(),
            BlockPresence::None | BlockPresence::Full => 0,
        };

        Self {
            is_complete,
            block_presence,
            block_presence_checksum,
        }
    }

    /// Checks whether the subtree at `self` is outdated compared to the subtree at `other` in
    /// terms of present blocks. That is, whether `other` has some blocks present that `self` is
    /// missing.
    pub fn is_outdated(&self, other: &Self) -> bool {
        match (self.block_presence, other.block_presence) {
            (_, BlockPresence::None) | (BlockPresence::Full, _) => false,
            (BlockPresence::None, BlockPresence::Some)
            | (BlockPresence::None, BlockPresence::Full)
            | (BlockPresence::Some, BlockPresence::Full) => true,
            (BlockPresence::Some, BlockPresence::Some) => {
                self.block_presence_checksum != other.block_presence_checksum
            }
        }
    }

    pub fn is_complete(&self) -> bool {
        self.is_complete
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[repr(u8)]
pub(crate) enum BlockPresence {
    /// No blocks are present / all are missing
    None = 0,
    /// Some blocks are present / some are missing
    Some = 1,
    /// All blocks are present / none are missing
    Full = 2,
}

impl From<BlockPresence> for u8 {
    fn from(block_presence: BlockPresence) -> Self {
        block_presence as u8
    }
}

impl TryFrom<u8> for BlockPresence {
    type Error = OutOfRange;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        const NONE: u8 = BlockPresence::None as u8;
        const SOME: u8 = BlockPresence::Some as u8;
        const FULL: u8 = BlockPresence::Full as u8;

        match value {
            NONE => Ok(Self::None),
            SOME => Ok(Self::Some),
            FULL => Ok(Self::Full),
            _ => Err(OutOfRange),
        }
    }
}

#[derive(Debug, Error)]
#[error("value out of range")]
pub(crate) struct OutOfRange;

impl Type<Sqlite> for BlockPresence {
    fn type_info() -> SqliteTypeInfo {
        <u8 as Type<Sqlite>>::type_info()
    }
}

impl<'q> Encode<'q, Sqlite> for BlockPresence {
    fn encode_by_ref(&self, args: &mut Vec<SqliteArgumentValue<'q>>) -> IsNull {
        Encode::<Sqlite>::encode_by_ref(&(*self as u8), args)
    }
}

impl<'r> Decode<'r, Sqlite> for BlockPresence {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let num = <u8 as Decode<Sqlite>>::decode(value)?;
        Ok(num.try_into()?)
    }
}
