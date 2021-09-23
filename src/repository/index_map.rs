use super::manager::RepositoryId;
use crate::{
    db,
    error::{Error, Result},
    index::Index,
    replica_id::ReplicaId,
};
use futures_util::{future, TryStreamExt};
use sqlx::Row;
use std::collections::HashMap;

/// Map of repository indices.
pub(crate) struct IndexMap {
    this_replica_id: ReplicaId,
    indices: HashMap<RepositoryId, Index>,
    ids: HashMap<String, RepositoryId>,
}

impl IndexMap {
    pub async fn new(main_pool: db::Pool, this_replica_id: ReplicaId) -> Result<Self> {
        let mut indices = HashMap::new();
        let mut ids = HashMap::new();

        sqlx::query("SELECT rowid, name, db_path FROM repositories")
            .fetch(&main_pool)
            .err_into()
            .and_then(|row| async move {
                // `unwrap` is OK because the error type here is `Infallible`.
                // TODO: replace with `into_ok` when stabilized.
                let store = row.get::<&str, _>(2).parse().unwrap();
                let pool = db::init(store).await?;
                let index = Index::load(pool, this_replica_id).await?;

                Ok::<_, Error>((row.get(0), row.get(1), index))
            })
            .try_for_each(|(id, name, index)| {
                indices.insert(id, index);
                ids.insert(name, id);

                future::ready(Ok(()))
            })
            .await?;

        Ok(Self {
            this_replica_id,
            indices,
            ids,
        })
    }
}
