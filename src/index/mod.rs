mod branch;

use crate::{
    db,
    error::{Error, Result},
};

pub use self::{
    branch::Branch,
};

/// Initializes the index. Creates the required database schema unless already exists.
pub async fn init(pool: &db::Pool) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS branches (
             snapshot_id   INTEGER PRIMARY KEY,
             replica_id    BLOB NOT NULL,
             branch_root   BLOB NOT NULL
         );
         CREATE TABLE IF NOT EXISTS branch_forest (
             /* Parent is a hash calculated from its children */
             parent  BLOB NOT NULL,
             bucket  INTEGER,
             /*
              * Node is a hash calculated from its children (as the `parent` is), or - if this is
              * a leaf layer - node is a blob serialized from the locator hash and BlockId
              */
             node BLOB NOT NULL
         );",
    )
    .execute(pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}
