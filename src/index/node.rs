// We're not repeating enumeration name
// https://rust-lang.github.io/rust-clippy/master/index.html#enum_variant_names
#![allow(clippy::enum_variant_names)]

use std::{iter::FromIterator, slice};

use crate::{
    block::BlockId,
    crypto::Hash,
    db,
    error::Result,
    index::{
        column, Crc, LeafData, MissingBlocksCount, SnapshotId, INNER_LAYER_COUNT,
        MAX_INNER_NODE_CHILD_COUNT,
    },
    replica_id::ReplicaId,
};
use async_recursion::async_recursion;
use sqlx::{sqlite::SqliteRow, Row};

#[derive(Clone, Debug)]
pub struct RootNode {
    pub snapshot_id: SnapshotId,
    pub is_complete: bool,
    pub missing_blocks_crc: Crc,
    pub missing_blocks_count: usize,
    pub root_hash: Hash,
}

impl RootNode {
    pub async fn get_latest_or_create(pool: db::Pool, replica_id: &ReplicaId) -> Result<Self> {
        let mut conn = pool.acquire().await?;

        let (snapshot_id, is_complete, missing_blocks_crc, missing_blocks_count, root_hash) = match sqlx::query(
            "SELECT snapshot_id, is_complete, missing_blocks_crc, missing_blocks_count, root_hash
             FROM snapshot_roots
             WHERE replica_id=? ORDER BY snapshot_id DESC LIMIT 1",
        )
        .bind(replica_id.as_ref())
        .fetch_optional(&mut conn)
        .await?
        {
            Some(row) => (
                row.get(0),
                row.get::<'_, u16, _>(1) != 0,
                row.get::<'_, u32, _>(2),
                row.get::<'_, MissingBlocksCount, _>(3) as usize,
                column::<Hash>(&row, 4)?,
            ),
            None => {
                let snapshot_id = sqlx::query(
                    "INSERT INTO snapshot_roots(replica_id, is_complete, missing_blocks_crc, missing_blocks_count, root_hash)
                     VALUES (?, 1, 0, 0, ?) RETURNING snapshot_id;",
                )
                .bind(replica_id.as_ref())
                .bind(Hash::null().as_ref())
                .fetch_optional(&mut conn)
                .await?
                .unwrap()
                .get(0);

                (snapshot_id, true, 0, 0, Hash::null())
            }
        };

        Ok(Self {
            snapshot_id,
            is_complete,
            missing_blocks_crc,
            missing_blocks_count,
            root_hash,
        })
    }

    pub async fn clone_with_new_root(
        &self,
        tx: &mut db::Transaction,
        root_hash: &Hash,
    ) -> Result<RootNode> {
        let new_id = sqlx::query(
            "INSERT INTO snapshot_roots(replica_id, is_complete, missing_blocks_crc, missing_blocks_count, root_hash)
             SELECT replica_id, is_complete, missing_blocks_crc, missing_blocks_count, ? FROM snapshot_roots
             WHERE snapshot_id=? RETURNING snapshot_id;",
        )
        .bind(root_hash.as_ref())
        .bind(self.snapshot_id)
        .fetch_optional(&mut *tx)
        .await
        .unwrap()
        .unwrap()
        .get(0);

        Ok(RootNode {
            snapshot_id: new_id,
            is_complete: self.is_complete,
            missing_blocks_crc: self.missing_blocks_crc,
            missing_blocks_count: self.missing_blocks_count,
            root_hash: *root_hash,
        })
    }

    pub async fn remove_recursive(&self, tx: &mut db::Transaction) -> Result<()> {
        self.as_link().remove_recursive(0, tx).await
    }

