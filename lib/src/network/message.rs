use super::{channel_id::ChannelId, peer_exchange::PexPayload};
use crate::{
    block::{BlockId, BlockNonce},
    crypto::Hash,
    format::Hex,
    index::{InnerNodeMap, LeafNodeSet, Summary, UntrustedProof},
};
use serde::{Deserialize, Serialize};
use std::{fmt, mem};

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
pub(crate) struct Resend {
    first_missing: SeqNum,
    up_to_exclusive: SeqNum,
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Debug)]
pub(crate) enum AckData {
    LastSeen(SeqNum),
    Resend(Resend),
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
        // Unwraps are OK because all sizes are known at compile time.

        let mut hdr = [0; Self::SIZE];

        match self.ack_data {
            AckData::LastSeen(last_seen) => {
                hdr[0] = 1;
                hdr[1..9].copy_from_slice(&self.seq_num.to_le_bytes());
                hdr[9..17].copy_from_slice(&last_seen.to_le_bytes());
                hdr[17..25].copy_from_slice(&0u64.to_le_bytes());
            }
            AckData::Resend(Resend {
                first_missing,
                up_to_exclusive,
            }) => {
                hdr[0] = 0;
                hdr[1..9].copy_from_slice(&self.seq_num.to_le_bytes());
                hdr[9..17].copy_from_slice(&first_missing.to_le_bytes());
                hdr[17..25].copy_from_slice(&up_to_exclusive.to_le_bytes());
            }
        };

        hdr[25..(25 + Hash::SIZE)].copy_from_slice(self.channel.as_ref());

        hdr
    }

    pub(crate) fn deserialize(hdr: &[u8; Self::SIZE]) -> Option<Header> {
        // Unwraps are OK because all sizes are known at compile time.

        let flag = hdr[0];

        let seq_num = SeqNum::from_le_bytes(hdr[1..9].try_into().unwrap());

        let ack_data = match flag {
            0 => AckData::LastSeen(SeqNum::from_le_bytes(hdr[9..17].try_into().unwrap())),
            1 => AckData::Resend(Resend {
                first_missing: SeqNum::from_le_bytes(hdr[9..17].try_into().unwrap()),
                up_to_exclusive: SeqNum::from_le_bytes(hdr[17..25].try_into().unwrap()),
            }),
            _ => return None,
        };

        let channel: [u8; Hash::SIZE] = hdr[25..(25 + Hash::SIZE)].try_into().unwrap();

        Some(Header {
            seq_num,
            ack_data,
            channel: channel.into(),
        })
    }
}

impl Default for Header {
    fn default() -> Self {
        Self {
            seq_num: 0,
            ack_data: AckData::Resend(Resend {
                first_missing: 0,
                up_to_exclusive: 0,
            }),
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

#[derive(Serialize, Deserialize, Debug)]
pub(crate) enum Content {
    Request(Request),
    Response(Response),
    // Peer exchange
    Pex(PexPayload),
}
