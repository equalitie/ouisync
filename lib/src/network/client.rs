use super::{
    message::{Content, Request, Response},
    repository_stats::RepositoryStats,
    request::{CompoundPermit, PendingRequests},
};
use crate::{
    block::{BlockData, BlockId, BlockNonce, BlockTrackerClient},
    crypto::{sign::PublicKey, CacheHash, Hash, Hashable},
    error::{Error, Result},
    index::{InnerNodeMap, LeafNodeSet, ReceiveError, ReceiveFilter, Summary, UntrustedProof},
    scoped_task::{self, ScopedJoinHandle},
    store::{BlockRequestMode, Store},
};
use std::{collections::VecDeque, future, sync::Arc};
use tokio::{
    select,
    sync::{mpsc, OwnedSemaphorePermit, Semaphore},
};
use tracing::instrument;

// Don't send more requests if the number of received responses in the `recv_queue` exceeds this
// number. Exceeding it means we're processing the responses too slowly so we can decrease the rate
// at which we're sending requests.
// TODO: The number is made up, what would be its ideal value?
const MAX_QUEUED_RESPONSES: usize = 512;

pub(super) struct Client {
    store: Store,
    rx: mpsc::Receiver<Response>,
    pending_requests: Arc<PendingRequests>,
    request_tx: mpsc::UnboundedSender<Request>,
    recv_queue: VecDeque<(Success, Option<OwnedSemaphorePermit>)>,
    receive_filter: ReceiveFilter,
    block_tracker: BlockTrackerClient,
    _request_sender: ScopedJoinHandle<()>,
}

impl Client {
    pub fn new(
        store: Store,
        tx: mpsc::Sender<Content>,
        rx: mpsc::Receiver<Response>,
        peer_request_limiter: Arc<Semaphore>,
        stats: Arc<RepositoryStats>,
    ) -> Self {
        let pool = store.db().clone();
        let block_tracker = store.block_tracker.client();

        let pending_requests = Arc::new(PendingRequests::new(stats.clone()));

        // We run the sender in a separate task so we can keep sending requests while we're
        // processing responses (which sometimes takes a while).
        let (request_sender, request_tx) =
            start_sender(tx, pending_requests.clone(), peer_request_limiter, stats);

        Self {
            store,
            rx,
            pending_requests,
            request_tx,
            recv_queue: VecDeque::new(),
            receive_filter: ReceiveFilter::new(pool),
            block_tracker,
            _request_sender: request_sender,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        self.receive_filter.reset().await?;

        loop {
            select! {
                block_id = self.block_tracker.accept() => {
                    self.send_request(Request::Block(block_id));
                }
                response = self.rx.recv() => {
                    if let Some(response) = response {
                        self.enqueue_response(response)?;
                    } else {
                        break;
                    }
                }
                _ = self.pending_requests.expired() => return Err(Error::RequestTimeout),
            }

            while let Some((response, _permit)) = self.dequeue_response() {
                self.handle_response(response).await?;
            }
        }

        Ok(())
    }

    fn send_request(&mut self, request: Request) {
        // Unwrap OK because the sending task never returns.
        self.request_tx.send(request).unwrap();
    }

    fn enqueue_response(&mut self, response: Response) -> Result<()> {
        let response = ProcessedResponse::from(response);
        let request = response.to_request();

        let permit = if let Some(permit) = self.pending_requests.remove(&request) {
            Some(permit)
        } else {
            // Only `RootNode` response is allowed to be unsolicited
            if !matches!(request, Request::RootNode(_)) {
                // Unsolicited response
                return Ok(());
            }
            None
        };

        match response {
            ProcessedResponse::Success(response) => {
                self.recv_queue.push_front((response, permit));
            }
            ProcessedResponse::Failure(request) => {
                tracing::trace!(?request, "request failed");

                match request {
                    Failure::Block(block_id) => {
                        self.block_tracker.cancel(&block_id);
                    }
                    Failure::RootNode(_) | Failure::ChildNodes(_) => (),
                }
            }
        }

        Ok(())
    }

    fn dequeue_response(&mut self) -> Option<(Success, Option<OwnedSemaphorePermit>)> {
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
        skip_all,
        fields(
            writer_id = ?proof.writer_id,
            vv = ?proof.version_vector,
            hash = ?proof.hash,
            block_presence = ?summary.block_presence,
        ),
        err(Debug)
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
            self.send_request(Request::ChildNodes(hash));
        } else {
            tracing::trace!("received outdated root node");
        }

        Ok(())
    }

    #[instrument(skip_all, fields(nodes.hash = ?nodes.hash()), err(Debug))]
    async fn handle_inner_nodes(
        &mut self,
        nodes: CacheHash<InnerNodeMap>,
    ) -> Result<(), ReceiveError> {
        let total = nodes.len();
        let (updated_nodes, completed_branches) = self
            .store
            .index
            .receive_inner_nodes(nodes, &mut self.receive_filter)
            .await?;

        tracing::trace!(
            "received {}/{} inner nodes: {:?}",
            updated_nodes.len(),
            total,
            updated_nodes
        );

        for hash in updated_nodes {
            self.send_request(Request::ChildNodes(hash));
        }

        // Request the branches that became completed again. See the comment in `handle_leaf_nodes`
        // for explanation.
        for branch_id in completed_branches {
            self.send_request(Request::RootNode(branch_id));
        }

        Ok(())
    }

