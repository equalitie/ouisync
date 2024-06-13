use super::{
    constants::{INTEREST_TIMEOUT, MAX_UNCHOKED_DURATION},
    debug_payload::{DebugRequest, DebugResponse},
    message::{Content, Request, Response, ResponseDisambiguator},
};
use crate::{
    crypto::{sign::PublicKey, Hash},
    error::{Error, Result},
    event::{Event, Payload},
    protocol::{BlockContent, BlockId, RootNode, RootNodeFilter},
    repository::Vault,
    store,
};
use futures_util::TryStreamExt;
use std::sync::Arc;
use tokio::{
    select,
    sync::{
        broadcast::{self, error::RecvError},
        mpsc, Semaphore,
    },
    time::{self, Instant},
};
use tracing::instrument;

pub(crate) struct Server {
    inner: Inner,
    request_rx: mpsc::Receiver<Request>,
    response_rx: mpsc::Receiver<Response>,
}

impl Server {
    pub fn new(
        vault: Vault,
        content_tx: mpsc::Sender<Content>,
        request_rx: mpsc::Receiver<Request>,
        response_limiter: Arc<Semaphore>,
    ) -> Self {
        let (response_tx, response_rx) = mpsc::channel(1);

        Self {
            inner: Inner {
                vault,
                response_tx,
                content_tx,
                response_limiter,
            },
            request_rx,
            response_rx,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let Self {
            inner,
            request_rx,
            response_rx,
        } = self;

        inner.run(request_rx, response_rx).await
    }
}

struct Inner {
    vault: Vault,
    response_tx: mpsc::Sender<Response>,
    content_tx: mpsc::Sender<Content>,
    response_limiter: Arc<Semaphore>,
}

impl Inner {
    async fn run(
        &self,
        request_rx: &mut mpsc::Receiver<Request>,
        response_rx: &mut mpsc::Receiver<Response>,
    ) -> Result<()> {
        let mut event_rx = self.vault.event_tx.subscribe();

        select! {
            result = self.handle_requests(request_rx) => result,
            result = self.handle_events(&mut event_rx) => result,
            _ = self.send_responses(response_rx) => Ok(()),
        }
    }

    async fn handle_requests(&self, request_rx: &mut mpsc::Receiver<Request>) -> Result<()> {
        while let Some(request) = request_rx.recv().await {
            self.handle_request(request).await?;
        }

        Ok(())
    }

    async fn handle_request(&self, request: Request) -> Result<()> {
        self.vault.monitor.requests_received.increment(1);

        match request {
            Request::RootNode(public_key, debug) => self.handle_root_node(public_key, debug).await,
            Request::ChildNodes(hash, disambiguator, debug) => {
                self.handle_child_nodes(hash, disambiguator, debug).await
            }
            Request::Block(block_id, debug) => self.handle_block(block_id, debug).await,
        }
    }

    #[instrument(skip(self, debug), err(Debug))]
    async fn handle_root_node(&self, writer_id: PublicKey, debug: DebugRequest) -> Result<()> {
        let debug = debug.begin_reply();

        let root_node = self
            .vault
            .store()
            .acquire_read()
            .await?
            .load_root_node(&writer_id, RootNodeFilter::Published)
            .await;

        match root_node {
            Ok(node) => {
                tracing::trace!("root node found");

                let response = Response::RootNode(
                    node.proof.into(),
                    node.summary.block_presence,
                    debug.send(),
                );

                self.enqueue_response(response).await;
                Ok(())
            }
            Err(store::Error::BranchNotFound) => {
                tracing::trace!("root node not found");
                self.enqueue_response(Response::RootNodeError(writer_id, debug.send()))
                    .await;
                Ok(())
            }
            Err(error) => {
                self.enqueue_response(Response::RootNodeError(writer_id, debug.send()))
                    .await;
                Err(error.into())
            }
        }
    }

    #[instrument(skip(self, debug), err(Debug))]
    async fn handle_child_nodes(
        &self,
        parent_hash: Hash,
        disambiguator: ResponseDisambiguator,
        debug: DebugRequest,
    ) -> Result<()> {
        let debug = debug.begin_reply();

        let mut reader = self.vault.store().acquire_read().await?;

        // At most one of these will be non-empty.
        let inner_nodes = reader.load_inner_nodes(&parent_hash).await?;
        let leaf_nodes = reader.load_leaf_nodes(&parent_hash).await?;

        drop(reader);

        if !inner_nodes.is_empty() || !leaf_nodes.is_empty() {
            if !inner_nodes.is_empty() {
                tracing::trace!("inner nodes found");
                self.enqueue_response(Response::InnerNodes(
                    inner_nodes,
                    disambiguator,
                    debug.clone().send(),
                ))
                .await;
            }

            if !leaf_nodes.is_empty() {
                tracing::trace!("leaf nodes found");
                self.enqueue_response(Response::LeafNodes(leaf_nodes, disambiguator, debug.send()))
                    .await;
            }
        } else {
            tracing::trace!("child nodes not found");
            self.enqueue_response(Response::ChildNodesError(
                parent_hash,
                disambiguator,
                debug.send(),
            ))
            .await;
        }

        Ok(())
    }

