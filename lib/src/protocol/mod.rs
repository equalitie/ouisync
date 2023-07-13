//! Datatypes for the ouisync protocol and for interoperability between the network layer and the
//! storage later

mod inner_node;
mod leaf_node;
mod proof;
mod root_node;
mod summary;
mod version_vector_op;

#[cfg(test)]
pub(crate) mod test_utils;

pub(crate) use self::{
    inner_node::{get_bucket, InnerNode, InnerNodeMap, EMPTY_INNER_HASH, INNER_LAYER_COUNT},
    leaf_node::{LeafNode, LeafNodeModifyStatus, LeafNodeSet, EMPTY_LEAF_HASH},
    proof::{Proof, ProofError, UntrustedProof},
    root_node::RootNode,
    summary::{MultiBlockPresence, NodeState, SingleBlockPresence, Summary},
    version_vector_op::VersionVectorOp,
};
