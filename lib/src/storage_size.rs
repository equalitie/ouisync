use crate::block::BLOCK_RECORD_SIZE;

/// Strongly typed storage size.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct StorageSize {
    bytes: u64,
}

impl StorageSize {
    pub fn from_bytes(value: u64) -> Self {
        Self { bytes: value }
    }

    pub fn from_blocks(value: u64) -> Self {
        Self {
            bytes: value * BLOCK_RECORD_SIZE,
        }
    }

    pub fn to_bytes(self) -> u64 {
        self.bytes
    }

    pub fn to_blocks(self) -> u64 {
        self.bytes / BLOCK_RECORD_SIZE
    }
}
