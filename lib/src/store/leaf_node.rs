use super::error::Error;
use crate::{
    crypto::Hash,
    db,
    protocol::{BlockId, LeafNode, LeafNodes, SingleBlockPresence},
};
use futures_util::{Stream, TryStreamExt};
use sqlx::{sqlite::SqliteRow, FromRow, Row};

#[cfg(test)]
use {super::inner_node, crate::protocol::INNER_LAYER_COUNT, async_recursion::async_recursion};

#[derive(Eq, PartialEq, Debug)]
pub(super) struct LeafNodeUpdate {
    pub parent: Hash,
    pub encoded_locator: Hash,
}

impl FromRow<'_, SqliteRow> for LeafNodeUpdate {
    fn from_row(row: &SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            parent: row.try_get(0)?,
            encoded_locator: row.try_get(1)?,
        })
    }
}

pub(super) async fn load_children(
    conn: &mut db::Connection,
    parent: &Hash,
) -> Result<LeafNodes, Error> {
    Ok(sqlx::query(
        "SELECT locator, block_id, block_presence
         FROM snapshot_leaf_nodes
         WHERE parent = ?",
    )
    .bind(parent)
    .fetch(conn)
    .map_ok(|row| LeafNode {
        locator: row.get(0),
        block_id: row.get(1),
        block_presence: row.get(2),
    })
    .try_collect::<Vec<_>>()
    .await?
    .into_iter()
    .collect())
}

/// Loads all locators (most of the time (always?) there will be at most one) pointing to the
/// block id.
pub(super) fn load_locators<'a>(
    conn: &'a mut db::Connection,
    block_id: &'a BlockId,
) -> impl Stream<Item = Result<Hash, Error>> + 'a {
    sqlx::query("SELECT locator FROM snapshot_leaf_nodes WHERE block_id = ?")
        .bind(block_id)
        .fetch(conn)
        .map_ok(|row| row.get(0))
        .err_into()
}

/// Fetches the block presence of the leaf node referencing the given block. Returns `None` if no
/// such node exists.
pub(super) async fn load_block_presence(
    conn: &mut db::Connection,
    block_id: &BlockId,
) -> Result<Option<SingleBlockPresence>, Error> {
    Ok(
        sqlx::query("SELECT block_presence FROM snapshot_leaf_nodes WHERE block_id = ? LIMIT 1")
            .bind(block_id)
            .fetch_optional(conn)
            .await?
            .map(|row| row.get(0)),
    )
}

/// Saves the node to the db unless it already exists.
pub(super) async fn save(
    tx: &mut db::WriteTransaction,
    node: &LeafNode,
    parent: &Hash,
) -> Result<bool, Error> {
    let result = sqlx::query(
        "INSERT INTO snapshot_leaf_nodes (parent, locator, block_id, block_presence)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (parent, locator, block_id) DO NOTHING",
    )
    .bind(parent)
    .bind(&node.locator)
    .bind(&node.block_id)
    .bind(node.block_presence)
    .execute(tx)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub(super) async fn save_all(
    tx: &mut db::WriteTransaction,
    nodes: &LeafNodes,
    parent: &Hash,
) -> Result<usize, Error> {
    let mut updated = 0;

    for node in nodes {
        if save(tx, node, parent).await? {
            updated += 1;
        }
    }

    Ok(updated)
}

/// Marks all leaf nodes that point to the specified block as present (not missing). Returns
/// the locators and parent hashes of the updated nodes.
pub(super) fn set_present<'a>(
    tx: &'a mut db::WriteTransaction,
    block_id: &'a BlockId,
) -> impl Stream<Item = Result<LeafNodeUpdate, Error>> + 'a {
    // Update only those nodes that have block_presence set to `Missing`.
    sqlx::query_as(
        "UPDATE snapshot_leaf_nodes
         SET block_presence = ?
         WHERE block_id = ? AND (block_presence = ? OR block_presence = ?)
         RETURNING parent, locator",
    )
    .bind(SingleBlockPresence::Present)
    .bind(block_id)
    .bind(SingleBlockPresence::Expired)
    .bind(SingleBlockPresence::Missing)
    .fetch(tx)
    .err_into()
}

/// Marks all leaf nodes that point to the specified block as missing. Returns the parent hashes of
/// the updated nodes.
pub(super) fn set_missing<'a>(
    tx: &'a mut db::WriteTransaction,
    block_id: &'a BlockId,
) -> impl Stream<Item = Result<Hash, Error>> + 'a {
    sqlx::query(
        "UPDATE snapshot_leaf_nodes
         SET block_presence = ?
         WHERE block_presence <> ? AND block_id = ?
         RETURNING parent",
    )
    .bind(SingleBlockPresence::Missing)
    .bind(SingleBlockPresence::Missing)
    .bind(block_id)
    .fetch(tx)
    .map_ok(|row| row.get(0))
    .err_into()
}

/// Marks the block as missing ony if it's currently expired. Returns the parent hashes of the
/// updated nodes.
pub(super) fn set_missing_if_expired<'a>(
    tx: &'a mut db::WriteTransaction,
    block_id: &'a BlockId,
) -> impl Stream<Item = Result<Hash, Error>> + 'a {
    sqlx::query(
        "UPDATE snapshot_leaf_nodes
         SET block_presence = ?
         WHERE block_id = ? AND block_presence = ?
         RETURNING parent",
    )
    .bind(SingleBlockPresence::Missing)
    .bind(block_id)
    .bind(SingleBlockPresence::Expired)
    .fetch(tx)
    .map_ok(|row| row.get(0))
    .err_into()
}

