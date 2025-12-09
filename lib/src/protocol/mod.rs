//! Datatypes for the ouisync protocol and for interoperability between the network layer and the
//! storage later

mod block;
mod bump;
mod inner_node;
mod leaf_node;
mod locator;
mod proof;
mod repository;
mod root_node;
mod storage_size;
mod summary;

#[cfg(test)]
pub(crate) mod test_utils;

pub use self::block::{BLOCK_RECORD_SIZE, BLOCK_SIZE};
pub use self::{
    proof::{Proof, UntrustedProof},
    repository::RepositoryId,
    root_node::RootNode,
    storage_size::StorageSize,
    summary::{MultiBlockPresence, NodeState, SingleBlockPresence, Summary},
};

pub(crate) use self::{
    block::{Block, BlockContent, BlockId, BlockNonce},
    bump::Bump,
    inner_node::{EMPTY_INNER_HASH, INNER_LAYER_COUNT, InnerNode, InnerNodes, get_bucket},
    leaf_node::{EMPTY_LEAF_HASH, LeafNode, LeafNodes},
    locator::Locator,
    proof::ProofError,
    root_node::{RootNodeFilter, RootNodeKind},
};

#[cfg(test)]
pub(crate) use self::{block::BLOCK_NONCE_SIZE, root_node::SnapshotId};
