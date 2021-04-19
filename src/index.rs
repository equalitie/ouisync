use crate::{db, error::Error};
use sqlx::SqlitePool;

/// The repository index. Contains information about the repository structure, e.g. which
/// blocks belong to which blobs.
pub struct Index {
    pool: SqlitePool,
}

impl Index {
    pub async fn open(pool: db::Pool) -> Result<Self, Error> {
        todo!()
    }
}
