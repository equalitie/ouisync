use crate::{
    db,
    error::Result,
    index::{column, Branch},
    ReplicaId,
};

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;

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
            sqlx::query("SELECT DISTINCT replica_id FROM snapshot_roots")
                .fetch_all(&mut *conn)
                .await
                .unwrap()
                .iter()
                .map(|row| column::<ReplicaId>(row, 0).unwrap())
                .into_iter()
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
