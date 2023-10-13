use super::{
    constants::MAX_PENDING_RESPONSES,
    debug_payload::{DebugReceivedResponse, DebugRequest},
    message::{Content, Response, ResponseDisambiguator},
    pending::{PendingRequest, PendingRequests, PendingResponse},
};
use crate::{
    crypto::{sign::PublicKey, CacheHash, Hashable},
    error::{Error, Result},
    missing_parts::{OfferState, PartPromise, TrackerClient},
    protocol::{Block, BlockId, InnerNodeMap, LeafNodeSet, MultiBlockPresence, UntrustedProof},
    repository::{BlockRequestMode, RepositoryMonitor, Vault},
    store::{self, ReceiveFilter},
};
use scoped_task::ScopedJoinHandle;
use std::{future, pin::pin, sync::Arc, time::Instant};
use tokio::{
    select,
    sync::{mpsc, Semaphore},
};
use tracing::{instrument, instrument::Instrument, Level, Span};

pub(super) struct Client {
    vault: Vault,
    pending_requests: Arc<PendingRequests>,
    request_tx: mpsc::UnboundedSender<(PendingRequest, Instant)>,
    receive_filter: ReceiveFilter,
    parts_tracker: TrackerClient,
    _request_sender: ScopedJoinHandle<()>,
}

impl Client {
    pub fn new(
        vault: Vault,
        tx: mpsc::Sender<Content>,
        peer_request_limiter: Arc<Semaphore>,
    ) -> Self {
        let receive_filter = vault.store().receive_filter();
        let parts_tracker = vault.parts_tracker.client();

        let pending_requests = Arc::new(PendingRequests::new(vault.monitor.clone()));

        // We run the sender in a separate task so we can keep sending requests while we're
        // processing responses (which sometimes takes a while).
        let (request_sender, request_tx) = start_sender(
            tx,
            pending_requests.clone(),
            peer_request_limiter,
            vault.monitor.clone(),
        );

        Self {
            vault,
            pending_requests,
            request_tx,
            receive_filter,
            parts_tracker,
            _request_sender: request_sender,
        }
    }

