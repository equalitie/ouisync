use super::{
    constants::RESPONSE_BATCH_SIZE,
    debug_payload::{DebugRequest, DebugResponse},
    message::{Message, Response},
    request_tracker::{PendingRequest, RequestTracker, RequestTrackerClient},
};
use crate::{
    block_tracker::{BlockOfferState, BlockTrackerClient},
    crypto::{sign::PublicKey, CacheHash, Hashable},
    error::Result,
    event::Payload,
    network::{
        message::Request,
        request_tracker::{CandidateRequest, MessageKey, RequestVariant},
    },
    protocol::{
        Block, BlockId, InnerNodes, LeafNodes, MultiBlockPresence, NodeState, ProofError,
        RootNodeFilter, UntrustedProof,
    },
    repository::Vault,
    store::{ClientReader, ClientWriter},
};
use std::iter;
use tokio::{select, sync::mpsc};
use tracing::{instrument, Level};

mod future {
    pub(super) use futures_util::future::try_join;
    pub(super) use std::future::pending;
}

pub(super) struct Client {
    inner: Inner,
    request_rx: mpsc::UnboundedReceiver<PendingRequest>,
    response_rx: mpsc::Receiver<Response>,
    block_rx: mpsc::UnboundedReceiver<BlockId>,
}

impl Client {
    pub fn new(
        vault: Vault,
        message_tx: mpsc::UnboundedSender<Message>,
        response_rx: mpsc::Receiver<Response>,
        request_tracker: &RequestTracker,
    ) -> Self {
        let (request_tracker, request_rx) = request_tracker.new_client();
        let (block_tracker, block_rx) = vault.block_tracker.new_client();

        let inner = Inner {
            vault,
            request_tracker,
            block_tracker,
            message_tx,
        };

        Self {
            inner,
            request_rx,
            response_rx,
            block_rx,
        }
    }
}

impl Client {
    pub async fn run(&mut self) -> Result<()> {
        let Self {
            inner,
            request_rx,
            response_rx,
            block_rx,
        } = self;

        inner.run(request_rx, response_rx, block_rx).await
    }
}

struct Inner {
    vault: Vault,
    request_tracker: RequestTrackerClient,
    block_tracker: BlockTrackerClient,
    message_tx: mpsc::UnboundedSender<Message>,
}

impl Inner {
    async fn run(
        &mut self,
        request_rx: &mut mpsc::UnboundedReceiver<PendingRequest>,
        response_rx: &mut mpsc::Receiver<Response>,
        block_rx: &mut mpsc::UnboundedReceiver<BlockId>,
    ) -> Result<()> {
        select! {
            result = self.handle_responses(response_rx) => result,
            _ = self.send_requests(request_rx) => Ok(()),
            _ = self.request_blocks(block_rx) => Ok(()),
            _ = self.handle_reload_index() => Ok(()),
        }
    }

    async fn send_requests(&self, request_rx: &mut mpsc::UnboundedReceiver<PendingRequest>) {
        while let Some(PendingRequest { payload, .. }) = request_rx.recv().await {
            self.message_tx.send(Message::Request(payload)).ok();
        }
    }

