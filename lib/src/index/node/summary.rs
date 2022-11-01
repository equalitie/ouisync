use super::{InnerNodeMap, LeafNodeSet};
use crc::{Crc, Digest, CRC_64_XZ};
use serde::{Deserialize, Serialize};
use sqlx::{
    encode::IsNull,
    error::BoxDynError,
    sqlite::{SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef},
    Decode, Encode, Sqlite, Type,
};

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
        block_presence: BlockPresence::None,
    };

    /// Summary indicating that the whole subtree is complete and all its blocks present.
    pub const FULL: Self = Self {
        is_complete: true,
        block_presence: BlockPresence::Full,
    };

    pub fn from_leaves(nodes: &LeafNodeSet) -> Self {
        let mut block_presence_builder = BlockPresenceBuilder::new();

        for node in nodes {
            if node.is_missing {
                block_presence_builder.update(BlockPresence::None);
            } else {
                block_presence_builder.update(BlockPresence::Full);
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
        self.block_presence.is_outdated(&other.block_presence)
    }

    pub fn is_complete(&self) -> bool {
        self.is_complete
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) enum BlockPresence {
    None,
    Some(Checksum),
    Full,
}

type Checksum = [u8; 8];
const NONE: Checksum = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
const FULL: Checksum = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];

impl BlockPresence {
    pub fn is_outdated(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Some(lhs), Self::Some(rhs)) => lhs != rhs,
            (Self::Full, _) | (_, Self::None) => false,
            (Self::None, _) | (_, Self::Full) => true,
        }
    }

    fn checksum(&self) -> &[u8] {
        match self {
            Self::None => NONE.as_slice(),
            Self::Some(checksum) => checksum.as_slice(),
            Self::Full => FULL.as_slice(),
        }
    }
}

impl Type<Sqlite> for BlockPresence {
    fn type_info() -> SqliteTypeInfo {
        <&[u8] as Type<Sqlite>>::type_info()
    }
}

impl<'q> Encode<'q, Sqlite> for &'q BlockPresence {
    fn encode_by_ref(&self, args: &mut Vec<SqliteArgumentValue<'q>>) -> IsNull {
        Encode::<Sqlite>::encode(self.checksum(), args)
    }
}

impl<'r> Decode<'r, Sqlite> for BlockPresence {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let slice = <&[u8] as Decode<Sqlite>>::decode(value)?;
        let array = slice.try_into()?;

        match array {
            NONE => Ok(Self::None),
            FULL => Ok(Self::Full),
            _ => Ok(Self::Some(array)),
        }
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
        self.digest.update(p.checksum());

        self.state = match (self.state, p) {
            (BuilderState::Init, BlockPresence::None) => BuilderState::None,
            (BuilderState::Init, BlockPresence::Some(_)) => BuilderState::Some,
            (BuilderState::Init, BlockPresence::Full) => BuilderState::Full,
            (BuilderState::None, BlockPresence::None) => BuilderState::None,
            (BuilderState::None, BlockPresence::Some(_))
            | (BuilderState::None, BlockPresence::Full)
            | (BuilderState::Some, _)
            | (BuilderState::Full, BlockPresence::None)
            | (BuilderState::Full, BlockPresence::Some(_)) => BuilderState::Some,
            (BuilderState::Full, BlockPresence::Full) => BuilderState::Full,
        }
    }

    fn build(self) -> BlockPresence {
        match self.state {
            BuilderState::Init | BuilderState::None => BlockPresence::None,
            BuilderState::Some => {
                BlockPresence::Some(sanitize_digest(self.digest.finalize()).to_le_bytes())
            }
            BuilderState::Full => BlockPresence::Full,
        }
    }
}

// Make sure the checksum is never 0 or u64::MAX as those are special values that indicate None or
// Full respectively.
const fn sanitize_digest(s: u64) -> u64 {
    if s == 0 {
        1
    } else if s == u64::MAX {
        u64::MAX - 1
    } else {
        s
    }
}
