use super::HEADER_SIZE;
use crate::protocol::BLOCK_SIZE;
use std::cmp::Ordering;

/// Position of the read/write cursor in a blob.
#[derive(Copy, Clone)]
pub(super) struct Position {
    /// Number of the current block
    pub block: u32,
    /// Byte offset within the current block
    pub offset: usize,
}

impl Position {
    /// Position at the beginning of the blob.
    pub const ZERO: Self = Self {
        block: 0,
        offset: HEADER_SIZE,
    };

    /// Gets the byte offset from the beginning of the blob
    pub fn get(&self) -> u64 {
        self.block as u64 * BLOCK_SIZE as u64 + self.offset as u64 - HEADER_SIZE as u64
    }

    /// Sets the byte offset from the beginning of the blob
    pub fn set(&mut self, pos: u64) {
        let actual_pos = pos + HEADER_SIZE as u64;
        self.block = (actual_pos / BLOCK_SIZE as u64) as u32;
        self.offset = (actual_pos % BLOCK_SIZE as u64) as usize;
    }

    /// Moves the position by `n` bytes but at most to the beginning of the next block.
    ///
    /// # Panics
    ///
    /// Panics if attempt to advance past the beginning of the next block.
    pub fn advance(&mut self, n: usize) {
        match (self.offset + n).cmp(&BLOCK_SIZE) {
            Ordering::Less => {
                self.offset += n;
            }
            Ordering::Equal => {
                self.block += 1;
                self.offset = 0;
            }
            Ordering::Greater => panic!("advancing too far"),
        }
    }
}
