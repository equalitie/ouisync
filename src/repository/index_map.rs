use super::manager::RepositoryId;
use crate::{error::Result, index::Index, replica_id::ReplicaId};
use futures_util::{future, TryStreamExt};
use std::collections::HashMap;

/// Map of repository indices.
pub(crate) struct IndexMap {
    this_replica_id: ReplicaId,
    indices: HashMap<RepositoryId, Index>,
    ids: HashMap<String, RepositoryId>,
}

impl IndexMap {
    // pub async fn new(main_pool: db::Pool, this_replica_id: ReplicaId) -> Result<Self> {
    //     let mut indices = HashMap::new();
    //     let mut ids = HashMap::new();

    //     sqlx::query("SELECT rowid, name, db_path FROM repositories")
    //         .fetch(&main_pool)
    //         .err_into()
    //         .and_then(|row| async move {
    //             todo!()
    //             // let store = row.get(2).parse();
    //             // let repo_pool =
    //             // let index = Index::
    //         })
    //         .try_for_each(|(id, name, index)| {
    //             indices.insert(id, index);
    //             ids.insert(name, id);

    //             future::ready(Ok(()))
    //         })
    //         .await?;

    //     Self {
    //         this_replica_id,
    //         indices,
    //         ids,
    //     }
    // }
}
