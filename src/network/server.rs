use super::{
    message::{Request, Response},
    message_broker::ServerStream,
};
use crate::{
    crypto::Hash,
    error::Result,
    index::{Index, InnerNode, LeafNode, RootNode},
    version_vector::VersionVector,
};
use std::{cmp::Ordering, sync::Arc};
use tokio::{select, sync::Notify};

pub struct Server {
    index: Index,
    notify: Arc<Notify>,
    stream: ServerStream,
}

impl Server {
    pub async fn new(index: Index, stream: ServerStream) -> Self {
        // subscribe to branch change notifications
        let branch = index.this_branch().await;
        let notify = branch.subscribe();

        Self {
            index,
            notify,
            stream,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        while let Some(request) = self.stream.recv().await {
            self.handle_request(request).await?;
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
        }
    }

    async fn handle_root_node(&mut self, their_versions: VersionVector) -> Result<()> {
        loop {
            let mut tx = self.index.pool.begin().await?;
            let node = RootNode::load_latest(&mut tx, &self.index.this_replica_id).await?;
            drop(tx);

            // Check whether we have a snapshot that is newer or concurrent to the one they have.
            if let Some(node) = node {
                if let Some(Ordering::Greater) | None = node.versions.partial_cmp(&their_versions) {
                    // We do have one - send the response.
                    self.stream
                        .send(Response::RootNode(node.hash))
                        .await
                        .unwrap_or(());
                    break;
                }
            }

            // We don't have one yet - wait until our branch changes or we get another request.
            select! {
                _ = self.notify.notified() => (),
                _ = self.stream.peek() => break,
            }
        }

        Ok(())
    }

    async fn handle_inner_nodes(&mut self, parent_hash: Hash, inner_layer: usize) -> Result<()> {
        let mut tx = self.index.pool.begin().await?;
        let nodes = InnerNode::load_children(&mut tx, &parent_hash).await?;
        drop(tx);

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

    async fn handle_leaf_nodes(&mut self, parent_hash: Hash) -> Result<()> {
        let mut tx = self.index.pool.begin().await?;
        let nodes = LeafNode::load_children(&mut tx, &parent_hash).await?;
        drop(tx);

        if !nodes.is_empty() {
            self.stream
                .send(Response::LeafNodes { parent_hash, nodes })
                .await
                .unwrap_or(())
        }

        Ok(())
    }
}