    async fn handle_responses(&self, rx: &mut mpsc::Receiver<Response>) -> Result<()> {
        let mut ephemeral = Vec::with_capacity(RESPONSE_BATCH_SIZE);
        let mut persistable = Vec::with_capacity(RESPONSE_BATCH_SIZE);

        loop {
            for response in recv_iter(rx).await {
                self.vault.monitor.responses_received.increment(1);

                match response {
                    Response::RootNode {
                        proof,
                        cookie,
                        block_presence,
                        debug,
                    } => {
                        persistable.push(PersistableResponse::RootNode {
                            proof,
                            cookie,
                            block_presence,
                            debug,
                        });
                    }
                    Response::InnerNodes(nodes, debug) => {
                        persistable.push(PersistableResponse::InnerNodes(nodes.into(), debug));
                    }
                    Response::LeafNodes(nodes, debug) => {
                        persistable.push(PersistableResponse::LeafNodes(nodes.into(), debug));
                    }
                    Response::Block(block_content, block_nonce, debug) => {
                        persistable.push(PersistableResponse::Block(
                            Block::new(block_content, block_nonce),
                            debug,
                        ));
                    }
                    Response::BlockOffer(block_id, debug) => {
                        ephemeral.push(EphemeralResponse::BlockOffer(block_id, debug));
                    }
                    Response::RootNodeError {
                        writer_id, cookie, ..
                    } => {
                        self.request_tracker
                            .failure(MessageKey::RootNode(writer_id, cookie));
                    }
                    Response::ChildNodesError(hash, _) => {
                        self.request_tracker.failure(MessageKey::ChildNodes(hash));
                    }
                    Response::BlockError(block_id, _) => {
                        self.request_tracker.failure(MessageKey::Block(block_id));
                    }
                }

                if ephemeral.len() >= RESPONSE_BATCH_SIZE {
                    break;
                }

                if persistable.len() >= RESPONSE_BATCH_SIZE {
                    break;
                }
            }

            future::try_join(
                self.handle_ephemeral_responses(&mut ephemeral),
                self.handle_persistable_responses(&mut persistable),
            )
            .await?;
        }
    }

