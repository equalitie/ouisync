use super::{error::Error, leaf_node};
use crate::{
    crypto::Hash,
    db,
    protocol::{InnerNode, InnerNodes, LeafNodes, Summary, EMPTY_INNER_HASH, EMPTY_LEAF_HASH},
};
use futures_util::{future, TryStreamExt};
use sqlx::Row;
use std::convert::TryInto;

#[derive(Default)]
pub(crate) struct ReceiveStatus {
    /// Which of the received nodes should we request the children of.
    pub request_children: Vec<InnerNode>,
}

/// Load all inner nodes with the specified parent hash.
pub(super) async fn load_children(
    conn: &mut db::Connection,
    parent: &Hash,
) -> Result<InnerNodes, Error> {
    sqlx::query(
        "SELECT
             bucket,
             hash,
             state,
             block_presence
         FROM snapshot_inner_nodes
         WHERE parent = ?",
    )
    .bind(parent)
    .fetch(conn)
    .map_ok(|row| {
        let bucket: u32 = row.get(0);
        let node = InnerNode {
            hash: row.get(1),
            summary: Summary {
                state: row.get(2),
                block_presence: row.get(3),
            },
        };

        (bucket, node)
    })
    .try_filter_map(|(bucket, node)| {
        if let Ok(bucket) = bucket.try_into() {
            future::ready(Ok(Some((bucket, node))))
        } else {
            tracing::error!("bucket out of range: {:?}", bucket);
            future::ready(Ok(None))
        }
    })
    .try_collect()
    .await
    .map_err(From::from)
}

pub(super) async fn load(
    conn: &mut db::Connection,
    hash: &Hash,
) -> Result<Option<InnerNode>, Error> {
    let node = sqlx::query(
        "SELECT state, block_presence
         FROM snapshot_inner_nodes
         WHERE hash = ?",
    )
    .bind(hash)
    .fetch_optional(conn)
    .await?
    .map(|row| InnerNode {
        hash: *hash,
        summary: Summary {
            state: row.get(0),
            block_presence: row.get(1),
        },
    });

    Ok(node)
}

/// Saves this inner node into the db unless it already exists.
pub(super) async fn save(
    tx: &mut db::WriteTransaction,
    node: &InnerNode,
    parent: &Hash,
    bucket: u8,
) -> Result<(), Error> {
    debug_assert_ne!(node.hash, *EMPTY_INNER_HASH);
    debug_assert_ne!(node.hash, *EMPTY_LEAF_HASH);

    sqlx::query(
        "INSERT INTO snapshot_inner_nodes (
             parent,
             bucket,
             hash,
             state,
             block_presence
         )
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (parent, bucket) DO NOTHING",
    )
    .bind(parent)
    .bind(bucket)
    .bind(&node.hash)
    .bind(node.summary.state)
    .bind(&node.summary.block_presence)
    .execute(tx)
    .await?;

    Ok(())
}

/// Atomically saves all nodes in this map to the db.
pub(super) async fn save_all(
    tx: &mut db::WriteTransaction,
    nodes: &InnerNodes,
    parent: &Hash,
) -> Result<(), Error> {
    for (bucket, node) in nodes {
        save(tx, node, parent, bucket).await?;
    }

    Ok(())
}

/// Compute summaries from the children nodes of the specified parent nodes.
pub(super) async fn compute_summary(
    conn: &mut db::Connection,
    parent_hash: &Hash,
) -> Result<Summary, Error> {
    // 1st attempt: empty inner nodes
    if parent_hash == &*EMPTY_INNER_HASH {
        let children = InnerNodes::default();
        return Ok(Summary::from_inners(&children));
    }

    // 2nd attempt: empty leaf nodes
    if parent_hash == &*EMPTY_LEAF_HASH {
        let children = LeafNodes::default();
        return Ok(Summary::from_leaves(&children));
    }

    // 3rd attempt: non-empty inner nodes
    let children = load_children(conn, parent_hash).await?;
    if !children.is_empty() {
        // We download all children nodes of a given parent together so when we know that
        // we have at least one we also know we have them all.
        return Ok(Summary::from_inners(&children));
    }

    // 4th attempt: non-empty leaf nodes
    let children = leaf_node::load_children(conn, parent_hash).await?;
    if !children.is_empty() {
        // Similarly as in the inner nodes case, we only need to check that we have at
        // least one leaf node child and that already tells us that we have them all.
        return Ok(Summary::from_leaves(&children));
    }

    // The parent hash doesn't correspond to any known node
    Ok(Summary::INCOMPLETE)
}

