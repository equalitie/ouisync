use super::{
    message::{Request, Response},
    message_broker::ClientStream,
};
use crate::{
    block::BlockNonce,
    error::{Error, Result},
    index::{Index, InnerNodeMap, LeafNodeSet, ReceiveError, Summary, UntrustedProof},
    store,
};

pub(crate) struct Client {
    index: Index,
    stream: ClientStream,
}

impl Client {
    pub fn new(index: Index, stream: ClientStream) -> Self {
        Self { index, stream }
    }

    pub async fn run(&mut self) -> Result<()> {
        while let Some(response) = self.stream.recv().await {
            match self.handle_response(response).await {
                Ok(()) => {}
                Err(error @ (ReceiveError::InvalidProof | ReceiveError::ParentNodeNotFound)) => {
                    log::warn!("failed to handle response: {}", error)
                }
                Err(ReceiveError::Fatal(error)) => return Err(error),
            }
        }

        Ok(())
    }

    async fn handle_response(&mut self, response: Response) -> Result<(), ReceiveError> {
        match response {
            Response::RootNode { proof, summary } => self.handle_root_node(proof, summary).await?,
            Response::InnerNodes(nodes) => self.handle_inner_nodes(nodes).await?,
            Response::LeafNodes(nodes) => self.handle_leaf_nodes(nodes).await?,
            Response::Block { content, nonce } => self.handle_block(content, nonce).await?,
        }

        Ok(())
    }

    async fn handle_root_node(
        &mut self,
        proof: UntrustedProof,
        summary: Summary,
    ) -> Result<(), ReceiveError> {
        let hash = proof.hash;
        let updated = self.index.receive_root_node(proof, summary).await?;

        if updated {
            self.stream.send(Request::ChildNodes(hash)).await;
        }

        Ok(())
    }

    async fn handle_inner_nodes(&self, nodes: InnerNodeMap) -> Result<(), ReceiveError> {
        let updated = self.index.receive_inner_nodes(nodes).await?;

        for hash in updated {
            self.stream.send(Request::ChildNodes(hash)).await;
        }

        Ok(())
    }

    async fn handle_leaf_nodes(&self, nodes: LeafNodeSet) -> Result<(), ReceiveError> {
        let updated = self.index.receive_leaf_nodes(nodes).await?;

        for block_id in updated {
            // TODO: avoid multiple clients downloading the same block
            self.stream.send(Request::Block(block_id)).await;
        }

        Ok(())
    }

    async fn handle_block(&self, content: Box<[u8]>, nonce: BlockNonce) -> Result<()> {
        match store::write_received_block(&self.index, &content, &nonce).await {
            Ok(_) => Ok(()),
            // Ignore `BlockNotReferenced` errors as they only mean that the block is no longer
            // needed.
            Err(Error::BlockNotReferenced) => Ok(()),
            Err(e) => Err(e),
        }
    }
}
