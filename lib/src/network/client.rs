use super::{
    constants::MAX_RESPONSE_BATCH_SIZE,
    debug_payload::{DebugResponse, PendingDebugRequest},
    message::{Content, Request, Response, ResponseDisambiguator},
    pending::{PendingRequest, PendingRequests, PreparedResponse},
};
use crate::{
    block_tracker::{BlockPromise, TrackerClient},
    crypto::{sign::PublicKey, CacheHash, Hashable},
    error::Result,
    event::Payload,
    protocol::{
        Block, BlockId, InnerNodes, LeafNodes, MultiBlockPresence, ProofError, RootNodeFilter,
        UntrustedProof,
    },
    repository::Vault,
    store::ClientWriter,
};
use std::{future, time::Instant};
use tokio::{select, sync::mpsc};
use tracing::{instrument, Level};

pub(super) struct Client {
    inner: Inner,
    response_rx: mpsc::Receiver<Response>,
    send_queue_rx: mpsc::UnboundedReceiver<(PendingRequest, Instant)>,
}

impl Client {
    pub fn new(
        vault: Vault,
        content_tx: mpsc::Sender<Content>,
        response_rx: mpsc::Receiver<Response>,
    ) -> Self {
        let pending_requests = PendingRequests::new(vault.monitor.clone());
        let block_tracker = vault.block_tracker.client();

        // We run the sender in a separate task so we can keep sending requests while we're
        // processing responses (which sometimes takes a while).
        let (send_queue_tx, send_queue_rx) = mpsc::unbounded_channel();

        let inner = Inner {
            vault,
            pending_requests,
            block_tracker,
            content_tx,
            send_queue_tx,
        };

        Self {
            inner,
            response_rx,
            send_queue_rx,
        }
    }
}

impl Client {
    pub async fn run(&mut self) -> Result<()> {
        let Self {
            inner,
            response_rx,
            send_queue_rx,
        } = self;

        inner.run(response_rx, send_queue_rx).await
    }
}

struct Inner {
    vault: Vault,
    pending_requests: PendingRequests,
    block_tracker: TrackerClient,
    content_tx: mpsc::Sender<Content>,
    send_queue_tx: mpsc::UnboundedSender<(PendingRequest, Instant)>,
}

impl Inner {
    async fn run(
        &mut self,
        response_rx: &mut mpsc::Receiver<Response>,
        send_queue_rx: &mut mpsc::UnboundedReceiver<(PendingRequest, Instant)>,
    ) -> Result<()> {
        select! {
            result = self.handle_responses(response_rx) => result,
            _ = self.send_requests(send_queue_rx) => Ok(()),
            _ = self.handle_available_block_offers() => Ok(()),
            _ = self.handle_reload_index() => Ok(()),
        }
    }

    fn enqueue_request(&self, request: PendingRequest) {
        self.send_queue_tx.send((request, Instant::now())).ok();
    }

    async fn send_requests(
        &self,
        send_queue_rx: &mut mpsc::UnboundedReceiver<(PendingRequest, Instant)>,
    ) {
        loop {
            let Some((request, timestamp)) = send_queue_rx.recv().await else {
                break;
            };

            self.vault
                .monitor
                .request_queue_time
                .record(timestamp.elapsed());

            if let Some(request) = self.pending_requests.insert(request) {
                self.send_request(request).await;
            }
        }
    }

    async fn send_request(&self, request: Request) {
        self.content_tx
            .send(Content::Request(request))
            .await
            .unwrap_or(());
    }

    async fn handle_responses(&self, rx: &mut mpsc::Receiver<Response>) -> Result<()> {
        let mut received = Vec::with_capacity(MAX_RESPONSE_BATCH_SIZE);
        let mut prepared = Vec::with_capacity(MAX_RESPONSE_BATCH_SIZE);

        loop {
            if rx.recv_many(&mut received, MAX_RESPONSE_BATCH_SIZE).await == 0 {
                break;
            }

            prepared.extend(
                received
                    .drain(..)
                    .map(|response| self.pending_requests.remove(response)),
            );

            self.handle_response_batch(&mut prepared).await?;
        }

        Ok(())
    }

