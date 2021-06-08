use super::{
    message::{Request, Response},
    message_broker::ClientStream,
};
use crate::{
    error::Result,
    index::{Index, RootNode},
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
                let mut tx = self.index.pool.begin().await?;
                let (node, changed) =
                    RootNode::create(&mut tx, &self.their_replica_id, hash).await?;
                tx.commit().await?;

                if changed {
                    let _ = self.stream.send(Request::ChildNodes(node.hash)).await;
                }
            }
            Response::InnerNodes { parent_hash, nodes } => {
                let mut tx = self.index.pool.begin().await?;

                for (index, node) in nodes.into_iter() {
                    if node.save(&mut tx, &parent_hash, index).await? {
                        let _ = self.stream.send(Request::ChildNodes(node.hash)).await;
                    }
                }

                tx.commit().await?;
            }
            Response::LeafNodes { .. } => todo!(),
        }

        Ok(())
    }
}
