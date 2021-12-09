use crate::{
    block::BlockId,
    crypto::{sign::PublicKey, AuthTag, Hash},
    index::{InnerNodeMap, LeafNodeSet, Summary},
    repository::PublicRepositoryId,
    version_vector::VersionVector,
};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Serialize, Deserialize, Debug)]
pub(crate) enum Request {
    /// Request a root node.
    RootNode { cookie: u64 },
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
    RootNode {
        cookie: u64,
        writer_id: PublicKey,
        versions: VersionVector,
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
                cookie,
                writer_id,
                versions,
                hash,
                summary,
            } => f
                .debug_struct("RootNode")
                .field("cookie", cookie)
                .field("writer_id", writer_id)
                .field("versions", versions)
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
            Self::Block { id, .. } => write!(f, "Block {{ id: {:?}, .. }}", id),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct Message {
    pub id: PublicRepositoryId,
    pub content: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) enum Content {
    Request(Request),
    Response(Response),
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
