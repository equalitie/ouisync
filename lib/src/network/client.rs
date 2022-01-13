use super::{
    message::{Request, Response},
    message_broker::ClientStream,
};
use crate::{
    block::BlockId,
    crypto::cipher::AuthTag,
    error::{Error, Result},
    index::{Index, InnerNodeMap, LeafNodeSet, Summary, UntrustedProof},
    store,
    version_vector::VersionVector,
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
            self.handle_response(response).await?;
        }

        Ok(())
    }

    async fn handle_response(&mut self, response: Response) -> Result<()> {
        match response {
            Response::RootNode {
                proof,
                version_vector,
                summary,
            } => self.handle_root_node(proof, version_vector, summary).await,
            Response::InnerNodes(nodes) => self.handle_inner_nodes(nodes).await,
            Response::LeafNodes(nodes) => self.handle_leaf_nodes(nodes).await,
            Response::Block {
                id,
                content,
                auth_tag,
            } => self.handle_block(id, content, auth_tag).await,
        }
    }

    async fn handle_root_node(
        &mut self,
        proof: UntrustedProof,
        version_vector: VersionVector,
        summary: Summary,
    ) -> Result<()> {
        let proof = if let Ok(proof) = proof.verify(self.index.repository_id()) {
            proof
        } else {
            log::warn!("root node proof verification failed");
            return Ok(());
        };

        let parent_hash = proof.hash;
        let updated = self
            .index
            .receive_root_node(proof, version_vector, summary)
            .await?;

        if updated {
            self.stream.send(Request::ChildNodes(parent_hash)).await;
        }

        Ok(())
    }

    async fn handle_inner_nodes(&self, nodes: InnerNodeMap) -> Result<()> {
        // TODO: require parent exists

        let updated = self.index.receive_inner_nodes(nodes).await?;

        for hash in updated {
            self.stream.send(Request::ChildNodes(hash)).await;
        }

        Ok(())
    }

    async fn handle_leaf_nodes(&self, nodes: LeafNodeSet) -> Result<()> {
        // TODO: require parent exists

        let updated = self.index.receive_leaf_nodes(nodes).await?;

        for block_id in updated {
            // TODO: avoid multiple clients downloading the same block
            self.stream.send(Request::Block(block_id)).await;
        }

        Ok(())
    }

    async fn handle_block(&self, id: BlockId, content: Box<[u8]>, auth_tag: AuthTag) -> Result<()> {
        // TODO: how to validate the block?
        match store::write_received_block(&self.index, &id, &content, &auth_tag).await {
            Ok(_) => Ok(()),
            // Ignore `BlockNotReferenced` errors as they only mean that the block is no longer
            // needed.
            Err(Error::BlockNotReferenced) => Ok(()),
            Err(e) => Err(e),
        }
    }
}
