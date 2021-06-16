use super::{
    message::{Request, Response},
    message_broker::ClientStream,
};
use crate::{
    crypto::{Hash, Hashable},
    error::Result,
    index::{self, Index, InnerNodeMap, LeafNodeSet, RootNode, INNER_LAYER_COUNT},
    replica_id::ReplicaId,
};

pub struct Client {
    index: Index,
    their_replica_id: ReplicaId,
    stream: ClientStream,
}

impl Client {
    pub fn new(index: Index, their_replica_id: ReplicaId, stream: ClientStream) -> Self {
        Self {
            index,
            their_replica_id,
            stream,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        while self.pull_snapshot().await? {}
        Ok(())
    }

    async fn pull_snapshot(&mut self) -> Result<bool> {
        let mut tx = self.index.pool.begin().await?;
        let this_versions = RootNode::load_latest(&mut tx, &self.index.this_replica_id)
            .await?
            .map(|node| node.versions)
            .unwrap_or_default();
        drop(tx);

        self.stream
            .send(Request::RootNode(this_versions))
            .await
            .unwrap_or(());

        while let Some(response) = self.stream.recv().await {
            self.handle_response(response).await?;

            if self.is_complete().await? {
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn handle_response(&mut self, response: Response) -> Result<()> {
        match response {
            Response::RootNode(hash) => self.handle_root_node(hash).await,
            Response::InnerNodes {
                parent_hash,
                inner_layer,
                nodes,
            } => {
                self.handle_inner_nodes(parent_hash, inner_layer, nodes)
                    .await
            }
            Response::LeafNodes { parent_hash, nodes } => {
                self.handle_leaf_nodes(parent_hash, nodes).await
            }
        }
    }

    async fn handle_root_node(&mut self, hash: Hash) -> Result<()> {
        let mut tx = self.index.pool.begin().await?;
        let (node, changed) = RootNode::create(&mut tx, &self.their_replica_id, hash).await?;
        index::detect_complete_snapshots(&mut tx, hash, 0).await?;
        tx.commit().await?;

        if changed {
            self.stream
                .send(Request::InnerNodes {
                    parent_hash: node.hash,
                    inner_layer: 0,
                })
                .await
                .unwrap_or(())
        }

        Ok(())
    }

    async fn handle_inner_nodes(
        &mut self,
        parent_hash: Hash,
        inner_layer: usize,
        nodes: InnerNodeMap,
    ) -> Result<()> {
        if parent_hash != nodes.hash() {
            log::warn!("inner nodes parent hash mismatch");
            return Ok(());
        }

        let mut tx = self.index.pool.begin().await?;
        let mut changed = vec![];
        for (bucket, node) in nodes {
            if node.save(&mut tx, &parent_hash, bucket).await? {
                changed.push(node.hash);
            }
        }
        index::detect_complete_snapshots(&mut tx, parent_hash, inner_layer).await?;
        tx.commit().await?;

        if inner_layer < INNER_LAYER_COUNT - 1 {
            for parent_hash in changed {
                self.stream
                    .send(Request::InnerNodes {
                        parent_hash,
                        inner_layer: inner_layer + 1,
                    })
                    .await
                    .unwrap_or(())
            }
        } else {
            for parent_hash in changed {
                self.stream
                    .send(Request::LeafNodes { parent_hash })
                    .await
                    .unwrap_or(())
            }
        }

        Ok(())
    }

    async fn handle_leaf_nodes(&mut self, parent_hash: Hash, nodes: LeafNodeSet) -> Result<()> {
        if parent_hash != nodes.hash() {
            log::warn!("leaf nodes parent hash mismatch");
            return Ok(());
        }

        let mut tx = self.index.pool.begin().await?;
        for node in nodes {
            node.save(&mut tx, &parent_hash).await?;
        }
        index::detect_complete_snapshots(&mut tx, parent_hash, INNER_LAYER_COUNT).await?;
        tx.commit().await?;

        Ok(())
    }

    async fn is_complete(&self) -> Result<bool> {
        let mut tx = self.index.pool.begin().await?;
        Ok(RootNode::load_latest(&mut tx, &self.their_replica_id)
            .await?
            .map(|node| node.is_complete)
            .unwrap_or(false))
    }
}