/// Returns true if the block changed status from present to expired
pub(super) async fn set_expired_if_present(
    tx: &mut db::WriteTransaction,
    block_id: &BlockId,
) -> Result<bool, Error> {
    let result = sqlx::query(
        "UPDATE snapshot_leaf_nodes
         SET block_presence = ?
         WHERE block_id = ? AND block_presence = ?",
    )
    .bind(SingleBlockPresence::Expired)
    .bind(block_id)
    .bind(SingleBlockPresence::Present)
    .execute(tx)
    .await?;

    if result.rows_affected() > 0 {
        tracing::debug!(?block_id, "Block expired");
        return Ok(true);
    }

    Ok(false)
}

// Number distinct block ids across all leaf nodes.
pub(super) async fn count_block_ids(conn: &mut db::Connection) -> Result<u64, Error> {
    Ok(db::decode_u64(
        sqlx::query("SELECT COUNT(DISTINCT block_id) FROM snapshot_leaf_nodes")
            .fetch_one(conn)
            .await?
            .get(0),
    ))
}

#[cfg(test)]
#[async_recursion]
pub(super) async fn count_in(
    conn: &mut db::Connection,
    current_layer: usize,
    node: &Hash,
) -> Result<usize, Error> {
    // TODO: this can be rewritten as a single query using CTE

    if current_layer < INNER_LAYER_COUNT {
        let children = inner_node::load_children(conn, node).await?;

        let mut sum = 0;

        for (_bucket, child) in children {
            sum += count_in(conn, current_layer + 1, &child.hash).await?;
        }

        Ok(sum)
    } else {
        Ok(load_children(conn, node).await?.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::TryStreamExt;
    use tempfile::TempDir;

    #[tokio::test(flavor = "multi_thread")]
    async fn save_new_present() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let encoded_locator = rand::random();
        let block_id = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node = LeafNode::present(encoded_locator, block_id);
        save(&mut tx, &node, &parent).await.unwrap();

        let nodes = load_children(&mut tx, &parent).await.unwrap();
        assert_eq!(nodes.len(), 1);

        let node = nodes.get(&encoded_locator).unwrap();
        assert_eq!(node.locator, encoded_locator);
        assert_eq!(node.block_id, block_id);
        assert_eq!(node.block_presence, SingleBlockPresence::Present);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn save_new_missing() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let encoded_locator = rand::random();
        let block_id = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node = LeafNode::missing(encoded_locator, block_id);
        save(&mut tx, &node, &parent).await.unwrap();

        let nodes = load_children(&mut tx, &parent).await.unwrap();
        assert_eq!(nodes.len(), 1);

        let node = nodes.get(&encoded_locator).unwrap();
        assert_eq!(node.locator, encoded_locator);
        assert_eq!(node.block_id, block_id);
        assert_eq!(node.block_presence, SingleBlockPresence::Missing);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn save_missing_node_over_existing_missing_one() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let encoded_locator = rand::random();
        let block_id = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node = LeafNode::missing(encoded_locator, block_id);
        save(&mut tx, &node, &parent).await.unwrap();

        let node = LeafNode::missing(encoded_locator, block_id);
        save(&mut tx, &node, &parent).await.unwrap();

        let nodes = load_children(&mut tx, &parent).await.unwrap();
        assert_eq!(nodes.len(), 1);

        let node = nodes.get(&encoded_locator).unwrap();
        assert_eq!(node.locator, encoded_locator);
        assert_eq!(node.block_id, block_id);
        assert_eq!(node.block_presence, SingleBlockPresence::Missing);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn save_missing_node_over_existing_present_one() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let encoded_locator = rand::random();
        let block_id = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node = LeafNode::present(encoded_locator, block_id);
        save(&mut tx, &node, &parent).await.unwrap();

        let node = LeafNode::missing(encoded_locator, block_id);
        save(&mut tx, &node, &parent).await.unwrap();

        let nodes = load_children(&mut tx, &parent).await.unwrap();
        assert_eq!(nodes.len(), 1);

        let node = nodes.get(&encoded_locator).unwrap();
        assert_eq!(node.locator, encoded_locator);
        assert_eq!(node.block_id, block_id);
        assert_eq!(node.block_presence, SingleBlockPresence::Present);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn set_present_on_node_with_missing_block() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let encoded_locator = rand::random();
        let block_id = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node = LeafNode::missing(encoded_locator, block_id);
        save(&mut tx, &node, &parent).await.unwrap();

        assert_eq!(
            set_present(&mut tx, &block_id)
                .try_collect::<Vec<_>>()
                .await
                .unwrap(),
            [LeafNodeUpdate {
                parent,
                encoded_locator
            }],
        );

        let nodes = load_children(&mut tx, &parent).await.unwrap();
        assert_eq!(
            nodes.get(&encoded_locator).unwrap().block_presence,
            SingleBlockPresence::Present
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn set_present_on_node_with_present_block() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let encoded_locator = rand::random();
        let block_id = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node = LeafNode::present(encoded_locator, block_id);
        save(&mut tx, &node, &parent).await.unwrap();

        assert_eq!(
            set_present(&mut tx, &block_id)
                .try_collect::<Vec<_>>()
                .await
                .unwrap(),
            [],
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn set_present_on_node_that_does_not_exist() {
        let (_base_dir, pool) = setup().await;

        let block_id = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        assert_eq!(
            set_present(&mut tx, &block_id)
                .try_collect::<Vec<_>>()
                .await
                .unwrap(),
            [],
        )
    }

    async fn setup() -> (TempDir, db::Pool) {
        db::create_temp().await.unwrap()
    }
}
