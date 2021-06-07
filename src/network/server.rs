use super::{
    message::{Request, Response},
    message_broker::ServerStream,
};
use crate::{
    error::Result,
    index::{Index, InnerNode, RootNode},
};
use futures::future;

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
        let request = match self.stream.recv().await {
            Some(request) => request,
            None => return Ok(false),
        };

        match request {
            Request::RootNode => {
                let mut tx = self.index.pool.begin().await?;
                let node =
                    RootNode::load_latest_or_create(&mut tx, &self.index.this_replica_id).await?;
                tx.commit().await?;

                let _ = self.stream.send(Response::RootNode(node.hash)).await;
            }
            Request::InnerNodes(parent_hash) => {
                let mut tx = self.index.pool.begin().await?;
                let nodes = InnerNode::load_children(&mut tx, &parent_hash).await?;

                let _ = self.stream.send(Response::InnerNodes {
                    parent_hash,
                    children: nodes.iter().map(|node| node.hash).collect(),
                });
            }
        }

        // TODO:
        future::pending().await

        // Ok(true)
    }
}
