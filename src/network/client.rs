use super::{
    message::{Request, Response},
    message_broker::ClientStream,
};
use crate::{
    crypto::Hashable,
    error::Result,
    index::{self, Index, RootNode},
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

    pub async fn run(&mut self) {
        loop {
            match self.pull_snapshot().await {
                Ok(true) => {}
                Ok(false) => break, // broker finished
                Err(error) => {
                    log::error!("Client failed: {}", error.verbose());
                    break;
                }
            }
        }
    }

    async fn pull_snapshot(&mut self) -> Result<bool> {
        let _ = self.stream.send(Request::RootNode).await;

        while let Some(response) = self.stream.recv().await {
            self.handle_response(response).await?;

            if self.is_complete().await? {
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn handle_response(&mut self, response: Response) -> Result<()> {
        let mut tx = self.index.pool.begin().await?;

        // TODO: have the `InnerNodes` response contain the layer number to simplify things

        let parent_hash = match response {
            Response::RootNode(hash) => {
                let (node, changed) =
                    RootNode::create(&mut tx, &self.their_replica_id, hash).await?;

                if changed {
                    let _ = self.stream.send(Request::ChildNodes(node.hash)).await;
                }

                hash
            }
            Response::InnerNodes { parent_hash, nodes } => {
                if parent_hash != nodes.hash() {
                    log::warn!("inner nodes parent hash mismatch");
                    return Ok(());
                }

                for (bucket, node) in nodes {
                    if node.save(&mut tx, &parent_hash, bucket).await? {
                        let _ = self.stream.send(Request::ChildNodes(node.hash)).await;
                    }
                }

                parent_hash
            }
            Response::LeafNodes { parent_hash, nodes } => {
                if parent_hash != nodes.hash() {
                    log::warn!("leaf nodes parent hash mismatch");
                    return Ok(());
                }

                for node in nodes {
                    node.save(&mut tx, &parent_hash).await?;
                }

                parent_hash
            }
        };

        index::detect_complete_snapshots(&mut tx, parent_hash).await?;

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