    async fn handle_response_batch(&self, batch: &mut Vec<PreparedResponse>) -> Result<()> {
        let count = batch.len();

        self.vault
            .monitor
            .responses_received
            .increment(count as u64);

        let mut writer = self.vault.store().begin_client_write().await?;

        self.vault
            .monitor
            .responses_in_processing
            .increment(count as f64);

        for response in batch.drain(..) {
            match response {
                PreparedResponse::RootNode(proof, block_presence, debug) => {
                    self.handle_root_node(&mut writer, proof, block_presence, debug)
                        .await?;
                }
                PreparedResponse::InnerNodes(nodes, _, debug) => {
                    self.handle_inner_nodes(&mut writer, nodes, debug).await?;
                }
                PreparedResponse::LeafNodes(nodes, _, debug) => {
                    self.handle_leaf_nodes(&mut writer, nodes, debug).await?;
                }
                PreparedResponse::BlockOffer(block_id, debug) => {
                    self.handle_block_offer(&mut writer, block_id, debug)
                        .await?;
                }
                PreparedResponse::Block(block, block_promise, debug) => {
                    self.handle_block(&mut writer, block, block_promise, debug)
                        .await?;
                }
                PreparedResponse::BlockError(block_id, debug) => {
                    self.handle_block_not_found(block_id, debug);
                }
                PreparedResponse::RootNodeError(..) | PreparedResponse::ChildNodesError(..) => {
                    continue;
                }
            }
        }

        self.commit_responses(writer).await?;

        self.vault
            .monitor
            .responses_in_processing
            .decrement(count as f64);

        Ok(())
    }

