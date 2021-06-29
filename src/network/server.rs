use super::{
    message::{Request, Response},
    message_broker::ServerStream,
};
use crate::{
    block::{self, BlockId, BLOCK_SIZE},
    crypto::Hash,
    error::Result,
    index::{Index, InnerNode, LeafNode, RootNode},
    version_vector::VersionVector,
};
use std::{cmp::Ordering, collections::VecDeque, sync::Arc};
use tokio::{select, sync::Notify};

const BACKLOG_CAPACITY: usize = 32;

pub struct Server {
    index: Index,
    notify: Arc<Notify>,
    stream: ServerStream,
    // `RootNode` requests which we can't fulfil because their version vector is strictly newer
    // than our latest. We backlog them here until we detect local branch change, then attempt to
    // handle them again.
    backlog: VecDeque<VersionVector>,
}

impl Server {
    pub async fn new(index: Index, stream: ServerStream) -> Self {
        // subscribe to branch change notifications
        let branch = index.local_branch().await;
        let notify = branch.subscribe();

        Self {
            index,
            notify,
            stream,
            backlog: VecDeque::with_capacity(BACKLOG_CAPACITY),
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
                _ = self.notify.notified() => self.handle_local_change().await?
            }
        }

        Ok(())
    }

    async fn handle_local_change(&mut self) -> Result<()> {
        while let Some(versions) = self.backlog.pop_front() {
            self.handle_root_node(versions).await?
        }

        Ok(())
    }

    async fn handle_request(&mut self, request: Request) -> Result<()> {
        match request {
            Request::RootNode(versions) => self.handle_root_node(versions).await,
            Request::InnerNodes {
                parent_hash,
                inner_layer,
            } => self.handle_inner_nodes(parent_hash, inner_layer).await,
            Request::LeafNodes { parent_hash } => self.handle_leaf_nodes(parent_hash).await,
            Request::Block(id) => self.handle_block(id).await,
        }
    }

    async fn handle_root_node(&mut self, their_versions: VersionVector) -> Result<()> {
        let node = RootNode::load_latest(&self.index.pool, &self.index.this_replica_id).await?;

        // Check whether we have a snapshot that is newer or concurrent to the one they have.
        if let Some(node) = node {
            if node
                .versions
                .partial_cmp(&their_versions)
                .map(Ordering::is_gt)
                .unwrap_or(true)
            {
                // We do have one - send the response.
                self.stream
                    .send(Response::RootNode {
                        versions: node.versions,
                        hash: node.hash,
                    })
                    .await
                    .unwrap_or(());
                return Ok(());
            }
        }

        // We don't have one yet - backlog the request and try to fulfil it next time when the
        // local branch changes.

        // If the backlog is at capacity, evict the oldest entries.
        while self.backlog.len() >= BACKLOG_CAPACITY {
            self.backlog.pop_front();
        }

        self.backlog.push_back(their_versions);

        Ok(())
    }

    async fn handle_inner_nodes(&self, parent_hash: Hash, inner_layer: usize) -> Result<()> {
        let nodes = InnerNode::load_children(&self.index.pool, &parent_hash).await?;

        if !nodes.is_empty() {
            self.stream
                .send(Response::InnerNodes {
                    parent_hash,
                    inner_layer,
                    nodes,
                })
                .await
                .unwrap_or(())
        }

        Ok(())
    }

    async fn handle_leaf_nodes(&self, parent_hash: Hash) -> Result<()> {
        let nodes = LeafNode::load_children(&self.index.pool, &parent_hash).await?;

        if !nodes.is_empty() {
            self.stream
                .send(Response::LeafNodes { parent_hash, nodes })
                .await
                .unwrap_or(())
        }

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
