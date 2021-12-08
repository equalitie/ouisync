use super::{
    message::{Request, Response},
    message_broker::ClientStream,
};
use crate::{
    block::BlockId,
    crypto::{sign::PublicKey, AuthTag, Hash, Hashable},
    error::{Error, Result},
    index::{Index, InnerNodeMap, LeafNodeSet, Summary, INNER_LAYER_COUNT},
    store,
    version_vector::VersionVector,
};
use tokio::{pin, select};

pub(crate) struct Client {
    index: Index,
    stream: ClientStream,
    // "Cookie" number of the last received `RootNode` response or zero if we haven't received one
    // yet. To be included in the next sent `RootNode` request. The server uses this to decide
    // whether the client is up to date.
    cookie: u64,
}

impl Client {
    pub fn new(index: Index, stream: ClientStream) -> Self {
        Self {
            index,
            stream,
            cookie: 0,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let index_closed = self.index.subscribe().closed();
        pin!(index_closed);

        loop {
            self.stream
                .send(Request::RootNode {
                    cookie: self.cookie,
                })
                .await;

            loop {
                // TODO: add a timeout here and send the request again if it expires before we
                // receive a response.
                select! {
                    response = self.stream.recv() => {
                        if let Some(response) = response {
                            if self.handle_response(response).await? {
                                break;
                            }
                        } else {
                            return Ok(());
                        }
                    }
                    _ = &mut index_closed => return Ok(()),
                }
            }
        }
    }

    async fn handle_response(&mut self, response: Response) -> Result<bool> {
        match response {
            Response::RootNode {
                cookie,
                replica_id,
                versions,
                hash,
                summary,
            } => {
                self.handle_root_node(cookie, replica_id, versions, hash, summary)
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
            } => {
                self.handle_block(id, content, auth_tag).await?;
                Ok(false)
            }
        }
    }

    async fn handle_root_node(
        &mut self,
        cookie: u64,
        replica_id: PublicKey,
        versions: VersionVector,
        hash: Hash,
        summary: Summary,
    ) -> Result<bool> {
        self.cookie = cookie;

        let status = self
            .index
            .receive_root_node(&replica_id, versions, hash, summary)
            .await?;

        if status.updated {
            self.stream
                .send(Request::InnerNodes {
                    parent_hash: hash,
                    inner_layer: 0,
                })
                .await;
        }

        Ok(status.complete)
    }

    async fn handle_inner_nodes(
        &self,
        parent_hash: Hash,
        inner_layer: usize,
        nodes: InnerNodeMap,
    ) -> Result<bool> {
        if parent_hash != nodes.hash() {
            log::warn!("inner nodes parent hash mismatch");
            return Ok(true);
        }

        let status = self
            .index
            .receive_inner_nodes(parent_hash, inner_layer, nodes)
            .await?;

        for hash in status.updated {
            self.stream.send(child_request(hash, inner_layer)).await;
        }

        Ok(status.complete)
    }

    async fn handle_leaf_nodes(&self, parent_hash: Hash, nodes: LeafNodeSet) -> Result<bool> {
        if parent_hash != nodes.hash() {
            log::warn!("leaf nodes parent hash mismatch");
            return Ok(true);
        }

        let status = self.index.receive_leaf_nodes(parent_hash, nodes).await?;

        for block_id in status.updated {
            // TODO: avoid multiple clients downloading the same block
            self.stream.send(Request::Block(block_id)).await;
        }

        Ok(status.complete)
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