    pub async fn run(&mut self, rx: &mut mpsc::Receiver<Response>) -> Result<()> {
        self.receive_filter.reset().await?;

        let mut reload_index_rx = self.vault.store().client_reload_index_tx.subscribe();

        let mut block_promise_acceptor = self.parts_tracker.acceptor();

        // We're making sure to not send more requests than MAX_PENDING_RESPONSES, but there may be
        // some unsolicited responses and also the peer may be malicious and send us too many
        // responses (so we shoulnd't use unbounded_channel).
        let (queued_responses_tx, queued_responses_rx) = mpsc::channel(2 * MAX_PENDING_RESPONSES);

        let mut handle_queued_responses = pin!(self.handle_responses(queued_responses_rx));

        // NOTE: It is important to keep `remove`ing requests from `pending_requests` in parallel
        // with response handling. It is because response handling may take a long time (e.g. due
        // to acquiring database write transaction if there are other write transactions currently
        // going on - such as when forking) and so waiting for it could cause some requests in
        // `pending_requests` to time out.
        let mut move_from_pending_into_queued = pin!(async {
            loop {
                let response = match rx.recv().await {
                    Some(response) => response,
                    None => break,
                };

                let response = match self.pending_requests.remove(response) {
                    Some(response) => response,
                    // Unsolicited and non-root response.
                    None => continue,
                };

                if queued_responses_tx.send(response).await.is_err() {
                    break;
                }
            }
        });

        loop {
            select! {
                part_promise = block_promise_acceptor.accept() => {
                    let debug = DebugRequest::start();
                    self.enqueue_request(PendingRequest::Block(part_promise, debug));
                }
                _ = &mut move_from_pending_into_queued => break,
                result = &mut handle_queued_responses => {
                    result?;
                    break;
                }
                branches_to_reload = reload_index_rx.changed() => {
                    if let Ok(branches_to_reload) = branches_to_reload {
                        for branch_to_reload in &branches_to_reload {
                            self.reload_index(branch_to_reload);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn enqueue_request(&self, request: PendingRequest) {
        // Unwrap OK because the sending task never returns.
        self.request_tx.send((request, Instant::now())).unwrap();
    }

    async fn handle_responses(
        &self,
        mut queued_responses_rx: mpsc::Receiver<PendingResponse>,
    ) -> Result<()> {
        loop {
            match queued_responses_rx.recv().await {
                Some(response) => {
                    self.vault
                        .monitor
                        .handle_response_metric
                        .measure_ok(self.handle_response(response))
                        .await?
                }
                None => return Ok(()),
            };
        }
    }

    async fn handle_response(&self, response: PendingResponse) -> Result<()> {
        match response {
            PendingResponse::RootNode {
                proof,
                block_presence,
                permit: _permit,
                debug,
            } => {
                self.vault
                    .monitor
                    .handle_root_node_metric
                    .measure_ok(self.handle_root_node(proof, block_presence, debug))
                    .await
            }
            PendingResponse::InnerNodes {
                hash,
                permit: _permit,
                debug,
            } => {
                self.vault
                    .monitor
                    .handle_inner_nodes_metric
                    .measure_ok(self.handle_inner_nodes(hash, debug))
                    .await
            }
            PendingResponse::LeafNodes {
                hash,
                permit: _permit,
                debug,
            } => {
                self.vault
                    .monitor
                    .handle_leaf_nodes_metric
                    .measure_ok(self.handle_leaf_nodes(hash, debug))
                    .await
            }
            PendingResponse::Block {
                block,
                part_promise,
                permit: _permit,
                debug,
            } => {
                self.vault
                    .monitor
                    .handle_block_metric
                    .measure_ok(self.handle_block(block, part_promise, debug))
                    .await
            }
            PendingResponse::BlockNotFound {
                block_id,
                permit: _permit,
                debug,
            } => {
                self.vault
                    .monitor
                    .handle_block_not_found_metric
                    .measure_ok(self.handle_block_not_found(block_id, debug))
                    .await
            }
        }
    }

    #[instrument(
        skip_all,
        fields(
            writer_id = ?proof.writer_id,
            vv = ?proof.version_vector,
            hash = ?proof.hash,
            ?block_presence,
        ),
        err(Debug)
    )]
    async fn handle_root_node(
        &self,
        proof: UntrustedProof,
        block_presence: MultiBlockPresence,
        _debug: DebugReceivedResponse,
    ) -> Result<()> {
        let hash = proof.hash;
        let status = self.vault.receive_root_node(proof, block_presence).await?;

        if status.request_children {
            self.enqueue_request(PendingRequest::ChildNodes(
                hash,
                ResponseDisambiguator::new(block_presence),
                DebugRequest::start(),
            ));
        }

        if status.new_snapshot {
            tracing::debug!("Received root node - new snapshot");
        } else if status.request_children {
            tracing::debug!("Received root node - new blocks");
        } else {
            tracing::trace!("Received root node - outdated");
        }

        self.log_approved(&status.new_approved).await;

        Ok(())
    }

    #[instrument(skip_all, fields(hash = ?nodes.hash()), err(Debug))]
    async fn handle_inner_nodes(
        &self,
        nodes: CacheHash<InnerNodeMap>,
        _debug: DebugReceivedResponse,
    ) -> Result<()> {
        let total = nodes.len();

        let quota = self.vault.quota().await?.map(Into::into);
        let status = self
            .vault
            .receive_inner_nodes(nodes, &self.receive_filter, quota)
            .await?;

        let debug = DebugRequest::start();

        tracing::trace!(
            "Received {}/{} inner nodes: {:?}",
            status.request_children.len(),
            total,
            status
                .request_children
                .iter()
                .map(|node| &node.hash)
                .collect::<Vec<_>>()
        );

        for node in status.request_children {
            self.enqueue_request(PendingRequest::ChildNodes(
                node.hash,
                ResponseDisambiguator::new(node.summary.block_presence),
                debug,
            ));
        }

        if quota.is_some() {
            for branch_id in &status.new_approved {
                self.vault.approve_offers(branch_id).await?;
            }
        }

        self.refresh_branches(&status.new_approved);
        self.log_approved(&status.new_approved).await;

        Ok(())
    }

    #[instrument(skip_all, fields(hash = ?nodes.hash()), err(Debug))]
    async fn handle_leaf_nodes(
        &self,
        nodes: CacheHash<LeafNodeSet>,
        _debug: DebugReceivedResponse,
    ) -> Result<()> {
        let total = nodes.len();
        let quota = self.vault.quota().await?.map(Into::into);
        let status = self.vault.receive_leaf_nodes(nodes, quota).await?;

        tracing::trace!(
            "Received {}/{} leaf nodes: {:?}",
            status.request_blocks.len(),
            total,
            status
                .request_blocks
                .iter()
                .map(|node| &node.block_id)
                .collect::<Vec<_>>(),
        );

        let offer_state =
            if quota.is_none() || !status.new_approved.is_empty() || status.old_approved {
                OfferState::Approved
            } else {
                OfferState::Pending
            };

        match self.vault.block_request_mode {
            BlockRequestMode::Lazy => {
                for node in status.request_blocks {
                    self.parts_tracker.offer(node.block_id, offer_state);
                }
            }
            BlockRequestMode::Greedy => {
                for node in status.request_blocks {
                    if self.parts_tracker.offer(node.block_id, offer_state) {
                        self.vault.parts_tracker.require(node.block_id);
                    }
                }
            }
        }

        if quota.is_some() {
            for branch_id in &status.new_approved {
                self.vault.approve_offers(branch_id).await?;
            }
        }

        self.refresh_branches(&status.new_approved);
        self.log_approved(&status.new_approved).await;

        Ok(())
    }

    #[instrument(skip_all, fields(id = ?block.id), err(Debug))]
    async fn handle_block(
        &self,
        block: Block,
        part_promise: Option<PartPromise>,
        _debug: DebugReceivedResponse,
    ) -> Result<()> {
        match self.vault.receive_block(&block, part_promise).await {
            // Ignore `BlockNotReferenced` errors as they only mean that the block is no longer
            // needed.
            Ok(()) | Err(Error::Store(store::Error::BlockNotReferenced)) => Ok(()),
            Err(error) => Err(error),
        }
    }

    #[instrument(skip_all, fields(block_id), err(Debug))]
    async fn handle_block_not_found(
        &self,
        block_id: BlockId,
        _debug: DebugReceivedResponse,
    ) -> Result<()> {
        tracing::trace!("Client received block not found {:?}", block_id);
        self.vault
            .receive_block_not_found(block_id, &self.receive_filter)
            .await
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
    fn refresh_branches(&self, branches: &[PublicKey]) {
        for branch_id in branches {
            self.reload_index(branch_id);
        }
    }

    /// Log new approved snapshots
    async fn log_approved(&self, branches: &[PublicKey]) {
        if !tracing::enabled!(Level::DEBUG) {
            return;
        }

        if branches.is_empty() {
            return;
        }

        let mut reader = match self.vault.store().acquire_read().await {
            Ok(reader) => reader,
            Err(error) => {
                tracing::error!(?error, "Failed to acquire reader");
                return;
            }
        };

        for branch_id in branches {
            let root_node = match reader.load_root_node(branch_id).await {
                Ok(root_node) => root_node,
                Err(error) => {
                    tracing::error!(?branch_id, ?error, "Failed to load root node");
                    continue;
                }
            };

            tracing::debug!(
                writer_id = ?root_node.proof.writer_id,
                hash = ?root_node.proof.hash,
                vv = ?root_node.proof.version_vector,
                block_presence = ?root_node.summary.block_presence,
                "Snapshot complete"
            );
        }
    }

    fn reload_index(&self, branch_id: &PublicKey) {
        self.enqueue_request(PendingRequest::RootNode(*branch_id, DebugRequest::start()));
    }
}

fn start_sender(
    tx: mpsc::Sender<Content>,
    pending_requests: Arc<PendingRequests>,
    peer_request_limiter: Arc<Semaphore>,
    monitor: Arc<RepositoryMonitor>,
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

                monitor.request_queued_metric.record(time_created.elapsed());

                let msg = request.to_message();

                if !pending_requests.insert(request, peer_permit, client_permit) {
                    // The same request is already in-flight.
                    continue;
                }

                *monitor.total_requests_cummulative.get() += 1;

                tx.send(Content::Request(msg)).await.unwrap_or(());
            }

            // Don't exist so we don't need to check whether `request_tx.send()` fails or not.
            future::pending::<()>().await;
        }
        .instrument(Span::current())
    });

    (handle, request_tx)
}
