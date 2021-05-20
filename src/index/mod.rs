// XXX: Until a better module name is found
#![allow(clippy::module_inception)]

mod branch;
mod index;
mod node;
mod path;

pub use self::branch::Branch;
pub use self::index::Index;

use self::node::{InnerData, LeafData, NodeData};
use crate::{
    crypto::Hash,
    db,
    error::{Error, Result},
    replica_id::ReplicaId,
};
use sqlx::{sqlite::SqliteRow, Row};
use std::convert::TryFrom;

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
        "CREATE TABLE IF NOT EXISTS snapshot_roots (
             snapshot_id          INTEGER PRIMARY KEY,
             replica_id           BLOB NOT NULL,

             -- Boolean indicating whether the subtree has been completely downloaded
             -- (excluding blocks)
             is_complete          INTEGER NOT NULL,

             -- XXX: Should be NOT NULL
             missing_blocks_crc   INTEGER,
             missing_blocks_count INTEGER NOT NULL,

             root_hash            BLOB NOT NULL
         );
         CREATE TABLE IF NOT EXISTS snapshot_forest (
             -- Parent is a hash calculated from its children
             parent               BLOB NOT NULL,

             -- XXX: Should be NOT NULL
             bucket               INTEGER,

             -- Boolean indicating whether the subtree has been completely downloaded
             -- (excluding blocks)
             is_complete          INTEGER NOT NULL,

             -- XXX: Should be NOT NULL
             missing_blocks_crc   INTEGER,
             missing_blocks_count INTEGER NOT NULL,

             -- Data is a hash calculated from its children (as the `parent` is), or - if this is
             -- a leaf layer - data is a blob serialized from the locator hash and BlockId
             data                 BLOB NOT NULL
         );",
    )
    .execute(pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}

#[derive(Debug)]
pub struct SnapshotRootRow {
    pub snapshot_id: SnapshotId,
    pub replica_id: ReplicaId,
    pub is_complete: bool,
    pub missing_blocks_crc: Crc,
    pub missing_blocks_count: MissingBlocksCount,
    pub root_hash: Hash,
}

impl TryFrom<&'_ SqliteRow> for SnapshotRootRow {
    type Error = Error;

    fn try_from(row: &SqliteRow) -> Result<Self, Self::Error> {
        Ok(Self {
            snapshot_id: row.get(0),
            replica_id: column::<ReplicaId>(row, 1)?,
            is_complete: row.get::<'_, u16, _>(2) != 0,
            missing_blocks_crc: row.get(3),
            missing_blocks_count: row.get(4),
            root_hash: column::<Hash>(row, 5)?,
        })
    }
}

#[derive(Debug)]
pub struct SnapshotForestRow {
    pub parent: Hash,
    pub bucket: usize,
    pub is_complete: bool,
    pub missing_blocks_crc: Crc,
    pub missing_blocks_count: MissingBlocksCount,
    pub data: NodeData,
}

impl TryFrom<&'_ SqliteRow> for SnapshotForestRow {
    type Error = Error;

    fn try_from(row: &SqliteRow) -> Result<Self, Self::Error> {
        let blob = row.get::<'_, &[u8], _>(5);

        let data = if blob.len() == std::mem::size_of::<Hash>() {
            let hash = Hash::try_from(blob).unwrap();
            NodeData::Inner(InnerData { hash })
        } else {
            NodeData::Leaf(LeafData::deserialize(blob).unwrap())
        };

        Ok(Self {
            parent: column::<Hash>(row, 0)?,
            bucket: row.get::<'_, u32, _>(1) as usize,
            is_complete: row.get::<'_, u16, _>(2) != 0,
            missing_blocks_crc: row.get(3),
            missing_blocks_count: row.get(4),
            data,
        })
    }
}

//// Debug
//async fn fetch_snapshot_roots(tx: &mut db::Transaction) -> Result<Vec<SnapshotRootRow>> {
//    sqlx::query("select * from snapshot_roots")
//        .fetch_all(&mut *tx)
//        .await?
//        .iter()
//        .map(SnapshotRootRow::try_from)
//        .collect()
//}
//
//// Debug
//async fn fetch_snapshot_nodes(tx: &mut db::Transaction) -> Result<Vec<SnapshotForestRow>> {
//    sqlx::query("select * from snapshot_forest")
//        .fetch_all(&mut *tx)
//        .await?
//        .iter()
//        .map(SnapshotForestRow::try_from)
//        .collect()
//}

fn column<'a, T: TryFrom<&'a [u8]>>(
    row: &'a SqliteRow,
    i: usize,
) -> std::result::Result<T, T::Error> {
    let value: &'a [u8] = row.get::<'a>(i);
    let value = T::try_from(value)?;
    Ok(value)
}
