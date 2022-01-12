use super::{
    message::{Request, Response},
    message_broker::ClientStream,
};
use crate::{
    block::BlockId,
    crypto::{cipher::AuthTag, sign::PublicKey, Hash, Hashable},
    error::{Error, Result},
    index::{Index, InnerNodeMap, LeafNodeSet, Summary, INNER_LAYER_COUNT},
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
                writer_id,
                version_vector,
                hash,
                summary,
            } => {
                self.handle_root_node(writer_id, version_vector, hash, summary)
                    .await
            }
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
            Response::Block {
                id,
                content,
                auth_tag,
            } => self.handle_block(id, content, auth_tag).await,
        }
    }

    async fn handle_root_node(
        &mut self,
        writer_id: PublicKey,
        version_vector: VersionVector,
        hash: Hash,
        summary: Summary,
    ) -> Result<()> {
        let updated = self
            .index
            .receive_root_node(writer_id, version_vector, hash, summary)
            .await?;

        if updated {
            self.stream
                .send(Request::InnerNodes {
                    parent_hash: hash,
                    inner_layer: 0,
                })
                .await;
        }

        Ok(())
    }

    async fn handle_inner_nodes(
        &self,
        parent_hash: Hash,
        inner_layer: usize,
        nodes: InnerNodeMap,
    ) -> Result<()> {
        if parent_hash != nodes.hash() {
            log::warn!("inner nodes parent hash mismatch");
            return Ok(());
        }

        let updated = self
            .index
            .receive_inner_nodes(parent_hash, inner_layer, nodes)
            .await?;

        for hash in updated {
            self.stream.send(child_request(hash, inner_layer)).await;
        }

        Ok(())
    }

    async fn handle_leaf_nodes(&self, parent_hash: Hash, nodes: LeafNodeSet) -> Result<()> {
        if parent_hash != nodes.hash() {
            log::warn!("leaf nodes parent hash mismatch");
            return Ok(());
        }

        let updated = self.index.receive_leaf_nodes(parent_hash, nodes).await?;

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

fn child_request(parent_hash: Hash, inner_layer: usize) -> Request {
    if inner_layer < INNER_LAYER_COUNT - 1 {
        Request::InnerNodes {
            parent_hash,
            inner_layer: inner_layer + 1,
        }
    } else {
        Request::LeafNodes { parent_hash }
    }
}
