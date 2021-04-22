use crate::{
    block::{BlockId, BlockName, BlockVersion},
    crypto::{Hash, SecretKey},
    db,
    error::{Error, Result},
};
use sha3::{Digest, Sha3_256};
use sqlx::{sqlite::SqliteRow, Row};
use std::{convert::TryFrom, slice};

/// Initializes the index. Creates the required database schema unless already exists.
pub async fn init(pool: &db::Pool) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS index_leaves (
             block_name    BLOB NOT NULL,
             block_version BLOB NOT NULL,
             child_tag     BLOB NOT NULL UNIQUE
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
    child_tag: &ChildTag,
) -> Result<()> {
    sqlx::query("INSERT OR REPLACE INTO index_leaves (block_name, block_version, child_tag) VALUES (?, ?, ?)")
        .bind(block_id.name.as_ref())
        .bind(block_id.version.as_ref())
        .bind(child_tag.as_ref())
        .execute(tx)
        .await?;

    Ok(())
}

/// Retrieve `BlockId` of a block with `child_tag`.
pub async fn get(tx: &mut db::Transaction, child_tag: &ChildTag) -> Result<BlockId> {
    match sqlx::query("SELECT block_name, block_version FROM index_leaves WHERE child_tag = ?")
        .bind(child_tag.as_ref())
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

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum BlockKind {
    // First block in a blob. The child tag points to the first block of the containing directory.
    Head = 0,
    // Other than first block in a blob. The child tag points to the first block of the same blob.
    Trunk = 1,
}

/// Encrypted block parent pointer.
pub struct ChildTag(Hash);

impl ChildTag {
    pub fn new(
        secret_key: &SecretKey,
        parent_name: &BlockName,
        block_seq: u32,
        block_kind: BlockKind,
    ) -> Self {
        let key_hash = Sha3_256::digest(secret_key.as_array().as_slice());
        Self(
            Sha3_256::new()
                .chain(key_hash)
                .chain(parent_name.as_ref())
                .chain(block_seq.to_le_bytes())
                .chain(slice::from_ref(&(block_kind as u8)))
                .finalize()
                .into(),
        )
    }
}

impl AsRef<[u8]> for ChildTag {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}
