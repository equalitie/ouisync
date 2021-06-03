use super::{Crc, MissingBlocksCount, SnapshotId, INNER_LAYER_COUNT, MAX_INNER_NODE_CHILD_COUNT};
use crate::{block::BlockId, crypto::Hash, db, error::Result, replica_id::ReplicaId};
use async_recursion::async_recursion;
use futures::{Stream, TryStreamExt};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::{iter::FromIterator, mem, slice};

#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct NodeData {
    pub hash: Hash,
    pub is_complete: bool,
    pub missing_blocks_crc: Crc,
    pub missing_blocks_count: usize,
}

#[derive(Clone, Debug)]
pub struct RootNode {
    pub snapshot_id: SnapshotId,
    pub data: NodeData,
}

impl RootNode {
    /// Returns the latest root node of the specified replica. If no such node exists yet, creates
    /// it first.
    pub async fn get_latest_or_create(
        tx: &mut db::Transaction,
        replica_id: &ReplicaId,
    ) -> Result<Self> {
        let node = Self::get_all(&mut *tx, replica_id, 1).try_next().await?;
        let node = if let Some(node) = node {
            node
        } else {
            let node = NewRootNode {
                replica_id: *replica_id,
                data: NodeData {
                    hash: Hash::null(),
                    is_complete: true,
                    missing_blocks_crc: 0,
                    missing_blocks_count: 0,
                },
            };

            node.insert(tx).await?.0
        };

        Ok(node)
    }

    /// Returns a stream of all (but at most `limit`) root nodes corresponding to the specified
    /// replica ordered from the most recent to the least recent.
    pub fn get_all<'a>(
        tx: &'a mut db::Transaction,
        replica_id: &'a ReplicaId,
        limit: u32,
    ) -> impl Stream<Item = Result<Self>> + 'a {
        sqlx::query(
            "SELECT
                 snapshot_id,
                 hash,
                 is_complete,
                 missing_blocks_crc,
                 missing_blocks_count
             FROM snapshot_root_nodes
             WHERE replica_id = ?
             ORDER BY snapshot_id DESC
             LIMIT ?",
        )
        .bind(replica_id)
        .bind(limit)
        .map(|row| Self {
            snapshot_id: row.get(0),
            data: NodeData {
                hash: row.get(1),
                is_complete: row.get(2),
                missing_blocks_crc: row.get(3),
                missing_blocks_count: row.get::<MissingBlocksCount, _>(4) as usize,
            },
        })
        .fetch(tx)
        .err_into()
    }

    pub async fn clone_with_new_root(
        &self,
        tx: &mut db::Transaction,
        root_hash: &Hash,
    ) -> Result<RootNode> {
        let new_id = sqlx::query(
            "INSERT INTO snapshot_root_nodes (
                 replica_id,
                 hash,
                 is_complete,
                 missing_blocks_crc,
                 missing_blocks_count
             )
             SELECT
                 replica_id,
                 ?,
                 is_complete,
                 missing_blocks_crc,
                 missing_blocks_count
             FROM snapshot_root_nodes
             WHERE snapshot_id = ?
             RETURNING snapshot_id;",
        )
        .bind(root_hash)
        .bind(self.snapshot_id)
        .fetch_one(tx)
        .await?
        .get(0);

        Ok(RootNode {
            snapshot_id: new_id,
            data: NodeData {
                hash: *root_hash,
                ..self.data
            },
        })
    }

    pub async fn remove_recursive(&self, tx: &mut db::Transaction) -> Result<()> {
        self.as_link().remove_recursive(0, tx).await
    }

    fn as_link(&self) -> Link {
        Link::ToRoot { node: self.clone() }
    }
}

/// Root node that hasn't been saved into the db yet.
#[derive(Clone, Debug)]
pub struct NewRootNode {
    pub replica_id: ReplicaId,
    pub data: NodeData,
}

