use crate::{db, error::Error};

/// The repository index. Contains information about the repository structure, e.g. which
/// blocks belong to which blobs.
pub struct Index {
    pool: db::Pool,
}

impl Index {
    /// Opens index using the given database pool.
    pub async fn open(pool: db::Pool) -> Result<Self, Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS block_index (
                 block_name    BLOB NOT NULL,
                 block_version BLOB NOT NULL,
                 child_tag     BLOB NOT NULL,
             )",
        )
        .execute(&pool)
        .await
        .map_err(Error::CreateDbSchema)?;

        Ok(Self { pool })
    }
}
