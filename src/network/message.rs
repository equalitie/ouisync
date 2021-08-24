use std::fmt;

use crate::{
    block::BlockId,
    crypto::{AuthTag, Hash},
    index::{InnerNodeMap, LeafNodeSet, Summary},
    version_vector::VersionVector,
};
use serde::{Deserialize, Serialize};

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
                versions,
                hash,
                summary,
            } => f
                .debug_struct("RootNode")
                .field("cookie", cookie)
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
pub(crate) enum Message {
    Request(Request),
    Response(Response),
}

impl From<Message> for Request {
    fn from(msg: Message) -> Self {
        match msg {
            Message::Request(rq) => rq,
            Message::Response(_) => panic!("Message is not Request"),
        }
    }
}

impl From<Message> for Response {
    fn from(msg: Message) -> Self {
        match msg {
            Message::Request(_) => panic!("Message is not Response"),
            Message::Response(rs) => rs,
        }
    }
}
