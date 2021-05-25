// XXX: Until a better module name is found
#![allow(clippy::module_inception)]

mod branch;
mod index;
mod node;
mod path;

pub use self::branch::Branch;
pub use self::index::Index;

use crate::{
    db,
    error::{Error, Result},
};

/// Number of layers in the tree excluding the layer with root and the layer with leaf nodes.
const INNER_LAYER_COUNT: usize = 3;
const MAX_INNER_NODE_CHILD_COUNT: usize = 256; // = sizeof(u8)

type Crc = u32;
type SnapshotId = u32;
// u64 doesn't seem to implement Decode<'_, Sqlite>
type MissingBlocksCount = i64;

/// Initializes the index. Creates the required database schema unless already exists.
pub async fn init(pool: &db::Pool) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS snapshot_root_nodes (
             snapshot_id          INTEGER PRIMARY KEY,
             replica_id           BLOB NOT NULL,

             -- Hash of the children
             hash                 BLOB NOT NULL,

             -- Boolean indicating whether the subtree has been completely downloaded
             -- (excluding blocks)
             is_complete          INTEGER NOT NULL,

             -- XXX: Should be NOT NULL
             missing_blocks_crc   INTEGER,
             missing_blocks_count INTEGER NOT NULL
         );

         CREATE TABLE IF NOT EXISTS snapshot_inner_nodes (
             -- Parent's `hash`
             parent               BLOB NOT NULL,

             -- Hash of the children
             hash                 BLOB NOT NULL,

             -- Index of this node within its siblings
             -- XXX: Should be NOT NULL
             bucket               INTEGER,

             -- Boolean indicating whether the subtree has been completely downloaded
             -- (excluding blocks)
             is_complete          INTEGER NOT NULL,

             -- XXX: Should be NOT NULL
             missing_blocks_crc   INTEGER,
             missing_blocks_count INTEGER NOT NULL
         );

         CREATE TABLE IF NOT EXISTS snapshot_leaf_nodes (
             -- Parent's `hash`
             parent               BLOB NOT NULL,

             locator              BLOB NOT NULL,
             block_id             BLOB NOT NULL,

             -- Is the block pointed to by this node missing?
             is_block_missing     INTEGER NOT NULL
         );",
    )
    .execute(pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}
