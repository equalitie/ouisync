use super::{
    debug_payload::{DebugRequest, DebugResponse},
    peer_exchange::PexPayload,
    runtime_id::PublicRuntimeId,
};
use crate::{
    crypto::{sign::PublicKey, Hash, Hashable},
    protocol::{
        BlockContent, BlockId, BlockNonce, InnerNodes, LeafNodes, MultiBlockPresence, RepositoryId,
        UntrustedProof,
    },
};
use net::bus::TopicId;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
pub(crate) enum Request {
    /// Request the latest root node of the given writer.
    RootNode(PublicKey, DebugRequest),
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
    RootNode(UntrustedProof, MultiBlockPresence, DebugResponse),
    /// Send that a RootNode request failed
    RootNodeError(PublicKey, DebugResponse),
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
    #[derive(Serialize, Deserialize)]
    pub(crate) struct MessageChannelId([u8; Hash::SIZE]);
}

impl MessageChannelId {
    pub(super) fn new(
        repo_id: &RepositoryId,
        this_runtime_id: &PublicRuntimeId,
        that_runtime_id: &PublicRuntimeId,
    ) -> Self {
        let (id1, id2) = if this_runtime_id > that_runtime_id {
            (this_runtime_id, that_runtime_id)
        } else {
            (that_runtime_id, this_runtime_id)
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

impl Default for MessageChannelId {
    fn default() -> Self {
        Self([0; Self::SIZE])
    }
}

impl From<MessageChannelId> for TopicId {
    fn from(id: MessageChannelId) -> Self {
        TopicId::from(id.0)
    }
}
