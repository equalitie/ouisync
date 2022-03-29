use super::message::{Content, Request, Response};
use crate::{
    block::{BlockData, BlockNonce},
    crypto::{CacheHash, Hashable},
    error::{Error, Result},
    index::{Index, InnerNodeMap, LeafNodeSet, ReceiveError, Summary, UntrustedProof},
    store,
};
use std::time::Duration;
use tokio::{
    select,
    sync::mpsc,
    time::{self, MissedTickBehavior},
};

const REPORT_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) struct Client {
    index: Index,
    tx: Sender,
    rx: Receiver,
    report: bool,
}

impl Client {
    pub fn new(index: Index, tx: mpsc::Sender<Content>, rx: mpsc::Receiver<Response>) -> Self {
        Self {
            index,
            tx: Sender(tx),
            rx,
            report: true,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut report_interval = time::interval(REPORT_INTERVAL);
        report_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            select! {
                Some(response) = self.rx.recv() => {
                    match self.handle_response(response).await {
                        Ok(()) => {}
                        Err(
                            error @ (ReceiveError::InvalidProof | ReceiveError::ParentNodeNotFound),
                        ) => {
                            log::warn!("failed to handle response: {}", error)
                        }
                        Err(ReceiveError::Fatal(error)) => return Err(error),
                    }
                }
                _ = report_interval.tick() => self.report().await?,
                else => break,
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
            Response::ChildNodesError(_) | Response::BlockError(_) => (),
        }

        Ok(())
    }

    async fn handle_root_node(
        &mut self,
        proof: UntrustedProof,
        summary: Summary,
    ) -> Result<(), ReceiveError> {
        log::trace!(
            "handle_root_node(hash: {:?}, vv: {:?}, missing_blocks: {})",
            proof.hash,
            proof.version_vector,
            summary.missing_blocks_count()
        );

        let hash = proof.hash;
        let updated = self.index.receive_root_node(proof, summary).await?;

        if updated {
            self.tx.send(Request::ChildNodes(hash)).await;
        }

        Ok(())
    }

    async fn handle_inner_nodes(&self, nodes: InnerNodeMap) -> Result<(), ReceiveError> {
        let nodes = CacheHash::from(nodes);
        log::trace!("handle_inner_nodes({:?})", nodes.hash());

        let updated = self.index.receive_inner_nodes(nodes).await?;

        for hash in updated {
            self.tx.send(Request::ChildNodes(hash)).await;
        }

        Ok(())
    }

    async fn handle_leaf_nodes(&mut self, nodes: LeafNodeSet) -> Result<(), ReceiveError> {
        let nodes = CacheHash::from(nodes);
        log::trace!("handle_leaf_nodes({:?})", nodes.hash());

        let updated = self.index.receive_leaf_nodes(nodes).await?;

        if !updated.is_empty() {
            self.report = true;
        }

        for block_id in updated {
            // TODO: avoid multiple clients downloading the same block
            self.tx.send(Request::Block(block_id)).await;
        }

        Ok(())
    }

    async fn handle_block(&mut self, content: Box<[u8]>, nonce: BlockNonce) -> Result<()> {
        let data = BlockData::from(content);
        log::trace!("handle_block({:?})", data.id);

        match store::write_received_block(&self.index, &data, &nonce).await {
            Ok(_) => {
                self.report = true;
                Ok(())
            }
            // Ignore `BlockNotReferenced` errors as they only mean that the block is no longer
            // needed.
            Err(Error::BlockNotReferenced) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn report(&mut self) -> Result<()> {
        if !self.report {
            return Ok(());
        }

        log::debug!(
            "missing blocks: {}",
            self.index.count_missing_blocks().await
        );
        self.report = false;

        Ok(())
    }
}

struct Sender(mpsc::Sender<Content>);

impl Sender {
    async fn send(&self, request: Request) -> bool {
        self.0.send(Content::Request(request)).await.is_ok()
    }
}

type Receiver = mpsc::Receiver<Response>;