    fn as_link(&self) -> Link {
        Link::ToRoot { node: self.clone() }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct InnerNode {
    pub hash: Hash,
    pub is_complete: bool,
    pub missing_blocks_crc: Crc,
    pub missing_blocks_count: usize,
}

impl InnerNode {
    pub async fn insert(
        node: &InnerNode,
        bucket: usize,
        parent: &Hash,
        tx: &mut db::Transaction,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO snapshot_forest
             (parent, bucket, is_complete, missing_blocks_crc, missing_blocks_count, data)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(parent.as_ref())
        .bind(bucket as u16)
        .bind(node.is_complete as u16)
        .bind(node.missing_blocks_crc)
        .bind(node.missing_blocks_count as MissingBlocksCount)
        .bind(node.hash.as_ref())
        .execute(&mut *tx)
        .await?;
        Ok(())
    }

    pub fn empty() -> Self {
        Self {
            hash: Hash::null(),
            is_complete: true,
            missing_blocks_crc: 0,
            missing_blocks_count: 0,
        }
    }
}

#[derive(Eq, PartialEq, Debug, Clone)]
pub struct LeafNode {
    pub data: LeafData,
    pub is_complete: bool,
    pub missing_blocks_crc: Crc,
    pub missing_blocks_count: usize,
}

impl LeafNode {
    pub async fn insert(leaf: &LeafNode, parent: &Hash, tx: &mut db::Transaction) -> Result<()> {
        let blob = leaf.data.serialize();
        sqlx::query(
            "INSERT INTO snapshot_forest
                     (parent, bucket, is_complete, missing_blocks_crc, missing_blocks_count, data)
                     VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(parent.as_ref())
        .bind(u16::MAX)
        .bind(leaf.is_complete as u16)
        .bind(leaf.missing_blocks_crc)
        .bind(leaf.missing_blocks_count as MissingBlocksCount)
        .bind(blob)
        .execute(&mut *tx)
        .await?;
        Ok(())
    }
}

/// Collection that acts as a ordered set of `LeafNode`s
#[derive(Default, Debug)]
pub struct LeafNodeSet(Vec<LeafNode>);

impl LeafNodeSet {
    /// Inserts a new node or updates it if already exists. Returns `true` if the node was inserted
    /// or updated and `false` if it already existed with the same `block_id`.
    pub fn insert_or_update(&mut self, locator: &Hash, block_id: &BlockId) -> bool {
        match self.lookup(locator) {
            Ok(index) => {
                let node = &mut self.0[index];

                if &node.data.block_id == block_id {
                    // node already exists with the same block id
                    return false;
                }

                node.data.block_id = *block_id;
            }
            Err(index) => self.0.insert(
                index,
                LeafNode {
                    data: LeafData {
                        locator: *locator,
                        block_id: *block_id,
                    },
                    is_complete: true,
                    missing_blocks_crc: 0,
                    missing_blocks_count: 0,
                },
            ),
        }

        true
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
        self.0
            .binary_search_by(|node| node.data.locator.cmp(locator))
    }
}

impl FromIterator<LeafNode> for LeafNodeSet {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = LeafNode>,
    {
        let mut vec: Vec<_> = iter.into_iter().collect();
        vec.sort_by(|lhs, rhs| lhs.data.locator.cmp(&rhs.data.locator));

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

pub async fn inner_children(
    parent: &Hash,
    tx: &mut db::Transaction,
) -> Result<[InnerNode; MAX_INNER_NODE_CHILD_COUNT]> {
    let rows = sqlx::query(
        "SELECT bucket, is_complete, data, missing_blocks_crc, missing_blocks_count
                     FROM snapshot_forest WHERE parent=?",
    )
    .bind(parent.as_ref())
    .fetch_all(&mut *tx)
    .await?;

    let mut children = [InnerNode::empty(); MAX_INNER_NODE_CHILD_COUNT];

    for row in rows {
        let bucket: u32 = row.get(0);
        let is_complete = row.get::<'_, u16, _>(1) != 0;
        let hash = column::<Hash>(&row, 2)?;
        let missing_blocks_crc = row.get(3);
        let missing_blocks_count = row.get::<'_, MissingBlocksCount, _>(4) as usize;
        children[bucket as usize] = InnerNode {
            hash,
            is_complete,
            missing_blocks_crc,
            missing_blocks_count,
        };
    }

    Ok(children)
}

pub async fn leaf_children(parent: &Hash, tx: &mut db::Transaction) -> Result<LeafNodeSet> {
    let rows = sqlx::query(
        "SELECT data, is_complete, missing_blocks_crc, missing_blocks_count
                            FROM snapshot_forest WHERE parent=?",
    )
    .bind(parent.as_ref())
    .fetch_all(&mut *tx)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(LeafNode {
                data: LeafData::deserialize(row.get(0))?,
                is_complete: row.get::<'_, u16, _>(1) != 0,
                missing_blocks_crc: row.get(2),
                missing_blocks_count: row.get::<'_, MissingBlocksCount, _>(3) as usize,
            })
        })
        .collect()
}

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
                sqlx::query("DELETE FROM snapshot_roots WHERE snapshot_id=?")
                    .bind(node.snapshot_id)
                    .execute(&mut *tx)
                    .await?;
            }
            Link::ToInner { parent, node } => {
                sqlx::query("DELETE FROM snapshot_forest WHERE parent=? AND data=?")
                    .bind(parent.as_ref())
                    .bind(node.hash.as_ref())
                    .execute(&mut *tx)
                    .await?;
            }
            Link::ToLeaf { parent, node } => {
                let blob = node.data.serialize();
                sqlx::query("DELETE FROM snapshot_forest WHERE parent=? AND data=?")
                    .bind(parent.as_ref())
                    .bind(blob)
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
                sqlx::query("SELECT 0 FROM snapshot_roots WHERE root_hash=? LIMIT 1")
                    .bind(root.root_hash.as_ref())
                    .fetch_optional(&mut *tx)
                    .await?
                    .is_some()
            }
            Link::ToInner { parent: _, node } => {
                sqlx::query("SELECT 0 FROM snapshot_forest WHERE data=? LIMIT 1")
                    .bind(node.hash.as_ref())
                    .fetch_optional(&mut *tx)
                    .await?
                    .is_some()
            }
            Link::ToLeaf { parent: _, node } => {
                let blob = node.data.serialize();
                sqlx::query("SELECT 0 FROM snapshot_forest WHERE data=? LIMIT 1")
                    .bind(blob)
                    .fetch_optional(&mut *tx)
                    .await?
                    .is_some()
            }
        };

