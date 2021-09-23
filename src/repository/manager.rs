use crate::{
    crypto::Cryptor,
    db,
    error::{Error, Result},
    replica_id::ReplicaId,
};
use serde::{Deserialize, Serialize};
use sqlx::{
    error::BoxDynError,
    sqlite::{Sqlite, SqliteTypeInfo, SqliteValueRef},
    Decode, Type,
};

/// Identifier of a repository unique only within a single replica. To obtain a globally unique
/// identifier, it needs to be paired with a `ReplicaId`.
// TODO: remove the `Default` impl, instead provide a test-only `dummy` constructor.
#[derive(Default, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize, Debug)]
#[repr(transparent)]
pub(crate) struct RepositoryId(pub(super) u32);

impl Type<Sqlite> for RepositoryId {
    fn type_info() -> SqliteTypeInfo {
        u32::type_info()
    }
}

impl<'r> Decode<'r, Sqlite> for RepositoryId {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let num = u32::decode(value)?;
        Ok(Self(num))
    }
}

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
