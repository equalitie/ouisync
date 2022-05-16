use super::BlockId;
use crate::{db, error::Result};
use sqlx::Row;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::{sync::Notify, task};

/// Helper for tracking required missing blocks.
pub(crate) struct BlockTracker {
    notify: Arc<Notify>,
}

impl BlockTracker {
    pub fn new() -> Self {
        Self {
            notify: Arc::new(Notify::new()),
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
             ON CONFLICT (block_id) DO UPDATE SET requested = 1",
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
                 (SELECT id FROM missing_blocks WHERE block_id = ?),
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
               AND missing_block_id = (SELECT id FROM missing_blocks WHERE block_id = ?)",
        )
        .bind(db::encode_u64(self.client_id))
        .bind(block_id)
        .execute(&mut *conn)
        .await?;

        self.notify.notify_waiters();

        Ok(())
    }

    /// Returns the next requested and accepted block. If there is no such block at the moment this
    /// function is called, waits until one appears.
    pub async fn next(&self) -> Result<BlockId> {
        loop {
            if let Some(block_id) = self.try_next().await? {
                return Ok(block_id);
            }

            self.notify.notified().await;
        }
    }

    /// Returns the next requested and accepted block or `None` if there is no such block currently.
    /// Note this is still async because it accesses the db, but unlike `next`, the await time is
    /// bounded.
    pub async fn try_next(&self) -> Result<Option<BlockId>> {
        let mut conn = self.db_pool.acquire().await?;

        let row = sqlx::query(
            "UPDATE block_requests SET active = 1
             WHERE rowid = (
                 SELECT rowid FROM block_requests
                 WHERE client_id = ?
                   AND missing_block_id IN
                       (SELECT id FROM missing_blocks WHERE requested = 1)
                   AND missing_block_id NOT IN
                       (SELECT missing_block_id FROM block_requests WHERE active = 1)
                 LIMIT 1
             )
             RETURNING missing_block_id
             ",
        )
        .bind(db::encode_u64(self.client_id))
        .fetch_optional(&mut *conn)
        .await?;

        let missing_block_id: i64 = if let Some(row) = row {
            row.get(0)
        } else {
            return Ok(None);
        };

        let block_id = sqlx::query("SELECT block_id FROM missing_blocks WHERE id = ?")
            .bind(missing_block_id)
            .fetch_one(&mut *conn)
            .await?
            .get(0);

        Ok(Some(block_id))
    }

