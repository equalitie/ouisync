//! Innternal state of Blob

use super::OpenBlock;
use crate::{branch::Branch, locator::Locator};

pub(super) struct Inner {
    pub branch: Branch,
    pub head_locator: Locator,
    pub current_block: OpenBlock,
    pub len_dirty: bool,
}
