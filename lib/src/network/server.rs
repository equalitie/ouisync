use super::{
    channel_info::ChannelInfo,
    message::{Content, Request, Response},
};
use crate::{
    block::{self, BlockId, BLOCK_SIZE},
    crypto::{sign::PublicKey, Hash},
    error::{Error, Result},
    event::BranchChangedReceiver,
    index::{Index, InnerNode, LeafNode, RootNode},
};
use futures_util::TryStreamExt;
use std::{fmt, time::Duration};
use tokio::{
    select,
    sync::{broadcast::error::RecvError, mpsc},
    time::{self, MissedTickBehavior},
};

const REPORT_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) struct Server {
    index: Index,
    tx: Sender,
    rx: Receiver,
}

impl Server {
    pub fn new(index: Index, tx: mpsc::Sender<Content>, rx: mpsc::Receiver<Request>) -> Self {
        Self {
            index,
            tx: Sender(tx),
            rx,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let Self { index, tx, rx } = self;
        let responder = Responder::new(index, tx, rx);
        let monitor = Monitor::new(index, tx);

        select! {
            result = responder.run() => result,
            result = monitor.run() => result,
        }
    }
}

/// Receives requests from the peer and replies with responses.
struct Responder<'a> {
    index: &'a Index,
    tx: &'a Sender,
    rx: &'a mut Receiver,
    stats: Stats,
}

impl<'a> Responder<'a> {
    fn new(index: &'a Index, tx: &'a Sender, rx: &'a mut Receiver) -> Self {
        Self {
            index,
            tx,
            rx,
            stats: Stats::new(),
        }
    }

    async fn run(mut self) -> Result<()> {
        let mut report_interval = time::interval(REPORT_INTERVAL);
        report_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            select! {
                request = self.rx.recv() => {
                    if let Some(request) = request {
                        self.handle_request(request).await?;
                    } else {
                        break;
                    }
                }
                _ = report_interval.tick() => {
                    self.stats.report()
                }
            }
        }

        Ok(())
    }

    async fn handle_request(&mut self, request: Request) -> Result<()> {
        match request {
            Request::ChildNodes(parent_hash) => self.handle_child_nodes(parent_hash).await,
            Request::Block(id) => self.handle_block(id).await,
        }
    }

    async fn handle_child_nodes(&mut self, parent_hash: Hash) -> Result<()> {
        let reporter = RequestReporter::new("handle_child_nodes", &parent_hash);
        self.stats.node();

        let mut conn = self.index.pool.acquire().await?;

        // At most one of these will be non-empty.
        let inner_nodes = InnerNode::load_children(&mut conn, &parent_hash).await?;
        let leaf_nodes = LeafNode::load_children(&mut conn, &parent_hash).await?;

        drop(conn);

        if !inner_nodes.is_empty() || !leaf_nodes.is_empty() {
            if !inner_nodes.is_empty() {
                self.tx.send(Response::InnerNodes(inner_nodes)).await;
            }

            if !leaf_nodes.is_empty() {
                self.tx.send(Response::LeafNodes(leaf_nodes)).await;
            }

            reporter.ok();
        } else {
            self.tx.send(Response::ChildNodesError(parent_hash)).await;
            reporter.not_found();
        }

        Ok(())
    }

    async fn handle_block(&mut self, id: BlockId) -> Result<()> {
        let reporter = RequestReporter::new("handle_block", &id);
        self.stats.block();

        let mut content = vec![0; BLOCK_SIZE].into_boxed_slice();
        let mut conn = self.index.pool.acquire().await?;
        let result = block::read(&mut conn, &id, &mut content).await;
        drop(conn); // don't hold the connection while sending is in progress

        match result {
            Ok(nonce) => {
                self.tx.send(Response::Block { content, nonce }).await;
                reporter.ok();
                Ok(())
            }
            Err(Error::BlockNotFound(_)) => {
                self.tx.send(Response::BlockError(id)).await;
                reporter.not_found();
                Ok(())
            }
            Err(error) => {
                self.tx.send(Response::BlockError(id)).await;
                Err(error)
            }
        }
    }
}

/// Monitors the repository for changes and notifies the peer.
struct Monitor<'a> {
    index: &'a Index,
    tx: &'a Sender,
}

