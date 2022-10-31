use super::{InnerNodeMap, LeafNodeSet};
use crc::{Crc, CRC_64_XZ};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// Summary info of a snapshot subtree. Contains whether the subtree has been completely downloaded
/// and the number of missing blocks in the subtree.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct Summary {
    pub(in super::super) is_complete: bool,
    pub(in super::super) missing_blocks_count: u64,
    // Checksum is used to disambiguate the situation where two replicas have exactly the same
    // missing blocks (both count and checksum would be the same) or they just happen to have the
    // same number of missing blocks, but those blocks are different (counts would be the same but
    // checksums would differ (unless there is a collision which should be rare)).
    pub(in super::super) missing_blocks_checksum: u64,
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

    /// Checks whether the subtree at `self` has potentially more missing blocks than the one at
    /// `other`.
    pub fn is_outdated(&self, other: &Self) -> bool {
        if self.missing_blocks_count == 0 {
            return false;
        }

        match self.missing_blocks_count.cmp(&other.missing_blocks_count) {
            Ordering::Less | Ordering::Greater => true,
            Ordering::Equal => self.missing_blocks_checksum != other.missing_blocks_checksum,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.is_complete
    }

    pub fn missing_blocks_count(&self) -> u64 {
        self.missing_blocks_count
    }
}