impl NewRootNode {
    /// Inserts or this `NewRootNode` into the db and returns the corresponding `RootNode`.
    /// If a root node with the same replica id and hash already exists, updates it instead of
    /// creating a new one.
    /// Also returns a flag indicating whether anything changed.
    pub async fn insert(&self, tx: &mut db::Transaction) -> Result<(RootNode, bool)> {
        let result = sqlx::query(
            "INSERT INTO snapshot_root_nodes (
                 replica_id,
                 hash,
                 is_complete,
                 missing_blocks_crc,
                 missing_blocks_count
             )
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (replica_id, hash) DO UPDATE SET
                 is_complete          = ?,
                 missing_blocks_crc   = ?,
                 missing_blocks_count = ?
             WHERE excluded.is_complete           <> is_complete
                 OR excluded.missing_blocks_crc   <> missing_blocks_crc
                 OR excluded.missing_blocks_count <> missing_blocks_count;",
        )
        .bind(&self.replica_id)
        .bind(&self.data.hash)
        .bind(self.data.is_complete)
        .bind(self.data.missing_blocks_crc)
        .bind(self.data.missing_blocks_count as MissingBlocksCount)
        .bind(self.data.is_complete)
        .bind(self.data.missing_blocks_crc)
        .bind(self.data.missing_blocks_count as MissingBlocksCount)
        .execute(tx)
        .await?;

        let changed = result.rows_affected() > 0;
        let snapshot_id = result.last_insert_rowid() as SnapshotId;

        Ok((
            RootNode {
                snapshot_id,
                data: self.data,
            },
            changed,
        ))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct InnerNode {
    pub data: NodeData,
}

impl InnerNode {
    pub async fn insert(
        &self,
        bucket: usize,
        parent: &Hash,
        tx: &mut db::Transaction,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO snapshot_inner_nodes (
                 parent,
                 hash,
                 bucket,
                 is_complete,
                 missing_blocks_crc,
                 missing_blocks_count
             )
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(parent)
        .bind(&self.data.hash)
        .bind(bucket as u16)
        .bind(self.data.is_complete)
        .bind(self.data.missing_blocks_crc)
        .bind(self.data.missing_blocks_count as MissingBlocksCount)
        .execute(&mut *tx)
        .await?;
        Ok(())
    }

    pub fn empty() -> Self {
        Self::from(NodeData {
            hash: Hash::null(),
            is_complete: true,
            missing_blocks_crc: 0,
            missing_blocks_count: 0,
        })
    }
}

impl From<NodeData> for InnerNode {
    fn from(data: NodeData) -> Self {
        Self { data }
    }
}

pub async fn inner_children(
    parent: &Hash,
    tx: &mut db::Transaction,
) -> Result<[InnerNode; MAX_INNER_NODE_CHILD_COUNT]> {
    let rows = sqlx::query(
        "SELECT
             hash,
             bucket,
             is_complete,
             missing_blocks_crc,
             missing_blocks_count
         FROM snapshot_inner_nodes
         WHERE parent = ?",
    )
    .bind(parent)
    .fetch_all(&mut *tx)
    .await?;

    let mut children = [InnerNode::empty(); MAX_INNER_NODE_CHILD_COUNT];

    for row in rows {
        let hash = row.get(0);
        let bucket: u32 = row.get(1);
        let is_complete = row.get(2);
        let missing_blocks_crc = row.get(3);
        let missing_blocks_count = row.get::<MissingBlocksCount, _>(4) as usize;

        if let Some(node) = children.get_mut(bucket as usize) {
            *node = InnerNode::from(NodeData {
                hash,
                is_complete,
                missing_blocks_crc,
                missing_blocks_count,
            });
        } else {
            log::error!("inner node ({:?}) bucket out of range: {}", hash, bucket);
            // TODO: should we return error here?
        }
    }

    Ok(children)
}

#[derive(Eq, PartialEq, Debug, Clone)]
pub struct LeafNode {
    locator: Hash,
    pub block_id: BlockId,
    pub is_block_missing: bool,
}

impl LeafNode {
    pub fn locator(&self) -> &Hash {
        &self.locator
    }

