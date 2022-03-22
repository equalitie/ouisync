use super::message::{Content, Request, Response};
use crate::{
    block::{self, BlockId, BLOCK_SIZE},
    crypto::{sign::PublicKey, Hash},
    error::{Error, Result},
    index::{Index, InnerNode, LeafNode},
};
use std::{fmt, time::Duration};
use tokio::{select, sync::mpsc, time};

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

        let mut subscription = index.subscribe();

        // send initial branches
        let branch_ids: Vec<_> = index.branches().await.keys().copied().collect();
        for branch_id in branch_ids {
            handle_branch_changed(index, tx, branch_id).await?;
        }

        let mut report_interval = time::interval(REPORT_INTERVAL);
        let mut stats = Stats::new();

        let handle_request = async {
            loop {
                select! {
                    Some(request) = rx.recv() => {
                        handle_request(index, tx, &mut stats, request).await?
                    }
                    _ = report_interval.tick() => {
                        stats.report()
                    }
                    else => break,
                }
            }

            Ok(())
        };

        let handle_branch_changed = async {
            while let Ok(branch_id) = subscription.recv().await {
                handle_branch_changed(index, tx, branch_id).await?;
            }

            Ok(())
        };

        select! {
            result = handle_request => result,
            result = handle_branch_changed => result,
        }
    }
}

async fn handle_branch_changed(index: &Index, tx: &Sender, branch_id: PublicKey) -> Result<()> {
    let branches = index.branches().await;
    let branch = if let Some(branch) = branches.get(&branch_id) {
        branch
    } else {
        // branch was removed after the notification was fired.
        return Ok(());
    };

    let root_node = branch.root().await;

    if !root_node.summary.is_complete() {
        // send only complete branches
        return Ok(());
    }

    log::trace!(
        "server: handle_branch_changed(branch_id: {:?}, hash: {:?}, vv: {:?}, missing blocks: {})",
        branch_id,
        root_node.proof.hash,
        root_node.proof.version_vector,
        root_node.summary.missing_blocks_count(),
    );

    let response = Response::RootNode {
        proof: root_node.proof.clone().into(),
        summary: root_node.summary,
    };

    // Don't hold the locks while sending is in progress.
    drop(root_node);
    drop(branches);

    tx.send(response).await;

    Ok(())
}

async fn handle_request(
    index: &Index,
    tx: &Sender,
    stats: &mut Stats,
    request: Request,
) -> Result<()> {
    match request {
        Request::ChildNodes(parent_hash) => handle_child_nodes(index, tx, stats, parent_hash).await,
        Request::Block(id) => handle_block(index, tx, stats, id).await,
    }
}

async fn handle_child_nodes(
    index: &Index,
    tx: &Sender,
    stats: &mut Stats,
    parent_hash: Hash,
) -> Result<()> {
    let reporter = RequestReporter::new("handle_child_nodes", &parent_hash);

    let mut conn = index.pool.acquire().await?;

    // At most one of these will be non-empty.
    let inner_nodes = InnerNode::load_children(&mut conn, &parent_hash).await?;
    let leaf_nodes = LeafNode::load_children(&mut conn, &parent_hash).await?;

    drop(conn);

    if !inner_nodes.is_empty() || !leaf_nodes.is_empty() {
        if !inner_nodes.is_empty() {
            tx.send(Response::InnerNodes(inner_nodes)).await;
        }

        if !leaf_nodes.is_empty() {
            tx.send(Response::LeafNodes(leaf_nodes)).await;
        }

        reporter.ok();
        stats.ok();
    } else {
        reporter.not_found();
        stats.not_found();
    }

    Ok(())
}

async fn handle_block(index: &Index, tx: &Sender, stats: &mut Stats, id: BlockId) -> Result<()> {
    let reporter = RequestReporter::new("handle_block", &id);

    let mut conn = index.pool.acquire().await?;
    let mut content = vec![0; BLOCK_SIZE].into_boxed_slice();

    let nonce = match block::read(&mut conn, &id, &mut content).await {
        Ok(nonce) => nonce,
        Err(Error::BlockNotFound(_)) => {
            // This is probably a request to an already deleted orphaned block from an
            // outdated branch. It should be safe to ignore this as the client will request
            // the correct blocks when it becomes up to date to our latest branch.
            reporter.not_found();
            stats.not_found();

            return Ok(());
        }
        Err(error) => return Err(error),
    };

    drop(conn);

    tx.send(Response::Block { content, nonce }).await;
    reporter.ok();
    stats.ok();

    Ok(())
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
        log::trace!("server: {}({:?}) - {}", self.label, self.id, self.status)
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
    count_ok: u64,
    count_not_found: u64,
    report: bool,
}

impl Stats {
    fn new() -> Self {
        Self {
            count_ok: 0,
            count_not_found: 0,
            report: true,
        }
    }

    fn ok(&mut self) {
        self.count_ok += 1;
        self.report = true;
    }

    fn not_found(&mut self) {
        self.count_not_found += 1;
        self.report = true;
    }

    fn report(&mut self) {
        if !self.report {
            return;
        }

        log::debug!(
            "server: request stats - ok: {}, not found: {}, total: {}",
            self.count_ok,
            self.count_not_found,
            self.count_ok + self.count_not_found
        );

        self.report = false;
    }
}
