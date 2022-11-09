use super::{channel_id::ChannelId, peer_exchange::PexPayload};
use crate::{
    block::{BlockId, BlockNonce},
    crypto::Hash,
    format::Hex,
    index::{InnerNodeMap, LeafNodeSet, Summary, UntrustedProof},
};
use serde::{Deserialize, Serialize};
use std::{fmt, io::Write, mem};

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

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Debug)]
pub(crate) enum AckData {
    HighestSeen(Option<SeqNum>),
    Resend {
        first_missing: SeqNum,
        up_to_exclusive: SeqNum,
    },
}

impl Default for AckData {
    fn default() -> Self {
        Self::HighestSeen(None)
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Debug)]
pub(crate) struct Header {
    pub seq_num: SeqNum,
    pub ack_data: AckData,
    pub channel: ChannelId,
}

impl Header {
    pub(crate) const SIZE: usize = 1 + // One byte for a flag
        mem::size_of::<SeqNum>() + // Sequence number
        2 * mem::size_of::<SeqNum>() + // AckData
        Hash::SIZE; // Channel

    pub(crate) fn serialize(&self) -> [u8; Self::SIZE] {
        let mut hdr = [0; Self::SIZE];
        let mut w = ArrayWriter { array: &mut hdr };

        w.write_u64(self.seq_num);

        match self.ack_data {
            AckData::HighestSeen(highest) => {
                w.write_u8(0);
                if let Some(highest) = highest {
                    w.write_u64(1);
                    w.write_u64(highest);
                } else {
                    w.write_u64(0);
                    w.write_u64(0);
                }
            }
            AckData::Resend {
                first_missing,
                up_to_exclusive,
            } => {
                w.write_u8(1);
                w.write_u64(first_missing);
                w.write_u64(up_to_exclusive);
            }
        };

        w.write_channel(&self.channel);

        hdr
    }

    pub(crate) fn deserialize(hdr: &[u8; Self::SIZE]) -> Option<Header> {
        let mut r = ArrayReader { array: &hdr[..] };

        let seq_num = r.read_u64();
        let flag = r.read_u8();

        let ack_data = match flag {
            0 => {
                let is_some = r.read_u64() != 0;
                let highest_seen = r.read_u64();
                if is_some {
                    AckData::HighestSeen(Some(highest_seen))
                } else {
                    AckData::HighestSeen(None)
                }
            }
            1 => AckData::Resend {
                first_missing: r.read_u64(),
                up_to_exclusive: r.read_u64(),
            },
            _ => return None,
        };

        Some(Header {
            seq_num,
            ack_data,
            channel: r.read_channel(),
        })
    }
}

impl Default for Header {
    fn default() -> Self {
        Self {
            seq_num: 0,
            ack_data: AckData::Resend {
                first_missing: 0,
                up_to_exclusive: 0,
            },
            channel: ChannelId::default(),
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
pub(crate) struct Message {
    pub seq_num: SeqNum,
    pub ack_data: AckData,
    pub channel: ChannelId,
    pub content: Vec<u8>,
}

impl Message {
    pub fn header(&self) -> Header {
        Header {
            seq_num: self.seq_num,
            ack_data: self.ack_data,
            channel: self.channel,
        }
    }
}

impl Default for Message {
    fn default() -> Self {
        (Header::default(), Vec::new()).into()
    }
}

impl From<(Header, Vec<u8>)> for Message {
    fn from(hdr_and_content: (Header, Vec<u8>)) -> Message {
        let hdr = hdr_and_content.0;
        let content = hdr_and_content.1;

        Self {
            seq_num: hdr.seq_num,
            ack_data: hdr.ack_data,
            channel: hdr.channel,
            content: content,
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

    fn read_u64(&mut self) -> u64 {
        let n = u64::from_le_bytes(self.array[..8].try_into().unwrap());
        self.array = &self.array[8..];
        n
    }

    fn read_channel(&mut self) -> ChannelId {
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
        self.array.write(&n.to_le_bytes()).unwrap();
    }

    fn write_u64(&mut self, n: u64) {
        self.array.write(&n.to_le_bytes()).unwrap();
    }

    fn write_channel(&mut self, channel: &ChannelId) {
        self.array.write(channel.as_ref()).unwrap();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_serialization() {
        let header = Header {
            seq_num: 891,
            ack_data: AckData::HighestSeen(Some(1)),
            channel: ChannelId::random(),
        };

        let serialized = header.serialize();
        assert_eq!(Header::deserialize(&serialized), Some(header));

        let header = Header {
            seq_num: 49201,
            ack_data: AckData::Resend {
                first_missing: 123,
                up_to_exclusive: 456,
            },
            channel: ChannelId::random(),
        };

        let serialized = header.serialize();
        assert_eq!(Header::deserialize(&serialized), Some(header));
    }
}
