use crate::{
    block::BlockId,
    crypto::Hash,
    db,
    error::Result,
    index::LocatorHash,
    index::{column, deserialize_leaf, INNER_LAYER_COUNT, MAX_INNER_NODE_CHILD_COUNT},
    replica_id::ReplicaId,
};
use async_recursion::async_recursion;
use sqlx::{sqlite::SqliteRow, Row};

type SnapshotId = u32;

#[derive(Clone)]
pub struct RootNode {
    pub snapshot_id: SnapshotId,
    pub root_hash: Hash,
}

impl RootNode {
    pub async fn get_latest_or_create(pool: db::Pool, replica_id: &ReplicaId) -> Result<Self> {
        let mut conn = pool.acquire().await?;

        let (snapshot_id, root_hash) = match sqlx::query(
            "SELECT snapshot_id, root_hash FROM branches WHERE replica_id=? ORDER BY snapshot_id DESC LIMIT 1",
        )
        .bind(replica_id.as_ref())
        .fetch_optional(&mut conn)
        .await?
        {
            Some(row) => {
                (row.get(0), column::<Hash>(&row, 1)?)
            },
            None => {
                let snapshot_id = sqlx::query(
                    "INSERT INTO branches(replica_id, root_hash)
                             VALUES (?, ?) RETURNING snapshot_id;",
                )
                .bind(replica_id.as_ref())
                .bind(Hash::null().as_ref())
                .fetch_optional(&mut conn)
                .await?
                .unwrap()
                .get(0);

                (snapshot_id, Hash::null())
            }
        };

        Ok(Self {
            snapshot_id,
            root_hash,
        })
    }

