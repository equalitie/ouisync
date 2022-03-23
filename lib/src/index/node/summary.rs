use super::{InnerNodeMap, LeafNodeSet};
use crc::{Crc, CRC_64_XZ};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// Summary info of a snapshot subtree. Contains whether the subtree has been completely downloaded
/// and the number of missing blocks in the subtree.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct Summary {
    pub(super) is_complete: bool,
    pub(super) missing_blocks_count: u64,
    // Checksum is used to disambiguate the situation where two replicas have exactly the same
    // missing blocks (both count and checksum would be the same) or they just happen to have the
    // same number of missing blocks, but those blocks are different (counts would be the same but
    // checksums would differ (unless there is a collision which should be rare)).
    pub(super) missing_blocks_checksum: u64,
}

impl Summary {
    /// Summary indicating the subtree hasn't been completely downloaded yet.
    // TODO: consider renaming this to `UNKNOWN`, `UNDECIDED`, `UNDEFINED`, etc...
    pub const INCOMPLETE: Self = Self {
        is_complete: false,
        missing_blocks_count: u64::MAX,
        missing_blocks_checksum: 0,
    };

    /// Summary indicating that the whole subtree is complete and all its blocks present.
    pub const FULL: Self = Self {
        is_complete: true,
        missing_blocks_count: 0,
        missing_blocks_checksum: 0,
    };

    pub fn from_leaves(nodes: &LeafNodeSet) -> Self {
        let crc = Crc::<u64>::new(&CRC_64_XZ);
        let mut digest = crc.digest();
        let mut missing_blocks_count = 0;

        for node in nodes {
            if node.is_missing {
                missing_blocks_count += 1;
                digest.update(&[1])
            } else {
                digest.update(&[0])
            }
        }

        Self {
            is_complete: true,
            missing_blocks_count,
            missing_blocks_checksum: digest.finalize(),
        }
    }

    pub fn from_inners(nodes: &InnerNodeMap) -> Self {
        let crc = Crc::<u64>::new(&CRC_64_XZ);
        let mut digest = crc.digest();
        let mut missing_blocks_count = 0u64;
        let mut is_complete = true;

        for (_, node) in nodes {
            // We should never store empty nodes, but in case someone sends us one anyway, ignore
            // it.
            if node.is_empty() {
                continue;
            }

            // If at least one node is incomplete the whole collection is incomplete as well.
            if node.summary == Self::INCOMPLETE {
                return Self::INCOMPLETE;
            }

            is_complete = is_complete && node.summary.is_complete;
            missing_blocks_count =
                missing_blocks_count.saturating_add(node.summary.missing_blocks_count);
            digest.update(&node.summary.missing_blocks_count.to_le_bytes());
            digest.update(&node.summary.missing_blocks_checksum.to_le_bytes());
        }

        Self {
            is_complete,
            missing_blocks_count,
            missing_blocks_checksum: digest.finalize(),
        }
    }

    /// Check whether the replica with `self` is up to date with the replica with `other` in terms
    /// of the blocks they have:
    ///
    /// - `Some(true)` means this replica has all the blocks that the other replica has (and
    ///   possibly some more) which means it's up to date and no further action needs to be taken.
    /// - `Some(false)` means that there are some blocks that the other replica has that this
    ///   replica is missing and it needs to request the child nodes to learn which are they.
    /// - `None` means it's not possible to tell which replica is more up to date and this replica
    ///   must wait for the other one to make progress first.
    ///
    /// Note that if `a.is_up_to_date_with(b)` returns `None` then `b.is_up_to_date_with(a)` is
    /// guaranteed to return `Some` which means that at least one replica is always able to make
    /// progress.
    pub fn is_up_to_date_with(&self, other: &Self) -> Option<bool> {
        use Ordering::*;

        // | checksum   | count      | outcome     |
        // +------------+------------+-------------+
        // | lhs == rhs | lhs == rhs | Some(true)  |
        // | lhs == rhs | lhs >  rhs | Some(false) | unlikely (CRC collision)
        // | lhs == rhs | lhs <  rhs | None        | unlikely (CRC collision)
        // | lhs != rhs | lhs == rhs | Some(false) |
        // | lhs != rhs | lhs >  rhs | Some(false) |
        // | lhs != rhs | lhs <  rhs | None        |

        if self.missing_blocks_count == 0 {
            return Some(true);
        }

        match (
            self.missing_blocks_checksum == other.missing_blocks_checksum,
            self.missing_blocks_count.cmp(&other.missing_blocks_count),
        ) {
            (true, Equal) => Some(true),
            (_, Greater) | (false, Equal) => Some(false),
            (_, Less) => None,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.is_complete
    }

    pub fn missing_blocks_count(&self) -> u64 {
        self.missing_blocks_count
    }
}
