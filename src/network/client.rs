use super::{
    message::{Request, Response},
    message_broker::ClientStream,
};
use crate::{
    error::Result,
    index::{Index, InnerNode, NewRootNode},
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

            // TODO: if snapshot complete, return true
        }

        Ok(false)
    }

    async fn handle_response(&mut self, response: Response) -> Result<()> {
        match response {
            Response::RootNode(hash) => {
                let node = NewRootNode {
                    replica_id: self.their_replica_id,
                    hash,
                };

                let mut tx = self.index.pool.begin().await?;
                let (node, changed) = node.insert(&mut tx).await?;
                tx.commit().await?;

                if changed {
                    let _ = self.stream.send(Request::InnerNodes(node.hash)).await;
                }
            }
            Response::InnerNodes {
                parent_hash,
                children,
            } => {
                let mut tx = self.index.pool.begin().await?;

                for (index, hash) in children.into_iter().enumerate() {
                    InnerNode { hash }
                        .insert(index, &parent_hash, &mut tx)
                        .await?;
                    // TODO: if the node is different from what we had, request the child nodes:
                    // let _ = self.stream.send(Request::InnerNodes(node.data.hash)).await;
                }

                tx.commit().await?;
            }
        }

        Ok(())
    }
}
