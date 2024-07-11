//! Datatypes for the ouisync protocol and for interoperability between the network layer and the
//! storage later

mod block;
mod bump;
mod inner_node;
mod leaf_node;
mod locator;
pub mod proof;
pub mod root_node;
pub mod summary;

#[cfg(test)]
pub(crate) mod test_utils;

pub use self::block::BLOCK_SIZE;

pub use self::{
    proof::{Proof, UntrustedProof},
    root_node::RootNode,
    summary::{MultiBlockPresence, NodeState, SingleBlockPresence, Summary},
};

pub(crate) use self::{
    block::{Block, BlockContent, BlockId, BlockNonce, BLOCK_RECORD_SIZE},
    bump::Bump,
    inner_node::{get_bucket, InnerNode, InnerNodes, EMPTY_INNER_HASH, INNER_LAYER_COUNT},
    leaf_node::{LeafNode, LeafNodes, EMPTY_LEAF_HASH},
    locator::Locator,
    proof::ProofError,
    root_node::{RootNodeFilter, RootNodeKind},
};

#[cfg(test)]
pub(crate) use self::block::BLOCK_NONCE_SIZE;
