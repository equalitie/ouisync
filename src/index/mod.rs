mod branch;
mod node;
mod path;

pub use self::{
    branch::Branch,
    node::{InnerNode, InnerNodeMap, LeafNode, LeafNodeSet, RootNode},
};

use crate::{
    block::BlockId,
    db,
    error::{Error, Result},
    ReplicaId,
};
use sqlx::Row;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::Mutex;

/// Number of layers in the tree excluding the layer with root and the layer with leaf nodes.
const INNER_LAYER_COUNT: usize = 3;

type SnapshotId = u32;

#[derive(Clone)]
pub struct Index {
    pub pool: db::Pool,
    pub this_replica_id: ReplicaId,
    branches: Arc<Mutex<HashMap<ReplicaId, Branch>>>,
}

impl Index {
    pub async fn load(pool: db::Pool, this_replica_id: ReplicaId) -> Result<Self> {
        let mut conn = pool.acquire().await?;
        let mut replica_ids = Self::replicas(&mut conn).await?;

        replica_ids.insert(this_replica_id);

        let index = Self {
            pool: pool.clone(),
            this_replica_id,
            branches: Arc::new(Mutex::new(HashMap::new())),
        };

        index.read_branches(&replica_ids).await?;

        Ok(index)
    }

    pub async fn branch(&self, replica_id: &ReplicaId) -> Option<Branch> {
        self.branches.lock().await.get(replica_id).cloned()
    }

    async fn replicas(conn: &mut db::Connection) -> Result<HashSet<ReplicaId>> {
        Ok(
            sqlx::query("SELECT DISTINCT replica_id FROM snapshot_root_nodes")
                .fetch_all(&mut *conn)
                .await?
                .iter()
                .map(|row| row.get(0))
                .collect(),
        )
    }

    async fn read_branches(&self, replica_ids: &HashSet<ReplicaId>) -> Result<()> {
        let mut branches = self.branches.lock().await;
        let mut tx = self.pool.begin().await?;

        for id in replica_ids {
            let branch = Branch::new(&mut tx, *id).await?;
            branches.insert(*id, branch);
        }

        tx.commit().await?;

        Ok(())
    }
}

/// Initializes the index. Creates the required database schema unless already exists.
pub async fn init(pool: &db::Pool) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS snapshot_root_nodes (
             snapshot_id INTEGER PRIMARY KEY,
             replica_id  BLOB NOT NULL,

             -- Hash of the children
             hash        BLOB NOT NULL,

             UNIQUE(replica_id, hash)
         );

         CREATE TABLE IF NOT EXISTS snapshot_inner_nodes (
             -- Parent's `hash`
             parent      BLOB NOT NULL,

             -- Index of this node within its siblings
             bucket      INTEGER NOT NULL,

             -- Hash of the children
             hash        BLOB NOT NULL,

             UNIQUE(parent, bucket)
         );

         CREATE TABLE IF NOT EXISTS snapshot_leaf_nodes (
             -- Parent's `hash`
             parent      BLOB NOT NULL,
             locator     BLOB NOT NULL,
             block_id    BLOB NOT NULL
         );

         -- Prevents creating multiple inner nodes with the same parent and bucket but different
         -- hash.
         CREATE TRIGGER IF NOT EXISTS snapshot_inner_nodes_conflict_check
         BEFORE INSERT ON snapshot_inner_nodes
         WHEN EXISTS(
             SELECT 0
             FROM snapshot_inner_nodes
             WHERE parent = new.parent
               AND bucket = new.bucket
               AND hash <> new.hash
         )
         BEGIN
             SELECT RAISE (ABORT, 'inner node conflict');
         END;",
    )
    .execute(pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}

/// Removes the block if it's orphaned (not referenced by any branch), otherwise does nothing.
/// Returns whether the block was removed.
pub async fn remove_orphaned_block(tx: &mut db::Transaction, id: &BlockId) -> Result<bool> {
    let result = sqlx::query(
        "DELETE FROM blocks
         WHERE id = ? AND (SELECT 0 FROM snapshot_leaf_nodes WHERE block_id = id) IS NULL",
    )
    .bind(id)
    .execute(tx)
    .await?;

    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        block::{self, BLOCK_SIZE},
        crypto::{AuthTag, Cryptor, Hashable},
        locator::Locator,
    };

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_block() {
        let pool = db::Pool::connect(":memory:").await.unwrap();
        init(&pool).await.unwrap();
        block::init(&pool).await.unwrap();

        let mut tx = pool.begin().await.unwrap();

        let cryptor = Cryptor::Null;

        let branch0 = Branch::new(&mut tx, ReplicaId::random()).await.unwrap();
        let branch1 = Branch::new(&mut tx, ReplicaId::random()).await.unwrap();

        let block_id = BlockId::random();
        let buffer = vec![0; BLOCK_SIZE];

        block::write(&mut tx, &block_id, &buffer, &AuthTag::default())
            .await
            .unwrap();

        let locator0 = Locator::Head(rand::random::<u64>().hash(), 0);
        let locator0 = locator0.encode(&cryptor);
        branch0.insert(&mut tx, &block_id, &locator0).await.unwrap();

        let locator1 = Locator::Head(rand::random::<u64>().hash(), 0);
        let locator1 = locator1.encode(&cryptor);
        branch1.insert(&mut tx, &block_id, &locator1).await.unwrap();

        assert!(!remove_orphaned_block(&mut tx, &block_id).await.unwrap());
        assert!(block::exists(&mut tx, &block_id).await.unwrap());

        branch0.remove(&mut tx, &locator0).await.unwrap();

        assert!(!remove_orphaned_block(&mut tx, &block_id).await.unwrap());
        assert!(block::exists(&mut tx, &block_id).await.unwrap());

        branch1.remove(&mut tx, &locator1).await.unwrap();

        assert!(remove_orphaned_block(&mut tx, &block_id).await.unwrap());
        assert!(!block::exists(&mut tx, &block_id).await.unwrap(),);
    }
}
