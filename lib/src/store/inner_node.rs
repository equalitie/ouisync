use super::{error::Error, leaf_node};
use crate::{
    crypto::Hash,
    db::{self, MAX_BIND_PARAMS},
    future::TryStreamExt as _,
    protocol::{EMPTY_INNER_HASH, EMPTY_LEAF_HASH, InnerNode, InnerNodes, LeafNodes, Summary},
};
use futures_util::{Stream, TryStreamExt, future};
use sqlx::{QueryBuilder, Row};
use std::{
    collections::{HashMap, HashSet},
    convert::TryInto,
};

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

/// Load all nodes with the given hashes.
pub(super) async fn load(
    conn: &mut db::Connection,
    hashes: &[Hash],
) -> Result<HashMap<Hash, InnerNode>, Error> {
    if hashes.is_empty() {
        return Ok(HashMap::new());
    }

    let mut output = HashMap::new();

    for hashes_chunk in hashes.chunks(MAX_BIND_PARAMS) {
        let mut builder = QueryBuilder::new(
            "SELECT hash, state, block_presence
             FROM snapshot_inner_nodes
             WHERE hash IN (",
        );
        let mut separated = builder.separated(", ");
        for hash in hashes_chunk {
            separated.push_bind(hash);
        }
        builder.push(")");
        builder
            .build()
            .persistent(false)
            .fetch(&mut *conn)
            .map_ok(|row| InnerNode {
                hash: row.get(0),
                summary: Summary {
                    state: row.get(1),
                    block_presence: row.get(2),
                },
            })
            .map_ok(|node| (node.hash, node))
            .try_collect_into(&mut output)
            .await?;
    }

    Ok(output)
}

/// Save multiple nodes to the store. Takes slice of (parent_hash, bucket, node) and returns a set
/// of parent node hashes whose children were updated.
pub(super) async fn save(
    tx: &mut db::WriteTransaction,
    nodes: &[(Hash, u8, InnerNode)],
) -> Result<HashSet<Hash>, Error> {
    if nodes.is_empty() {
        return Ok(HashSet::new());
    }

    let mut output = HashSet::new();

    // `MAX_BIND_PARAMS / 5` because we bind 5 parameters per record
    for nodes_chunk in nodes.chunks(MAX_BIND_PARAMS / 5) {
        QueryBuilder::new(
            "INSERT INTO snapshot_inner_nodes (
                parent,
                bucket,
                hash,
                state,
                block_presence
             ) ",
        )
        .push_values(nodes_chunk, |mut builder, (parent_hash, bucket, node)| {
            builder
                .push_bind(parent_hash)
                .push_bind(bucket)
                .push_bind(&node.hash)
                .push_bind(node.summary.state)
                .push_bind(&node.summary.block_presence);
        })
        .push(" ON CONFLICT (parent, bucket) DO NOTHING RETURNING parent")
        .build()
        .persistent(false)
        .fetch(&mut *tx)
        .map_ok(|row| row.get::<Hash, _>(0))
        .try_collect_into(&mut output)
        .await?;
    }

    Ok(output)
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
pub(super) fn update_summaries<'a>(
    tx: &'a mut db::WriteTransaction,
    hash: &'a Hash,
    summary: &'a Summary,
) -> impl Stream<Item = Result<Hash, Error>> + 'a {
    sqlx::query(
        "UPDATE snapshot_inner_nodes
         SET state = ?, block_presence = ?
         WHERE hash = ?
         RETURNING parent",
    )
    .bind(summary.state)
    .bind(&summary.block_presence)
    .bind(hash)
    .fetch(tx)
    .map_ok(|row| row.get(0))
    .err_into()
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
        save(&mut tx, &[(parent, bucket, node)]).await.unwrap();

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
        save(&mut tx, &[(parent, bucket, node0)]).await.unwrap();

        let node1 = InnerNode::new(hash, Summary::INCOMPLETE);
        save(&mut tx, &[(parent, bucket, node1)]).await.unwrap();

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
        save(&mut tx, &[(parent, bucket, node0)]).await.unwrap();

        let node1 = InnerNode::new(hash1, Summary::INCOMPLETE);
        assert_matches!(save(&mut tx, &[(parent, bucket, node1)]).await, Err(_)); // TODO: match concrete error type
    }

    async fn setup() -> (TempDir, db::Pool) {
        db::create_temp().await.unwrap()
    }
}