/// Updates summaries of all nodes with the specified hash at the specified inner layer.
pub(super) async fn update_summaries(
    tx: &mut db::WriteTransaction,
    hash: &Hash,
    summary: Summary,
) -> Result<Vec<(Hash, u8)>, Error> {
    sqlx::query(
        "UPDATE snapshot_inner_nodes
         SET state = ?, block_presence = ?
         WHERE hash = ?
         RETURNING parent, bucket",
    )
    .bind(summary.state)
    .bind(&summary.block_presence)
    .bind(hash)
    .fetch(tx)
    .map_ok(|row| (row.get(0), row.get(1)))
    .err_into()
    .try_collect()
    .await
}

pub(super) async fn inherit_summaries(
    conn: &mut db::Connection,
    nodes: &mut InnerNodes,
) -> Result<(), Error> {
    for (_, node) in nodes {
        inherit_summary(conn, node).await?;
    }

    Ok(())
}

/// If the summary of this node is `INCOMPLETE` and there exists another node with the same
/// hash as this one, copy the summary of that node into this node.
///
/// Note this is hack/workaround due to the database schema currently not being fully
/// normalized. That is, when there is a node that has more than one parent, we actually
/// represent it as multiple records, each with distinct parent_hash. The summaries of those
/// records need to be identical (because they conceptually represent a single node) so we need
/// this function to copy them manually.
/// Ideally, we should change the db schema to be normalized, which in this case would mean
/// to have only one record per node and to represent the parent-child relation using a
/// separate db table (many-to-many relation).
async fn inherit_summary(conn: &mut db::Connection, node: &mut InnerNode) -> Result<(), Error> {
    if node.summary != Summary::INCOMPLETE {
        return Ok(());
    }

    let summary = sqlx::query(
        "SELECT state, block_presence
             FROM snapshot_inner_nodes
             WHERE hash = ?",
    )
    .bind(&node.hash)
    .fetch_optional(conn)
    .await?
    .map(|row| Summary {
        state: row.get(0),
        block_presence: row.get(1),
    });

    if let Some(summary) = summary {
        node.summary = summary;
    }

    Ok(())
}

/// Filter nodes that the remote replica has some blocks in that the local one is missing.
pub(super) async fn filter_nodes_with_new_blocks(
    tx: &mut db::WriteTransaction,
    remote_nodes: &InnerNodes,
) -> Result<Vec<InnerNode>, Error> {
    let mut output = Vec::with_capacity(remote_nodes.len());

    for (_, remote_node) in remote_nodes {
        let local_node = load(tx, &remote_node.hash).await?;
        let insert = if let Some(local_node) = local_node {
            local_node.summary.is_outdated(&remote_node.summary)
        } else {
            // node not present locally - we implicitly treat this as if the local replica
            // had zero blocks under this node unless the remote node is empty, in that
            // case we ignore it.
            !remote_node.is_empty()
        };

        if insert {
            output.push(*remote_node);
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Hashable;
    use assert_matches::assert_matches;
    use tempfile::TempDir;

    #[test]
    fn empty_map_hash() {
        assert_eq!(*EMPTY_INNER_HASH, InnerNodes::default().hash())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_new_inner_node() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let hash = rand::random();
        let bucket = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node = InnerNode::new(hash, Summary::INCOMPLETE);
        save(&mut tx, &node, &parent, bucket).await.unwrap();

        let nodes = load_children(&mut tx, &parent).await.unwrap();

        assert_eq!(nodes.get(bucket), Some(&node));

        assert!((0..bucket).all(|b| nodes.get(b).is_none()));

        if bucket < u8::MAX {
            assert!((bucket + 1..=u8::MAX).all(|b| nodes.get(b).is_none()));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_existing_inner_node() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let hash = rand::random();
        let bucket = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node0 = InnerNode::new(hash, Summary::INCOMPLETE);
        save(&mut tx, &node0, &parent, bucket).await.unwrap();

        let node1 = InnerNode::new(hash, Summary::INCOMPLETE);
        save(&mut tx, &node1, &parent, bucket).await.unwrap();

        let nodes = load_children(&mut tx, &parent).await.unwrap();

        assert_eq!(nodes.get(bucket), Some(&node0));
        assert!((0..bucket).all(|b| nodes.get(b).is_none()));

        if bucket < u8::MAX {
            assert!((bucket + 1..=u8::MAX).all(|b| nodes.get(b).is_none()));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn attempt_to_create_conflicting_node() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let bucket = rand::random();

        let hash0 = rand::random();
        let hash1 = loop {
            let hash = rand::random();
            if hash != hash0 {
                break hash;
            }
        };

        let mut tx = pool.begin_write().await.unwrap();

        let node0 = InnerNode::new(hash0, Summary::INCOMPLETE);
        save(&mut tx, &node0, &parent, bucket).await.unwrap();

        let node1 = InnerNode::new(hash1, Summary::INCOMPLETE);
        assert_matches!(save(&mut tx, &node1, &parent, bucket).await, Err(_)); // TODO: match concrete error type
    }

    async fn setup() -> (TempDir, db::Pool) {
        db::create_temp().await.unwrap()
    }
}
