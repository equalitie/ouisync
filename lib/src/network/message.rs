use super::peer_exchange::PexPayload;
use crate::{
    block::{BlockId, BlockNonce},
    crypto::Hash,
    format::Hex,
    index::{InnerNodeMap, LeafNodeSet, Summary, UntrustedProof},
    repository::RepositoryId,
};
use serde::{Deserialize, Serialize};
use std::{fmt, io::Write};

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
