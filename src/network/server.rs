use super::{
    message::{Request, Response},
    message_broker::ServerStream,
};
use crate::{
    block::{self, BlockId, BLOCK_SIZE},
    crypto::Hash,
    error::Result,
    index::{Index, InnerNode, LeafNode, RootNode},
};
use tokio::{select, sync::watch};

pub struct Server {
    index: Index,
    notify: watch::Receiver<()>,
    stream: ServerStream,
    // "Cookie" number that gets included in the next sent `RootNode` response. The client stores
    // it and sends it back in their next `RootNode` request. This is then used by the server to
    // decide whether the client is up to date (if their cookie is equal (or greater) to ours, they
    // are up to date, otherwise they are not). The server increments this every time there is a
    // change to the local branch.
    cookie: u64,
    // Flag indicating whether the server is waiting for a local change before sending a `RootNode`
    // response to the client. This gets set to true when the client send us a `RootNode` request
    // whose cookie is equal (or greater) than ours which indicates that they are up to date with
    // us.
    waiting: bool,
}

impl Server {
    pub async fn new(index: Index, stream: ServerStream) -> Self {
        // subscribe to branch change notifications
        let notify = index.branches().await.local().subscribe();

        Self {
            index,
            notify,
            stream,
            cookie: 1,
            waiting: false,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
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
                _ = self.notify.changed() => self.handle_local_change().await?
            }
        }

        Ok(())
    }

    async fn handle_local_change(&mut self) -> Result<()> {
        self.cookie = self.cookie.wrapping_add(1);

        if self.waiting {
            self.waiting = false;
            self.handle_root_node(0).await?;
        }

        Ok(())
    }

    async fn handle_request(&mut self, request: Request) -> Result<()> {
        match request {
            Request::RootNode { cookie } => self.handle_root_node(cookie).await,
            Request::InnerNodes {
                parent_hash,
                inner_layer,
            } => self.handle_inner_nodes(parent_hash, inner_layer).await,
            Request::LeafNodes { parent_hash } => self.handle_leaf_nodes(parent_hash).await,
            Request::Block(id) => self.handle_block(id).await,
        }
    }

    async fn handle_root_node(&mut self, cookie: u64) -> Result<()> {
        log::trace!("cookies: server={}, client={}", self.cookie, cookie);

        // Note: the comparison with zero is there to handle the case when the cookie wraps around.
        if cookie < self.cookie || self.cookie == 0 {
            if let Some(node) =
                RootNode::load_latest(&self.index.pool, &self.index.this_replica_id).await?
            {
                self.stream
                    .send(Response::RootNode {
                        cookie: self.cookie,
                        versions: node.versions,
                        hash: node.hash,
                        summary: node.summary,
                    })
                    .await
                    .unwrap_or(());
                return Ok(());
            }
        }

        self.waiting = true;

        Ok(())
    }

    async fn handle_inner_nodes(&self, parent_hash: Hash, inner_layer: usize) -> Result<()> {
        let nodes = InnerNode::load_children(&self.index.pool, &parent_hash).await?;

        self.stream
            .send(Response::InnerNodes {
                parent_hash,
                inner_layer,
                nodes,
            })
            .await
            .unwrap_or(());

        Ok(())
    }

    async fn handle_leaf_nodes(&self, parent_hash: Hash) -> Result<()> {
        let nodes = LeafNode::load_children(&self.index.pool, &parent_hash).await?;

        self.stream
            .send(Response::LeafNodes { parent_hash, nodes })
            .await
            .unwrap_or(());

        Ok(())
    }

    async fn handle_block(&self, id: BlockId) -> Result<()> {
        let mut content = vec![0; BLOCK_SIZE].into_boxed_slice();
        let auth_tag = block::read(&self.index.pool, &id, &mut content).await?;

        self.stream
            .send(Response::Block {
                id,
                content,
                auth_tag,
            })
            .await
            .unwrap_or(());

        Ok(())
    }
}