    #[instrument(skip_all, fields(nodes.hash = ?nodes.hash()), err(Debug))]
    async fn handle_leaf_nodes(
        &mut self,
        nodes: CacheHash<LeafNodeSet>,
    ) -> Result<(), ReceiveError> {
        let total = nodes.len();
        let (updated_blocks, completed_branches) =
            self.store.index.receive_leaf_nodes(nodes).await?;

        tracing::trace!("received {}/{} leaf nodes", updated_blocks.len(), total);

        match self.store.block_request_mode {
            BlockRequestMode::Lazy => {
                for block_id in updated_blocks {
                    self.block_tracker.offer(block_id);
                }
            }
            BlockRequestMode::Greedy => {
                for block_id in updated_blocks {
                    if self.block_tracker.offer(block_id) {
                        self.store.require_missing_block(block_id).await?;
                    }
                }
            }
        }

        // Request again the branches that became completed. This is to cover the following edge
        // case:
        //
        // A block is initially present, but is part of a an outdated file/directory. A new snapshot
        // is in the process of being downloaded from a remote replica. During this download, the
        // block is still present and so is not marked as offered (because at least one of its
        // local ancestor nodes is still seen as up-to-date). Then before the download completes,
        // the worker garbage-collects the block. Then the download completes and triggers another
        // worker run. During this run the block might be marked as required again (because e.g.
        // the file was modified by the remote replica). But the block hasn't been marked as
        // offered (because it was still present during the last snapshot download) and so is not
        // requested. We would now have to wait for the next snapshot update from the remote replica
        // before the block is marked as offered and only then we proceed with requesting it. This
        // can take arbitrarily long (even indefinitely).
        //
        // By requesting the root node again immediatelly, we ensure that the missing block is
        // requested as soon as possible.
        for branch_id in completed_branches {
            self.send_request(Request::RootNode(branch_id));
        }

        Ok(())
    }

    #[instrument(skip_all, fields(id = ?data.id), err(Debug))]
    async fn handle_block(
        &mut self,
        data: BlockData,
        nonce: BlockNonce,
    ) -> Result<(), ReceiveError> {
        match self.store.write_received_block(&data, &nonce).await {
            // Ignore `BlockNotReferenced` errors as they only mean that the block is no longer
            // needed.
            Ok(()) | Err(Error::BlockNotReferenced) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

fn start_sender(
    tx: mpsc::Sender<Content>,
    pending_requests: Arc<PendingRequests>,
    peer_request_limiter: Arc<Semaphore>,
    stats: Arc<RepositoryStats>,
) -> (ScopedJoinHandle<()>, mpsc::UnboundedSender<Request>) {
    let (request_tx, mut request_rx) = mpsc::unbounded_channel();

    let handle = scoped_task::spawn({
        async move {
            let client_request_limiter = Arc::new(Semaphore::new(MAX_QUEUED_RESPONSES));

            loop {
                let request = match request_rx.recv().await {
                    Some(request) => request,
                    None => break,
                };

                // Unwraps OK because we never `close()` the semaphores.
                //
                // NOTE that the order here is important, we don't want to block the other clients
                // on this peer if we have too many responses queued up (which is what the
                // `client_permit` is responsible for limiting)..
                let client_permit = client_request_limiter
                    .clone()
                    .acquire_owned()
                    .await
                    .unwrap();

                let peer_permit = peer_request_limiter.clone().acquire_owned().await.unwrap();

                let send_permit = CompoundPermit {
                    _peer_permit: peer_permit,
                    client_permit,
                };

                if !pending_requests.insert(request, send_permit) {
                    // The same request is already in-flight.
                    continue;
                }

                stats.write().total_requests_cummulative += 1;

                tx.send(Content::Request(request)).await.unwrap_or(());
            }

            // Don't exist so we don't need to check whether `request_tx.send()` fails or not.
            future::pending::<()>().await;
        }
    });

    (handle, request_tx)
}

enum ProcessedResponse {
    Success(Success),
    Failure(Failure),
}

impl ProcessedResponse {
    fn to_request(&self) -> Request {
        match self {
            Self::Success(Success::RootNode { proof, .. }) => Request::RootNode(proof.writer_id),
            Self::Success(Success::InnerNodes(nodes)) => Request::ChildNodes(nodes.hash()),
            Self::Success(Success::LeafNodes(nodes)) => Request::ChildNodes(nodes.hash()),
            Self::Success(Success::Block { data, .. }) => Request::Block(data.id),
            Self::Failure(Failure::RootNode(branch_id)) => Request::RootNode(*branch_id),
            Self::Failure(Failure::ChildNodes(hash)) => Request::ChildNodes(*hash),
            Self::Failure(Failure::Block(block_id)) => Request::Block(*block_id),
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
            Response::RootNodeError(branch_id) => Self::Failure(Failure::RootNode(branch_id)),
            Response::ChildNodesError(hash) => Self::Failure(Failure::ChildNodes(hash)),
            Response::BlockError(block_id) => Self::Failure(Failure::Block(block_id)),
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

#[derive(Debug)]
enum Failure {
    RootNode(PublicKey),
    ChildNodes(Hash),
    Block(BlockId),
}
