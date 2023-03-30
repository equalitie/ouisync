use super::{
    constants::MAX_PENDING_RESPONSES,
    debug_payload::{DebugReceivedResponse, DebugRequest},
    message::{Content, Response, ResponseDisambiguator},
    pending::{PendingRequest, PendingRequests, PendingResponse},
};
use crate::{
    block::{tracker::BlockPromise, BlockData, BlockNonce, BlockTrackerClient},
    crypto::{CacheHash, Hashable},
    error::{Error, Result},
    index::{InnerNodeMap, LeafNodeSet, ReceiveError, ReceiveFilter, Summary, UntrustedProof},
    repository_stats::RepositoryStats,
    store::{BlockRequestMode, Store},
};
use scoped_task::ScopedJoinHandle;
use std::{future, pin::pin, sync::Arc, time::Instant};
use tokio::{
    select,
    sync::{mpsc, Semaphore},
};
use tracing::instrument;

pub(super) struct Client {
    store: Store,
    pending_requests: Arc<PendingRequests>,
    request_tx: mpsc::UnboundedSender<(PendingRequest, Instant)>,
    //recv_queue: VecDeque<(PendingResponse, Option<OwnedSemaphorePermit>)>,
    receive_filter: ReceiveFilter,
    block_tracker: Arc<BlockTrackerClient>,
    _request_sender: ScopedJoinHandle<()>,
}

impl Client {
    pub fn new(
        store: Store,
        tx: mpsc::Sender<Content>,
        peer_request_limiter: Arc<Semaphore>,
    ) -> Self {
        let pool = store.db().clone();
        let block_tracker = Arc::new(store.block_tracker.client());

        let pending_requests = Arc::new(PendingRequests::new(store.stats().clone()));

        // We run the sender in a separate task so we can keep sending requests while we're
        // processing responses (which sometimes takes a while).
        let (request_sender, request_tx) = start_sender(
            tx,
            pending_requests.clone(),
            peer_request_limiter,
            store.stats().clone(),
        );

        Self {
            store,
            pending_requests,
            request_tx,
            //recv_queue: VecDeque::new(),
            receive_filter: ReceiveFilter::new(pool),
            block_tracker,
            _request_sender: request_sender,
        }
    }

    pub async fn run(&mut self, rx: &mut mpsc::Receiver<Response>) -> Result<()> {
        self.receive_filter.reset().await?;

        let mut block_promise_acceptor = self.block_tracker.acceptor();
        let request_tx = self.request_tx.clone();
        let pending_requests = self.pending_requests.clone();

        // We're making sure to not send more requests than MAX_PENDING_RESPONSES, but there may be
        // some unsolicited responses and also the peer may be malicious and send us too many
        // responses (so we shoulnd't use unbounded_channel).
        let (queued_responses_tx, queued_responses_rx) = mpsc::channel(2 * MAX_PENDING_RESPONSES);

        let mut response_handler = pin!(self.handle_responses(queued_responses_rx));

        // NOTE: It is important to keep `remove`ing requests from `pending_requests` in parallel
        // with response handling. It is because response handling may take a long time (e.g. due
        // to acquiring database write transaction if there are other write transactions currently
        // going on - such as when forking) and so waiting for it could cause some requests in
        // `pending_requests` to time out.
        loop {
            select! {
                block_promise = block_promise_acceptor.accept() => {
                    let debug = DebugRequest::start();
                    // Unwrap OK because the sending task never returns.
                    request_tx.send((PendingRequest::Block(block_promise, debug), Instant::now())).unwrap();
                }
                response = rx.recv() => {
                    if let Some(response) = response {
                        if let Some(response) = pending_requests.remove(response) {
                            if queued_responses_tx.send(response).await.is_err() {
                                break;
                            }
                        }
                    } else {
                        break;
                    }
                }
                _ = &mut response_handler => break,
            }
        }

        Ok(())
    }

    fn enqueue_request(&self, request: PendingRequest) {
        // Unwrap OK because the sending task never returns.
        self.request_tx.send((request, Instant::now())).unwrap();
    }

    async fn handle_responses(
        &mut self,
        mut queued_responses_rx: mpsc::Receiver<PendingResponse>,
    ) -> Result<()> {
        loop {
            match queued_responses_rx.recv().await {
                Some(response) => self.handle_response(response).await?,
                None => return Ok(()),
            };
        }
    }

