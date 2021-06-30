use super::{InnerNodeMap, LeafNodeSet};
use crc::{Crc, CRC_64_XZ};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

#[derive(Default, Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct MissingBlocksSummary {
    pub(super) count: u64,
    pub(super) checksum: u64,
}

impl MissingBlocksSummary {
    /// Placeholder value indicating that all blocks are missing.
    pub const ALL: Self = Self {
        count: u64::MAX,
        checksum: 0,
    };

    pub fn from_leaves(nodes: &LeafNodeSet) -> Self {
        let crc = Crc::<u64>::new(&CRC_64_XZ);
        let mut digest = crc.digest();
        let mut count = 0;

        for node in nodes {
            if node.is_missing {
                count += 1;
                digest.update(&[1])
            } else {
                digest.update(&[0])
            }
        }

        Self {
            count,
            checksum: digest.finalize(),
        }
    }

    pub fn from_inners(nodes: &InnerNodeMap) -> Self {
        let crc = Crc::<u64>::new(&CRC_64_XZ);
        let mut digest = crc.digest();
        let mut count = 0u64;

        for (_, node) in nodes {
            digest.update(&node.missing_blocks.count.to_le_bytes());
            digest.update(&node.missing_blocks.checksum.to_le_bytes());
            count = count.saturating_add(node.missing_blocks.count);
        }

        Self {
            count,
            checksum: digest.finalize(),
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
        // | checksum   | count      | outcome     |
        // +------------+------------+-------------+
        // | lhs == rhs | lhs == rhs | Some(true)  |
        // | lhs == rhs | lhs >  rhs | Some(false) | unlikely (CRC collision)
        // | lhs == rhs | lhs <  rhs | None        | unlikely (CRC collision)
        // | lhs != rhs | lhs == rhs | Some(false) |
        // | lhs != rhs | lhs >  rhs | Some(false) |
        // | lhs != rhs | lhs <  rhs | None        |

        use Ordering::*;

        match (
            self.checksum == other.checksum,
            self.count.cmp(&other.count),
        ) {
            (true, Equal) => Some(true),
            (_, Greater) | (false, Equal) => Some(false),
            (_, Less) => None,
        }
    }
}
