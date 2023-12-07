use super::{
    choke::Choker,
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
    sync::stream::Throttle,
};
use futures_util::{
    stream::{self, FuturesUnordered},
    Stream, StreamExt, TryStreamExt,
};
use std::{collections::HashSet, mem, pin::pin, time::Duration};
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
    rx: Receiver,
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
            inner: Inner {
                vault,
                tx: Sender(tx),
            },
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
    tx: Sender,
}

impl Inner {
    async fn run(&self, rx: &mut Receiver, choker: &mut Choker) -> Result<()> {
        // Important: make sure to create the event subscription first, before calling
        // `handle_all_branches_changed` otherwise we might miss some events.
        let mut events = pin!(Throttle::new(
            events(self.vault.event_tx.subscribe()),
            Duration::from_secs(1)
        ));

        // Because we're using the choker, we can't handle events that we receive on `events` right
        // a way. We need to wait for being unchoked. In the meanwhile we "accumulate" events from
        // `events` into the `accumulator` and once we get unchoked we process them all at once.
        // Note that this allows us to process each branch event only once even if we received
        // multiple events per particular branch.
        let mut accumulator = BranchAccumulator::All;

        // This is to handle multiple requests / events at once.
        // TODO: Do we need to limit the number of concurrent request handlers? We have a limit on
        // the number of read DB transactions, so that might be enough, but perhaps we also want to
        // limit per peer?
        let mut request_handlers = FuturesUnordered::new();
        let mut one_branch_event_handlers = FuturesUnordered::new();
        let mut all_branch_event_handlers = FuturesUnordered::new();

        // Send the initial root node messages
        self.handle_all_branches_changed().await?;

        loop {
            select! {
                request = rx.recv(), if !choker.is_choked() => {
                    let Some(request)  = request else {
                        break;
                    };

                    let handler = self
                        .vault
                        .monitor
                        .handle_request_metric
                        .measure_ok(self.handle_request(request));

                    request_handlers.push(handler);
                }
                event = events.next() => {
                    let Some(event) = event else {
                        break;
                    };

                    if choker.is_choked() {
                        accumulator.insert(event);
                    } else {
                        one_branch_event_handlers.push(self.handle_event(event));
                    }
                },
                Some(result) = request_handlers.next() => result?,
                Some(result) = one_branch_event_handlers.next() => result?,
                Some(result) = all_branch_event_handlers.next() => result?,
                _ = choker.changed() => {
                    if !choker.is_choked() {
                        all_branch_event_handlers
                            .push(self.handle_accumulated_events(mem::take(&mut accumulator)));
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_request(&self, request: Request) -> Result<()> {
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

                self.tx.send(response).await;
                Ok(())
            }
            Err(store::Error::BranchNotFound) => {
                tracing::trace!("root node not found");
                self.tx
                    .send(Response::RootNodeError(writer_id, debug.send()))
                    .await;
                Ok(())
            }
            Err(error) => {
                self.tx
                    .send(Response::RootNodeError(writer_id, debug.send()))
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
                self.tx
                    .send(Response::InnerNodes(
                        inner_nodes,
                        disambiguator,
                        debug.clone().send(),
                    ))
                    .await;
            }

            if !leaf_nodes.is_empty() {
                tracing::trace!("leaf nodes found");
                self.tx
                    .send(Response::LeafNodes(leaf_nodes, disambiguator, debug.send()))
                    .await;
            }
        } else {
            tracing::trace!("child nodes not found");
            self.tx
                .send(Response::ChildNodesError(
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
                self.tx
                    .send(Response::Block(content, nonce, debug.send()))
                    .await;
                Ok(())
            }
            Err(store::Error::BlockNotFound) => {
                tracing::trace!("block not found");
                self.tx
                    .send(Response::BlockError(block_id, debug.send()))
                    .await;
                Ok(())
            }
            Err(error) => {
                self.tx
                    .send(Response::BlockError(block_id, debug.send()))
                    .await;
                Err(error.into())
            }
        }
    }

    async fn handle_event(&self, event: BranchChanged) -> Result<()> {
        match event {
            BranchChanged::One(branch_id) => self.handle_branch_changed(branch_id).await,
            BranchChanged::All => self.handle_all_branches_changed().await,
        }
    }

    async fn handle_accumulated_events(&self, accumulator: BranchAccumulator) -> Result<()> {
        match accumulator {
            BranchAccumulator::All => {
                self.handle_all_branches_changed().await?;
            }
            BranchAccumulator::Some(branches) => {
                for branch_id in branches {
                    self.handle_branch_changed(branch_id).await?;
                }
            }
            BranchAccumulator::None => (),
        }

        Ok(())
    }

    async fn handle_all_branches_changed(&self) -> Result<()> {
        let root_nodes = self.load_root_nodes().await?;
        for root_node in root_nodes {
            self.handle_root_node_changed(root_node).await?;
        }

        Ok(())
    }

    async fn handle_branch_changed(&self, branch_id: PublicKey) -> Result<()> {
        let root_node = match self.load_root_node(&branch_id).await {
            Ok(node) => node,
            Err(Error::Store(store::Error::BranchNotFound)) => {
                // branch was removed after the notification was fired.
                return Ok(());
            }
            Err(error) => return Err(error),
        };

        self.handle_root_node_changed(root_node).await
    }

    async fn handle_root_node_changed(&self, root_node: RootNode) -> Result<()> {
        if !root_node.summary.state.is_approved() {
            // send only approved branches
            return Ok(());
        }

        if root_node.proof.version_vector.is_empty() {
            // Do not send branches with empty version vectors because they have no content yet
            return Ok(());
        }

        tracing::trace!(
            branch_id = ?root_node.proof.writer_id,
            hash = ?root_node.proof.hash,
            vv = ?root_node.proof.version_vector,
            block_presence = ?root_node.summary.block_presence,
            "handle_branch_changed",
        );

        let response = Response::RootNode(
            root_node.proof.into(),
            root_node.summary.block_presence,
            DebugResponse::unsolicited(),
        );

        self.tx.send(response).await;

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
}

type Receiver = mpsc::Receiver<Request>;

struct Sender(mpsc::Sender<Content>);

impl Sender {
    async fn send(&self, response: Response) -> bool {
        self.0.send(Content::Response(response)).await.is_ok()
    }
}

fn events(rx: broadcast::Receiver<Event>) -> impl Stream<Item = BranchChanged> {
    stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(Event { payload, .. }) => match payload {
                    Payload::BranchChanged(branch_id)
                    | Payload::BlockReceived { branch_id, .. } => {
                        return Some((BranchChanged::One(branch_id), rx))
                    }
                    Payload::MaintenanceCompleted => continue,
                },
                Err(RecvError::Lagged(_)) => return Some((BranchChanged::All, rx)),
                Err(RecvError::Closed) => return None,
            }
        }
    })
}

#[derive(Eq, PartialEq, Clone, Debug, Hash)]
enum BranchChanged {
    One(PublicKey),
    All,
}

/// When we receive events that a branch has changed, we accumulate those events into this
/// structure and use it once we receive a permit from the choker.
#[derive(Default)]
enum BranchAccumulator {
    All,
    Some(HashSet<PublicKey>),
    #[default]
    None,
}

impl BranchAccumulator {
    fn insert(&mut self, event: BranchChanged) {
        match event {
            BranchChanged::One(branch_id) => {
                self.insert_one(branch_id);
            }
            BranchChanged::All => {
                self.insert_all();
            }
        }
    }

    fn insert_one(&mut self, branch_id: PublicKey) {
        match self {
            Self::All => (),
            Self::Some(branches) => {
                branches.insert(branch_id);
            }
            Self::None => {
                let mut branches = HashSet::new();
                branches.insert(branch_id);
                *self = BranchAccumulator::Some(branches);
            }
        }
    }

    fn insert_all(&mut self) {
        *self = BranchAccumulator::All;
    }
}
