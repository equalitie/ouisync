use std::cmp::Ordering;

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

pub struct Server {
    index: Index,
    stream: ServerStream,
}

impl Server {
    pub fn new(index: Index, stream: ServerStream) -> Self {
        Self { index, stream }
    }

    pub async fn run(&mut self) {
        loop {
            match self.push_snapshot().await {
                Ok(true) => {}
                Ok(false) => break, // finished
                Err(error) => {
                    log::error!("Server failed: {}", error.verbose());
                    break;
                }
            }
        }
    }

    async fn push_snapshot(&mut self) -> Result<bool> {
        while let Some(request) = self.stream.recv().await {
            self.handle_request(request).await?;
        }

        Ok(false)
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
        let mut tx = self.index.pool.begin().await?;
        let node = RootNode::load_latest(&mut tx, &self.index.this_replica_id).await?;
        drop(tx);

        if let Some(node) = node {
            if let Some(Ordering::Greater) | None = node.versions.partial_cmp(&their_versions) {
                self.stream
                    .send(Response::RootNode(node.hash))
                    .await
                    .unwrap_or(());
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
