use crate::{
    block::BlockId,
    crypto::Hash,
    db,
    error::Result,
    index::LocatorHash,
    index::{
        column,
        deserialize_leaf,
        serialize_leaf,
        INNER_LAYER_COUNT
    },
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

        Ok(Self{snapshot_id, root_hash})
    }

    pub fn as_node(&self) -> Node {
        Node::Root {
            root: self.root_hash,
            snapshot_id: self.snapshot_id,
        }
    }
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
                let blob = serialize_leaf(&node.0, &node.1);
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
                let blob = serialize_leaf(&node.0, &node.1);
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
