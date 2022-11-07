use super::peer_exchange::PexPayload;
use crate::{
    block::{BlockId, BlockNonce},
    crypto::Hash,
    format::Hex,
    index::{InnerNodeMap, LeafNodeSet, Summary, UntrustedProof},
    repository::RepositoryId,
};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize, Debug)]
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
    /// Send that a ChildNodes request failed
    ChildNodesError(Hash),
    /// Send a requested block.
    Block {
        content: Box<[u8]>,
        nonce: BlockNonce,
    },
    /// Send that a Block request failed
    BlockError(BlockId),
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
            Self::ChildNodesError(hash) => f.debug_tuple("ChildNodesError").field(hash).finish(),
            Self::Block { content, .. } => f
                .debug_struct("Block")
                .field("content", &format_args!("{:6x}", Hex(content)))
                .finish_non_exhaustive(),
            Self::BlockError(id) => f.debug_tuple("BlockError").field(id).finish(),
        }
    }
}

type SeqNum = u64;

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
pub(crate) struct Message {
    pub seq_num: SeqNum,
    pub channel: MessageChannel,
    pub content: Vec<u8>,
}

impl Message {
    pub(crate) const HEADER_SIZE: usize = Hash::SIZE + std::mem::size_of::<SeqNum>();

    pub(crate) fn new_keep_alive() -> Self {
        Self {
            seq_num: 0,
            channel: MessageChannel::default(),
            content: Vec::new(),
        }
    }

    pub(crate) fn header(&self) -> [u8; Self::HEADER_SIZE] {
        let mut hdr = [0; Self::HEADER_SIZE];
        hdr.copy_from_slice(&self.channel.0);
        hdr[Hash::SIZE..].copy_from_slice(&self.seq_num.to_le_bytes());
        hdr
    }

    pub(crate) fn parse_header(hdr: &[u8; Self::HEADER_SIZE]) -> (SeqNum, MessageChannel) {
        // Unwraps OK because sizes are know at compile time.
        let channel = hdr[0..Hash::SIZE].try_into().unwrap();
        let seq_num = SeqNum::from_le_bytes(hdr[Hash::SIZE..].try_into().unwrap());
        (seq_num, MessageChannel(channel))
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) enum Content {
    Request(Request),
    Response(Response),
    // Peer exchange
    Pex(PexPayload),
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
            Content::Response(_) | Content::Pex(_) => {
                panic!("not a request: {:?}", content)
            }
        }
    }
}

#[cfg(test)]
impl From<Content> for Response {
    fn from(content: Content) -> Self {
        match content {
            Content::Response(response) => response,
            Content::Request(_) | Content::Pex(_) => {
                panic!("not a response: {:?}", content)
            }
        }
    }
}
