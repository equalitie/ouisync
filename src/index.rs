use crate::{
    block::{BlockId, BlockName, BlockVersion},
    crypto::Hash,
    db,
    error::{Error, Result},
};
use sqlx::{sqlite::SqliteRow, Row};
use std::convert::TryFrom;

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

/// Insert the root block into the index
pub async fn insert_root(tx: &mut db::Transaction, block_id: &BlockId) -> Result<()> {
    // NOTE: currently only one branch is supported
    sqlx::query(
        "DELETE FROM branches;
         INSERT INTO branches (root_block_name, root_block_version) VALUES (?, ?);",
    )
    .bind(block_id.name.as_ref())
    .bind(block_id.version.as_ref())
    .execute(tx)
    .await?;

    Ok(())
}

/// Get the root block from the index.
pub async fn get_root(tx: &mut db::Transaction) -> Result<BlockId> {
    // NOTE: currently only one branch is supported
    match sqlx::query("SELECT root_block_name, root_block_version FROM branches LIMIT 1")
        .fetch_optional(tx)
        .await?
    {
        Some(row) => get_block_id(&row),
        None => Err(Error::BlockIdNotFound),
    }
}

/// Insert a new block into the index.
// TODO: take `Transaction` instead of `Pool`
pub async fn insert(
    tx: &mut db::Transaction,
    block_id: &BlockId,
    encoded_locator: &Hash,
) -> Result<()> {
    sqlx::query(
        "INSERT OR REPLACE INTO index_leaves (block_name, block_version, locator) VALUES (?, ?, ?)",
    )
    .bind(block_id.name.as_ref())
    .bind(block_id.version.as_ref())
    .bind(encoded_locator.as_ref())
    .execute(tx)
    .await?;

    Ok(())
}

/// Retrieve `BlockId` of a block with the given encoded `Locator`.
pub async fn get(tx: &mut db::Transaction, encoded_locator: &Hash) -> Result<BlockId> {
    match sqlx::query("SELECT block_name, block_version FROM index_leaves WHERE locator = ?")
        .bind(encoded_locator.as_ref())
        .fetch_optional(tx)
        .await?
    {
        Some(row) => get_block_id(&row),
        None => Err(Error::BlockIdNotFound),
    }
}

fn get_block_id(row: &SqliteRow) -> Result<BlockId> {
    let name: &[u8] = row.get(0);
    let name = BlockName::try_from(name)?;

    let version: &[u8] = row.get(1);
    let version = BlockVersion::try_from(version)?;

    Ok(BlockId { name, version })
}