    pub async fn clone_with_new_root(
        &self,
        tx: &mut db::Transaction,
        root_hash: &Hash,
    ) -> Result<RootNode> {
        let new_id = sqlx::query(
            "INSERT INTO branches(replica_id, root_hash)
             SELECT replica_id, ? FROM branches
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
            root_hash: *root_hash,
        })
    }

    pub fn as_node(&self) -> Node {
        Node::Root {
            root: self.root_hash,
            snapshot_id: self.snapshot_id,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct InnerNode {
    pub hash: Hash,
}

impl InnerNode {
    pub async fn insert(
        node: &InnerNode,
        bucket: usize,
        parent: &Hash,
        tx: &mut db::Transaction,
    ) -> Result<()> {
        sqlx::query("INSERT INTO branch_forest (parent, bucket, node) VALUES (?, ?, ?)")
            .bind(parent.as_ref())
            .bind(bucket as u16)
            .bind(node.hash.as_ref())
            .execute(&mut *tx)
            .await?;
        Ok(())
    }
}

#[derive(Eq, PartialEq, Ord, PartialOrd, Debug, Clone)]
pub struct LeafNode {
    pub locator: Hash,
    pub block_id: BlockId,
}

impl LeafNode {
    pub async fn insert(leaf: &LeafNode, parent: &Hash, tx: &mut db::Transaction) -> Result<()> {
        let blob = Self::serialize(&leaf.locator, &leaf.block_id);
        sqlx::query("INSERT INTO branch_forest (parent, bucket, node) VALUES (?, ?, ?)")
            .bind(parent.as_ref())
            .bind(u16::MAX)
            .bind(blob)
            .execute(&mut *tx)
            .await?;
        Ok(())
    }

    fn serialize(locator: &Hash, block_id: &BlockId) -> Vec<u8> {
        locator
            .as_ref()
            .iter()
            .chain(block_id.name.as_ref().iter())
            .chain(block_id.version.as_ref().iter())
            .cloned()
            .collect()
    }
}

pub async fn inner_children(
    parent: &Hash,
    tx: &mut db::Transaction,
) -> Result<[InnerNode; MAX_INNER_NODE_CHILD_COUNT]> {
    let rows = sqlx::query("SELECT bucket, node FROM branch_forest WHERE parent=?")
        .bind(parent.as_ref())
        .fetch_all(&mut *tx)
        .await?;

    let mut children = [InnerNode{hash: Hash::null()}; MAX_INNER_NODE_CHILD_COUNT];

    for ref row in rows {
        let bucket: u32 = row.get(0);
        let hash = column::<Hash>(row, 1)?;
        children[bucket as usize] = InnerNode{hash};
    }

    Ok(children)
}

pub async fn leaf_children(parent: &Hash, tx: &mut db::Transaction) -> Result<Vec<LeafNode>> {
    let rows = sqlx::query("SELECT node FROM branch_forest WHERE parent=?")
        .bind(parent.as_ref())
        .fetch_all(&mut *tx)
        .await?;

    let mut children = Vec::new();
    children.reserve(rows.len());

    for ref row in rows {
        let (locator, block_id) = deserialize_leaf(row.get(0))?;
        children.push(LeafNode { locator, block_id });
    }

    Ok(children)
}

#[derive(Debug)]
pub enum Node {
    Root {
        root: Hash,
        snapshot_id: SnapshotId,
    },
    Inner {
        node: Hash,
        parent: Hash,
    },
    Leaf {
        node: (LocatorHash, BlockId),
        parent: Hash,
    },
}

impl Node {
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
            Node::Root {
                root: _,
                snapshot_id,
            } => {
                sqlx::query("DELETE FROM branches WHERE snapshot_id = ?")
                    .bind(snapshot_id)
                    .execute(&mut *tx)
                    .await?;
            }
            Node::Inner { node, parent } => {
                sqlx::query("DELETE FROM branch_forest WHERE parent = ? AND node = ?")
                    .bind(parent.as_ref())
                    .bind(node.as_ref())
                    .execute(&mut *tx)
                    .await?;
            }
            Node::Leaf { node, parent } => {
                let blob = LeafNode::serialize(&node.0, &node.1);
                sqlx::query("DELETE FROM branch_forest WHERE parent = ? AND node = ?")
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
            Node::Root {
                root,
                snapshot_id: _,
            } => sqlx::query("SELECT 0 FROM branches WHERE root_hash=? LIMIT 1")
                .bind(root.as_ref())
                .fetch_optional(&mut *tx)
                .await?
                .is_some(),
            Node::Inner { node, parent: _ } => {
                sqlx::query("SELECT 0 FROM branch_forest WHERE node=? LIMIT 1")
                    .bind(node.as_ref())
                    .fetch_optional(&mut *tx)
                    .await?
                    .is_some()
            }
            Node::Leaf { node, parent: _ } => {
                let blob = LeafNode::serialize(&node.0, &node.1);
                sqlx::query("SELECT 0 FROM branch_forest WHERE node=? LIMIT 1")
                    .bind(blob)
                    .fetch_optional(&mut *tx)
                    .await?
                    .is_some()
            }
        };

        Ok(!has_parent)
    }

    async fn children(&self, layer: usize, tx: &mut db::Transaction) -> Result<Vec<Node>> {
        match self {
            Node::Root {
                root,
                snapshot_id: _,
            } => sqlx::query("SELECT node, parent FROM branch_forest WHERE parent=?;")
                .bind(root.as_ref())
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
            Node::Inner { node, parent: _ } => {
                sqlx::query("SELECT node, parent FROM branch_forest WHERE parent=?;")
                    .bind(node.as_ref())
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
                    .collect()
            }
            Node::Leaf { node: _, parent: _ } => Ok(Vec::new()),
        }
    }

    fn row_to_inner(row: &SqliteRow) -> Result<Node> {
        Ok(Node::Inner {
            node: column::<Hash>(row, 0)?,
            parent: column::<Hash>(row, 1)?,
        })
    }

    fn row_to_leaf(row: &SqliteRow) -> Result<Node> {
        Ok(Node::Leaf {
            node: deserialize_leaf(row.get(0))?,
            parent: column::<Hash>(row, 1)?,
        })
    }
}
