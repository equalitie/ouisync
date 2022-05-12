use super::BlockId;
use crate::{db, error::Result};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::sync::Notify;

/// Helper for tracking required missing blocks.
pub(crate) struct BlockTracker {
    notify: Arc<Notify>,
    client_id: u64,
}

impl BlockTracker {
    pub fn new() -> Self {
        Self {
            notify: Arc::new(Notify::new()),
            client_id: next_client_id(),
        }
    }

    pub fn requester(&self) -> BlockTrackerRequester {
        BlockTrackerRequester {
            notify: self.notify.clone(),
        }
    }

    pub fn client(&self, db_pool: db::Pool) -> BlockTrackerClient {
        BlockTrackerClient {
            db_pool,
            notify: self.notify.clone(),
            client_id: next_client_id(),
        }
    }
}

pub(crate) struct BlockTrackerRequester {
    notify: Arc<Notify>,
}

impl BlockTrackerRequester {
    /// Request a block with the given id.
    pub async fn request(&self, conn: &mut db::Connection, block_id: &BlockId) -> Result<()> {
        sqlx::query(
            "INSERT INTO missing_blocks (block_id, requested)
             VALUES (?, 1)
             ON CONFLICT (block_id) DO UPDATE requested = 1",
        )
        .bind(block_id)
        .execute(conn)
        .await?;

        self.notify.notify_waiters();

        Ok(())
    }
}

pub(crate) struct BlockTrackerClient {
    db_pool: db::Pool,
    notify: Arc<Notify>,
    client_id: u64,
}

impl BlockTrackerClient {
    /// Accept a (existing or future) request for a block.
    pub async fn accept(&self, block_id: &BlockId) -> Result<()> {
        let mut conn = self.db_pool.acquire().await?;

        sqlx::query(
            "INSERT INTO missing_blocks (block_id, requested)
             VALUES (?, 0)
             ON CONFLICT DO NOTHING;

             INSERT INTO block_requests (missing_block_id, client_id, active)
             VALUES (
                 SELECT rowid FROM missing_blocks WHERE block_id = ?,
                 ?,
                 0
             );
            ",
        )
        .bind(block_id)
        .bind(block_id)
        .bind(db::encode_u64(self.client_id))
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    /// Reject a previously accepted block request. Call this after receiving a failed block
    /// response.
    pub async fn reject(&self, block_id: &BlockId) -> Result<()> {
        let mut conn = self.db_pool.acquire().await?;

        sqlx::query(
            "DELETE FROM block_requests
             WHERE client_id = ?
               AND missing_block_id = (SELECT rowid FROM missing_blocks WHERE block_id = ?)",
        )
        .bind(db::encode_u64(self.client_id))
        .bind(block_id)
        .execute(&mut *conn)
        .await?;

        self.notify.notify_waiters();

        Ok(())
    }

    /// Wait for the next requested block accepted by this client.
    pub async fn requested(&self) -> Result<BlockId> {
        todo!()
    }
}

impl Clone for BlockTrackerClient {
    fn clone(&self) -> Self {
        Self {
            db_pool: self.db_pool.clone(),
            notify: self.notify.clone(),
            client_id: next_client_id(),
        }
    }
}

fn next_client_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
