use crate::{
    db,
    error::{Error, Result},
    repository::SecretRepositoryId,
};
use sqlx::Row;

pub(crate) async fn init(pool: &db::Pool) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS metadata (
             name  BLOB NOT NULL PRIMARY KEY,
             value BLOB NOT NULL
         ) WITHOUT ROWID",
    )
    .execute(&*pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}

pub(crate)async fn get_repository_id(db: impl db::Executor<'_>) -> Result<SecretRepositoryId> {
    sqlx::query("SELECT value FROM metadata WHERE name = ?")
        .bind(metadata::ID)
        .map(|row| row.get(0))
        .fetch_optional(db)
        .await?
        .ok_or(Error::EntryNotFound)
}

pub(crate) async fn set_repository_id(db: impl db::Executor<'_>, id: &SecretRepositoryId) -> Result<()> {
    let result =
        sqlx::query("INSERT INTO metadata(name, value) VALUES (?, ?) ON CONFLICT DO NOTHING")
            .bind(metadata::ID)
            .bind(id)
            .execute(db)
            .await?;

    if result.rows_affected() > 0 {
        Ok(())
    } else {
        Err(Error::EntryExists)
    }
}

// Metadata keys
mod metadata {
    pub(super) const ID: &[u8] = b"id";
}