    pub async fn insert(&self, parent: &Hash, tx: &mut db::Transaction) -> Result<()> {
        sqlx::query(
            "INSERT INTO snapshot_leaf_nodes (
                 parent,
                 locator,
                 block_id,
                 is_block_missing
             )
             VALUES (?, ?, ?, ?)",
        )
        .bind(parent)
        .bind(&self.locator)
        .bind(&self.block_id)
        .bind(self.is_block_missing)
        .execute(&mut *tx)
        .await?;
        Ok(())
    }
}

pub async fn leaf_children(parent: &Hash, tx: &mut db::Transaction) -> Result<LeafNodeSet> {
    let rows = sqlx::query(
        "SELECT locator, block_id, is_block_missing
         FROM snapshot_leaf_nodes
         WHERE parent = ?",
    )
    .bind(parent)
    .fetch_all(&mut *tx)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(LeafNode {
                locator: row.get(0),
                block_id: row.get(1),
                is_block_missing: row.get(2),
            })
        })
        .collect()
}

/// Collection that acts as a ordered set of `LeafNode`s
#[derive(Default, Debug)]
pub struct LeafNodeSet(Vec<LeafNode>);

impl LeafNodeSet {
    /// Inserts a new node or updates it if already exists.
    pub fn modify(&mut self, locator: &Hash, block_id: &BlockId) -> ModifyStatus {
        match self.lookup(locator) {
            Ok(index) => {
                let node = &mut self.0[index];

                if &node.block_id == block_id {
                    ModifyStatus::Unchanged
                } else {
                    ModifyStatus::Updated(mem::replace(&mut node.block_id, *block_id))
                }
            }
            Err(index) => {
                self.0.insert(
                    index,
                    LeafNode {
                        locator: *locator,
                        block_id: *block_id,
                        is_block_missing: false,
                    },
                );
                ModifyStatus::Inserted
            }
        }
    }

    pub fn remove(&mut self, locator: &Hash) -> Option<LeafNode> {
        let index = self.lookup(locator).ok()?;
        Some(self.0.remove(index))
    }

    pub fn get(&self, locator: &Hash) -> Option<&LeafNode> {
        self.lookup(locator).ok().map(|index| &self.0[index])
    }

    pub fn iter(&self) -> slice::Iter<LeafNode> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[LeafNode] {
        &self.0
    }

    fn lookup(&self, locator: &Hash) -> Result<usize, usize> {
        self.0.binary_search_by(|node| node.locator.cmp(locator))
    }
}

impl FromIterator<LeafNode> for LeafNodeSet {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = LeafNode>,
    {
        let mut vec: Vec<_> = iter.into_iter().collect();
        vec.sort_by(|lhs, rhs| lhs.locator.cmp(&rhs.locator));

        Self(vec)
    }
}

impl<'a> IntoIterator for &'a LeafNodeSet {
    type Item = &'a LeafNode;
    type IntoIter = slice::Iter<'a, LeafNode>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

pub enum ModifyStatus {
    Updated(BlockId),
    Inserted,
    Unchanged,
}

// We're not repeating enumeration name
// https://rust-lang.github.io/rust-clippy/master/index.html#enum_variant_names
#[allow(clippy::enum_variant_names)]
#[derive(Debug)]
enum Link {
    ToRoot { node: RootNode },
    ToInner { parent: Hash, node: InnerNode },
    ToLeaf { parent: Hash, node: LeafNode },
}

impl Link {
    #[async_recursion]
    pub async fn remove_recursive(&self, layer: usize, tx: &mut db::Transaction) -> Result<()> {
        self.remove_single(tx).await?;

        if !self.is_dangling(tx).await? {
            return Ok(());
        }

        for child in self.children(layer, tx).await? {
            child.remove_recursive(layer + 1, tx).await?;
        }

        Ok(())
    }

