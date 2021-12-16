use crate::{
    block::BlockId,
    crypto::{sign::PublicKey, AuthTag, Hash},
    index::{InnerNodeMap, LeafNodeSet, Summary},
    repository::SecretRepositoryId,
    version_vector::VersionVector,
};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Serialize, Deserialize, Debug)]
pub(crate) enum Request {
    /// Request inner nodes with the given parent hash and inner layer.
    InnerNodes {
        parent_hash: Hash,
        inner_layer: usize,
    },
    /// Request leaf nodes with the given parent hash.
    LeafNodes { parent_hash: Hash },
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
        writer_id: PublicKey,
        version_vector: VersionVector,
        hash: Hash,
        summary: Summary,
    },
    /// Send inner nodes with the given parent hash and inner layer.
    InnerNodes {
        parent_hash: Hash,
        inner_layer: usize,
        nodes: InnerNodeMap,
    },
    /// Send leaf nodes with the given parent hash.
    LeafNodes {
        parent_hash: Hash,
        nodes: LeafNodeSet,
    },
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
            Self::RootNode {
                writer_id,
                version_vector,
                hash,
                summary,
            } => f
                .debug_struct("RootNode")
                .field("writer_id", writer_id)
                .field("version_vector", version_vector)
                .field("hash", hash)
                .field("summary", summary)
                .finish(),
            Self::InnerNodes {
                parent_hash,
                inner_layer,
                nodes,
            } => f
                .debug_struct("InnerNodes")
                .field("parent_hash", parent_hash)
                .field("inner_layer", inner_layer)
                .field("nodes", nodes)
                .finish(),
            Self::LeafNodes { parent_hash, nodes } => f
                .debug_struct("LeafNodes")
                .field("parent", parent_hash)
                .field("nodes", nodes)
                .finish(),
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

impl<'a> From<&'a SecretRepositoryId> for MessageChannel {
    fn from(id: &'a SecretRepositoryId) -> Self {
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
