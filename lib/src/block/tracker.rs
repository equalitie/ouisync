use super::BlockId;
use crate::{db, error::Result};
use sqlx::Row;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::{sync::Notify, task};

/// Helper for tracking required missing blocks.
#[derive(Clone)]
pub(crate) struct BlockTracker {
    notify: Arc<Notify>,
    mode: Mode,
}

impl BlockTracker {
    /// Create block tracker with lazy block request mode.
    pub fn lazy() -> Self {
        Self {
            notify: Arc::new(Notify::new()),
            mode: Mode::Lazy,
        }
    }

    /// Create block tracker with greedy block request mode.
    pub fn greedy() -> Self {
        Self {
            notify: Arc::new(Notify::new()),
            mode: Mode::Greedy,
        }
    }

    /// Mark the block with the given id as required.
    ///
    /// # Panics
    ///
    /// Panics if this tracker is in greedy mode.
    pub async fn require(&self, conn: &mut db::Connection, block_id: &BlockId) -> Result<()> {
        assert!(
            matches!(self.mode, Mode::Lazy),
            "`require` can be called only in lazy mode"
        );

        let query_result = sqlx::query(
            "INSERT INTO missing_blocks (block_id, required)
             VALUES (?, 1)
             ON CONFLICT (block_id) DO UPDATE SET required = 1 WHERE required = 0",
        )
        .bind(block_id)
        .execute(&mut *conn)
        .await?;

        if query_result.rows_affected() > 0 {
            self.notify.notify_waiters();
        } else {
        }

        Ok(())
    }

    /// Mark the block request as successfuly completed.
    pub async fn complete(&self, _block_id: &BlockId) -> Result<()> {
        // This is handled by db triggers so there is nothing else to do.
        Ok(())
    }

    pub fn client(&self, db_pool: db::Pool) -> BlockTrackerClient {
        BlockTrackerClient {
            db_pool,
            notify: self.notify.clone(),
            mode: self.mode,
            client_id: next_client_id(),
        }
    }
}

pub(crate) struct BlockTrackerClient {
    db_pool: db::Pool,
    notify: Arc<Notify>,
    mode: Mode,
    client_id: u64,
}