    /// Close this client by rejecting any accepted requests. Normally it's not necessary to call
    /// this as the cleanup happens automatically on drop. It's still useful if one wants to make
    /// sure the cleanup fully completed or to check its result (mostly in tests).
    pub async fn close(self) -> Result<()> {
        try_close_client(&self.db_pool, &self.notify, self.client_id).await
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

impl Drop for BlockTrackerClient {
    fn drop(&mut self) {
        task::spawn(close_client(
            self.db_pool.clone(),
            self.notify.clone(),
            self.client_id,
        ));
    }
}

async fn close_client(db_pool: db::Pool, notify: Arc<Notify>, client_id: u64) {
    if let Err(error) = try_close_client(&db_pool, &notify, client_id).await {
        log::error!(
            "Failed to close BlockTrackerClient(client_id: {}): {:?}",
            client_id,
            error
        );
    }
}

async fn try_close_client(db_pool: &db::Pool, notify: &Notify, client_id: u64) -> Result<()> {
    let mut conn = db_pool.acquire().await?;
    sqlx::query("DELETE FROM block_requests WHERE client_id = ?")
        .bind(db::encode_u64(client_id))
        .execute(&mut *conn)
        .await?;

    notify.notify_waiters();

    Ok(())
}

fn next_client_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::{
        super::{store, BlockData, BLOCK_SIZE},
        *,
    };
    use crate::repository;
    use futures_util::future;
    use rand::Rng;

    #[tokio::test(flavor = "multi_thread")]
    async fn sanity() {
        let pool = setup().await;
        let tracker = BlockTracker::new();

        let requester = tracker.requester();
        let client = tracker.client(pool.clone());

        // Initially no blocks are returned
        assert_eq!(client.try_next().await.unwrap(), None);

        // Requested but not accepted blocks are not returned
        let block0 = make_block();
        requester
            .request(&mut *pool.acquire().await.unwrap(), &block0.id)
            .await
            .unwrap();
        assert_eq!(client.try_next().await.unwrap(), None);

        // Accepted but not requested blocks are not returned
        let block1 = make_block();
        client.accept(&block1.id).await.unwrap();
        assert_eq!(client.try_next().await.unwrap(), None);

        // Requested + accepted blocks are returned
        client.accept(&block0.id).await.unwrap();
        assert_eq!(client.try_next().await.unwrap(), Some(block0.id));

        // Inserted blocks are no longer returned
        let nonce = rand::random();
        store::write(
            &mut *pool.acquire().await.unwrap(),
            &block0.id,
            &block0.content,
            &nonce,
        )
        .await
        .unwrap();
        assert_eq!(client.try_next().await.unwrap(), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fallback_on_reject() {
        let pool = setup().await;
        let tracker = BlockTracker::new();

        let requester = tracker.requester();
        let client0 = tracker.client(pool.clone());
        let client1 = tracker.client(pool.clone());

        let block = make_block();

        requester
            .request(&mut pool.acquire().await.unwrap(), &block.id)
            .await
            .unwrap();
        client0.accept(&block.id).await.unwrap();
        client1.accept(&block.id).await.unwrap();

        client0.reject(&block.id).await.unwrap();

        assert_eq!(client0.try_next().await.unwrap(), None);
        assert_eq!(client1.try_next().await.unwrap(), Some(block.id));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fallback_on_client_close_after_request() {
        let pool = setup().await;
        let tracker = BlockTracker::new();

        let requester = tracker.requester();
        let client0 = tracker.client(pool.clone());
        let client1 = tracker.client(pool.clone());

        let block = make_block();

        client0.accept(&block.id).await.unwrap();
        client1.accept(&block.id).await.unwrap();

        requester
            .request(&mut pool.acquire().await.unwrap(), &block.id)
            .await
            .unwrap();

        client0.close().await.unwrap();

        assert_eq!(client1.try_next().await.unwrap(), Some(block.id));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fallback_on_client_close_before_request() {
        let pool = setup().await;
        let tracker = BlockTracker::new();

        let requester = tracker.requester();
        let client0 = tracker.client(pool.clone());
        let client1 = tracker.client(pool.clone());

        let block = make_block();

        client0.accept(&block.id).await.unwrap();
        client1.accept(&block.id).await.unwrap();

        client0.close().await.unwrap();

        requester
            .request(&mut pool.acquire().await.unwrap(), &block.id)
            .await
            .unwrap();

        assert_eq!(client1.try_next().await.unwrap(), Some(block.id));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn race() {
        let num_clients = 10;

        let pool = setup().await;
        let tracker = BlockTracker::new();

        let requester = tracker.requester();
        let clients: Vec<_> = (0..num_clients)
            .map(|_| tracker.client(pool.clone()))
            .collect();

        let block = make_block();

        for client in &clients {
            client.accept(&block.id).await.unwrap();
        }

        requester
            .request(&mut pool.acquire().await.unwrap(), &block.id)
            .await
            .unwrap();

        // Run the clients in parallel
        let handles = clients
            .into_iter()
            .map(|client| task::spawn(async move { client.try_next().await }));

        let block_ids =
            future::try_join_all(handles.map(|handle| async move { handle.await.unwrap() }))
                .await
                .unwrap();

        // Exactly one client gets the block id
        let mut block_ids = block_ids.into_iter().flatten();
        assert_eq!(block_ids.next(), Some(block.id));
        assert_eq!(block_ids.next(), None);
    }

    // TODO: test that requested and accepted blocks are no longer returned when not
    // referenced

    async fn setup() -> db::Pool {
        repository::create_db(&db::Store::Temporary).await.unwrap()
    }

    fn make_block() -> BlockData {
        let mut content = vec![0; BLOCK_SIZE].into_boxed_slice();
        rand::thread_rng().fill(&mut content[..]);

        BlockData::from(content)
    }
}
