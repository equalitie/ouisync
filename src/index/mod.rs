mod branch;
mod node;

use crate::{
    block::{BlockId, BlockName, BlockVersion},
    crypto::Hash,
    db,
    error::{Error, Result},
};
use sqlx::{sqlite::SqliteRow, Row};
use std::convert::TryFrom;

/// Number of layers in the tree excluding the layer with root and the layer with leaf nodes.
const INNER_LAYER_COUNT: usize = 3;
const MAX_INNER_NODE_CHILD_COUNT: usize = 256; // = sizeof(u8)

pub use self::branch::Branch;

type LocatorHash = Hash;

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

fn serialize_leaf(locator: &Hash, block_id: &BlockId) -> Vec<u8> {
    locator
        .as_ref()
        .iter()
        .chain(block_id.name.as_ref().iter())
        .chain(block_id.version.as_ref().iter())
        .cloned()
        .collect()
}

fn deserialize_leaf(blob: &[u8]) -> Result<(LocatorHash, BlockId)> {
    let (b1, b2) = blob.split_at(std::mem::size_of::<Hash>());
    let (b2, b3) = b2.split_at(std::mem::size_of::<BlockName>());
    let l = Hash::try_from(b1)?;
    let name = BlockName::try_from(b2)?;
    let version = BlockVersion::try_from(b3)?;
    Ok((l, BlockId { name, version }))
}

fn column<'a, T: TryFrom<&'a [u8]>>(
    row: &'a SqliteRow,
    i: usize,
) -> std::result::Result<T, T::Error> {
    let value: &'a [u8] = row.get::<'a>(i);
    let value = T::try_from(value)?;
    Ok(value)
}
