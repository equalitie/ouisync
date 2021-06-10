use super::{
    message::{Request, Response},
    message_broker::ServerStream,
};
use crate::{
    error::Result,
    index::{Index, InnerNode, LeafNode, RootNode},
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
            Request::RootNode => {
                let mut tx = self.index.pool.begin().await?;
                let node =
                    RootNode::load_latest_or_create(&mut tx, &self.index.this_replica_id).await?;
                tx.commit().await?;

                let _ = self.stream.send(Response::RootNode(node.hash)).await;
            }
            Request::ChildNodes(parent_hash) => {
                let mut tx = self.index.pool.begin().await?;

                let nodes = InnerNode::load_children(&mut tx, &parent_hash).await?;
                if !nodes.is_empty() {
                    let _ = self
                        .stream
                        .send(Response::InnerNodes { parent_hash, nodes });
                    return Ok(());
                }

                let nodes = LeafNode::load_children(&mut tx, &parent_hash).await?;
                if !nodes.is_empty() {
                    let _ = self.stream.send(Response::LeafNodes { parent_hash, nodes });
                }
            }
        }

        Ok(())
    }
}
