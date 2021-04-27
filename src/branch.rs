use crate::{
    block::{BlockId, BlockName, BlockVersion},
    crypto::Hash,
    db,
    error::{Error, Result},
};
use sha3::{Digest, Sha3_256};
use sqlx::{sqlite::SqliteRow, Row};
use std::convert::TryFrom;

pub type SnapshotId = Hash;

pub struct Branch {
    /// Changes on each branch mutation
    _snapshot_id: SnapshotId,
}

impl Branch {
    pub fn new() -> Self {
        Self {
            /// Dummy for now.
            _snapshot_id: Sha3_256::digest(&[0; 0]).into(),
        }
    }

    /// Insert the root block into the index
    pub async fn insert_root(&self, tx: &mut db::Transaction, block_id: &BlockId) -> Result<()> {
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
    pub async fn get_root(&self, tx: &mut db::Transaction) -> Result<BlockId> {
        // NOTE: currently only one branch is supported
        match sqlx::query("SELECT root_block_name, root_block_version FROM branches LIMIT 1")
            .fetch_optional(tx)
            .await?
        {
            Some(row) => Self::get_block_id(&row),
            None => Err(Error::BlockIdNotFound),
        }
    }
    
    /// Insert a new block into the index.
    // TODO: take `Transaction` instead of `Pool`
    pub async fn insert(
        &self,
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
    pub async fn get(&self, tx: &mut db::Transaction, encoded_locator: &Hash) -> Result<BlockId> {
        match sqlx::query("SELECT block_name, block_version FROM index_leaves WHERE locator = ?")
            .bind(encoded_locator.as_ref())
            .fetch_optional(tx)
            .await?
        {
            Some(row) => Self::get_block_id(&row),
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
}

//#[cfg(test)]
//mod tests {
//    use super::*;
//
//    #[tokio::test(flavor = "multi_thread")]
//    async fn empty_blob() {
//        let pool = init_db().await;
//    }
//
//    async fn init_db() -> db::Pool {
//        let pool = db::Pool::connect(":memory:").await.unwrap();
//        index::init(&pool).await.unwrap();
//        pool
//    }
//}
