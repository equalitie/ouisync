use super::error::Error;
use crate::db;
use sqlx::Row;
use std::path::Path;
use tracing::instrument;

#[instrument(skip_all)]
pub(super) async fn check_integrity(conn: &mut db::Connection) -> Result<bool, Error> {
    // Check orphaned nodes
    let count = db::decode_u64(
        sqlx::query(
            "SELECT COUNT(*)
             FROM snapshot_inner_nodes
             WHERE parent NOT IN (
                 SELECT hash FROM snapshot_root_nodes UNION SELECT hash FROM snapshot_inner_nodes
             )
             UNION
             SELECT COUNT(*)
             FROM snapshot_leaf_nodes
             WHERE parent NOT IN (SELECT hash FROM snapshot_inner_nodes)",
        )
        .fetch_one(&mut *conn)
        .await?
        .get(0),
    );

    if count > 0 {
        tracing::warn!("Found {} orphaned nodes", count);
        return Ok(false);
    }

    // Check orphaned blocks
    let count = db::decode_u64(
        sqlx::query(
            "SELECT COUNT(*)
             FROM blocks
             WHERE id NOT IN (SELECT block_id FROM snapshot_leaf_nodes)",
        )
        .fetch_one(&mut *conn)
        .await?
        .get(0),
    );

    if count > 0 {
        tracing::warn!("Found {} orphaned blocks", count);
        return Ok(false);
    }

    // TODO: Check for root nodes with invalid signatures
    // TODO: Check for child nodes with invalid hashes
    // TODO: Check for blocks with invalid ids

    Ok(true)
}

pub(super) async fn export(conn: &mut db::Connection, dst: &Path) -> Result<(), Error> {
    sqlx::query("VACUUM INTO ?")
        .bind(dst.to_str().ok_or(Error::MalformedData)?)
        .execute(conn)
        .await?;
    Ok(())
}
