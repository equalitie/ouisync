use super::{
    message::{Content, Request, Response},
    request::PendingRequests,
};
use crate::{
    block::{BlockData, BlockId, BlockNonce, BlockTrackerClient},
    crypto::{CacheHash, Hash, Hashable},
    error::{Error, Result},
    index::{InnerNodeMap, LeafNodeSet, ReceiveError, ReceiveFilter, Summary, UntrustedProof},
    repository::MonitoredValues,
    store::Store,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Weak},
};
use tokio::{
    select,
    sync::{mpsc, OwnedSemaphorePermit, Semaphore},
};
use tracing::instrument;

pub(crate) struct Client {
    store: Store,
    tx: mpsc::Sender<Content>,
    rx: mpsc::Receiver<Response>,
    request_limiter: Arc<Semaphore>,
    pending_requests: PendingRequests,
    send_queue: VecDeque<Request>,
    recv_queue: VecDeque<Success>,
    receive_filter: ReceiveFilter,
    block_tracker: BlockTrackerClient,
    monitored: Weak<MonitoredValues>,
}

impl Client {
    pub fn new(
        store: Store,
        tx: mpsc::Sender<Content>,
        rx: mpsc::Receiver<Response>,
        request_limiter: Arc<Semaphore>,
    ) -> Self {
        let pool = store.db().clone();
        let block_tracker = store.block_tracker.client();
        let monitored = store.monitored.clone();

        Self {
            store,
            tx,
            rx,
            request_limiter,
            pending_requests: PendingRequests::new(monitored.clone()),
            send_queue: VecDeque::new(),
            recv_queue: VecDeque::new(),
            receive_filter: ReceiveFilter::new(pool),
            block_tracker,
            monitored,
        }
    }

    #[instrument(name = "client", skip_all, err)]
    pub async fn run(&mut self) -> Result<()> {
        loop {
            select! {
                block_id = self.block_tracker.accept() => {
                    self.send_queue.push_front(Request::Block(block_id));
                }
                response = self.rx.recv() => {
                    if let Some(response) = response {
                        self.enqueue_response(response).await?;
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

        if let Some(monitored) = self.monitored.upgrade() {
            *monitored.total_request_cummulative.get() += 1;
        }

        self.tx.send(Content::Request(request)).await.unwrap_or(());
    }

    async fn enqueue_response(&mut self, response: Response) -> Result<()> {
        let response = ProcessedResponse::from(response);

        if let Some(request) = response.to_request() {
            if !self.pending_requests.remove(&request) {
                // unsolicited response
                return Ok(());
            }
        }

        match response {
            ProcessedResponse::Success(response) => {
                self.recv_queue.push_front(response);
            }
            ProcessedResponse::Failure(Failure::Block(block_id)) => {
                self.block_tracker.cancel(&block_id);
            }
            ProcessedResponse::Failure(Failure::ChildNodes(_)) => (),
        }

        Ok(())
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
            Ok(()) | Err(ReceiveError::InvalidProof | ReceiveError::ParentNodeNotFound) => Ok(()),
            Err(ReceiveError::Fatal(error)) => Err(error),
        }
    }

    #[instrument(
        level = "trace",
        skip_all,
        fields(
            writer_id = ?proof.writer_id,
            vv = ?proof.version_vector,
            hash = ?proof.hash,
            missing_blocks = summary.missing_blocks_count(),
        ),
        err
    )]
    async fn handle_root_node(
        &mut self,
        proof: UntrustedProof,
        summary: Summary,
    ) -> Result<(), ReceiveError> {
        let hash = proof.hash;
        let updated = self.store.index.receive_root_node(proof, summary).await?;

        if updated {
            tracing::trace!("received updated root node");
            self.send_queue.push_front(Request::ChildNodes(hash));
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(nodes.hash = ?nodes.hash()), err)]
    async fn handle_inner_nodes(
        &mut self,
        nodes: CacheHash<InnerNodeMap>,
    ) -> Result<(), ReceiveError> {
        let updated = self
            .store
            .index
            .receive_inner_nodes(nodes, &mut self.receive_filter)
            .await?;

        if !updated.is_empty() {
            tracing::trace!("received {} updated inner nodes", updated.len());
        }

        for hash in updated {
            self.send_queue.push_front(Request::ChildNodes(hash));
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(nodes.hash = ?nodes.hash()), err)]
    async fn handle_leaf_nodes(
        &mut self,
        nodes: CacheHash<LeafNodeSet>,
    ) -> Result<(), ReceiveError> {
        let updated = self.store.index.receive_leaf_nodes(nodes).await?;

        if !updated.is_empty() {
            tracing::trace!("received {} updated leaf nodes", updated.len());
        }

        for block_id in updated {
            self.block_tracker.offer(block_id);
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(id = ?data.id), err)]
    async fn handle_block(
        &mut self,
        data: BlockData,
        nonce: BlockNonce,
    ) -> Result<(), ReceiveError> {
        match self.store.write_received_block(&data, &nonce).await {
            Ok(()) => {
                tracing::trace!("received block");
                Ok(())
            }
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
