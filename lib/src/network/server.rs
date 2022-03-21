use super::{
    message::{Request, Response},
    message_broker::ServerStream,
};
use crate::{
    block::{self, BlockId, BLOCK_SIZE},
    crypto::{sign::PublicKey, Hash},
    error::{Error, Result},
    index::{Index, InnerNode, LeafNode},
};
use std::fmt;
use tokio::select;

pub(crate) struct Server {
    index: Index,
    stream: ServerStream,
}

impl Server {
    pub fn new(index: Index, stream: ServerStream) -> Self {
        Self { index, stream }
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut subscription = self.index.subscribe();

        // send initial branches
        let branch_ids: Vec<_> = self.index.branches().await.keys().copied().collect();
        for branch_id in branch_ids {
            self.handle_branch_changed(branch_id).await?;
        }

        loop {
            select! {
                request = self.stream.recv() => {
                    let request = if let Some(request) = request {
                        request
                    } else {
                        break;
                    };

                    self.handle_request(request).await?
                }
                branch_id = subscription.recv() => {
                    let branch_id = if let Ok(branch_id) = branch_id {
                        branch_id
                    } else {
                        break;
                    };

                    self.handle_branch_changed(branch_id).await?
                }
            }
        }

        Ok(())
    }

    async fn handle_branch_changed(&self, branch_id: PublicKey) -> Result<()> {
        let branches = self.index.branches().await;
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

        log::debug!(
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

        self.stream.send(response).await;

        Ok(())
    }

    async fn handle_request(&self, request: Request) -> Result<()> {
        match request {
            Request::ChildNodes(parent_hash) => self.handle_child_nodes(parent_hash).await,
            Request::Block(id) => self.handle_block(id).await,
        }
    }

    async fn handle_child_nodes(&self, parent_hash: Hash) -> Result<()> {
        let reporter = RequestReporter::new("handle_child_nodes", &parent_hash);

        let mut conn = self.index.pool.acquire().await?;

        // At most one of these will be non-empty.
        let inner_nodes = InnerNode::load_children(&mut conn, &parent_hash).await?;
        let leaf_nodes = LeafNode::load_children(&mut conn, &parent_hash).await?;

        drop(conn);

        if !inner_nodes.is_empty() || !leaf_nodes.is_empty() {
            if !inner_nodes.is_empty() {
                self.stream.send(Response::InnerNodes(inner_nodes)).await;
            }

            if !leaf_nodes.is_empty() {
                self.stream.send(Response::LeafNodes(leaf_nodes)).await;
            }

            reporter.ok();
        } else {
            reporter.not_found();
        }

        Ok(())
    }

    async fn handle_block(&self, id: BlockId) -> Result<()> {
        let reporter = RequestReporter::new("handle_block", &id);

        let mut conn = self.index.pool.acquire().await?;
        let mut content = vec![0; BLOCK_SIZE].into_boxed_slice();

        let nonce = match block::read(&mut conn, &id, &mut content).await {
            Ok(nonce) => nonce,
            Err(Error::BlockNotFound(_)) => {
                // This is probably a request to an already deleted orphaned block from an
                // outdated branch. It should be safe to ignore this as the client will request
                // the correct blocks when it becomes up to date to our latest branch.
                reporter.not_found();
                return Ok(());
            }
            Err(error) => return Err(error),
        };

        drop(conn);

        self.stream.send(Response::Block { content, nonce }).await;
        reporter.ok();

        Ok(())
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
        log::debug!("server: {}({:?}) - {}", self.label, self.id, self.status)
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
