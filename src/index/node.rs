use crate::{
    block::BlockId,
    crypto::Hash,
    db,
    error::Result,
    index::{Crc, MissingBlocksCount, SnapshotId, INNER_LAYER_COUNT, MAX_INNER_NODE_CHILD_COUNT},
    replica_id::ReplicaId,
};
use async_recursion::async_recursion;
use sqlx::Row;
use std::{iter::FromIterator, slice};

#[derive(Clone, Debug)]
pub struct RootNode {
    pub snapshot_id: SnapshotId,
    pub hash: Hash,
    pub is_complete: bool,
    pub missing_blocks_crc: Crc,
    pub missing_blocks_count: usize,
}

impl RootNode {
    pub async fn get_latest_or_create(pool: db::Pool, replica_id: &ReplicaId) -> Result<Self> {
        let mut conn = pool.acquire().await?;

        let (snapshot_id, hash, is_complete, missing_blocks_crc, missing_blocks_count) =
            match sqlx::query(
                "SELECT
                     snapshot_id,
                     hash,
                     is_complete,
                     missing_blocks_crc,
                     missing_blocks_count
                 FROM snapshot_root_nodes
                 WHERE replica_id=?
                 ORDER BY snapshot_id DESC
                 LIMIT 1",
            )
            .bind(replica_id)
            .fetch_optional(&mut conn)
            .await?
            {
                Some(row) => (
                    row.get(0),
                    row.get(1),
                    row.get(2),
                    row.get(3),
                    row.get::<MissingBlocksCount, _>(4) as usize,
                ),
                None => {
                    let snapshot_id = sqlx::query(
                        "INSERT INTO snapshot_root_nodes (
                             replica_id,
                             hash,
                             is_complete,
                             missing_blocks_crc,
                             missing_blocks_count
                         )
                         VALUES (?, ?, 1, 0, 0)
                         RETURNING snapshot_id;",
                    )
                    .bind(replica_id)
                    .bind(&Hash::null())
                    .fetch_one(&mut conn)
                    .await?
                    .get(0);

                    (snapshot_id, Hash::null(), true, 0, 0)
                }
            };

        Ok(Self {
            snapshot_id,
            hash,
            is_complete,
            missing_blocks_crc,
            missing_blocks_count,
        })
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
        .fetch_one(&mut *tx)
        .await?
        .get(0);

        Ok(RootNode {
            snapshot_id: new_id,
            hash: *root_hash,
            is_complete: self.is_complete,
            missing_blocks_crc: self.missing_blocks_crc,
            missing_blocks_count: self.missing_blocks_count,
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
        .bind(&self.hash)
        .bind(bucket as u16)
        .bind(self.is_complete)
        .bind(self.missing_blocks_crc)
        .bind(self.missing_blocks_count as MissingBlocksCount)
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
            *node = InnerNode {
                hash,
                is_complete,
                missing_blocks_crc,
                missing_blocks_count,
            };
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
        .bind(self.block_id.as_array().as_ref())
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
    /// Inserts a new node or updates it if already exists. Returns `true` if the node was inserted
    /// or updated and `false` if it already existed with the same `block_id`.
    pub fn insert_or_update(&mut self, locator: &Hash, block_id: &BlockId) -> bool {
        match self.lookup(locator) {
            Ok(index) => {
                let node = &mut self.0[index];

                if &node.block_id == block_id {
                    // node already exists with the same block id
                    return false;
                }

                node.block_id = *block_id;
            }
            Err(index) => self.0.insert(
                index,
                LeafNode {
                    locator: *locator,
                    block_id: *block_id,
                    is_block_missing: false,
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
                    .bind(&node.hash)
                    .execute(&mut *tx)
                    .await?;
            }
            Link::ToLeaf { parent, node } => {
                sqlx::query("DELETE FROM snapshot_leaf_nodes WHERE parent = ? AND locator = ? AND block_id = ?")
                    .bind(parent)
                    .bind(node.locator())
                    .bind(node.block_id.as_array().as_ref())
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
                    .bind(&root.hash)
                    .fetch_optional(&mut *tx)
                    .await?
                    .is_some()
            }
            Link::ToInner { parent: _, node } => {
                sqlx::query("SELECT 0 FROM snapshot_inner_nodes WHERE hash = ? LIMIT 1")
                    .bind(&node.hash)
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
            .bind(node.block_id.as_array().as_ref())
            .fetch_optional(&mut *tx)
            .await?
            .is_some(),
        };

        Ok(!has_parent)
    }

    async fn children(&self, layer: usize, tx: &mut db::Transaction) -> Result<Vec<Link>> {
        match self {
            Link::ToRoot { node: root } => self.inner_children(tx, &root.hash).await,
            Link::ToInner { node, .. } if layer < INNER_LAYER_COUNT => {
                self.inner_children(tx, &node.hash).await
            }
            Link::ToInner { node, .. } => self.leaf_children(tx, &node.hash).await,
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
                    hash: row.get(1),
                    is_complete: row.get(2),
                    missing_blocks_crc: row.get(3),
                    missing_blocks_count: row.get::<MissingBlocksCount, _>(4) as usize,
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
