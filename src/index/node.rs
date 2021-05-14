// We're not repeating enumeration name
// https://rust-lang.github.io/rust-clippy/master/index.html#enum_variant_names
#![allow(clippy::enum_variant_names)]

use crate::{
    crypto::Hash,
    db,
    error::Result,
    index::{column, Crc, LeafData, SnapshotId, INNER_LAYER_COUNT, MAX_INNER_NODE_CHILD_COUNT},
    replica_id::ReplicaId,
};
use async_recursion::async_recursion;
use sqlx::{sqlite::SqliteRow, Row};

#[derive(Clone, Debug)]
pub struct RootNode {
    pub snapshot_id: SnapshotId,
    pub missing_blocks_crc: Crc,
    pub root_hash: Hash,
}

impl RootNode {
    pub async fn get_latest_or_create(pool: db::Pool, replica_id: &ReplicaId) -> Result<Self> {
        let mut conn = pool.acquire().await?;

        let (snapshot_id, missing_blocks_crc, root_hash) = match sqlx::query(
            "SELECT snapshot_id, missing_blocks_crc, root_hash FROM snapshot_roots
             WHERE replica_id=? ORDER BY snapshot_id DESC LIMIT 1",
        )
        .bind(replica_id.as_ref())
        .fetch_optional(&mut conn)
        .await?
        {
            Some(row) => (
                row.get(0),
                row.get::<'_, u32, _>(1),
                column::<Hash>(&row, 2)?,
            ),
            None => {
                let snapshot_id = sqlx::query(
                    "INSERT INTO snapshot_roots(replica_id, missing_blocks_crc, root_hash)
                     VALUES (?, 0, ?) RETURNING snapshot_id;",
                )
                .bind(replica_id.as_ref())
                .bind(Hash::null().as_ref())
                .fetch_optional(&mut conn)
                .await?
                .unwrap()
                .get(0);

                (snapshot_id, 0, Hash::null())
            }
        };

        Ok(Self {
            snapshot_id,
            missing_blocks_crc,
            root_hash,
        })
    }

    pub async fn clone_with_new_root(
        &self,
        tx: &mut db::Transaction,
        root_hash: &Hash,
    ) -> Result<RootNode> {
        let new_id = sqlx::query(
            "INSERT INTO snapshot_roots(replica_id, missing_blocks_crc, root_hash)
             SELECT replica_id, missing_blocks_crc, ? FROM snapshot_roots
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
            missing_blocks_crc: self.missing_blocks_crc,
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
    pub missing_blocks_crc: Crc,
}

impl InnerNode {
    pub async fn insert(
        node: &InnerNode,
        bucket: usize,
        parent: &Hash,
        tx: &mut db::Transaction,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO snapshot_forest (parent, bucket, missing_blocks_crc, data) 
                     VALUES (?, ?, 0, ?)",
        )
        .bind(parent.as_ref())
        .bind(bucket as u16)
        .bind(node.hash.as_ref())
        .execute(&mut *tx)
        .await?;
        Ok(())
    }

    pub fn empty() -> Self {
        Self {
            hash: Hash::null(),
            missing_blocks_crc: 0,
        }
    }
}

#[derive(Eq, PartialEq, Ord, PartialOrd, Debug, Clone)]
pub struct LeafNode {
    pub data: LeafData,
    pub missing_blocks_crc: Crc,
}

impl LeafNode {
    pub async fn insert(leaf: &LeafNode, parent: &Hash, tx: &mut db::Transaction) -> Result<()> {
        let blob = leaf.data.serialize();
        sqlx::query("INSERT INTO snapshot_forest (parent, bucket, data) VALUES (?, ?, ?)")
            .bind(parent.as_ref())
            .bind(u16::MAX)
            .bind(blob)
            .execute(&mut *tx)
            .await?;
        Ok(())
    }
}

pub async fn inner_children(
    parent: &Hash,
    tx: &mut db::Transaction,
) -> Result<[InnerNode; MAX_INNER_NODE_CHILD_COUNT]> {
    let rows =
        sqlx::query("SELECT bucket, data, missing_blocks_crc FROM snapshot_forest WHERE parent=?")
            .bind(parent.as_ref())
            .fetch_all(&mut *tx)
            .await?;

    let mut children = [InnerNode {
        hash: Hash::null(),
        missing_blocks_crc: 0,
    }; MAX_INNER_NODE_CHILD_COUNT];

    for ref row in rows {
        let bucket: u32 = row.get(0);
        let hash = column::<Hash>(row, 1)?;
        let missing_blocks_crc = row.get(2);
        children[bucket as usize] = InnerNode {
            hash,
            missing_blocks_crc,
        };
    }

    Ok(children)
}

pub async fn leaf_children(parent: &Hash, tx: &mut db::Transaction) -> Result<Vec<LeafNode>> {
    let rows = sqlx::query("SELECT data, missing_blocks_crc FROM snapshot_forest WHERE parent=?")
        .bind(parent.as_ref())
        .fetch_all(&mut *tx)
        .await?;

    let mut children = Vec::new();
    children.reserve(rows.len());

    for ref row in rows {
        children.push(LeafNode {
            data: LeafData::deserialize(row.get(0))?,
            missing_blocks_crc: row.get(1),
        });
    }

    Ok(children)
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
                "SELECT data, parent, missing_blocks_crc FROM snapshot_forest WHERE parent=?;",
            )
            .bind(root.root_hash.as_ref())
            .fetch_all(&mut *tx)
            .await?
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
                "SELECT data, parent, missing_blocks_crc FROM snapshot_forest WHERE parent=?;",
            )
            .bind(node.hash.as_ref())
            .fetch_all(&mut *tx)
            .await?
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
                missing_blocks_crc: row.get(2),
            },
        })
    }

    fn row_to_leaf(row: &SqliteRow) -> Result<Link> {
        Ok(Link::ToLeaf {
            parent: column::<Hash>(row, 1)?,
            node: LeafNode {
                data: LeafData::deserialize(row.get(0))?,
                missing_blocks_crc: row.get(2),
            },
        })
    }
}
