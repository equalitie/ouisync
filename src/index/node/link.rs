use super::{
    inner::{InnerNode, INNER_LAYER_COUNT},
    leaf::LeafNode,
    root::RootNode,
    summary::Summary,
};
use crate::{crypto::Hash, db, error::Result};
use async_recursion::async_recursion;
use futures_util::TryStreamExt;
use sqlx::Row;

// TODO: this can probably be all done with database triggers.

// We're not repeating enumeration name
// https://rust-lang.github.io/rust-clippy/master/index.html#enum_variant_names
#[allow(clippy::enum_variant_names)]
#[derive(Debug)]
pub enum Link {
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

    async fn remove_single(&self, tx: &mut db::Transaction<'_>) -> Result<()> {
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
                    .bind(&node.block_id)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        Ok(())
    }

    /// Return true if there is nothing that references this node
    async fn is_dangling(&self, tx: &mut db::Transaction<'_>) -> Result<bool> {
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
            .bind(&node.block_id)
            .fetch_optional(&mut *tx)
            .await?
            .is_some(),
        };

        Ok(!has_parent)
    }

    async fn children(&self, layer: usize, tx: &mut db::Transaction<'_>) -> Result<Vec<Link>> {
        match self {
            Link::ToRoot { node: root } => self.inner_children(tx, &root.hash).await,
            Link::ToInner { node, .. } if layer < INNER_LAYER_COUNT => {
                self.inner_children(tx, &node.hash).await
            }
            Link::ToInner { node, .. } => self.leaf_children(tx, &node.hash).await,
            Link::ToLeaf { parent: _, node: _ } => Ok(Vec::new()),
        }
    }

    async fn inner_children(
        &self,
        tx: &mut db::Transaction<'_>,
        parent: &Hash,
    ) -> Result<Vec<Link>> {
        sqlx::query(
            "SELECT parent, hash, is_complete
             FROM snapshot_inner_nodes
             WHERE parent = ?;",
        )
        .bind(parent)
        .map(|row| Link::ToInner {
            parent: row.get(0),
            node: InnerNode {
                hash: row.get(1),
                summary: Summary {
                    is_complete: row.get(2),
                    ..Summary::FULL
                },
            },
        })
        .fetch(tx)
        .try_collect()
        .await
        .map_err(From::from)
    }

    async fn leaf_children(
        &self,
        tx: &mut db::Transaction<'_>,
        parent: &Hash,
    ) -> Result<Vec<Link>> {
        sqlx::query(
            "SELECT parent, locator, block_id
             FROM snapshot_leaf_nodes
             WHERE parent = ?;",
        )
        .bind(parent)
        .map(|row| Link::ToLeaf {
            parent: row.get(0),
            node: LeafNode::present(row.get(1), row.get(2)),
        })
        .fetch(tx)
        .try_collect()
        .await
        .map_err(From::from)
    }
}
