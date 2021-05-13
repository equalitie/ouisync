mod branch;
mod node;
mod path;

use crate::{
    db,
    error::{Error, Result},
};

/// Number of layers in the tree excluding the layer with root and the layer with leaf nodes.
const INNER_LAYER_COUNT: usize = 3;
const MAX_INNER_NODE_CHILD_COUNT: usize = 256; // = sizeof(u8)

pub use self::branch::Branch;

/// Initializes the index. Creates the required database schema unless already exists.
pub async fn init(pool: &db::Pool) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS branches (
             snapshot_id   INTEGER PRIMARY KEY,
             replica_id    BLOB NOT NULL,
             root_hash   BLOB NOT NULL
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
