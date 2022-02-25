use crate::block::BLOCK_SIZE;
use std::mem;

// Using u64 instead of usize because HEADER_SIZE must be the same irrespective of whether we're on
// a 32bit or 64bit processor (if we want two such replicas to be able to sync).
pub(super) const HEADER_SIZE: usize = mem::size_of::<u64>();

#[derive(Debug)]
pub(crate) struct Core {
    pub len: u64,
    pub len_dirty: bool,
}

impl Core {
    // Total number of blocks in this blob including the possibly partially filled final block.
    pub fn block_count(&self) -> u32 {
        // https://stackoverflow.com/questions/2745074/fast-ceiling-of-an-integer-division-in-c-c
        (1 + (self.len + HEADER_SIZE as u64 - 1) / BLOCK_SIZE as u64)
            .try_into()
            .unwrap_or(u32::MAX)
    }
}
