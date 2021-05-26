mod branch;
mod node;
mod path;

pub use self::branch::Branch;

use crate::{
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
const MAX_INNER_NODE_CHILD_COUNT: usize = 256; // = sizeof(u8)

type Crc = u32;
type SnapshotId = u32;
// u64 doesn't seem to implement Decode<'_, Sqlite>
type MissingBlocksCount = i64;

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

        for id in replica_ids {
            let branch = Branch::new(self.pool.clone(), *id).await?;
            branches.insert(*id, branch);
        }

        Ok(())
    }
}

/// Initializes the index. Creates the required database schema unless already exists.
pub async fn init(pool: &db::Pool) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS snapshot_root_nodes (
             snapshot_id          INTEGER PRIMARY KEY,
             replica_id           BLOB NOT NULL,

             -- Hash of the children
             hash                 BLOB NOT NULL,

             -- Boolean indicating whether the subtree has been completely downloaded
             -- (excluding blocks)
             is_complete          INTEGER NOT NULL,

             -- XXX: Should be NOT NULL
             missing_blocks_crc   INTEGER,
             missing_blocks_count INTEGER NOT NULL
         );

         CREATE TABLE IF NOT EXISTS snapshot_inner_nodes (
             -- Parent's `hash`
             parent               BLOB NOT NULL,

             -- Hash of the children
             hash                 BLOB NOT NULL,

             -- Index of this node within its siblings
             -- XXX: Should be NOT NULL
             bucket               INTEGER,

             -- Boolean indicating whether the subtree has been completely downloaded
             -- (excluding blocks)
             is_complete          INTEGER NOT NULL,

             -- XXX: Should be NOT NULL
             missing_blocks_crc   INTEGER,
             missing_blocks_count INTEGER NOT NULL
         );

         CREATE TABLE IF NOT EXISTS snapshot_leaf_nodes (
             -- Parent's `hash`
             parent               BLOB NOT NULL,

             locator              BLOB NOT NULL,
             block_id             BLOB NOT NULL,

             -- Is the block pointed to by this node missing?
             is_block_missing     INTEGER NOT NULL
         );",
    )
    .execute(pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}
