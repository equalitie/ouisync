use super::{
    debug_payload::{DebugRequest, DebugResponse},
    peer_exchange::PexPayload,
};
use crate::{
    crypto::{sign::PublicKey, Hash},
    protocol::{
        BlockContent, BlockId, BlockNonce, InnerNodes, LeafNodes, MultiBlockPresence,
        UntrustedProof,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize, Debug)]
pub(crate) enum Request {
    /// Request the latest root node of the given writer.
    RootNode {
        writer_id: PublicKey,
        // This value is returned in the response without change. It can be used to distinguish
        // multiple otherwise identical requests. This is useful because multiple identical
        // `RootNode` requests can yield different responses.
        cookie: u64,
        debug: DebugRequest,
    },
    /// Request child nodes of the given parent node.
    ChildNodes(Hash, ResponseDisambiguator, DebugRequest),
    /// Request block with the given id.
    Block(BlockId, DebugRequest),
}

/// ResponseDisambiguator is used to uniquelly assign a response to a request.
/// What we want to avoid is that an outdated response clears out a newer pending request.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize, Debug)]
#[serde(transparent)]
pub(crate) struct ResponseDisambiguator(MultiBlockPresence);

impl ResponseDisambiguator {
    pub fn new(multi_block_presence: MultiBlockPresence) -> Self {
        Self(multi_block_presence)
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) enum Response {
    /// Send the latest root node of this replica to another replica.
    /// NOTE: This is both a response and notification - the server sends this as a response to
    /// `Request::RootNode` but also on its own when it detects change in the repo.
    RootNode {
        proof: UntrustedProof,
        block_presence: MultiBlockPresence,
        // If this is a reponse, the `cookie` value from the request. If this is a notification,
        // zero.
        cookie: u64,
        debug: DebugResponse,
    },
    /// Send that a RootNode request failed
    RootNodeError {
        writer_id: PublicKey,
        cookie: u64,
        debug: DebugResponse,
    },
    /// Send inner nodes.
    InnerNodes(InnerNodes, ResponseDisambiguator, DebugResponse),
    /// Send leaf nodes.
    LeafNodes(LeafNodes, ResponseDisambiguator, DebugResponse),
    /// Send that a ChildNodes request failed
    ChildNodesError(Hash, ResponseDisambiguator, DebugResponse),
    /// Send a notification that a block became available on this replica.
    /// NOTE: This is always unsolicited - the server sends it on its own when it detects a newly
    /// received block.
    BlockOffer(BlockId, DebugResponse),
    /// Send a requested block.
    Block(BlockContent, BlockNonce, DebugResponse),
    /// Send that a Block request failed
    BlockError(BlockId, DebugResponse),
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) enum Message {
    Request(Request),
    Response(Response),
    // Peer exchange
    Pex(PexPayload),
}

#[cfg(test)]
impl From<Message> for Request {
    fn from(message: Message) -> Self {
        match message {
            Message::Request(request) => request,
            Message::Response(_) | Message::Pex(_) => {
                panic!("not a request: {:?}", message)
            }
        }
    }
}

#[cfg(test)]
impl From<Message> for Response {
    fn from(message: Message) -> Self {
        match message {
            Message::Response(response) => response,
            Message::Request(_) | Message::Pex(_) => {
                panic!("not a response: {:?}", message)
            }
        }
    }
}