    async fn remove_single(&self, tx: &mut db::Transaction) -> Result<()> {
        match self {
            Link::ToRoot { node } => {
                sqlx::query("DELETE FROM snapshot_root_nodes WHERE snapshot_id = ?")
                    .bind(node.snapshot_id)
                    .execute(&mut *tx)
                    .await?;
            }
            Link::ToInner { parent, node } => {
                sqlx::query("DELETE FROM snapshot_inner_nodes WHERE parent = ? AND hash = ?")
                    .bind(parent)
                    .bind(&node.data.hash)
                    .execute(&mut *tx)
                    .await?;
            }
            Link::ToLeaf { parent, node } => {
                sqlx::query("DELETE FROM snapshot_leaf_nodes WHERE parent = ? AND locator = ? AND block_id = ?")
                    .bind(parent)
                    .bind(node.locator())
                    .bind(&node.block_id)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        Ok(())
    }

    /// Return true if there is nothing that references this node
    async fn is_dangling(&self, tx: &mut db::Transaction) -> Result<bool> {
        let has_parent = match self {
            Link::ToRoot { node: root } => {
                sqlx::query("SELECT 0 FROM snapshot_root_nodes WHERE hash = ? LIMIT 1")
                    .bind(&root.data.hash)
                    .fetch_optional(&mut *tx)
                    .await?
                    .is_some()
            }
            Link::ToInner { parent: _, node } => {
                sqlx::query("SELECT 0 FROM snapshot_inner_nodes WHERE hash = ? LIMIT 1")
                    .bind(&node.data.hash)
                    .fetch_optional(&mut *tx)
                    .await?
                    .is_some()
            }
            Link::ToLeaf { parent: _, node } => sqlx::query(
                "SELECT 0
                 FROM snapshot_leaf_nodes
                 WHERE locator = ? AND block_id = ?
                 LIMIT 1",
            )
            .bind(node.locator())
            .bind(&node.block_id)
            .fetch_optional(&mut *tx)
            .await?
            .is_some(),
        };

        Ok(!has_parent)
    }

    async fn children(&self, layer: usize, tx: &mut db::Transaction) -> Result<Vec<Link>> {
        match self {
            Link::ToRoot { node: root } => self.inner_children(tx, &root.data.hash).await,
            Link::ToInner { node, .. } if layer < INNER_LAYER_COUNT => {
                self.inner_children(tx, &node.data.hash).await
            }
            Link::ToInner { node, .. } => self.leaf_children(tx, &node.data.hash).await,
            Link::ToLeaf { parent: _, node: _ } => Ok(Vec::new()),
        }
    }

    async fn inner_children(&self, tx: &mut db::Transaction, parent: &Hash) -> Result<Vec<Link>> {
        sqlx::query(
            "SELECT
                 parent, hash, is_complete, missing_blocks_crc, missing_blocks_count
             FROM snapshot_inner_nodes
             WHERE parent = ?;",
        )
        .bind(parent)
        .fetch_all(tx)
        .await?
        .iter()
        .map(|row| {
            Ok(Link::ToInner {
                parent: row.get(0),
                node: InnerNode {
                    data: NodeData {
                        hash: row.get(1),
                        is_complete: row.get(2),
                        missing_blocks_crc: row.get(3),
                        missing_blocks_count: row.get::<MissingBlocksCount, _>(4) as usize,
                    },
                },
            })
        })
        .collect()
    }

    async fn leaf_children(&self, tx: &mut db::Transaction, parent: &Hash) -> Result<Vec<Link>> {
        sqlx::query(
            "SELECT parent, locator, block_id, is_block_missing
             FROM snapshot_leaf_nodes
             WHERE parent = ?;",
        )
        .bind(parent)
        .fetch_all(&mut *tx)
        .await?
        .iter()
        .map(|row| {
            Ok(Link::ToLeaf {
                parent: row.get(0),
                node: LeafNode {
                    locator: row.get(1),
                    block_id: row.get(2),
                    is_block_missing: row.get(3),
                },
            })
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{crypto::Hashable, test_utils};
    use rand::{rngs::StdRng, Rng, SeedableRng};
    use test_strategy::proptest;

    #[proptest]
    fn insert_new_root_node(
        hash_seed: u64,
        is_complete: bool,
        missing_blocks_crc: u32,
        missing_blocks_count: usize,
        #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
    ) {
        test_utils::run(insert_new_root_node_case(
            hash_seed.hash(),
            is_complete,
            missing_blocks_crc,
            missing_blocks_count,
            StdRng::seed_from_u64(rng_seed),
        ))
    }

    async fn insert_new_root_node_case(
        hash: Hash,
        is_complete: bool,
        missing_blocks_crc: u32,
        missing_blocks_count: usize,
        mut rng: StdRng,
    ) {
        let pool = setup().await;
        let mut tx = pool.begin().await.unwrap();

        let replica_id = rng.gen();

        let new_node = NewRootNode {
            replica_id,
            data: NodeData {
                hash,
                is_complete,
                missing_blocks_crc,
                missing_blocks_count,
            },
        };
        let (node, changed) = new_node.insert(&mut tx).await.unwrap();
        let snapshot_id = node.snapshot_id;

        assert!(changed);
        assert_eq!(node.data, new_node.data);

        let node = RootNode::get_all(&mut tx, &replica_id, 1)
            .try_next()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(node.snapshot_id, snapshot_id);
        assert_eq!(node.data, new_node.data);
    }

    #[proptest]
    fn update_existing_root_node(
        hash_seed: u64,
        old_is_complete: bool,
        new_is_complete: bool,
        old_missing_blocks_crc: u32,
        new_missing_blocks_crc: u32,
        #[strategy(1usize..)] old_missing_blocks_count: usize,
        #[strategy(0..=#old_missing_blocks_count)] new_missing_blocks_count: usize,
        #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
    ) {
        test_utils::run(update_existing_root_node_case(
            hash_seed.hash(),
            old_is_complete,
            new_is_complete,
            old_missing_blocks_crc,
            new_missing_blocks_crc,
            old_missing_blocks_count,
            new_missing_blocks_count,
            StdRng::seed_from_u64(rng_seed),
        ))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn update_existing_root_node_with_no_change() {
        let mut rng = StdRng::seed_from_u64(0);
        let hash = rng.gen::<u64>().hash();
        update_existing_root_node_case(hash, false, false, 0, 0, 0, 0, rng).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn update_existing_root_node_case(
        hash: Hash,
        old_is_complete: bool,
        new_is_complete: bool,
        old_missing_blocks_crc: u32,
        new_missing_blocks_crc: u32,
        old_missing_blocks_count: usize,
        new_missing_blocks_count: usize,
        mut rng: StdRng,
    ) {
        let pool = setup().await;
        let mut tx = pool.begin().await.unwrap();

        let replica_id = rng.gen();

        let new_node0 = NewRootNode {
            replica_id,
            data: NodeData {
                hash,
                is_complete: old_is_complete,
                missing_blocks_crc: old_missing_blocks_crc,
                missing_blocks_count: old_missing_blocks_count,
            },
        };
        let (node, _) = new_node0.insert(&mut tx).await.unwrap();
        let snapshot_id = node.snapshot_id;

        let new_node1 = NewRootNode {
            replica_id,
            data: NodeData {
                hash,
                is_complete: new_is_complete,
                missing_blocks_crc: new_missing_blocks_crc,
                missing_blocks_count: new_missing_blocks_count,
            },
        };
        let (node, changed) = new_node1.insert(&mut tx).await.unwrap();

        assert_eq!(changed, new_node0.data != new_node1.data);
        assert_eq!(node.snapshot_id, snapshot_id);
        assert_eq!(node.data, new_node1.data);

        let nodes: Vec<_> = RootNode::get_all(&mut tx, &replica_id, 2)
            .try_collect()
            .await
            .unwrap();

        assert_eq!(nodes.len(), 1);

        assert_eq!(nodes[0].snapshot_id, snapshot_id);
        assert_eq!(nodes[0].data, new_node1.data);
    }

    async fn setup() -> db::Pool {
        let pool = db::Pool::connect(":memory:").await.unwrap();
        super::super::init(&pool).await.unwrap();
        pool
    }
}
