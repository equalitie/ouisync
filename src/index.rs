use crate::{
    db,
    error::{Error, Result},
};

/// Initializes the index. Creates the required database schema unless already exists.
pub async fn init(pool: &db::Pool) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS index_leaves (
             block_name    BLOB NOT NULL,
             block_version BLOB NOT NULL,
             locator       BLOB NOT NULL UNIQUE
         );
         CREATE TABLE IF NOT EXISTS branches (
             root_block_name    BLOB NOT NULL,
             root_block_version BLOB NOT NULL
         );",
    )
    .execute(pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}