    async fn handle_persistable_responses(
        &self,
        batch: &mut Vec<PersistableResponse>,
    ) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }

        let mut writer = self.vault.store().begin_client_write().await?;

        for response in batch.drain(..) {
            match response {
                PersistableResponse::RootNode {
                    proof,
                    cookie,
                    block_presence,
                    debug,
                } => {
                    self.handle_root_node(&mut writer, proof, cookie, block_presence, debug)
                        .await?;
                }
                PersistableResponse::InnerNodes(nodes, debug) => {
                    self.handle_inner_nodes(&mut writer, nodes, debug).await?;
                }
                PersistableResponse::LeafNodes(nodes, debug) => {
                    self.handle_leaf_nodes(&mut writer, nodes, debug).await?;
                }
                PersistableResponse::Block(block, debug) => {
                    self.handle_block(&mut writer, block, debug).await?;
                }
            }
        }

        self.commit_responses(writer).await?;

        Ok(())
    }

    async fn handle_ephemeral_responses(&self, batch: &mut Vec<EphemeralResponse>) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }

        let mut reader = self.vault.store().begin_client_read().await?;

        for response in batch.drain(..) {
            match response {
                EphemeralResponse::BlockOffer(block_id, debug) => {
                    self.handle_block_offer(&mut reader, block_id, debug)
                        .await?;
                }
            }
        }

        Ok(())
    }

    #[instrument(
        skip_all,
        fields(
            writer_id = ?proof.writer_id,
            vv = ?proof.version_vector,
            hash = ?proof.hash,
            ?block_presence,
            cookie = cookie,
            ?debug_payload,
        ),
        err(Debug)
    )]
    async fn handle_root_node(
        &self,
        writer: &mut ClientWriter,
        proof: UntrustedProof,
        cookie: u64,
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
        let writer_id = proof.writer_id;
        let status = writer.save_root_node(proof, &block_presence).await?;

        tracing::debug!("Received root node - {status}");

        self.request_tracker.success(
            MessageKey::RootNode(writer_id, cookie),
            status
                .request_children()
                .map(|local_block_presence| {
                    CandidateRequest::new(Request::ChildNodes(hash, debug_payload.follow_up()))
                        .variant(RequestVariant::new(local_block_presence, block_presence))
                })
                .into_iter()
                .collect(),
        );

        Ok(())
    }

    #[instrument(skip_all, fields(hash = ?nodes.hash(), ?debug_payload), err(Debug))]
    async fn handle_inner_nodes(
        &self,
        writer: &mut ClientWriter,
        nodes: CacheHash<InnerNodes>,
        debug_payload: DebugResponse,
    ) -> Result<()> {
        let hash = nodes.hash();
        let total = nodes.len();
        let statuses = writer.save_inner_nodes(nodes).await?;

        tracing::trace!("Received {}/{} inner nodes", statuses.len(), total);

        self.request_tracker.success(
            MessageKey::ChildNodes(hash),
            statuses
                .into_iter()
                .map(|status| {
                    CandidateRequest::new(Request::ChildNodes(
                        status.hash,
                        debug_payload.follow_up(),
                    ))
                    .variant(RequestVariant::new(
                        status.local_block_presence,
                        status.remote_block_presence,
                    ))
                })
                .collect(),
        );

        Ok(())
    }

    #[instrument(skip_all, fields(hash = ?nodes.hash(), ?debug_payload), err(Debug))]
    async fn handle_leaf_nodes(
        &self,
        writer: &mut ClientWriter,
        nodes: CacheHash<LeafNodes>,
        debug_payload: DebugResponse,
    ) -> Result<()> {
        let hash = nodes.hash();
        let total = nodes.len();
        let status = writer.save_leaf_nodes(nodes).await?;

        tracing::trace!(
            "Received {}/{} leaf nodes",
            status.new_block_offers.len(),
            total,
        );

        // IMPORTANT: Make sure the request tracker is processed before the block tracker to ensure
        // the request is first inserted and only then resumed.

        let offers: Vec<_> = status
            .new_block_offers
            .into_iter()
            .filter_map(|(block_id, root_node_state)| {
                block_offer_state(root_node_state).map(move |offer_state| (block_id, offer_state))
            })
            .collect();

        self.request_tracker.success(
            MessageKey::ChildNodes(hash),
            offers
                .iter()
                .map(|(block_id, _)| {
                    CandidateRequest::new(Request::Block(*block_id, debug_payload.follow_up()))
                        .suspended()
                })
                .collect(),
        );

        for (block_id, offer_state) in offers {
            self.block_tracker.offer(block_id, offer_state);
        }

        Ok(())
    }

    #[instrument(skip_all, fields(id = ?block_id, ?debug_payload), err(Debug))]
    async fn handle_block_offer(
        &self,
        reader: &mut ClientReader,
        block_id: BlockId,
        debug_payload: DebugResponse,
    ) -> Result<()> {
        let root_node_state = reader
            .load_effective_root_node_state_for_block(&block_id)
            .await?;

        let Some(offer_state) = block_offer_state(root_node_state) else {
            return Ok(());
        };

        // IMPORTANT: Make sure the request tracker is processed before the block tracker to ensure
        // the request is first inserted and only then resumed.

        self.request_tracker.initial(
            CandidateRequest::new(Request::Block(block_id, debug_payload.follow_up())).suspended(),
        );

        self.block_tracker.offer(block_id, offer_state);

        Ok(())
    }

    #[instrument(skip_all, fields(id = ?block.id, ?debug_payload), err(Debug))]
    async fn handle_block(
        &self,
        writer: &mut ClientWriter,
        block: Block,
        debug_payload: DebugResponse,
    ) -> Result<()> {
        writer.save_block(&block).await?;

        tracing::trace!("Received block");

        self.request_tracker
            .success(MessageKey::Block(block.id), vec![]);

        Ok(())
    }

    async fn commit_responses(&self, writer: ClientWriter) -> Result<()> {
        let status = writer
            .commit_and_then({
                let committer = self.request_tracker.new_committer();
                let event_tx = self.vault.event_tx.clone();

                move |status| {
                    committer.commit();

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
                }
            })
            .await?;

        // Approve pending block offers referenced from the newly approved snapshots.
        for block_id in status.approved_missing_blocks {
            self.block_tracker.approve(block_id);
        }

        self.refresh_branches(status.approved_branches.iter().copied());
        self.log_approved(&status.approved_branches).await;

        Ok(())
    }

    async fn request_blocks(&self, block_rx: &mut mpsc::UnboundedReceiver<BlockId>) {
        while let Some(block_id) = block_rx.recv().await {
            self.request_tracker
                .resume(MessageKey::Block(block_id), RequestVariant::default());
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
    // By requesting the root node again immediately, we ensure that the missing block is
    // requested as soon as possible.
    fn refresh_branches(&self, branches: impl IntoIterator<Item = PublicKey>) {
        for writer_id in branches {
            self.request_tracker
                .initial(CandidateRequest::new(Request::RootNode {
                    writer_id,
                    cookie: next_root_node_cookie(),
                    debug: DebugRequest::start(),
                }));
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

/// Response whose processing requires only read access to the store or no access at all.
enum EphemeralResponse {
    BlockOffer(BlockId, DebugResponse),
}

/// Response whose processing requires write access to the store.
enum PersistableResponse {
    RootNode {
        proof: UntrustedProof,
        cookie: u64,
        block_presence: MultiBlockPresence,
        debug: DebugResponse,
    },
    InnerNodes(CacheHash<InnerNodes>, DebugResponse),
    LeafNodes(CacheHash<LeafNodes>, DebugResponse),
    Block(Block, DebugResponse),
}

/// Waits for at least one item to become available (or the chanel getting closed) and then yields
/// all the buffered items from the channel.
async fn recv_iter<T>(rx: &mut mpsc::Receiver<T>) -> impl Iterator<Item = T> + '_ {
    rx.recv()
        .await
        .into_iter()
        .chain(iter::from_fn(|| rx.try_recv().ok()))
}

fn block_offer_state(root_node_state: NodeState) -> Option<BlockOfferState> {
    match root_node_state {
        NodeState::Approved => Some(BlockOfferState::Approved),
        NodeState::Complete | NodeState::Incomplete => Some(BlockOfferState::Pending),
        NodeState::Rejected => None,
    }
}

// Generate cookie for the next `RootNode` request. This value is guaranteed to be non-zero (zero is
// used for unsolicited responses).
fn next_root_node_cookie() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    loop {
        let next = NEXT.fetch_add(1, Ordering::Relaxed);

        if next != 0 {
            break next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        access_control::WriteSecrets,
        block_tracker::BlockRequestMode,
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
        let (_base_dir, inner, _) = setup().await;
        let remote_id = PublicKey::random();

        // Receive invalid root node from the remote replica.
        let invalid_write_keys = Keypair::random();
        inner
            .handle_persistable_responses(&mut vec![PersistableResponse::RootNode {
                proof: Proof::new(
                    remote_id,
                    VersionVector::first(remote_id),
                    *EMPTY_INNER_HASH,
                    &invalid_write_keys,
                )
                .into(),
                block_presence: MultiBlockPresence::None,
                cookie: 0,
                debug: DebugResponse::unsolicited(),
            }])
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
        let (_base_dir, inner, secrets) = setup().await;
        let remote_id = PublicKey::random();

        inner
            .handle_persistable_responses(&mut vec![PersistableResponse::RootNode {
                proof: Proof::new(
                    remote_id,
                    VersionVector::new(),
                    *EMPTY_INNER_HASH,
                    &secrets.write_keys,
                )
                .into(),
                block_presence: MultiBlockPresence::None,
                cookie: 0,
                debug: DebugResponse::unsolicited(),
            }])
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

    async fn setup() -> (TempDir, Inner, WriteSecrets) {
        let (base_dir, pool) = db::create_temp().await.unwrap();

        let secrets = WriteSecrets::random();
        let repository_id = RepositoryId::from(secrets.write_keys.public_key());

        let vault = Vault::new(
            repository_id,
            EventSender::new(1),
            pool,
            RepositoryMonitor::new(StateMonitor::make_root(), &NoopRecorder),
        );

        vault.block_tracker.set_request_mode(BlockRequestMode::Lazy);

        let request_tracker = RequestTracker::new();
        let (request_tracker, _request_rx) = request_tracker.new_client();
        let (block_tracker, _block_rx) = vault.block_tracker.new_client();

        let (message_tx, _message_rx) = mpsc::unbounded_channel();

        let inner = Inner {
            vault,
            request_tracker,
            block_tracker,
            message_tx,
        };

        (base_dir, inner, secrets)
    }
}
