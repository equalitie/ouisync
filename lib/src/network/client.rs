use super::message::{Content, Request, Response};
use crate::{
    block::{BlockData, BlockId, BlockNonce},
    crypto::{CacheHash, Hash, Hashable},
    error::{Error, Result},
    index::{Index, InnerNodeMap, LeafNodeSet, ReceiveError, Summary, UntrustedProof},
    store,
};
use std::{
    collections::{HashMap, VecDeque},
    future,
    time::{Duration, Instant},
};
use tokio::{select, sync::mpsc, time};

// Maximum number of sent request for which we haven't received a response yet.
// Higher values give better performance but too high risks congesting the network. Also there is a
// point of diminishing returns. 32 seems to be the sweet spot based on a simple experiment.
// TODO: run more precise benchmarks to find the actual optimum.
const MAX_PENDING_REQUESTS: usize = 32;

// If a response to a pending request is not received within this time, the request is expired so
// that it doesn't block other requests from being sent.
const PENDING_REQUEST_EXPIRY: Duration = Duration::from_secs(10);

pub(crate) struct Client {
    index: Index,
    tx: mpsc::Sender<Content>,
    rx: mpsc::Receiver<Response>,
    // TODO: share this among all clients of a given peer so even when there are multiple repos
    // shared with the same peer, the total number of in-flight requests to that peer is still
    // bounded.
    request_limiter: RequestLimiter,
    send_queue: VecDeque<Request>,
    recv_queue: VecDeque<Success>,
}

impl Client {
    pub fn new(index: Index, tx: mpsc::Sender<Content>, rx: mpsc::Receiver<Response>) -> Self {
        Self {
            index,
            tx,
            rx,
            request_limiter: RequestLimiter::new(),
            send_queue: VecDeque::new(),
            recv_queue: VecDeque::new(),
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        loop {
            select! {
                Some(response) = self.rx.recv() => {
                    self.enqueue_response(response);
                }
                _ = self.request_limiter.expired() => (),
                else => break,
            }

            loop {
                self.send_requests().await;

                if let Some(response) = self.dequeue_response() {
                    self.handle_response(response).await?;
                } else {
                    break;
                }
            }
        }

        Ok(())
    }

    async fn send_requests(&mut self) {
        loop {
            if self.request_limiter.is_full() {
                // Too many requests already in-flight.
                break;
            }

            let request = if let Some(request) = self.send_queue.pop_back() {
                request
            } else {
                // No request scheduled for sending.
                break;
            };

            if !self.request_limiter.insert(request) {
                // The same request is already in-flight.
                continue;
            }

            self.tx.send(Content::Request(request)).await.unwrap_or(());
        }
    }

    fn enqueue_response(&mut self, response: Response) {
        let response = ProcessedResponse::from(response);

        if let Some(request) = response.to_request() {
            if !self.request_limiter.remove(&request) {
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
                log::warn!("failed to handle response: {}", error);
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
            "handle_root_node(hash: {:?}, vv: {:?}, missing_blocks: {})",
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
        log::trace!("handle_inner_nodes({:?})", nodes.hash());

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
        log::trace!("handle_leaf_nodes({:?})", nodes.hash());

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
        log::trace!("handle_block({:?})", data.id);

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

struct RequestLimiter(HashMap<Request, Instant>);

impl RequestLimiter {
    fn new() -> Self {
        Self(HashMap::new())
    }

    fn is_full(&self) -> bool {
        self.0.len() >= MAX_PENDING_REQUESTS
    }

    fn insert(&mut self, request: Request) -> bool {
        self.0.insert(request, Instant::now()).is_none()
    }

    fn remove(&mut self, request: &Request) -> bool {
        self.0.remove(request).is_some()
    }

    async fn expired(&mut self) {
        if let Some((&request, &timestamp)) =
            self.0.iter().min_by(|(_, lhs), (_, rhs)| lhs.cmp(rhs))
        {
            time::sleep_until((timestamp + PENDING_REQUEST_EXPIRY).into()).await;
            self.0.remove(&request);
        } else {
            future::pending().await
        }
    }
}
