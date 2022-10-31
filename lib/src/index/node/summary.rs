use super::{InnerNodeMap, LeafNodeSet};
use crate::db;
use crc::{Crc, Digest, CRC_64_XZ};
use serde::{Deserialize, Serialize};
use sqlx::{
    encode::IsNull,
    error::BoxDynError,
    sqlite::{SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef},
    Database, Decode, Encode, Sqlite, Type,
};
use std::fmt;

/// Summary info of a snapshot subtree. Contains whether the subtree has been completely downloaded
/// and the number of missing blocks in the subtree.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct Summary {
    pub is_complete: bool,
    pub block_presence: BlockPresence,
}

impl Summary {
    /// Summary indicating the subtree hasn't been completely downloaded yet.
    pub const INCOMPLETE: Self = Self {
        is_complete: false,
        block_presence: BlockPresence::NONE,
    };

    /// Summary indicating that the whole subtree is complete and all its blocks present.
    pub const FULL: Self = Self {
        is_complete: true,
        block_presence: BlockPresence::FULL,
    };

    pub fn from_leaves(nodes: &LeafNodeSet) -> Self {
        let mut block_presence_builder = BlockPresenceBuilder::new();

        for node in nodes {
            if node.is_missing {
                block_presence_builder.update(BlockPresence::NONE);
            } else {
                block_presence_builder.update(BlockPresence::FULL);
            }
        }

        Self {
            is_complete: true,
            block_presence: block_presence_builder.build(),
        }
    }

    pub fn from_inners(nodes: &InnerNodeMap) -> Self {
        let mut block_presence_builder = BlockPresenceBuilder::new();
        let mut is_complete = true;

        for (_, node) in nodes {
            // We should never store empty nodes, but in case someone sends us one anyway, ignore
            // it.
            if node.is_empty() {
                continue;
            }

            block_presence_builder.update(node.summary.block_presence);
            is_complete = is_complete && node.summary.is_complete;
        }

        Self {
            is_complete,
            block_presence: block_presence_builder.build(),
        }
    }

    /// Checks whether the subtree at `self` is outdated compared to the subtree at `other` in
    /// terms of present blocks. That is, whether `other` has some blocks present that `self` is
    /// missing.
    pub fn is_outdated(&self, other: &Self) -> bool {
        self.block_presence.is_outdated(other.block_presence)
    }

    pub fn is_complete(&self) -> bool {
        self.is_complete
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct BlockPresence {
    checksum: u64,
}

impl BlockPresence {
    pub const NONE: Self = Self { checksum: 0 };
    pub const FULL: Self = Self { checksum: u64::MAX };

    const fn some(checksum: u64) -> Self {
        Self { checksum }
    }

    #[cfg(test)]
    pub fn is_some(self) -> bool {
        self != Self::NONE && self != Self::FULL
    }

    pub fn is_outdated(self, them: Self) -> bool {
        self.checksum != them.checksum
            && self.checksum != BlockPresence::FULL.checksum
            && them.checksum != BlockPresence::NONE.checksum
    }
}

impl fmt::Debug for BlockPresence {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self == &Self::NONE {
            write!(f, "BlockPresence::NONE")
        } else if self == &Self::FULL {
            write!(f, "BlockPresence::FULL")
        } else {
            write!(f, "BlockPresence::some({})", self.checksum)
        }
    }
}

impl Type<Sqlite> for BlockPresence {
    fn type_info() -> SqliteTypeInfo {
        <i64 as Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &<Sqlite as Database>::TypeInfo) -> bool {
        // NOTE: i64 is internally `DataType::Int64` but an INTEGER column is `DataType::Int`
        // (`i32` is also `DataType::Int`) so we need to declare compatibility with both otherwise
        // we get error on decoding. No idea why sqlx even distinguishes these two type when sqlite
        // itself doesn't.
        ty == &Self::type_info() || ty == &<i32 as Type<Sqlite>>::type_info()
    }
}

impl<'q> Encode<'q, Sqlite> for BlockPresence {
    fn encode_by_ref(&self, args: &mut Vec<SqliteArgumentValue<'q>>) -> IsNull {
        Encode::<Sqlite>::encode_by_ref(&db::encode_u64(self.checksum), args)
    }
}

impl<'r> Decode<'r, Sqlite> for BlockPresence {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let checksum = <i64 as Decode<Sqlite>>::decode(value)?;

        Ok(Self {
            checksum: db::decode_u64(checksum),
        })
    }
}

struct BlockPresenceBuilder {
    state: BuilderState,
    digest: Digest<'static, u64>,
}

#[derive(Copy, Clone, Debug)]
enum BuilderState {
    Init,
    None,
    Some,
    Full,
}

impl BlockPresenceBuilder {
    fn new() -> Self {
        const CRC: Crc<u64> = Crc::<u64>::new(&CRC_64_XZ);

        Self {
            state: BuilderState::Init,
            digest: CRC.digest(),
        }
    }

    fn update(&mut self, p: BlockPresence) {
        self.digest.update(&p.checksum.to_le_bytes());

        self.state = match (self.state, p) {
            (BuilderState::Init, BlockPresence::NONE) => BuilderState::None,
            (BuilderState::Init, BlockPresence::FULL) => BuilderState::Full,
            (BuilderState::Init, _ /* some        */) => BuilderState::Some,
            (BuilderState::None, BlockPresence::NONE) => BuilderState::None,
            (BuilderState::None, BlockPresence::FULL) => BuilderState::Some,
            (BuilderState::None, _ /* some        */) => BuilderState::Some,
            (BuilderState::Some, _) => BuilderState::Some,
            (BuilderState::Full, BlockPresence::NONE) => BuilderState::Some,
            (BuilderState::Full, BlockPresence::FULL) => BuilderState::Full,
            (BuilderState::Full, _ /* some        */) => BuilderState::Some,
        }
    }

    fn build(self) -> BlockPresence {
        match self.state {
            BuilderState::Init | BuilderState::None => BlockPresence::NONE,
            BuilderState::Some => BlockPresence::some(sanitize_digest(self.digest.finalize())),
            BuilderState::Full => BlockPresence::FULL,
        }
    }
}

// Make sure the checksum is never 0 or u64::MAX as those are special values that indicate NONE or
// FULL respectively.
const fn sanitize_digest(s: u64) -> u64 {
    if s == 0 {
        1
    } else if s == u64::MAX {
        u64::MAX - 1
    } else {
        s
    }
}
