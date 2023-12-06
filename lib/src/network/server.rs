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
    vault: Vault,
    tx: Sender,
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
            vault,
            tx: Sender(tx),
            rx,
            choker,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let Self {
            vault,
            tx,
            rx,
            choker,
        } = self;
        let responder = Responder::new(vault, tx);
        let monitor = Monitor::new(vault, tx);

        select! {
            result = responder.run(rx, choker.clone()) => result,
            result = monitor.run(choker.clone()) => result,
        }
    }
}

/// Receives requests from the peer and replies with responses.
struct Responder<'a> {
    vault: &'a Vault,
    tx: &'a Sender,
}

impl<'a> Responder<'a> {
    fn new(vault: &'a Vault, tx: &'a Sender) -> Self {
        Self { vault, tx }
    }

    async fn run(self, rx: &'a mut Receiver, mut choker: Choker) -> Result<()> {
        let mut handlers = FuturesUnordered::new();

        loop {
            // The `select!` is used so we can handle multiple requests at once.
            // TODO: Do we need to limit the number of concurrent handlers? We have a limit on the
            // number of read DB transactions, so that might be enough, but perhaps we also want to
            // limit per peer?
            select! {
                request = rx.recv() => {
                    match request {
                        Some(request) => {
                            choker.wait_until_unchoked().await;

                            let handler = self
                                .vault
                                .monitor
                                .handle_request_metric
                                .measure_ok(self.handle_request(request));

                            handlers.push(handler);
                        },
                        None => break,
                    }
                },
                Some(result) = handlers.next() => {
                    if result.is_err() {
                        break;
                    }
                },
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
}

/// Monitors the repository for changes and notifies the peer.
struct Monitor<'a> {
    vault: &'a Vault,
    tx: &'a Sender,
}

impl<'a> Monitor<'a> {
    fn new(vault: &'a Vault, tx: &'a Sender) -> Self {
        Self { vault, tx }
    }

    async fn run(self, mut choker: Choker) -> Result<()> {
        // Important: make sure to create the event subscription first, before calling
        // `handle_all_branches_changed` otherwise we might miss some events.
        let mut events = pin!(Throttle::new(
            events(self.vault.event_tx.subscribe()),
            Duration::from_secs(1)
        ));

        // Explanation of the code below: Because we're using the choker, we can't handle events
        // that we receive on `events` right a way. We need to wait for the choker to give us a
        // permit. In the mean while we "accumulate" events from `events` into the `accumulator`
        // and once we receive a permit from the `choker` we process them all at once. Note that
        // this allows us to process each branch event only once even if we received multiple
        // events per particular branch.

        let mut accumulator = BranchAccumulator::All;
        let mut choked = true;

        loop {
            select! {
                event = events.next() => {
                    let Some(event) = event else {
                        break;
                    };

                    match event {
                        BranchChanged::One(branch_id) => {
                            accumulator.insert_one_branch(branch_id);
                        },
                        BranchChanged::All => {
                            accumulator.insert_all_branches();
                        }
                    }

                    choked = true;
                },
                _ = choker.wait_until_unchoked(), if choked => {
                    self.apply_accumulator(mem::take(&mut accumulator)).await?;
                    choked = false;
                }
            }
        }

        Ok(())
    }

    async fn apply_accumulator(&self, accumulator: BranchAccumulator) -> Result<()> {
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
    fn insert_one_branch(&mut self, branch_id: PublicKey) {
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

    fn insert_all_branches(&mut self) {
        *self = BranchAccumulator::All;
    }
}
