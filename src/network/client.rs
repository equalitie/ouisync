use super::{
    message::{Request, Response},
    message_broker::ClientStream,
};
use crate::{
    error::Result,
    index::{Index, NewRootNode},
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
        let request = Request::RootNode;
        log::trace!("send {:?}", request);
        let _ = self.stream.send(request).await;

        let response = match self.stream.recv().await {
            Some(response) => {
                log::trace!("recv {:?}", response);
                response
            }
            None => return Ok(false),
        };

        match response {
            Response::RootNode(data) => {
                let node = NewRootNode {
                    replica_id: self.their_replica_id,
                    data,
                };

                let mut tx = self.index.pool.begin().await?;
                let (_node, _changed) = node.insert(&mut tx).await?;

                // TODO: if changed == true, fetch inner nodes

                tx.commit().await?;
            }
        }

        Ok(true)
    }
}
