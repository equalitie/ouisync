use crate::{
    block::BlockId,
    crypto::{Hash, Hashable},
    db,
    error::Result,
};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use sqlx::Row;
use std::{iter::FromIterator, mem, slice, vec};

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct LeafNode {
    locator: Hash,
    pub block_id: BlockId,
}

impl LeafNode {
    pub fn new(locator: Hash, block_id: BlockId) -> Self {
        Self { locator, block_id }
    }

    pub fn locator(&self) -> &Hash {
        &self.locator
    }

    pub async fn save(&self, tx: &mut db::Transaction, parent: &Hash) -> Result<()> {
        sqlx::query(
            "INSERT INTO snapshot_leaf_nodes (parent, locator, block_id)
             VALUES (?, ?, ?)",
        )
        .bind(parent)
        .bind(&self.locator)
        .bind(&self.block_id)
        .execute(tx)
        .await?;

        Ok(())
    }

    pub async fn load_children(db: impl db::Executor<'_>, parent: &Hash) -> Result<LeafNodeSet> {
        Ok(sqlx::query(
            "SELECT locator, block_id
             FROM snapshot_leaf_nodes
             WHERE parent = ?",
        )
        .bind(parent)
        .map(|row| LeafNode {
            locator: row.get(0),
            block_id: row.get(1),
        })
        .fetch_all(db)
        .await?
        .into_iter()
        .collect())
    }

    pub async fn has_children(pool: &db::Pool, parent: &Hash) -> Result<bool> {
        Ok(
            sqlx::query("SELECT 1 FROM snapshot_leaf_nodes WHERE parent = ?")
                .bind(parent)
                .fetch_optional(pool)
                .await?
                .is_some(),
        )
    }
}

/// Collection that acts as a ordered set of `LeafNode`s
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct LeafNodeSet(Vec<LeafNode>);

impl LeafNodeSet {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn get(&self, locator: &Hash) -> Option<&LeafNode> {
        self.lookup(locator).ok().map(|index| &self.0[index])
    }

    pub fn iter(&self) -> impl Iterator<Item = &LeafNode> {
        self.0.iter()
    }

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

    /// Atomically saves all nodes in this set into the db.
    pub async fn save(&self, pool: &db::Pool, parent: &Hash) -> Result<()> {
        let mut tx = pool.begin().await?;
        for node in self {
            node.save(&mut tx, parent).await?;
        }
        tx.commit().await?;

        Ok(())
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

impl IntoIterator for LeafNodeSet {
    type Item = LeafNode;
    type IntoIter = vec::IntoIter<LeafNode>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl Hashable for LeafNodeSet {
    fn hash(&self) -> Hash {
        let mut hasher = Sha3_256::new();
        // XXX: Is updating with length enough to prevent attacks?
        hasher.update((self.len() as u64).to_le_bytes());
        for node in self.iter() {
            hasher.update(node.locator());
            hasher.update(node.block_id);
        }
        hasher.finalize().into()
    }
}

pub enum ModifyStatus {
    Updated(BlockId),
    Inserted,
    Unchanged,
}
