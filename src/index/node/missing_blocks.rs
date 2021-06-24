use super::{InnerNodeMap, LeafNodeSet};
use crc::{Crc, CRC_64_XZ};
use serde::{Deserialize, Serialize};

#[derive(Default, Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct MissingBlocksSummary {
    pub(super) count: u64,
    pub(super) checksum: u64,
}

impl MissingBlocksSummary {
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
        let mut count = 0;

        for (_, node) in nodes {
            digest.update(&node.missing_blocks.count.to_le_bytes());
            digest.update(&node.missing_blocks.checksum.to_le_bytes());
            count += node.missing_blocks.count;
        }

        Self {
            count,
            checksum: digest.finalize(),
        }
    }
}

// | checksum     | count        | outcome   |
// +--------------+--------------+-----------+
// | our == their | our == their | stop      |
// | our == their | our >  their | continue  |
// | our == their | our <  their | undecided |
// | our != their | our == their | continue  |
// | our != their | our >  their | continue  |
// | our != their | our <  their | undecided |
