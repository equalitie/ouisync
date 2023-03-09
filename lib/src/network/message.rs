use super::{crypto::Role, peer_exchange::PexPayload, runtime_id::PublicRuntimeId};
use crate::{
    block::{BlockId, BlockNonce},
    crypto::{sign::PublicKey, Hash, Hashable},
    format::Hex,
    index::{InnerNodeMap, LeafNodeSet, MultiBlockPresence, Summary, UntrustedProof},
    repository::RepositoryId,
};
use serde::{Deserialize, Serialize};
use std::{fmt, io::Write};

#[derive(Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize, Debug)]
pub(crate) enum Request {
    /// Request root node for the given branch.
    RootNode(PublicKey),
    /// Request child nodes (inner or leaf) with the given parent hash. Responded with either
    /// `InnerNodes` or `LeafNodes` response.
    ChildNodes {
        parent_node_hash: Hash,
        trigger_block_presence: MultiBlockPresence,
    },
    /// Request block with the given id.
    Block(BlockId),
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Response {
    /// Send the latest root node of this replica to another replica.
    /// NOTE: This is both a response and notification - the server sends this as a response to
    /// `Request::RootNode` but also on its own when it detects change in the repo.
    RootNode {
        proof: UntrustedProof,
        summary: Summary,
    },
    /// Send that a RootNode request failed
    RootNodeError(PublicKey),
    /// Send inner nodes.
    InnerNodes {
        nodes: InnerNodeMap,
        trigger_block_presence: MultiBlockPresence,
    },
    /// Send leaf nodes.
    LeafNodes {
        nodes: LeafNodeSet,
        trigger_block_presence: MultiBlockPresence,
    },
    /// Send that a ChildNodes request failed
    ChildNodesError(Hash, MultiBlockPresence),
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
            Self::RootNodeError(branch_id) => {
                f.debug_tuple("RootNodeError").field(branch_id).finish()
            }
            Self::InnerNodes {
                nodes,
                trigger_block_presence,
            } => f
                .debug_struct("InnerNodes")
                .field("nodes", nodes)
                .field("trigger_block_presence", trigger_block_presence)
                .finish(),
            Self::LeafNodes {
                nodes,
                trigger_block_presence,
            } => f
                .debug_struct("LeafNodes")
                .field("nodes", nodes)
                .field("trigger_block_presence", trigger_block_presence)
                .finish(),
            Self::ChildNodesError(hash, trigger_block_presence) => f
                .debug_tuple("ChildNodesError")
                .field(hash)
                .field(trigger_block_presence)
                .finish(),
            Self::Block { content, .. } => f
                .debug_struct("Block")
                .field("content", &format_args!("{:6x}", Hex(content)))
                .finish_non_exhaustive(),
            Self::BlockError(id) => f.debug_tuple("BlockError").field(id).finish(),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Debug)]
pub(crate) enum Type {
    KeepAlive,
    Barrier,
    Content,
}

impl Type {
    fn as_u8(&self) -> u8 {
        match self {
            Type::KeepAlive => 0,
            Type::Barrier => 1,
            Type::Content => 2,
        }
    }

    fn from_u8(tag: u8) -> Option<Type> {
        match tag {
            0 => Some(Type::KeepAlive),
            1 => Some(Type::Barrier),
            2 => Some(Type::Content),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Debug)]
pub(crate) struct Header {
    pub tag: Type,
    pub channel: MessageChannel,
}

impl Header {
    pub(crate) const SIZE: usize = 1 + // One byte for the tag.
        Hash::SIZE; // Channel

    pub(crate) fn serialize(&self) -> [u8; Self::SIZE] {
        let mut hdr = [0; Self::SIZE];
        let mut w = ArrayWriter { array: &mut hdr };

        w.write_u8(self.tag.as_u8());
        w.write_channel(&self.channel);

        hdr
    }

    pub(crate) fn deserialize(hdr: &[u8; Self::SIZE]) -> Option<Header> {
        let mut r = ArrayReader { array: &hdr[..] };

        let tag = match Type::from_u8(r.read_u8()) {
            Some(tag) => tag,
            None => return None,
        };

        Some(Header {
            tag,
            channel: r.read_channel(),
        })
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
pub(crate) struct Message {
    pub tag: Type,
    pub channel: MessageChannel,
    pub content: Vec<u8>,
}

impl Message {
    pub fn new_keep_alive() -> Self {
        Self {
            tag: Type::KeepAlive,
            channel: MessageChannel::default(),
            content: Vec::new(),
        }
    }

    pub fn header(&self) -> Header {
        Header {
            tag: self.tag,
            channel: self.channel,
        }
    }

    pub fn is_keep_alive(&self) -> bool {
        self.tag == Type::KeepAlive
    }
}

impl From<(Header, Vec<u8>)> for Message {
    fn from(hdr_and_content: (Header, Vec<u8>)) -> Message {
        let hdr = hdr_and_content.0;
        let content = hdr_and_content.1;

        Self {
            tag: hdr.tag,
            channel: hdr.channel,
            content,
        }
    }
}

struct ArrayReader<'a> {
    array: &'a [u8],
}

impl ArrayReader<'_> {
    // Unwraps are OK because all sizes are known at compile time.

    fn read_u8(&mut self) -> u8 {
        let n = u8::from_le_bytes(self.array[..1].try_into().unwrap());
        self.array = &self.array[1..];
        n
    }

    fn read_channel(&mut self) -> MessageChannel {
        let hash: [u8; Hash::SIZE] = self.array[..Hash::SIZE].try_into().unwrap();
        self.array = &self.array[Hash::SIZE..];
        hash.into()
    }
}

struct ArrayWriter<'a> {
    array: &'a mut [u8],
}

impl ArrayWriter<'_> {
    // Unwraps are OK because all sizes are known at compile time.

    fn write_u8(&mut self, n: u8) {
        self.array.write_all(&n.to_le_bytes()).unwrap();
    }

    fn write_channel(&mut self, channel: &MessageChannel) {
        self.array.write_all(channel.as_ref()).unwrap();
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) enum Content {
    Request(Request),
    Response(Response),
    // Peer exchange
    Pex(PexPayload),
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

define_byte_array_wrapper! {
    // TODO: consider lower size (truncate the hash) which should still be enough to be unique
    // while reducing the message size.
    pub(crate) struct MessageChannel([u8; Hash::SIZE]);
}

impl MessageChannel {
    pub(super) fn new(
        repo_id: &'_ RepositoryId,
        this_runtime_id: &'_ PublicRuntimeId,
        that_runtime_id: &'_ PublicRuntimeId,
        role: Role,
    ) -> Self {
        let (id1, id2) = match role {
            Role::Initiator => (this_runtime_id, that_runtime_id),
            Role::Responder => (that_runtime_id, this_runtime_id),
        };

        Self(
            (repo_id, id1, id2, b"ouisync message channel id")
                .hash()
                .into(),
        )
    }

    #[cfg(test)]
    pub(crate) fn random() -> Self {
        Self(rand::random())
    }
}

impl Default for MessageChannel {
    fn default() -> Self {
        Self([0; Self::SIZE])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_serialization() {
        let header = Header {
            tag: Type::Content,
            channel: MessageChannel::random(),
        };

        let serialized = header.serialize();
        assert_eq!(Header::deserialize(&serialized), Some(header));
    }
}