    #[instrument(skip(self, debug), err(Debug))]
    async fn handle_block(&self, block_id: BlockId, debug: DebugRequest) -> Result<()> {
        let debug = debug.begin_reply();
        let mut content = BlockContent::new();
        let result = self
            .vault
            .store()
            .acquire_read()
            .await?
            .read_block(&block_id, &mut content)
            .await;

        match result {
            Ok(nonce) => {
                tracing::trace!("block found");
                self.enqueue_response(Response::Block(content, nonce, debug.send()))
                    .await;
                Ok(())
            }
            Err(store::Error::BlockNotFound) => {
                tracing::trace!("block not found");
                self.enqueue_response(Response::BlockError(block_id, debug.send()))
                    .await;
                Ok(())
            }
            Err(error) => {
                self.enqueue_response(Response::BlockError(block_id, debug.send()))
                    .await;
                Err(error.into())
            }
        }
    }

    async fn handle_events(&self, event_rx: &mut broadcast::Receiver<Event>) -> Result<()> {
        // Initially notify the peer about all root nodes we have.
        self.handle_unknown_event().await?;

        // Then keep notifying every change.
        loop {
            match event_rx.recv().await {
                Ok(Event { payload, .. }) => match payload {
                    Payload::BranchChanged(branch_id) => {
                        self.handle_branch_changed_event(branch_id).await?
                    }
                    Payload::BlockReceived(block_id) => {
                        self.handle_block_received_event(block_id).await?;
                    }
                    Payload::MaintenanceCompleted => continue,
                },
                Err(RecvError::Lagged(_)) => self.handle_unknown_event().await?,
                Err(RecvError::Closed) => return Ok(()),
            }
        }
    }

    async fn handle_branch_changed_event(&self, branch_id: PublicKey) -> Result<()> {
        let root_node = match self.load_root_node(&branch_id).await {
            Ok(node) => node,
            Err(Error::Store(store::Error::BranchNotFound)) => {
                // branch was removed after the notification was fired.
                return Ok(());
            }
            Err(error) => return Err(error),
        };

        self.send_root_node(root_node).await
    }

    async fn handle_block_received_event(&self, block_id: BlockId) -> Result<()> {
        self.enqueue_response(Response::BlockOffer(block_id, DebugResponse::unsolicited()))
            .await;
        Ok(())
    }

    async fn handle_unknown_event(&self) -> Result<()> {
        let root_nodes = self.load_root_nodes().await?;
        for root_node in root_nodes {
            self.send_root_node(root_node).await?;
        }

        Ok(())
    }

    async fn send_root_node(&self, root_node: RootNode) -> Result<()> {
        if !root_node.summary.state.is_approved() {
            // send only approved snapshots
            return Ok(());
        }

        if root_node.proof.version_vector.is_empty() {
            // Do not send snapshots with empty version vectors because they have no content yet
            return Ok(());
        }

        tracing::trace!(
            branch_id = ?root_node.proof.writer_id,
            hash = ?root_node.proof.hash,
            vv = ?root_node.proof.version_vector,
            block_presence = ?root_node.summary.block_presence,
            "send_root_node",
        );

        let response = Response::RootNode(
            root_node.proof.into(),
            root_node.summary.block_presence,
            DebugResponse::unsolicited(),
        );

        // TODO: maybe this should use different metric counter, to distinguish
        // solicited/unsolicited responses?
        self.enqueue_response(response).await;

        Ok(())
    }

    async fn load_root_nodes(&self) -> Result<Vec<RootNode>> {
        // TODO: Consider finding a way to do this in a single query. Not high priority because
        // this function is not called often.

        let mut tx = self.vault.store().begin_read().await?;

        let writer_ids: Vec<_> = tx.load_writer_ids().try_collect().await?;
        let mut root_nodes = Vec::with_capacity(writer_ids.len());

        for writer_id in writer_ids {
            match tx
                .load_root_node(&writer_id, RootNodeFilter::Published)
                .await
            {
                Ok(node) => root_nodes.push(node),
                Err(store::Error::BranchNotFound) => {
                    // A branch exists, but has no approved root node.
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
        }

        Ok(root_nodes)
    }

    async fn load_root_node(&self, writer_id: &PublicKey) -> Result<RootNode> {
        Ok(self
            .vault
            .store()
            .acquire_read()
            .await?
            .load_root_node(writer_id, RootNodeFilter::Published)
            .await?)
    }

    async fn enqueue_response(&self, response: Response) {
        // unwrap is OK because the receiver lives longer than the sender.
        self.response_tx.send(response).await.unwrap();
    }

    async fn send_responses(&self, response_rx: &mut mpsc::Receiver<Response>) {
        loop {
            let _permit = self.response_limiter.acquire().await.unwrap();
            let permit_expiry = Instant::now() + MAX_UNCHOKED_DURATION;

            loop {
                select! {
                    Some(response) = response_rx.recv() => self.send_response(response).await,
                    _ = time::sleep_until(permit_expiry) => break,
                    _ = time::sleep(INTEREST_TIMEOUT) => break,
                    else => return,
                }
            }
        }
    }

    async fn send_response(&self, response: Response) {
        if self
            .content_tx
            .send(Content::Response(response))
            .await
            .is_ok()
        {
            self.vault.monitor.responses_sent.increment(1);
        }
    }
}