impl BlockTrackerClient {
    /// Offer to request the given block if it is, or will become, required.
    pub async fn offer(&self, block_id: &BlockId) -> Result<()> {
        let mut conn = self.db_pool.acquire().await?;

        let required = match self.mode {
            Mode::Greedy => true,
            Mode::Lazy => false,
        };

        sqlx::query(
            "INSERT INTO missing_blocks (block_id, required)
             VALUES (?, ?)
             ON CONFLICT (block_id) DO UPDATE SET required = ? WHERE required = 0;

             INSERT INTO missing_block_offers (missing_block_id, client_id, accepted)
             VALUES (
                 (SELECT id FROM missing_blocks WHERE block_id = ?),
                 ?,
                 0
             );
            ",
        )
        .bind(block_id)
        .bind(required)
        .bind(required)
        .bind(block_id)
        .bind(db::encode_u64(self.client_id))
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    /// Cancel a previously accepted request so it can be attempted by another client.
    pub async fn cancel(&self, block_id: &BlockId) -> Result<()> {
        let mut conn = self.db_pool.acquire().await?;

        sqlx::query(
            "DELETE FROM missing_block_offers
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

    /// Returns the next required and offered block request. If there is no such request at the
    /// moment this function is called, waits until one appears.
    pub async fn accept(&self) -> Result<BlockId> {
        loop {
            if let Some(block_id) = self.try_accept().await? {
                return Ok(block_id);
            }

            self.notify.notified().await;
        }
    }

    /// Returns the next required and offered block request or `None` if there is no such request
    /// currently.
    /// Note this is still async because it accesses the db, but unlike `next`, the await time is
    /// bounded.
    pub async fn try_accept(&self) -> Result<Option<BlockId>> {
        let mut conn = self.db_pool.acquire().await?;

        let row_id: Option<i64> = sqlx::query(
            "SELECT rowid FROM missing_block_offers
             WHERE client_id = ?
               AND missing_block_id IN
                   (SELECT id FROM missing_blocks WHERE required = 1)
               AND missing_block_id NOT IN
                   (SELECT missing_block_id FROM missing_block_offers WHERE accepted = 1)
             LIMIT 1
             ",
        )
        .bind(db::encode_u64(self.client_id))
        .map(|row| row.get(0))
        .fetch_optional(&mut *conn)
        .await?;

        let row_id = if let Some(row_id) = row_id {
            row_id
        } else {
            return Ok(None);
        };

        let block_id = sqlx::query(
            "UPDATE missing_block_offers SET accepted = 1
             WHERE rowid = ?
             RETURNING (SELECT block_id FROM missing_blocks WHERE id = missing_block_id)",
        )
        .bind(row_id)
        .fetch_one(&mut *conn)
        .await?
        .get(0);

        Ok(Some(block_id))
    }

    /// Close this client by canceling any accepted requests. Normally it's not necessary to call
    /// this as the cleanup happens automatically on drop. It's still useful if one wants to make
    /// sure the cleanup fully completed or to check its result (mostly in tests).
    #[cfg(test)]
    pub async fn close(self) -> Result<()> {
        try_close_client(&self.db_pool, &self.notify, self.client_id).await
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
    sqlx::query("DELETE FROM missing_block_offers WHERE client_id = ?")
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

#[derive(Copy, Clone)]
enum Mode {
    // Blocks are downloaded only when needed.
    Lazy,
    // Blocks are downloaded as soon as we learn about them from the index.
    Greedy,
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
    use tokio::sync::Barrier;

    #[tokio::test(flavor = "multi_thread")]
    async fn lazy_simple() {
        let pool = setup().await;
        let tracker = BlockTracker::lazy();

        let client = tracker.client(pool.clone());

        // Initially no blocks are returned
        assert_eq!(client.try_accept().await.unwrap(), None);

        // Required but not offered blocks are not returned
        let block0 = make_block();
        tracker
            .require(&mut *pool.acquire().await.unwrap(), &block0.id)
            .await
            .unwrap();
        assert_eq!(client.try_accept().await.unwrap(), None);

        // Offered but not required blocks are not returned
        let block1 = make_block();
        client.offer(&block1.id).await.unwrap();
        assert_eq!(client.try_accept().await.unwrap(), None);

        // Required + offered blocks are returned...
        client.offer(&block0.id).await.unwrap();
        assert_eq!(client.try_accept().await.unwrap(), Some(block0.id));

        // ...but only once.
        assert_eq!(client.try_accept().await.unwrap(), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn greedy_simple() {
        let pool = setup().await;
        let tracker = BlockTracker::greedy();

        let client = tracker.client(pool.clone());

        // Initially no blocks are returned
        assert_eq!(client.try_accept().await.unwrap(), None);

        // Offered blocks are returned...
        let block = make_block();
        client.offer(&block.id).await.unwrap();
        assert_eq!(client.try_accept().await.unwrap(), Some(block.id));

        // ...but only once.
        assert_eq!(client.try_accept().await.unwrap(), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lazy_fallback_on_cancel_before_next() {
        let pool = setup().await;
        let tracker = BlockTracker::lazy();

        let client0 = tracker.client(pool.clone());
        let client1 = tracker.client(pool.clone());

        let block = make_block();

        tracker
            .require(&mut pool.acquire().await.unwrap(), &block.id)
            .await
            .unwrap();
        client0.offer(&block.id).await.unwrap();
        client1.offer(&block.id).await.unwrap();

        client0.cancel(&block.id).await.unwrap();

        assert_eq!(client0.try_accept().await.unwrap(), None);
        assert_eq!(client1.try_accept().await.unwrap(), Some(block.id));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn greedy_fallback_on_cancel_before_next() {
        let pool = setup().await;
        let tracker = BlockTracker::greedy();

        let client0 = tracker.client(pool.clone());
        let client1 = tracker.client(pool.clone());

        let block = make_block();

        client0.offer(&block.id).await.unwrap();
        client1.offer(&block.id).await.unwrap();

        client0.cancel(&block.id).await.unwrap();

        assert_eq!(client0.try_accept().await.unwrap(), None);
        assert_eq!(client1.try_accept().await.unwrap(), Some(block.id));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lazy_fallback_on_cancel_after_next() {
        let pool = setup().await;
        let tracker = BlockTracker::lazy();

        let client0 = tracker.client(pool.clone());
        let client1 = tracker.client(pool.clone());

        let block = make_block();

        tracker
            .require(&mut pool.acquire().await.unwrap(), &block.id)
            .await
            .unwrap();
        client0.offer(&block.id).await.unwrap();
        client1.offer(&block.id).await.unwrap();

        assert_eq!(client0.try_accept().await.unwrap(), Some(block.id));
        assert_eq!(client1.try_accept().await.unwrap(), None);

        client0.cancel(&block.id).await.unwrap();

        assert_eq!(client0.try_accept().await.unwrap(), None);
        assert_eq!(client1.try_accept().await.unwrap(), Some(block.id));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn greedy_fallback_on_cancel_after_next() {
        let pool = setup().await;
        let tracker = BlockTracker::greedy();

        let client0 = tracker.client(pool.clone());
        let client1 = tracker.client(pool.clone());

        let block = make_block();

        client0.offer(&block.id).await.unwrap();
        client1.offer(&block.id).await.unwrap();

        assert_eq!(client0.try_accept().await.unwrap(), Some(block.id));
        assert_eq!(client1.try_accept().await.unwrap(), None);

        client0.cancel(&block.id).await.unwrap();

        assert_eq!(client0.try_accept().await.unwrap(), None);
        assert_eq!(client1.try_accept().await.unwrap(), Some(block.id));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lazy_fallback_on_client_close_after_require_before_next() {
        let pool = setup().await;
        let tracker = BlockTracker::lazy();

        let client0 = tracker.client(pool.clone());
        let client1 = tracker.client(pool.clone());

        let block = make_block();

        client0.offer(&block.id).await.unwrap();
        client1.offer(&block.id).await.unwrap();

        tracker
            .require(&mut pool.acquire().await.unwrap(), &block.id)
            .await
            .unwrap();

        client0.close().await.unwrap();

        assert_eq!(client1.try_accept().await.unwrap(), Some(block.id));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lazy_fallback_on_client_close_after_require_after_next() {
        let pool = setup().await;
        let tracker = BlockTracker::lazy();

        let client0 = tracker.client(pool.clone());
        let client1 = tracker.client(pool.clone());

        let block = make_block();

        client0.offer(&block.id).await.unwrap();
        client1.offer(&block.id).await.unwrap();

        tracker
            .require(&mut pool.acquire().await.unwrap(), &block.id)
            .await
            .unwrap();

        assert_eq!(client0.try_accept().await.unwrap(), Some(block.id));
        assert_eq!(client1.try_accept().await.unwrap(), None);

        client0.close().await.unwrap();

        assert_eq!(client1.try_accept().await.unwrap(), Some(block.id));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lazy_fallback_on_client_close_before_request() {
        let pool = setup().await;
        let tracker = BlockTracker::lazy();

        let client0 = tracker.client(pool.clone());
        let client1 = tracker.client(pool.clone());

        let block = make_block();

        client0.offer(&block.id).await.unwrap();
        client1.offer(&block.id).await.unwrap();

        client0.close().await.unwrap();

        tracker
            .require(&mut pool.acquire().await.unwrap(), &block.id)
            .await
            .unwrap();

        assert_eq!(client1.try_accept().await.unwrap(), Some(block.id));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn greedy_fallback_on_client_close_before_next() {
        let pool = setup().await;
        let tracker = BlockTracker::greedy();

        let client0 = tracker.client(pool.clone());
        let client1 = tracker.client(pool.clone());

        let block = make_block();

        client0.offer(&block.id).await.unwrap();
        client1.offer(&block.id).await.unwrap();

        client0.close().await.unwrap();

        assert_eq!(client1.try_accept().await.unwrap(), Some(block.id));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn greedy_fallback_on_client_close_after_next() {
        let pool = setup().await;
        let tracker = BlockTracker::greedy();

        let client0 = tracker.client(pool.clone());
        let client1 = tracker.client(pool.clone());

        let block = make_block();

        client0.offer(&block.id).await.unwrap();
        client1.offer(&block.id).await.unwrap();

        assert_eq!(client0.try_accept().await.unwrap(), Some(block.id));
        assert_eq!(client1.try_accept().await.unwrap(), None);

        client0.close().await.unwrap();

        assert_eq!(client1.try_accept().await.unwrap(), Some(block.id));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn race() {
        let num_clients = 10;

        let pool = setup().await;
        let tracker = BlockTracker::greedy();

        let clients: Vec<_> = (0..num_clients)
            .map(|_| tracker.client(pool.clone()))
            .collect();

        let block = make_block();

        for client in &clients {
            client.offer(&block.id).await.unwrap();
        }

        // Make sure all clients stay alive until we are done so that any acquired requests are not
        // released prematurelly.
        let barrier = Arc::new(Barrier::new(clients.len()));

        // Run the clients in parallel
        let handles = clients.into_iter().map(|client| {
            task::spawn({
                let barrier = barrier.clone();
                async move {
                    let result = client.try_accept().await;
                    barrier.wait().await;
                    result
                }
            })
        });

        let block_ids =
            future::try_join_all(handles.map(|handle| async move { handle.await.unwrap() }))
                .await
                .unwrap();

        // Exactly one client gets the block id
        let mut block_ids = block_ids.into_iter().flatten();
        assert_eq!(block_ids.next(), Some(block.id));
        assert_eq!(block_ids.next(), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn untrack_received_block() {
        let pool = setup().await;
        let tracker = BlockTracker::greedy();

        let client = tracker.client(pool.clone());

        let block = make_block();

        client.offer(&block.id).await.unwrap();

        let nonce = rand::random();
        store::write(
            &mut *pool.acquire().await.unwrap(),
            &block.id,
            &block.content,
            &nonce,
        )
        .await
        .unwrap();

        assert_eq!(client.try_accept().await.unwrap(), None);
    }

    // TODO: test that required and offered blocks are no longer returned when not
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
