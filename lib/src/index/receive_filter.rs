use super::node::Summary;
use crate::{crypto::Hash, db, error::Result};
use sqlx::{Connection, Row};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::task;

/// Filter for received nodes to avoid processing a node that doesn't contain any new information
/// compared to the last time we received that same node.
pub(crate) struct ReceiveFilter {
    id: u64,
    db_pool: db::Pool,
}

impl ReceiveFilter {
    pub fn new(db_pool: db::Pool) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);

        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            db_pool,
        }
    }

    pub async fn check(
        &self,
        conn: &mut db::Connection,
        hash: &Hash,
        new_summary: &Summary,
    ) -> Result<bool> {
        let mut tx = conn.begin().await?;

        if let Some((row_id, old_summary)) = load(&mut tx, self.id, hash).await? {
            if old_summary.is_up_to_date_with(new_summary).unwrap_or(false) {
                return Ok(false);
            }

            update(&mut tx, row_id, new_summary).await?;
        } else {
            insert(&mut tx, self.id, hash, new_summary).await?;
        }

        tx.commit().await?;

        Ok(true)
    }
}

impl Drop for ReceiveFilter {
    fn drop(&mut self) {
        task::spawn(remove_all(self.db_pool.clone(), self.id));
    }
}

pub(super) async fn init(conn: &mut db::Connection) -> Result<(), db::Error> {
    sqlx::query(
        "CREATE TEMPORARY TABLE IF NOT EXISTS received_inner_nodes (
             client_id               INTEGER NOT NULL,
             hash                    BLOB NOT NULL,
             missing_blocks_count    INTEGER NOT NULL,
             missing_blocks_checksum INTEGER NOT NULL,

             UNIQUE(client_id, hash)
         );

         CREATE INDEX IF NOT EXISTS index_received_inner_nodes_on_hash
             ON received_inner_nodes (hash);

         -- Delete from received_inner_nodes if the corresponding snapshot_inner_nodes row is
         -- deleted
         CREATE TEMPORARY TRIGGER IF NOT EXISTS received_inner_nodes_delete_on_snapshot_deleted
         AFTER DELETE ON snapshot_inner_nodes
         WHEN NOT EXISTS (SELECT 0 FROM snapshot_inner_nodes WHERE hash = old.hash)
         BEGIN
             DELETE FROM received_inner_nodes WHERE hash = old.hash;
         END;

         -- Delete from received_inner_nodes if the corresponding snapshot_inner_nodes row has no
         -- missing blocks
         CREATE TEMPORARY TRIGGER IF NOT EXISTS
             received_inner_nodes_delete_on_no_blocks_missing_after_insert
         AFTER INSERT ON snapshot_inner_nodes
         WHEN new.missing_blocks_count = 0
         BEGIN
             DELETE FROM received_inner_nodes WHERE hash = new.hash;
         END;

         CREATE TEMPORARY TRIGGER IF NOT EXISTS
             received_inner_nodes_delete_on_no_blocks_missing_after_update
         AFTER UPDATE ON snapshot_inner_nodes
         WHEN new.missing_blocks_count = 0
         BEGIN
             DELETE FROM received_inner_nodes WHERE hash = new.hash;
         END;
         ",
    )
    .execute(conn)
    .await
    .map_err(db::Error::CreateSchema)?;

    Ok(())
}

async fn load(
    conn: &mut db::Connection,
    client_id: u64,
    hash: &Hash,
) -> Result<Option<(u64, Summary)>> {
    let row = sqlx::query(
        "SELECT rowid, missing_blocks_count, missing_blocks_checksum
         FROM received_inner_nodes
         WHERE client_id = ? AND hash = ?",
    )
    .bind(db::encode_u64(client_id))
    .bind(hash)
    .fetch_optional(conn)
    .await?;

    let row = if let Some(row) = row {
        row
    } else {
        return Ok(None);
    };

    let id = db::decode_u64(row.get(0));
    let summary = Summary {
        is_complete: true,
        missing_blocks_count: db::decode_u64(row.get(1)),
        missing_blocks_checksum: db::decode_u64(row.get(2)),
    };

    Ok(Some((id, summary)))
}

async fn insert(
    conn: &mut db::Connection,
    client_id: u64,
    hash: &Hash,
    summary: &Summary,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO received_inner_nodes
         (client_id, hash, missing_blocks_count, missing_blocks_checksum)
         VALUES (?, ?, ?, ?)",
    )
    .bind(db::encode_u64(client_id))
    .bind(hash)
    .bind(db::encode_u64(summary.missing_blocks_count))
    .bind(db::encode_u64(summary.missing_blocks_checksum))
    .execute(conn)
    .await?;

    Ok(())
}

async fn update(conn: &mut db::Connection, row_id: u64, summary: &Summary) -> Result<()> {
    sqlx::query(
        "UPDATE received_inner_nodes
         SET missing_blocks_count = ?, missing_blocks_checksum = ?
         WHERE rowid = ?",
    )
    .bind(db::encode_u64(summary.missing_blocks_count))
    .bind(db::encode_u64(summary.missing_blocks_checksum))
    .bind(db::encode_u64(row_id))
    .execute(conn)
    .await?;

    Ok(())
}

async fn remove_all(pool: db::Pool, client_id: u64) {
    if let Err(error) = try_remove_all(pool, client_id).await {
        log::error!(
            "Failed to cleanup ReceiveFilter(client_id: {}): {:?}",
            client_id,
            error
        );
    }
}

async fn try_remove_all(pool: db::Pool, client_id: u64) -> Result<()> {
    let mut conn = pool.acquire().await?;
    sqlx::query("DELETE FROM received_inner_nodes WHERE client_id = ?")
        .bind(db::encode_u64(client_id))
        .execute(&mut *conn)
        .await?;
    Ok(())
}
