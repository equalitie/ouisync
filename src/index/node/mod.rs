mod inner;
mod leaf;
mod link;
mod root;
#[cfg(test)]
mod tests;

pub use self::{
    inner::{InnerNode, InnerNodeMap},
    leaf::{LeafNode, LeafNodeSet, ModifyStatus},
    root::RootNode,
};

use crate::crypto::Hash;

/// Get the bucket for `locator` at the specified `inner_layer`.
pub fn get_bucket(locator: &Hash, inner_layer: usize) -> u8 {
    locator.as_ref()[inner_layer]
}
