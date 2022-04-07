use super::{
    channel_info::ChannelInfo,
    message::{Content, Request, Response},
    request::PendingRequests,
};
use crate::{
    block::{BlockData, BlockId, BlockNonce},
    crypto::{CacheHash, Hash, Hashable},
    error::{Error, Result},
    index::{Index, InnerNodeMap, LeafNodeSet, ReceiveError, Summary, UntrustedProof},
    store,
};
use std::{collections::VecDeque, sync::Arc};
use tokio::{
    select,
    sync::{mpsc, OwnedSemaphorePermit, Semaphore},
};

pub(crate) struct Client {
    index: Index,
    tx: mpsc::Sender<Content>,
    rx: mpsc::Receiver<Response>,
    request_limiter: Arc<Semaphore>,
    pending_requests: PendingRequests,
    send_queue: VecDeque<Request>,
    recv_queue: VecDeque<Success>,
}

impl Client {
    pub fn new(
        index: Index,
        tx: mpsc::Sender<Content>,
        rx: mpsc::Receiver<Response>,
        request_limiter: Arc<Semaphore>,
    ) -> Self {
        Self {
            index,
            tx,
            rx,
            request_limiter,
            pending_requests: PendingRequests::new(),
            send_queue: VecDeque::new(),
            recv_queue: VecDeque::new(),
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let _enable = self.index.enable_receive_filter();

        loop {
            select! {
                response = self.rx.recv() => {
                    if let Some(response) = response {
                        self.enqueue_response(response);
                    } else {
                        break;
                    }
                }
                send_permit = self.request_limiter.clone().acquire_owned(),
                    if !self.send_queue.is_empty() =>
                {
                    // unwrap is OK because we never `close()` the semaphore.
                    self.send_request(send_permit.unwrap()).await;
                }
                _ = self.pending_requests.expired() => return Err(Error::RequestTimeout),
            }

            while let Some(response) = self.dequeue_response() {
                self.handle_response(response).await?;
            }
        }

        Ok(())
    }

    async fn send_request(&mut self, permit: OwnedSemaphorePermit) {
        let request = if let Some(request) = self.send_queue.pop_back() {
            request
        } else {
            // No request scheduled for sending.
            return;
        };

        if !self.pending_requests.insert(request, permit) {
            // The same request is already in-flight.
            return;
        }

        self.tx.send(Content::Request(request)).await.unwrap_or(());
    }

    fn enqueue_response(&mut self, response: Response) {
        let response = ProcessedResponse::from(response);

        if let Some(request) = response.to_request() {
            if !self.pending_requests.remove(&request) {
                // unsolicited response
                return;
            }
        }

        if let ProcessedResponse::Success(response) = response {
            self.recv_queue.push_front(response);
        }
    }

    fn dequeue_response(&mut self) -> Option<Success> {
        // To avoid en-queueing too many requests to sent (which might become outdated by the time
        // we get to actually send them) we process a response (which usually produces more
        // requests to send) only when there are no more requests queued.
        if !self.send_queue.is_empty() {
            return None;
        }

        self.recv_queue.pop_back()
    }

    async fn handle_response(&mut self, response: Success) -> Result<()> {
        let result = match response {
            Success::RootNode { proof, summary } => self.handle_root_node(proof, summary).await,
            Success::InnerNodes(nodes) => self.handle_inner_nodes(nodes).await,
            Success::LeafNodes(nodes) => self.handle_leaf_nodes(nodes).await,
            Success::Block { data, nonce } => self.handle_block(data, nonce).await,
        };

        match result {
            Ok(()) => Ok(()),
            Err(error @ (ReceiveError::InvalidProof | ReceiveError::ParentNodeNotFound)) => {
                log::warn!(
                    "{} failed to handle response: {}",
                    ChannelInfo::current(),
                    error
                );
                Ok(())
            }
            Err(ReceiveError::Fatal(error)) => Err(error),
        }
    }

    async fn handle_root_node(
        &mut self,
        proof: UntrustedProof,
        summary: Summary,
    ) -> Result<(), ReceiveError> {
        log::trace!(
            "{} handle_root_node(hash: {:?}, vv: {:?}, missing_blocks: {})",
            ChannelInfo::current(),
            proof.hash,
            proof.version_vector,
            summary.missing_blocks_count()
        );

        let hash = proof.hash;
        let updated = self.index.receive_root_node(proof, summary).await?;

        if updated {
            self.send_queue.push_front(Request::ChildNodes(hash));
        }

        Ok(())
    }

    async fn handle_inner_nodes(
        &mut self,
        nodes: CacheHash<InnerNodeMap>,
    ) -> Result<(), ReceiveError> {
        log::trace!(
            "{} handle_inner_nodes({:?})",
            ChannelInfo::current(),
            nodes.hash()
        );

        let updated = self.index.receive_inner_nodes(nodes).await?;

        for hash in updated {
            self.send_queue.push_front(Request::ChildNodes(hash));
        }

        Ok(())
    }

    async fn handle_leaf_nodes(
        &mut self,
        nodes: CacheHash<LeafNodeSet>,
    ) -> Result<(), ReceiveError> {
        log::trace!(
            "{} handle_leaf_nodes({:?})",
            ChannelInfo::current(),
            nodes.hash()
        );

        let updated = self.index.receive_leaf_nodes(nodes).await?;

        for block_id in updated {
            // TODO: avoid multiple clients downloading the same block
            self.send_queue.push_front(Request::Block(block_id));
        }

        Ok(())
    }

    async fn handle_block(
        &mut self,
        data: BlockData,
        nonce: BlockNonce,
    ) -> Result<(), ReceiveError> {
        log::trace!("{} handle_block({:?})", ChannelInfo::current(), data.id);

        match store::write_received_block(&self.index, &data, &nonce).await {
            Ok(_) => Ok(()),
            // Ignore `BlockNotReferenced` errors as they only mean that the block is no longer
            // needed.
            Err(Error::BlockNotReferenced) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

enum ProcessedResponse {
    Success(Success),
    Failure(Failure),
}

impl ProcessedResponse {
    fn to_request(&self) -> Option<Request> {
        match self {
            Self::Success(Success::RootNode { .. }) => None,
            Self::Success(Success::InnerNodes(nodes)) => Some(Request::ChildNodes(nodes.hash())),
            Self::Success(Success::LeafNodes(nodes)) => Some(Request::ChildNodes(nodes.hash())),
            Self::Success(Success::Block { data, .. }) => Some(Request::Block(data.id)),
            Self::Failure(Failure::ChildNodes(hash)) => Some(Request::ChildNodes(*hash)),
            Self::Failure(Failure::Block(id)) => Some(Request::Block(*id)),
        }
    }
}

impl From<Response> for ProcessedResponse {
    fn from(response: Response) -> Self {
        match response {
            Response::RootNode { proof, summary } => {
                Self::Success(Success::RootNode { proof, summary })
            }
            Response::InnerNodes(nodes) => Self::Success(Success::InnerNodes(nodes.into())),
            Response::LeafNodes(nodes) => Self::Success(Success::LeafNodes(nodes.into())),
            Response::Block { content, nonce } => Self::Success(Success::Block {
                data: content.into(),
                nonce,
            }),
            Response::ChildNodesError(hash) => Self::Failure(Failure::ChildNodes(hash)),
            Response::BlockError(id) => Self::Failure(Failure::Block(id)),
        }
    }
}

enum Success {
    RootNode {
        proof: UntrustedProof,
        summary: Summary,
    },
    InnerNodes(CacheHash<InnerNodeMap>),
    LeafNodes(CacheHash<LeafNodeSet>),
    Block {
        data: BlockData,
        nonce: BlockNonce,
    },
}

enum Failure {
    ChildNodes(Hash),
    Block(BlockId),
}
