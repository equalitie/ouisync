#[cfg(test)]
pub mod test_utils;

mod inner;
mod leaf;
mod root;
mod summary;
#[cfg(test)]
mod tests;

pub(crate) use self::{
    inner::{InnerNode, InnerNodeMap, EMPTY_INNER_HASH, INNER_LAYER_COUNT},
    leaf::{LeafNode, LeafNodeSet, ModifyStatus},
    root::RootNode,
    summary::{SingleBlockPresence, Summary},
};

use self::summary::MultiBlockPresence;
use crate::{
    block::BlockId,
    crypto::{sign::PublicKey, Hash},
    db,
    error::Result,
};
use futures_util::{future, stream, Stream, TryStreamExt};

/// Get the bucket for `locator` at the specified `inner_layer`.
pub(super) fn get_bucket(locator: &Hash, inner_layer: usize) -> u8 {
    locator.as_ref()[inner_layer]
}

/// Update summary of the nodes with the specified hash and all their ancestor nodes.
/// Returns a map `PublicKey -> bool` indicating which branches were affected and whether they
/// became complete.
pub(crate) async fn update_summaries(
    tx: &mut db::Transaction,
    hash: Hash,
) -> Result<Vec<(PublicKey, bool)>> {
    update_summaries_with_stack(tx, vec![hash]).await
}

/// Receive a block from other replica. This marks the block as not missing by the local replica.
/// Returns the replica ids whose branches reference the received block (if any).
pub(crate) async fn receive_block(
    tx: &mut db::Transaction,
    id: &BlockId,
) -> Result<Vec<PublicKey>> {
    if !LeafNode::set_present(tx, id).await? {
        return Ok(Vec::new());
    }

    let nodes = LeafNode::load_parent_hashes(tx, id).try_collect().await?;

    let ids = update_summaries_with_stack(tx, nodes)
        .await?
        .into_iter()
        .map(|(writer_id, _)| writer_id)
        .collect();

    Ok(ids)
}

/// Does a parent node (root or inner) with the given hash exist?
pub(crate) async fn parent_exists(conn: &mut db::Connection, hash: &Hash) -> Result<bool> {
    use sqlx::Row;

    Ok(sqlx::query(
        "SELECT
             EXISTS(SELECT 0 FROM snapshot_root_nodes  WHERE hash = ?) OR
             EXISTS(SELECT 0 FROM snapshot_inner_nodes WHERE hash = ?)",
    )
    .bind(hash)
    .bind(hash)
    .fetch_one(conn)
    .await?
    .get(0))
}

/// All leaf nodes belonging to the given root
pub(crate) fn leaf_nodes<'a>(
    conn: &'a mut db::Connection,
    root: &RootNode,
    block_presence_filter: SingleBlockPresence,
) -> impl Stream<Item = Result<LeafNode>> + 'a {
    // TODO: it might be more efficient to rewrite this to a single query using "Recursive Common
    //       Table Expressions" (https://www.sqlite.org/lang_with.html)

    let stream = LeafNodesStream::new(root.clone(), block_presence_filter);

    stream::try_unfold((conn, stream), |(conn, mut stream)| async {
        stream
            .try_next(conn)
            .await
            .map(|item| item.map(|item| (item, (conn, stream))))
    })
}

struct LeafNodesStream {
    stack: Vec<Node>,
    filter: SingleBlockPresence,
}

impl LeafNodesStream {
    fn new(root: RootNode, filter: SingleBlockPresence) -> Self {
        Self {
            stack: vec![Node::Root(root)],
            filter,
        }
    }

    async fn try_next(&mut self, conn: &mut db::Connection) -> Result<Option<LeafNode>> {
        while let Some(node) = self.stack.pop() {
            match node {
                Node::Root(node) => {
                    if !self.filter_parent(&node.summary.block_presence) {
                        continue;
                    }

                    self.stack.extend(
                        InnerNode::load_children(conn, &node.proof.hash)
                            .await?
                            .into_iter()
                            .map(|(_, node)| node)
                            .map(Node::Inner),
                    );
                }
                Node::Inner(node) => {
                    if !self.filter_parent(&node.summary.block_presence) {
                        continue;
                    }

                    self.stack.extend(
                        InnerNode::load_children(conn, &node.hash)
                            .await?
                            .into_iter()
                            .map(|(_, node)| node)
                            .map(Node::Inner),
                    );

                    self.stack.extend(
                        LeafNode::load_children(conn, &node.hash)
                            .await?
                            .into_iter()
                            .map(Node::Leaf),
                    );
                }
                Node::Leaf(node) => {
                    if node.block_presence != self.filter {
                        continue;
                    }

                    return Ok(Some(node));
                }
            }
        }

        Ok(None)
    }

    fn filter_parent(&self, block_presence: &MultiBlockPresence) -> bool {
        !matches!(
            (block_presence, self.filter),
            (MultiBlockPresence::Full, SingleBlockPresence::Missing)
                | (MultiBlockPresence::None, SingleBlockPresence::Present)
        )
    }
}

enum Node {
    Root(RootNode),
    Inner(InnerNode),
    Leaf(LeafNode),
}

enum ParentNodeKind {
    Root,
    Inner,
}

async fn parent_kind(conn: &mut db::Connection, hash: &Hash) -> Result<Option<ParentNodeKind>> {
    use sqlx::Row;

    let kind: u8 = sqlx::query(
        "SELECT CASE
             WHEN EXISTS(SELECT 0 FROM snapshot_root_nodes  WHERE hash = ?) THEN 1
             WHEN EXISTS(SELECT 0 FROM snapshot_inner_nodes WHERE hash = ?) THEN 2
             ELSE 0
         END",
    )
    .bind(hash)
    .bind(hash)
    .fetch_one(conn)
    .await?
    .get(0);

    match kind {
        0 => Ok(None),
        1 => Ok(Some(ParentNodeKind::Root)),
        2 => Ok(Some(ParentNodeKind::Inner)),
        _ => unreachable!(),
    }
}

async fn update_summaries_with_stack(
    tx: &mut db::Transaction,
    mut nodes: Vec<Hash>,
) -> Result<Vec<(PublicKey, bool)>> {
    let mut statuses = Vec::new();

    while let Some(hash) = nodes.pop() {
        match parent_kind(tx, &hash).await? {
            Some(ParentNodeKind::Root) => {
                let complete = RootNode::update_summaries(tx, &hash).await?;
                RootNode::load_writer_ids(tx, &hash)
                    .try_for_each(|writer_id| {
                        statuses.push((writer_id, complete));
                        future::ready(Ok(()))
                    })
                    .await?;
            }
            Some(ParentNodeKind::Inner) => {
                InnerNode::update_summaries(tx, &hash).await?;
                InnerNode::load_parent_hashes(tx, &hash)
                    .try_for_each(|parent_hash| {
                        nodes.push(parent_hash);
                        future::ready(Ok(()))
                    })
                    .await?;
            }
            None => (),
        }
    }

    Ok(statuses)
}
