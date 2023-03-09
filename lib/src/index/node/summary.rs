use crate::format::Hex;

use super::{InnerNodeMap, LeafNodeSet};
use serde::{Deserialize, Serialize};
use sqlx::{
    encode::IsNull,
    error::BoxDynError,
    sqlite::{SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef},
    Decode, Encode, Sqlite, Type,
};
use std::{fmt, hash::Hasher};
use twox_hash::xxh3::{Hash128, HasherExt};

/// Summary info of a snapshot subtree. Contains whether the subtree has been completely downloaded
/// and the number of missing blocks in the subtree.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct Summary {
    pub is_complete: bool,
    pub block_presence: MultiBlockPresence,
}

impl Summary {
    /// Summary indicating the subtree hasn't been completely downloaded yet.
    pub const INCOMPLETE: Self = Self {
        is_complete: false,
        block_presence: MultiBlockPresence::None,
    };

    /// Summary indicating that the whole subtree is complete and all its blocks present.
    pub const FULL: Self = Self {
        is_complete: true,
        block_presence: MultiBlockPresence::Full,
    };

    pub fn from_leaves(nodes: &LeafNodeSet) -> Self {
        let mut block_presence_builder = MultiBlockPresenceBuilder::new();

        for node in nodes {
            match node.block_presence {
                SingleBlockPresence::Missing => {
                    block_presence_builder.update(MultiBlockPresence::None)
                }
                SingleBlockPresence::Present => {
                    block_presence_builder.update(MultiBlockPresence::Full)
                }
            }
        }

        Self {
            is_complete: true,
            block_presence: block_presence_builder.build(),
        }
    }

    pub fn from_inners(nodes: &InnerNodeMap) -> Self {
        let mut block_presence_builder = MultiBlockPresenceBuilder::new();
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

/// Information about the presence of a single block.
#[derive(Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum SingleBlockPresence {
    Missing,
    Present,
}

impl SingleBlockPresence {
    pub fn is_present(self) -> bool {
        match self {
            Self::Missing => false,
            Self::Present => true,
        }
    }
}

impl Type<Sqlite> for SingleBlockPresence {
    fn type_info() -> SqliteTypeInfo {
        <bool as Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <bool as Type<Sqlite>>::compatible(ty)
    }
}

impl<'q> Encode<'q, Sqlite> for SingleBlockPresence {
    fn encode_by_ref(&self, args: &mut Vec<SqliteArgumentValue<'q>>) -> IsNull {
        Encode::<Sqlite>::encode(self.is_present(), args)
    }
}

impl<'r> Decode<'r, Sqlite> for SingleBlockPresence {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        if <bool as Decode<'r, Sqlite>>::decode(value)? {
            Ok(SingleBlockPresence::Present)
        } else {
            Ok(SingleBlockPresence::Missing)
        }
    }
}

impl fmt::Debug for SingleBlockPresence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "Missing"),
            Self::Present => write!(f, "Present"),
        }
    }
}

/// Summary information about the presence of multiple blocks belonging to a subtree.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub(crate) enum MultiBlockPresence {
    /// All blocks missing
    None,
    /// Some blocks present. The contained checksum is used to determine whether two subtrees have
    /// the same set of present blocks.
    Some(Checksum),
    /// All blocks present.
    Full,
}

type Checksum = [u8; 16];

const NONE: Checksum = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
const FULL: Checksum = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
];

impl MultiBlockPresence {
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

impl Type<Sqlite> for MultiBlockPresence {
    fn type_info() -> SqliteTypeInfo {
        <&[u8] as Type<Sqlite>>::type_info()
    }
}

impl<'q> Encode<'q, Sqlite> for &'q MultiBlockPresence {
    fn encode_by_ref(&self, args: &mut Vec<SqliteArgumentValue<'q>>) -> IsNull {
        Encode::<Sqlite>::encode(self.checksum(), args)
    }
}

impl<'r> Decode<'r, Sqlite> for MultiBlockPresence {
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

impl fmt::Debug for MultiBlockPresence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::Some(checksum) => write!(f, "Some({:10x})", Hex(checksum)),
            Self::Full => write!(f, "Full"),
        }
    }
}

struct MultiBlockPresenceBuilder {
    state: BuilderState,
    hasher: Hash128,
}

#[derive(Copy, Clone, Debug)]
enum BuilderState {
    Init,
    None,
    Some,
    Full,
}

impl MultiBlockPresenceBuilder {
    fn new() -> Self {
        Self {
            state: BuilderState::Init,
            hasher: Hash128::default(),
        }
    }

    fn update(&mut self, p: MultiBlockPresence) {
        self.hasher.write(p.checksum());

        self.state = match (self.state, p) {
            (BuilderState::Init, MultiBlockPresence::None) => BuilderState::None,
            (BuilderState::Init, MultiBlockPresence::Some(_)) => BuilderState::Some,
            (BuilderState::Init, MultiBlockPresence::Full) => BuilderState::Full,
            (BuilderState::None, MultiBlockPresence::None) => BuilderState::None,
            (BuilderState::None, MultiBlockPresence::Some(_))
            | (BuilderState::None, MultiBlockPresence::Full)
            | (BuilderState::Some, _)
            | (BuilderState::Full, MultiBlockPresence::None)
            | (BuilderState::Full, MultiBlockPresence::Some(_)) => BuilderState::Some,
            (BuilderState::Full, MultiBlockPresence::Full) => BuilderState::Full,
        }
    }

    fn build(self) -> MultiBlockPresence {
        match self.state {
            BuilderState::Init | BuilderState::None => MultiBlockPresence::None,
            BuilderState::Some => {
                MultiBlockPresence::Some(clamp(self.hasher.finish_ext()).to_le_bytes())
            }
            BuilderState::Full => MultiBlockPresence::Full,
        }
    }
}

// Make sure the checksum is never 0 or u128::MAX as those are special values that indicate None or
// Full respectively.
const fn clamp(s: u128) -> u128 {
    if s == 0 {
        1
    } else if s == u128::MAX {
        u128::MAX - 1
    } else {
        s
    }
}
