use super::{InnerNodeMap, LeafNodeSet};
use crate::format::Hex;
use serde::{Deserialize, Serialize};
use sqlx::{
    encode::IsNull,
    error::BoxDynError,
    sqlite::{SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef},
    Decode, Encode, Sqlite, Type,
};
use std::{fmt, hash::Hasher};
use thiserror::Error;
use twox_hash::xxh3::{Hash128, HasherExt};

/// Summary info of a snapshot subtree. Contains whether the subtree has been completely downloaded
/// and the number of missing blocks in the subtree.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct Summary {
    // TODO: The `state` field is not used by the peer after deserialization. Consider using
    // `#[serde(skip)]` on it.
    pub state: NodeState,
    pub block_presence: MultiBlockPresence,
}

impl Summary {
    /// Summary indicating the subtree hasn't been completely downloaded yet.
    pub const INCOMPLETE: Self = Self {
        state: NodeState::Incomplete,
        block_presence: MultiBlockPresence::None,
    };

    pub fn from_leaves(nodes: &LeafNodeSet) -> Self {
        let mut block_presence_builder = MultiBlockPresenceBuilder::new();

        for node in nodes {
            match node.block_presence {
                SingleBlockPresence::Missing => {
                    block_presence_builder.update(MultiBlockPresence::None)
                }
                SingleBlockPresence::Expired => {
                    // If a _peer_ asks if we have a block, we tell them we do even if it's been
                    // expired.  If they ask for the block we flip its status from `Expired` to
                    // `Missing` and will try to download it again.
                    //
                    // On the other hand, if _we_ want to find out which blocks we need to
                    // download, `Expired` blocks should not make it into the list.
                    block_presence_builder.update(MultiBlockPresence::Full)
                }
                SingleBlockPresence::Present => {
                    block_presence_builder.update(MultiBlockPresence::Full)
                }
            }
        }

        Self {
            state: NodeState::Complete,
            block_presence: block_presence_builder.build(),
        }
    }

    pub fn from_inners(nodes: &InnerNodeMap) -> Self {
        let mut block_presence_builder = MultiBlockPresenceBuilder::new();
        let mut state = NodeState::Complete;

        for (_, node) in nodes {
            // We should never store empty nodes, but in case someone sends us one anyway, ignore
            // it.
            if node.is_empty() {
                continue;
            }

            block_presence_builder.update(node.summary.block_presence);
            state.update(node.summary.state);
        }

        Self {
            state,
            block_presence: block_presence_builder.build(),
        }
    }

    /// Checks whether the subtree at `self` is outdated compared to the subtree at `other` in
    /// terms of present blocks. That is, whether `other` has some blocks present that `self` is
    /// missing.
    ///
    /// NOTE: This function is NOT antisymetric, that is, `is_outdated(A, B)` does not imply
    /// !is_outdated(B, A)` (and vice-versa).
    pub fn is_outdated(&self, other: &Self) -> bool {
        self.block_presence.is_outdated(&other.block_presence)
    }

    pub fn with_state(self, state: NodeState) -> Self {
        Self { state, ..self }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[repr(u8)]
pub(crate) enum NodeState {
    Incomplete = 0, // Some nodes are missing
    Complete = 1,   // All nodes are present, but the quota check wasn't performed yet
    Approved = 2,   // Quota check passed
    Rejected = 3,   // Quota check failed
}

impl NodeState {
    pub fn is_approved(self) -> bool {
        matches!(self, Self::Approved)
    }

    pub fn update(&mut self, other: Self) {
        *self = match (*self, other) {
            (Self::Incomplete, _) | (_, Self::Incomplete) => Self::Incomplete,
            (Self::Complete, _) | (_, Self::Complete) => Self::Complete,
            (Self::Approved, Self::Approved) => Self::Approved,
            (Self::Rejected, Self::Rejected) => Self::Rejected,
            (Self::Approved, Self::Rejected) | (Self::Rejected, Self::Approved) => unreachable!(),
        }
    }
}

impl Type<Sqlite> for NodeState {
    fn type_info() -> SqliteTypeInfo {
        <u8 as Type<Sqlite>>::type_info()
    }
}

impl<'q> Encode<'q, Sqlite> for NodeState {
    fn encode_by_ref(&self, args: &mut Vec<SqliteArgumentValue<'q>>) -> IsNull {
        Encode::<Sqlite>::encode(*self as u8, args)
    }
}

impl<'r> Decode<'r, Sqlite> for NodeState {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let num = <u8 as Decode<Sqlite>>::decode(value)?;

        match num {
            0 => Ok(Self::Incomplete),
            1 => Ok(Self::Complete),
            2 => Ok(Self::Approved),
            3 => Ok(Self::Rejected),
            _ => Err(InvalidValue(num).into()),
        }
    }
}

#[derive(Debug, Error)]
#[error("invalid value: {0}")]
pub(crate) struct InvalidValue(u8);

/// Information about the presence of a single block.
#[derive(Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum SingleBlockPresence {
    Missing,
    Present,
    Expired,
}

impl SingleBlockPresence {
    pub fn is_present(self) -> bool {
        match self {
            Self::Missing | Self::Expired => false,
            Self::Present => true,
        }
    }

    pub fn is_missing(self) -> bool {
        match self {
            Self::Missing => true,
            Self::Present => false,
            Self::Expired => false,
        }
    }
}

impl Type<Sqlite> for SingleBlockPresence {
    fn type_info() -> SqliteTypeInfo {
        <u8 as Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <u8 as Type<Sqlite>>::compatible(ty)
    }
}

impl<'q> Encode<'q, Sqlite> for SingleBlockPresence {
    fn encode_by_ref(&self, args: &mut Vec<SqliteArgumentValue<'q>>) -> IsNull {
        let n = match self {
            SingleBlockPresence::Missing => 0,
            SingleBlockPresence::Present => 1,
            SingleBlockPresence::Expired => 2,
        };

        Encode::<Sqlite>::encode(n, args)
    }
}

impl<'r> Decode<'r, Sqlite> for SingleBlockPresence {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        match <u8 as Decode<'r, Sqlite>>::decode(value)? {
            0 => Ok(SingleBlockPresence::Missing),
            1 => Ok(SingleBlockPresence::Present),
            2 => Ok(SingleBlockPresence::Expired),
            n => Err(InvalidValue(n).into()),
        }
    }
}

impl fmt::Debug for SingleBlockPresence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "Missing"),
            Self::Present => write!(f, "Present"),
            Self::Expired => write!(f, "Expired"),
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