        Ok(!has_parent)
    }

    async fn children(&self, layer: usize, tx: &mut db::Transaction) -> Result<Vec<Link>> {
        match self {
            Link::ToRoot { node: root } => sqlx::query(
                "SELECT data, parent, is_complete, missing_blocks_crc, missing_blocks_count FROM snapshot_forest WHERE parent=?;",
            )
            .bind(root.root_hash.as_ref())
            .fetch_all(&mut *tx)
            .await.unwrap()
            .iter()
            .map(|row| {
                if INNER_LAYER_COUNT > 0 {
                    Self::row_to_inner(row)
                } else {
                    Self::row_to_leaf(row)
                }
            })
            .collect(),
            Link::ToInner { parent: _, node } => sqlx::query(
                "SELECT data, parent, is_complete, missing_blocks_crc, missing_blocks_count FROM snapshot_forest WHERE parent=?;",
            )
            .bind(node.hash.as_ref())
            .fetch_all(&mut *tx)
            .await.unwrap()
            .iter()
            .map(|row| {
                if layer < INNER_LAYER_COUNT {
                    Self::row_to_inner(row)
                } else {
                    Self::row_to_leaf(row)
                }
            })
            .collect(),
            Link::ToLeaf { parent: _, node: _ } => Ok(Vec::new()),
        }
    }

    fn row_to_inner(row: &SqliteRow) -> Result<Link> {
        Ok(Link::ToInner {
            parent: column::<Hash>(row, 1)?,
            node: InnerNode {
                hash: column::<Hash>(row, 0)?,
                is_complete: row.get::<'_, u16, _>(2) != 0,
                missing_blocks_crc: row.get(3),
                missing_blocks_count: row.get::<'_, MissingBlocksCount, _>(4) as usize,
            },
        })
    }

    fn row_to_leaf(row: &SqliteRow) -> Result<Link> {
        Ok(Link::ToLeaf {
            parent: column::<Hash>(row, 1)?,
            node: LeafNode {
                data: LeafData::deserialize(row.get(0))?,
                is_complete: row.get::<'_, u16, _>(2) != 0,
                missing_blocks_crc: row.get(3),
                missing_blocks_count: row.get::<'_, MissingBlocksCount, _>(4) as usize,
            },
        })
    }
}