    #[instrument(
        skip_all,
        fields(
            writer_id = ?proof.writer_id,
            vv = ?proof.version_vector,
            hash = ?proof.hash,
            ?block_presence,
            ?debug_payload,
        ),
        err(Debug)
    )]
    async fn handle_root_node(
        &self,
        writer: &mut ClientWriter,
        proof: UntrustedProof,
        block_presence: MultiBlockPresence,
        debug_payload: DebugResponse,
    ) -> Result<()> {
        let proof = match proof.verify(self.vault.repository_id()) {
            Ok(proof) => proof,
            Err(ProofError(_)) => {
                tracing::trace!("Invalid proof");
                return Ok(());
            }
        };

        // Ignore branches with empty version vectors because they have no content yet.
        if proof.version_vector.is_empty() {
            return Ok(());
        }

        let hash = proof.hash;
        let status = writer.save_root_node(proof, &block_presence).await?;

        tracing::debug!("Received root node - {status}");

        if status.request_children() {
            self.enqueue_request(PendingRequest::ChildNodes(
                hash,
                ResponseDisambiguator::new(block_presence),
                debug_payload.follow_up(),
            ));
        }

        Ok(())
    }

    #[instrument(skip_all, fields(hash = ?nodes.hash(), ?debug_payload), err(Debug))]
    async fn handle_inner_nodes(
        &self,
        writer: &mut ClientWriter,
        nodes: CacheHash<InnerNodes>,
        debug_payload: DebugResponse,
    ) -> Result<()> {
        let total = nodes.len();
        let status = writer.save_inner_nodes(nodes).await?;

        tracing::trace!(
            "Received {}/{} inner nodes",
            status.new_children.len(),
            total
        );

        for node in status.new_children {
            self.enqueue_request(PendingRequest::ChildNodes(
                node.hash,
                ResponseDisambiguator::new(node.summary.block_presence),
                debug_payload.clone().follow_up(),
            ));
        }

        Ok(())
    }

    #[instrument(skip_all, fields(hash = ?nodes.hash(), ?debug_payload), err(Debug))]
    async fn handle_leaf_nodes(
        &self,
        writer: &mut ClientWriter,
        nodes: CacheHash<LeafNodes>,
        debug_payload: DebugResponse,
    ) -> Result<()> {
        let total = nodes.len();
        let status = writer.save_leaf_nodes(nodes).await?;

        tracing::trace!(
            "Received {}/{} leaf nodes",
            status.new_block_offers.len(),
            total,
        );

        for (block_id, state) in status.new_block_offers {
            self.block_tracker.register(block_id, state);
        }

        Ok(())
    }

    #[instrument(skip_all, fields(id = ?block_id, ?debug_payload), err(Debug))]
    async fn handle_block_offer(
        &self,
        writer: &mut ClientWriter,
        block_id: BlockId,
        debug_payload: DebugResponse,
    ) -> Result<()> {
        let Some(offer_state) = writer.load_block_offer_state(&block_id).await? else {
            return Ok(());
        };

        tracing::trace!(?offer_state, "Received block offer");

        self.block_tracker.register(block_id, offer_state);

        Ok(())
    }

    #[instrument(skip_all, fields(id = ?block.id, ?debug_payload), err(Debug))]
    async fn handle_block(
        &self,
        writer: &mut ClientWriter,
        block: Block,
        block_promise: Option<BlockPromise>,
        debug_payload: DebugResponse,
    ) -> Result<()> {
        writer.save_block(&block, block_promise).await?;

        tracing::trace!("Received block");

        Ok(())
    }

    #[instrument(skip_all, fields(block_id = ?_block_id, ?debug_payload))]
    fn handle_block_not_found(&self, _block_id: BlockId, debug_payload: DebugResponse) {
        tracing::trace!("Received block not found");
    }

    async fn commit_responses(&self, writer: ClientWriter) -> Result<()> {
        let event_tx = self.vault.event_tx.clone();
        let status = writer
            .commit_and_then(move |status| {
                // Notify about newly written blocks.
                for block_id in &status.new_blocks {
                    event_tx.send(Payload::BlockReceived(*block_id));
                }

                // Notify about newly approved snapshots
                for branch_id in &status.approved_branches {
                    event_tx.send(Payload::SnapshotApproved(*branch_id));
                }

                // Notify about newly rejected snapshots
                for branch_id in &status.rejected_branches {
                    event_tx.send(Payload::SnapshotRejected(*branch_id));
                }

                status
            })
            .await?;

        // Approve pending block offers referenced from the newly approved snapshots.
        for block_id in status.approved_missing_blocks {
            self.vault.block_tracker.approve(block_id);
        }

        self.refresh_branches(status.approved_branches.iter().copied());
        self.log_approved(&status.approved_branches).await;

        Ok(())
    }

    async fn handle_available_block_offers(&self) {
        let mut block_offers = self.block_tracker.offers();

        loop {
            let block_offer = block_offers.next().await;
            let debug = PendingDebugRequest::start();
            self.enqueue_request(PendingRequest::Block(block_offer, debug));
        }
    }

    async fn handle_reload_index(&self) {
        let mut reload_index_rx = self.vault.store().client_reload_index_tx.subscribe();

        while let Ok(branch_ids) = reload_index_rx.changed().await {
            self.refresh_branches(branch_ids);
        }

        future::pending().await
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
    fn refresh_branches(&self, branches: impl IntoIterator<Item = PublicKey>) {
        for branch_id in branches {
            self.enqueue_request(PendingRequest::RootNode(
                branch_id,
                PendingDebugRequest::start(),
            ));
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
            let root_node = match reader
                .load_latest_approved_root_node(branch_id, RootNodeFilter::Any)
                .await
            {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        access_control::WriteSecrets,
        block_tracker::RequestMode,
        crypto::sign::Keypair,
        db,
        event::EventSender,
        protocol::{Proof, RepositoryId, EMPTY_INNER_HASH},
        repository::RepositoryMonitor,
        version_vector::VersionVector,
    };
    use futures_util::TryStreamExt;
    use metrics::NoopRecorder;
    use state_monitor::StateMonitor;
    use tempfile::TempDir;

    #[tokio::test]
    async fn receive_root_node_with_invalid_proof() {
        let (_base_dir, inner, _, _) = setup().await;
        let remote_id = PublicKey::random();

        // Receive invalid root node from the remote replica.
        let invalid_write_keys = Keypair::random();
        inner
            .handle_response_batch(&mut vec![PreparedResponse::RootNode(
                Proof::new(
                    remote_id,
                    VersionVector::first(remote_id),
                    *EMPTY_INNER_HASH,
                    &invalid_write_keys,
                )
                .into(),
                MultiBlockPresence::None,
                DebugResponse::unsolicited(),
            )])
            .await
            .unwrap();

        // The invalid root was not written to the db.
        assert!(inner
            .vault
            .store()
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&remote_id)
            .try_next()
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn receive_root_node_with_empty_version_vector() {
        let (_base_dir, inner, _, secrets) = setup().await;
        let remote_id = PublicKey::random();

        inner
            .handle_response_batch(&mut vec![PreparedResponse::RootNode(
                Proof::new(
                    remote_id,
                    VersionVector::new(),
                    *EMPTY_INNER_HASH,
                    &secrets.write_keys,
                )
                .into(),
                MultiBlockPresence::None,
                DebugResponse::unsolicited(),
            )])
            .await
            .unwrap();

        assert!(inner
            .vault
            .store()
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&remote_id)
            .try_next()
            .await
            .unwrap()
            .is_none());
    }

    async fn setup() -> (
        TempDir,
        Inner,
        mpsc::UnboundedReceiver<(PendingRequest, Instant)>,
        WriteSecrets,
    ) {
        let (base_dir, pool) = db::create_temp().await.unwrap();

        let secrets = WriteSecrets::random();
        let repository_id = RepositoryId::from(secrets.write_keys.public_key());

        let vault = Vault::new(
            repository_id,
            EventSender::new(1),
            pool,
            RepositoryMonitor::new(StateMonitor::make_root(), &NoopRecorder),
        );

        vault.block_tracker.set_request_mode(RequestMode::Lazy);

        let pending_requests = PendingRequests::new(vault.monitor.clone());
        let block_tracker = vault.block_tracker.client();

        let (content_tx, _content_rx) = mpsc::channel(1);
        let (send_queue_tx, send_queue_rx) = mpsc::unbounded_channel();

        let inner = Inner {
            vault,
            pending_requests,
            block_tracker,
            content_tx,
            send_queue_tx,
        };

        (base_dir, inner, send_queue_rx, secrets)
    }
}
