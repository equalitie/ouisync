use crate::{
    block::BlockId,
    crypto::{cipher::AuthTag, Hash},
    index::{InnerNodeMap, LeafNodeSet, Summary, UntrustedProof},
    repository::RepositoryId,
};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Serialize, Deserialize, Debug)]
pub(crate) enum Request {
    /// Request child nodes (inner or leaf) with the given parent hash. Responded with either
    /// `InnerNodes` or `LeafNodes` response.
    ChildNodes(Hash),
    /// Request block with the given id.
    Block(BlockId),
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Response {
    /// Send the latest root node of this replica to another replica.
    /// NOTE: this is technically a notification, not a response. There is no corresponding
    /// `Request` variant -  the server sends these proactively any time there is change in the
    /// repo.
    RootNode {
        proof: UntrustedProof,
        summary: Summary,
    },
    /// Send inner nodes.
    InnerNodes(InnerNodeMap),
    /// Send leaf nodes.
    LeafNodes(LeafNodeSet),
    /// Send a requested block.
    Block {
        id: BlockId,
        content: Box<[u8]>,
        auth_tag: AuthTag,
    },
}

// Custom `Debug` impl to avoid printing the whole block content in the `Block` variant.
impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::RootNode { proof, summary } => f
                .debug_struct("RootNode")
                .field("proof", proof)
                .field("summary", summary)
                .finish(),
            Self::InnerNodes(nodes) => f.debug_tuple("InnerNodes").field(nodes).finish(),
            Self::LeafNodes(nodes) => f.debug_tuple("LeafNodes").field(nodes).finish(),
            Self::Block { id, .. } => f
                .debug_struct("Block")
                .field("id", id)
                .finish_non_exhaustive(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct Message {
    pub channel: MessageChannel,
    pub content: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) enum Content {
    Request(Request),
    Response(Response),
}

define_byte_array_wrapper! {
    // TODO: consider lower size (truncate the hash) which should still be enough to be unique
    // while reducing the message size.
    pub(crate) struct MessageChannel([u8; Hash::SIZE]);
}

impl MessageChannel {
    #[cfg(test)]
    pub(crate) fn random() -> Self {
        Self(rand::random())
    }
}

impl<'a> From<&'a RepositoryId> for MessageChannel {
    fn from(id: &'a RepositoryId) -> Self {
        Self(id.salted_hash(b"ouisync message channel").into())
    }
}

impl Default for MessageChannel {
    fn default() -> Self {
        Self([0; Self::SIZE])
    }
}

#[cfg(test)]
impl From<Content> for Request {
    fn from(content: Content) -> Self {
        match content {
            Content::Request(request) => request,
            Content::Response(_) => {
                panic!("not a request: {:?}", content)
            }
        }
    }
}

#[cfg(test)]
impl From<Content> for Response {
    fn from(content: Content) -> Self {
        match content {
            Content::Request(_) => {
                panic!("not a response: {:?}", content)
            }
            Content::Response(response) => response,
        }
    }
}
