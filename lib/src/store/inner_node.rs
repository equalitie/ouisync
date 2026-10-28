use super::{error::Error, leaf_node};
use crate::{
    crypto::Hash,
    db::{self, MAX_BIND_PARAMS},
    future::TryStreamExt as _,
    protocol::{EMPTY_INNER_HASH, EMPTY_LEAF_HASH, InnerNode, InnerNodes, LeafNodes, Summary},
};
use futures_util::{TryStreamExt, future};
use sqlx::{QueryBuilder, Row};
use std::{
    collections::{HashMap, HashSet},
    convert::TryInto,
};

/// Load all inner nodes with the specified parent hashes. Returns map of parent hash -> child
/// nodes.
pub(super) async fn load_children(
    conn: &mut db::Connection,
    parent_hashes: &[Hash],
) -> Result<HashMap<Hash, InnerNodes>, Error> {
    let mut output = HashMap::with_capacity(parent_hashes.len());

    for parent_hashes_chunk in parent_hashes.chunks(MAX_BIND_PARAMS) {
        let mut builder = QueryBuilder::new(
            "SELECT parent, bucket, hash, state, block_presence
             FROM snapshot_inner_nodes
             WHERE parent IN (",
        );
        let mut separated = builder.separated(", ");
        for hash in parent_hashes_chunk {
            separated.push_bind(hash);
        }
        builder.push(")");
        builder
            .build()
            .persistent(false)
            .fetch(&mut *conn)
            .map_ok(|row| {
                (
                    row.get::<Hash, _>(0),
                    row.get::<u32, _>(1),
                    InnerNode {
                        hash: row.get(2),
                        summary: Summary {
                            state: row.get(3),
                            block_presence: row.get(4),
                        },
                    },
                )
            })
            .try_filter_map(|(parent, bucket, node)| {
                if let Ok(bucket) = bucket.try_into() {
                    future::ready(Ok(Some((parent, bucket, node))))
                } else {
                    tracing::error!("bucket out of range: {:?}", bucket);
                    future::ready(Ok(None))
                }
            })
            .try_for_each(|(parent, bucket, node)| {
                output
                    .entry(parent)
                    .or_insert_with(InnerNodes::default)
                    .insert(bucket, node);
                future::ready(Ok(()))
            })
            .await?;
    }

    Ok(output)
}

/// Load all nodes with the given hashes.
pub(super) async fn load(
    conn: &mut db::Connection,
    hashes: &[Hash],
) -> Result<HashMap<Hash, InnerNode>, Error> {
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

/// Compute summaries of the nodes with the specified hashes.
pub(super) async fn compute_summaries(
    conn: &mut db::Connection,
    hashes: &[Hash],
) -> Result<Vec<(Hash, Summary)>, Error> {
    let mut output = Vec::with_capacity(hashes.len());
    let mut remaining = hashes.to_vec();

    // Handle non-empty inner nodes
    let children = load_children(conn, &remaining).await?;
    for (hash, nodes) in &children {
        // We download all children nodes of a given parent together so when we know that
        // we have at least one we also know we have them all.
        if nodes.is_empty() {
            continue;
        }

        output.push((*hash, Summary::from_inners(nodes)));
    }

    remaining.retain(|hash| !children.contains_key(hash));

    // Handle non-empty leaf nodes
    let children = leaf_node::load_children(conn, &remaining).await?;
    for (hash, nodes) in &children {
        // Similarly as in the inner nodes case, we only need to check that we have at
        // least one leaf node child and that already tells us that we have them all.
        if nodes.is_empty() {
            continue;
        }

        output.push((*hash, Summary::from_leaves(nodes)));
    }

    remaining.retain(|hash| !children.contains_key(hash));

    // Handle empty and invalid nodes
    for hash in remaining {
        if hash == *EMPTY_INNER_HASH {
            output.push((hash, Summary::from_inners(&InnerNodes::default())));
            continue;
        }

        if hash == *EMPTY_LEAF_HASH {
            output.push((hash, Summary::from_leaves(&LeafNodes::default())));
            continue;
        }

        output.push((hash, Summary::INCOMPLETE));
    }

    Ok(output)
}

/// Updates summaries according to the given map (keys are hashes of nodes to update, values are the
/// summaries to update the node with).
/// Returns map of updated node hash -> it's parent
pub(super) async fn update_summaries(
    tx: &mut db::WriteTransaction,
    summaries: &[(Hash, Summary)],
) -> Result<HashMap<Hash, Hash>, Error> {
    let mut output = HashMap::with_capacity(summaries.len());

    for summaries_chunk in summaries.chunks(MAX_BIND_PARAMS / 3) {
        let mut builder = QueryBuilder::new("WITH input(hash, state, block_presence) AS (");
        builder.push_values(summaries_chunk, |mut builder, (hash, summary)| {
            builder
                .push_bind(hash)
                .push_bind(summary.state)
                .push_bind(&summary.block_presence);
        });
        builder.push(
            ") UPDATE snapshot_inner_nodes
             SET state = input.state, block_presence = input.block_presence
             FROM input
             WHERE snapshot_inner_nodes.hash = input.hash
             RETURNING hash, parent",
        );
        builder
            .build()
            .persistent(false)
            .fetch(&mut *tx)
            .map_ok(|row| (row.get::<Hash, _>(0), row.get(1)))
            .try_collect_into(&mut output)
            .await?;
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::slice;

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

        let nodes = load_children(&mut tx, slice::from_ref(&parent))
            .await
            .unwrap()
            .into_values()
            .next()
            .unwrap();

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

        let nodes = load_children(&mut tx, slice::from_ref(&parent))
            .await
            .unwrap()
            .into_values()
            .next()
            .unwrap();

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
