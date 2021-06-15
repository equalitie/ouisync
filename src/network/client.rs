use super::{
    message::{Request, Response},
    message_broker::ClientStream,
};
use crate::{
    crypto::Hashable,
    error::Result,
    index::{self, Index, RootNode, INNER_LAYER_COUNT},
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
        self.stream.send(Request::RootNode).await.unwrap_or(());

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

        let (parent_hash, layer) = match response {
            Response::RootNode(hash) => {
                let (node, changed) =
                    RootNode::create(&mut tx, &self.their_replica_id, hash).await?;

                if changed {
                    self.stream
                        .send(Request::InnerNodes {
                            parent_hash: node.hash,
                            inner_layer: 0,
                        })
                        .await
                        .unwrap_or(())
                }

                (hash, 0)
            }
            Response::InnerNodes {
                parent_hash,
                inner_layer,
                nodes,
            } => {
                if parent_hash != nodes.hash() {
                    log::warn!("inner nodes parent hash mismatch");
                    return Ok(());
                }

                for (bucket, node) in nodes {
                    if node.save(&mut tx, &parent_hash, bucket).await? {
                        let message = if inner_layer < INNER_LAYER_COUNT - 1 {
                            Request::InnerNodes {
                                parent_hash: node.hash,
                                inner_layer: inner_layer + 1,
                            }
                        } else {
                            Request::LeafNodes {
                                parent_hash: node.hash,
                            }
                        };

                        self.stream.send(message).await.unwrap_or(())
                    }
                }

                (parent_hash, inner_layer)
            }
            Response::LeafNodes { parent_hash, nodes } => {
                if parent_hash != nodes.hash() {
                    log::warn!("leaf nodes parent hash mismatch");
                    return Ok(());
                }

                for node in nodes {
                    node.save(&mut tx, &parent_hash).await?;
                }

                (parent_hash, INNER_LAYER_COUNT)
            }
        };

        index::detect_complete_snapshots(&mut tx, parent_hash, layer).await?;

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