    async fn handle_response(&mut self, response: PendingResponse) -> Result<()> {
        let result = match response {
            PendingResponse::RootNode {
                proof,
                summary,
                permit: _permit,
                debug,
            } => self.handle_root_node(proof, summary, debug).await,
            PendingResponse::InnerNodes {
                hash,
                permit: _permit,
                debug,
            } => self.handle_inner_nodes(hash, debug).await,
            PendingResponse::LeafNodes {
                hash,
                permit: _permit,
                debug,
            } => self.handle_leaf_nodes(hash, debug).await,
            PendingResponse::Block {
                data,
                nonce,
                block_promise,
                permit: _permit,
                debug,
            } => self.handle_block(data, nonce, block_promise, debug).await,
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
        &self,
        proof: UntrustedProof,
        summary: Summary,
        _debug: DebugReceivedResponse,
    ) -> Result<(), ReceiveError> {
        let hash = proof.hash;
        let updated = self.store.index.receive_root_node(proof, summary).await?;

        if updated {
            tracing::trace!("received updated root node");
            self.enqueue_request(PendingRequest::ChildNodes(
                hash,
                ResponseDisambiguator::new(summary.block_presence),
                DebugRequest::start(),
            ));
        } else {
            tracing::trace!("received outdated root node");
        }

        Ok(())
    }

    #[instrument(skip_all, fields(nodes.hash = ?nodes.hash()), err(Debug))]
    async fn handle_inner_nodes(
        &mut self,
        nodes: CacheHash<InnerNodeMap>,
        _debug: DebugReceivedResponse,
    ) -> Result<(), ReceiveError> {
        let total = nodes.len();

        let (updated_nodes, completed_branches) = self
            .store
            .index
            .receive_inner_nodes(nodes, &mut self.receive_filter)
            .await?;

        let debug = DebugRequest::start();

        tracing::trace!(
            "received {}/{} inner nodes: {:?}",
            updated_nodes.len(),
            total,
            updated_nodes
                .iter()
                .map(|node| &node.hash)
                .collect::<Vec<_>>()
        );

        for node in updated_nodes {
            self.enqueue_request(PendingRequest::ChildNodes(
                node.hash,
                ResponseDisambiguator::new(node.summary.block_presence),
                debug,
            ));
        }

        // Request the branches that became completed again. See the comment in `handle_leaf_nodes`
        // for explanation.
        for branch_id in completed_branches {
            self.enqueue_request(PendingRequest::RootNode(branch_id, debug));
        }

        Ok(())
    }

    #[instrument(skip_all, fields(nodes.hash = ?nodes.hash()), err(Debug))]
    async fn handle_leaf_nodes(
        &mut self,
        nodes: CacheHash<LeafNodeSet>,
        _debug: DebugReceivedResponse,
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
            self.enqueue_request(PendingRequest::RootNode(branch_id, DebugRequest::start()));
        }

        Ok(())
    }

    #[instrument(skip_all, fields(id = ?data.id), err(Debug))]
    async fn handle_block(
        &mut self,
        data: BlockData,
        nonce: BlockNonce,
        // We need to preserve the lifetime of `_block_promise` until the response is processed.
        _block_promise: Option<BlockPromise>,
        _debug: DebugReceivedResponse,
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
) -> (
    ScopedJoinHandle<()>,
    mpsc::UnboundedSender<(PendingRequest, Instant)>,
) {
    let (request_tx, mut request_rx) = mpsc::unbounded_channel::<(PendingRequest, Instant)>();

    let handle = scoped_task::spawn({
        async move {
            let client_request_limiter = Arc::new(Semaphore::new(MAX_PENDING_RESPONSES));

            loop {
                let (request, time_created) = match request_rx.recv().await {
                    Some((request, time_created)) => (request, time_created),
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

                stats
                    .write()
                    .note_request_queue_duration(Instant::now() - time_created);

                let msg = request.to_message();

                if !pending_requests.insert(request, peer_permit, client_permit) {
                    // The same request is already in-flight.
                    continue;
                }

                stats.write().total_requests_cummulative += 1;

                tx.send(Content::Request(msg)).await.unwrap_or(());
            }

            // Don't exist so we don't need to check whether `request_tx.send()` fails or not.
            future::pending::<()>().await;
        }
    });

    (handle, request_tx)
}