impl<'a> Monitor<'a> {
    fn new(index: &'a Index, tx: &'a Sender) -> Self {
        Self { index, tx }
    }

    async fn run(self) -> Result<()> {
        let mut subscription = BranchChangedReceiver::new(self.index.subscribe());

        // send initial branches
        self.handle_all_branches_changed().await?;

        loop {
            match subscription.recv().await {
                Ok(branch_id) => self.handle_branch_changed(branch_id).await?,
                Err(RecvError::Lagged(_)) => self.handle_all_branches_changed().await?,
                Err(RecvError::Closed) => break,
            }
        }

        Ok(())
    }

    async fn handle_all_branches_changed(&self) -> Result<()> {
        let root_nodes = self.load_all_root_nodes().await?;
        for root_node in root_nodes {
            self.handle_root_node_changed(root_node).await?;
        }

        Ok(())
    }

    async fn handle_branch_changed(&self, branch_id: PublicKey) -> Result<()> {
        let root_node = match self.load_root_node(branch_id).await {
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
        if !root_node.summary.is_complete() {
            // send only complete branches
            return Ok(());
        }

        if root_node.proof.version_vector.is_empty() {
            // Do not send branches with empty version vectors because they have no content yet
            return Ok(());
        }

        tracing::trace!(
            "{} handle_branch_changed(branch_id: {:?}, hash: {:?}, vv: {:?}, missing blocks: {})",
            ChannelInfo::current(),
            root_node.proof.writer_id,
            root_node.proof.hash,
            root_node.proof.version_vector,
            root_node.summary.missing_blocks_count(),
        );

        let response = Response::RootNode {
            proof: root_node.proof.into(),
            summary: root_node.summary,
        };

        self.tx.send(response).await;

        Ok(())
    }

    async fn load_all_root_nodes(&self) -> Result<Vec<RootNode>> {
        let mut conn = self.index.pool.acquire().await?;
        RootNode::load_all_latest_complete(&mut conn)
            .try_collect()
            .await
    }

    async fn load_root_node(&self, branch_id: PublicKey) -> Result<RootNode> {
        let mut conn = self.index.pool.acquire().await?;
        RootNode::load_latest_complete_by_writer(&mut conn, branch_id).await
    }
}

type Receiver = mpsc::Receiver<Request>;

struct Sender(mpsc::Sender<Content>);

impl Sender {
    async fn send(&self, response: Response) -> bool {
        self.0.send(Content::Response(response)).await.is_ok()
    }
}

struct RequestReporter<'a, T>
where
    T: fmt::Debug,
{
    label: &'static str,
    id: &'a T,
    status: Status,
}

impl<'a, T> RequestReporter<'a, T>
where
    T: fmt::Debug,
{
    fn new(label: &'static str, id: &'a T) -> Self {
        Self {
            label,
            id,
            status: Status::Error,
        }
    }

    fn not_found(mut self) {
        self.status = Status::NotFound;
    }

    fn ok(mut self) {
        self.status = Status::Ok;
    }
}

impl<'a, T> Drop for RequestReporter<'a, T>
where
    T: fmt::Debug,
{
    fn drop(&mut self) {
        tracing::trace!(
            "{} {}({:?}) - {}",
            ChannelInfo::current(),
            self.label,
            self.id,
            self.status
        )
    }
}

enum Status {
    Ok,
    NotFound,
    Error,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Ok => write!(f, "ok"),
            Self::NotFound => write!(f, "not found"),
            Self::Error => write!(f, "error"),
        }
    }
}

struct Stats {
    nodes: u64,
    blocks: u64,
    report: bool,
}

impl Stats {
    fn new() -> Self {
        Self {
            nodes: 0,
            blocks: 0,
            report: true,
        }
    }

    fn node(&mut self) {
        self.nodes += 1;
        self.report = true;
    }

    fn block(&mut self) {
        self.blocks += 1;
        self.report = true;
    }

    fn report(&mut self) {
        if !self.report {
            return;
        }

        tracing::debug!(
            "{} request stats - nodes: {}, blocks: {}, total: {}",
            ChannelInfo::current(),
            self.nodes,
            self.blocks,
            self.nodes + self.blocks
        );

        self.report = false;
    }
}
