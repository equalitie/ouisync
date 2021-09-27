use crate::{
    crypto::Cryptor,
    db,
    error::{Error, Result},
    replica_id::ReplicaId,
};

pub(crate) struct RepositoryManager {}

impl RepositoryManager {
    pub fn new(pool: db::Pool, this_replica_id: ReplicaId, cryptor: Cryptor) -> Self {
        todo!()
    }
}

/// Initialize the database schema for the repository manager.
pub(crate) async fn init(pool: &db::Pool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS repositories (
             name    TEXT NOT NULL UNIQUE,
             db_path TEXT NOT NULL,
         )",
    )
    .execute(pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}
