use super::{
    message::{Request, Response},
    message_broker::ClientStream,
};
use crate::{
    block::BlockId,
    crypto::{AuthTag, Hash, Hashable},
    error::Result,
    index::{Index, InnerNodeMap, LeafNodeSet, RootNode, Summary, INNER_LAYER_COUNT},
    replica_id::ReplicaId,
    store,
    version_vector::VersionVector,
};

pub struct Client {
    index: Index,
    their_replica_id: ReplicaId,
    stream: ClientStream,
    // "Cookie" number of the last received `RootNode` response or zero if we haven't received one
    // yet. To be included in the next sent `RootNode` request. The server uses this to decide
    // whether the client is up to date.
    cookie: u64,
}

impl Client {
    pub fn new(index: Index, their_replica_id: ReplicaId, stream: ClientStream) -> Self {
        Self {
            index,
            their_replica_id,
            stream,
            cookie: 0,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        while self.pull_snapshot().await? {}
        Ok(())
    }

    async fn pull_snapshot(&mut self) -> Result<bool> {
        self.stream
            .send(Request::RootNode {
                cookie: self.cookie,
            })
            .await
            .unwrap_or(());

        while let Some(response) = self.stream.recv().await {
            // Check competion only if the response affects the index (that is, it is not `Block`)
            // to avoid sending unnecessary duplicate `RootNode` requests.
            let check_complete = !matches!(response, Response::Block { .. });

            self.handle_response(response).await?;

            if check_complete && self.is_complete().await? {
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn handle_response(&mut self, response: Response) -> Result<()> {
        match response {
            Response::RootNode {
                cookie,
                versions,
                hash,
                summary,
            } => self.handle_root_node(cookie, versions, hash, summary).await,
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
        cookie: u64,
        versions: VersionVector,
        hash: Hash,
        summary: Summary,
    ) -> Result<()> {
        self.cookie = cookie;

        if self
            .index
            .receive_root_node(&self.their_replica_id, versions, hash, summary)
            .await?
        {
            self.stream
                .send(Request::InnerNodes {
                    parent_hash: hash,
                    inner_layer: 0,
                })
                .await
                .unwrap_or(());
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

        for hash in self
            .index
            .receive_inner_nodes(parent_hash, inner_layer, nodes)
            .await?
        {
            self.stream
                .send(child_request(hash, inner_layer))
                .await
                .unwrap_or(())
        }

        Ok(())
    }

    async fn handle_leaf_nodes(&self, parent_hash: Hash, nodes: LeafNodeSet) -> Result<()> {
        if parent_hash != nodes.hash() {
            log::warn!("leaf nodes parent hash mismatch");
            return Ok(());
        }

        for block_id in self.index.receive_leaf_nodes(parent_hash, nodes).await? {
            // TODO: avoid multiple clients downloading the same block
            self.stream
                .send(Request::Block(block_id))
                .await
                .unwrap_or(());
        }

        Ok(())
    }

    async fn handle_block(&self, id: BlockId, content: Box<[u8]>, auth_tag: AuthTag) -> Result<()> {
        // TODO: how to validate the block?
        store::write_received_block(&self.index, &id, &content, &auth_tag).await
    }

    async fn is_complete(&self) -> Result<bool> {
        Ok(
            RootNode::load_latest(&self.index.pool, &self.their_replica_id)
                .await?
                .map(|node| node.summary.is_complete())
                .unwrap_or(false),
        )
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
