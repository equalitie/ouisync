use super::{
    debug_payload::{DebugRequestPayload, DebugResponsePayload},
    message::{Content, Request, Response, ResponseDisambiguator},
};
use crate::{
    block::{BlockId, BLOCK_SIZE},
    crypto::{sign::PublicKey, Hash},
    error::{Error, Result},
    event::{Event, Payload},
    repository::RepositoryState,
    store::{self, RootNode},
};
use futures_util::{stream::FuturesUnordered, StreamExt, TryStreamExt};
use tokio::{
    select,
    sync::{broadcast::error::RecvError, mpsc},
};
use tracing::instrument;

pub(crate) struct Server {
    repository: RepositoryState,
    tx: Sender,
    rx: Receiver,
}

impl Server {
    pub fn new(
        repository: RepositoryState,
        tx: mpsc::Sender<Content>,
        rx: mpsc::Receiver<Request>,
    ) -> Self {
        Self {
            repository,
            tx: Sender(tx),
            rx,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let Self { repository, tx, rx } = self;
        let responder = Responder::new(repository, tx);
        let monitor = Monitor::new(repository, tx);

        select! {
            result = responder.run(rx) => result,
            result = monitor.run() => result,
        }
    }
}

/// Receives requests from the peer and replies with responses.
struct Responder<'a> {
    repository: &'a RepositoryState,
    tx: &'a Sender,
}

impl<'a> Responder<'a> {
    fn new(repository: &'a RepositoryState, tx: &'a Sender) -> Self {
        Self { repository, tx }
    }

    async fn run(self, rx: &'a mut Receiver) -> Result<()> {
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
                            let handler = self
                                .repository
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
    async fn handle_root_node(
        &self,
        branch_id: PublicKey,
        debug: DebugRequestPayload,
    ) -> Result<()> {
        let debug = debug.begin_reply();

        let root_node = self
            .repository
            .store()
            .acquire_read()
            .await?
            .load_root_node(&branch_id)
            .await;

        match root_node {
            Ok(node) => {
                tracing::trace!("root node found");

                let response = Response::RootNode {
                    proof: node.proof.into(),
                    block_presence: node.summary.block_presence,
                    debug: debug.send(),
                };

                self.tx.send(response).await;
                Ok(())
            }
            Err(store::Error::BranchNotFound) => {
                tracing::warn!("root node not found");
                self.tx
                    .send(Response::RootNodeError(branch_id, debug.send()))
                    .await;
                Ok(())
            }
            Err(error) => {
                self.tx
                    .send(Response::RootNodeError(branch_id, debug.send()))
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
        debug: DebugRequestPayload,
    ) -> Result<()> {
        let debug = debug.begin_reply();

        let mut reader = self.repository.store().acquire_read().await?;

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
                        debug.send(),
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
            tracing::warn!("child nodes not found");
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
    async fn handle_block(&self, id: BlockId, debug: DebugRequestPayload) -> Result<()> {
        let debug = debug.begin_reply();
        let mut content = vec![0; BLOCK_SIZE].into_boxed_slice();
        let mut reader = self.repository.store().acquire_read().await?;
        let result = reader.read_block(&id, &mut content).await;
        drop(reader); // don't hold the store Reader while sending is in progress

        match result {
            Ok(nonce) => {
                tracing::trace!("block found");
                self.tx
                    .send(Response::Block {
                        content,
                        nonce,
                        debug: debug.send(),
                    })
                    .await;
                Ok(())
            }
            Err(store::Error::BlockNotFound) => {
                tracing::warn!("block not found");
                self.tx.send(Response::BlockError(id, debug.send())).await;
                Ok(())
            }
            Err(error) => {
                self.tx.send(Response::BlockError(id, debug.send())).await;
                Err(error.into())
            }
        }
    }
}

/// Monitors the repository for changes and notifies the peer.
struct Monitor<'a> {
    repository: &'a RepositoryState,
    tx: &'a Sender,
}

impl<'a> Monitor<'a> {
    fn new(repository: &'a RepositoryState, tx: &'a Sender) -> Self {
        Self { repository, tx }
    }

    async fn run(self) -> Result<()> {
        let mut subscription = self.repository.index.subscribe();

        // send initial branches
        self.handle_all_branches_changed().await?;

        loop {
            match subscription.recv().await {
                Ok(Event {
                    payload:
                        Payload::BranchChanged(branch_id) | Payload::BlockReceived { branch_id, .. },
                    ..
                }) => self.handle_branch_changed(branch_id).await?,
                Err(RecvError::Lagged(_)) => {
                    tracing::warn!("event receiver lagged");
                    self.handle_all_branches_changed().await?
                }
                Err(RecvError::Closed) => break,
            }
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
            Err(Error::EntryNotFound) => {
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

        let response = Response::RootNode {
            proof: root_node.proof.into(),
            block_presence: root_node.summary.block_presence,
            debug: DebugResponsePayload::unsolicited(),
        };

        self.tx.send(response).await;

        Ok(())
    }

    async fn load_root_nodes(&self) -> Result<Vec<RootNode>> {
        self.repository
            .store()
            .acquire_read()
            .await?
            .load_root_nodes()
            .err_into()
            .try_collect()
            .await
    }

    async fn load_root_node(&self, branch_id: &PublicKey) -> Result<RootNode> {
        Ok(self
            .repository
            .store()
            .acquire_read()
            .await?
            .load_root_node(branch_id)
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
