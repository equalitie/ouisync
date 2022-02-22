use crate::{block::BLOCK_SIZE, branch::Branch, locator::Locator};
use std::{fmt, mem};

pub(super) const HEADER_SIZE: usize = mem::size_of::<usize>();

pub(crate) struct Core {
    pub branch: Branch,
    pub head_locator: Locator,
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

    pub fn locators(&self) -> impl Iterator<Item = Locator> {
        self.head_locator
            .sequence()
            .take(self.block_count() as usize)
    }
}

impl fmt::Debug for Core {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("blob::Core")
            .field("head_locator", &self.head_locator)
            .finish()
    }
}
