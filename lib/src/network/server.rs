use super::{
    choke::Choker,
    debug_payload::{DebugRequest, DebugResponse},
    message::{Content, Request, Response, ResponseDisambiguator},
};
use crate::{
    crypto::{sign::PublicKey, Hash},
    error::{Error, Result},
    event,
    protocol::{BlockContent, BlockId, RootNode, RootNodeFilter},
    repository::Vault,
    store,
};
use futures_util::{
    stream::{self, FuturesUnordered},
    Stream, StreamExt, TryStreamExt,
};
use std::{collections::HashSet, pin::pin};
use tokio::{
    select,
    sync::{
        broadcast::{self, error::RecvError},
        mpsc,
    },
};
use tracing::instrument;

pub(crate) struct Server {
    inner: Inner,
    rx: mpsc::Receiver<Request>,
    choker: Choker,
}

impl Server {
    pub fn new(
        vault: Vault,
        tx: mpsc::Sender<Content>,
        rx: mpsc::Receiver<Request>,
        choker: Choker,
    ) -> Self {
        Self {
            inner: Inner { vault, tx },
            rx,
            choker,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let Self { inner, rx, choker } = self;

        inner.run(rx, choker).await
    }
}

struct Inner {
    vault: Vault,
    tx: mpsc::Sender<Content>,
}

impl Inner {
    async fn run(&self, rx: &mut mpsc::Receiver<Request>, choker: &mut Choker) -> Result<()> {
        // Important: make sure to create the event subscription first, before calling
        // `handle_all_branches_changed` otherwise we might miss some events.
        let mut events = pin!(events(self.vault.event_tx.subscribe()));

        // Because we're using the choker, we can't handle events that we receive on `events` right
        // a way. We need to wait for being unchoked. In the meanwhile we "accumulate" events from
        // `events` into the `accumulator` and once we get unchoked we process them all at once.
        // Note that this allows us to process each branch event only once even if we received
        // multiple events per particular branch.
        let mut accumulator = EventAccumulator::default();
        let mut choked = true;

        // This is to handle multiple requests / events at once.
        // TODO: Do we need to limit the number of concurrent request handlers? We have a limit on
        // the number of read DB transactions, so that might be enough, but perhaps we also want to
        // limit per peer?
        let mut request_handlers = FuturesUnordered::new();
        let mut event_handlers = FuturesUnordered::new();

        // Send the initial root messages, but only after we get unchoked (which can happen
        // immediatelly)
        accumulator.insert(Event::Unknown);

        loop {
            select! {
                request = rx.recv(), if !choked => {
                    let Some(request)  = request else {
                        break;
                    };

                    let handler = self.handle_request(request);
                    request_handlers.push(handler);
                }
                event = events.next() => {
                    let Some(event) = event else {
                        break;
                    };

                    if choked {
                        accumulator.insert(event);
                    } else {
                        event_handlers.push(self.handle_event(event));
                    }
                },
                new_choked = choker.changed() => {
                    choked = new_choked;

                    if choked {
                        continue;
                    }

                    for event in accumulator.drain() {
                        event_handlers.push(self.handle_event(event));
                    }
                }
                Some(result) = request_handlers.next() => result?,
                Some(result) = event_handlers.next() => result?,
            }
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

                self.send_response(response).await;
                Ok(())
            }
            Err(store::Error::BranchNotFound) => {
                tracing::trace!("root node not found");
                self.send_response(Response::RootNodeError(writer_id, debug.send()))
                    .await;
                Ok(())
            }
            Err(error) => {
                self.send_response(Response::RootNodeError(writer_id, debug.send()))
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
                self.send_response(Response::InnerNodes(
                    inner_nodes,
                    disambiguator,
                    debug.clone().send(),
                ))
                .await;
            }

            if !leaf_nodes.is_empty() {
                tracing::trace!("leaf nodes found");
                self.send_response(Response::LeafNodes(leaf_nodes, disambiguator, debug.send()))
                    .await;
            }
        } else {
            tracing::trace!("child nodes not found");
            self.send_response(Response::ChildNodesError(
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
                self.send_response(Response::Block(content, nonce, debug.send()))
                    .await;
                Ok(())
            }
            Err(store::Error::BlockNotFound) => {
                tracing::trace!("block not found");
                self.send_response(Response::BlockError(block_id, debug.send()))
                    .await;
                Ok(())
            }
            Err(error) => {
                self.send_response(Response::BlockError(block_id, debug.send()))
                    .await;
                Err(error.into())
            }
        }
    }

    async fn handle_event(&self, event: Event) -> Result<()> {
        match event {
            Event::BranchChanged(branch_id) => self.handle_branch_changed_event(branch_id).await,
            Event::BlockReceived(block_id) => self.handle_block_received_event(block_id).await,
            Event::Unknown => self.handle_unknown_event().await,
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
        self.send_response(Response::BlockOffer(block_id, DebugResponse::unsolicited()))
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
        self.send_response(response).await;

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

    async fn send_response(&self, response: Response) {
        if self.tx.send(Content::Response(response)).await.is_ok() {
            self.vault.monitor.responses_sent.increment(1);
        }
    }
}

fn events(rx: broadcast::Receiver<event::Event>) -> impl Stream<Item = Event> {
    stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(event::Event { payload, .. }) => match payload {
                    event::Payload::BranchChanged(branch_id) => {
                        return Some((Event::BranchChanged(branch_id), rx))
                    }
                    event::Payload::BlockReceived(block_id) => {
                        return Some((Event::BlockReceived(block_id), rx))
                    }
                    event::Payload::MaintenanceCompleted => continue,
                },
                Err(RecvError::Lagged(_)) => return Some((Event::Unknown, rx)),
                Err(RecvError::Closed) => return None,
            }
        }
    })
}

#[derive(Eq, PartialEq, Clone, Debug, Hash)]
enum Event {
    BranchChanged(PublicKey),
    BlockReceived(BlockId),
    Unknown,
}

#[derive(Default)]
struct EventAccumulator(HashSet<Event>);

impl EventAccumulator {
    fn insert(&mut self, event: Event) {
        if self.0.contains(&Event::Unknown) {
            return;
        }

        if matches!(event, Event::Unknown) {
            self.0.clear();
        }

        self.0.insert(event);
    }

    fn drain(&mut self) -> impl Iterator<Item = Event> + '_ {
        self.0.drain()
    }
}
