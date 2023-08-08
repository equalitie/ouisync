use super::error::Error;
use crate::{
    collections::{hash_map, HashMap, HashSet},
    db,
    deadlock::BlockingMutex,
    protocol::{BlockId, SingleBlockPresence},
    sync::uninitialized_watch,
};
use futures_util::{StreamExt, TryStreamExt};
use scoped_task::{self, ScopedJoinHandle};
use sqlx::Row;
use std::{
    collections::{btree_map, BTreeMap},
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::{select, sync::watch, time::sleep};

pub(crate) struct BlockExpirationTracker {
    pool: db::Pool,
    shared: Arc<BlockingMutex<Shared>>,
    watch_tx: uninitialized_watch::Sender<()>,
    expiration_time_tx: watch::Sender<Duration>,
    _task: ScopedJoinHandle<()>,
}

impl BlockExpirationTracker {
    pub async fn enable_expiration(
        pool: db::Pool,
        expiration_time: Duration,
    ) -> Result<Self, Error> {
        let mut shared = Shared {
            blocks_by_id: Default::default(),
            blocks_by_expiration: Default::default(),
        };

        let mut tx = pool.begin_read().await?;

        let mut ids =
            sqlx::query("SELECT block_id FROM snapshot_leaf_nodes WHERE block_presence = ?")
                .bind(SingleBlockPresence::Present)
                .fetch(&mut tx)
                .map_ok(|row| row.get(0));

        let now = SystemTime::now();

        while let Some(id) = ids.next().await {
            shared.handle_block_update(&id?, now);
        }

        let (watch_tx, watch_rx) = uninitialized_watch::channel();
        let shared = Arc::new(BlockingMutex::new(shared));

        let (expiration_time_tx, expiration_time_rx) = watch::channel(expiration_time);

        let _task = scoped_task::spawn({
            let shared = shared.clone();
            let pool = pool.clone();

            async move {
                if let Err(err) = run_task(shared, pool, watch_rx, expiration_time_rx).await {
                    tracing::error!("BlockExpirationTracker task has ended with {err:?}");
                }
            }
        });

        Ok(Self {
            pool,
            shared,
            watch_tx,
            expiration_time_tx,
            _task,
        })
    }

    pub fn handle_block_update(&self, block: &BlockId) {
        // Not inlining these lines to call `SystemTime::now()` only once the `lock` is acquired.
        let mut lock = self.shared.lock().unwrap();
        lock.handle_block_update(block, SystemTime::now());
        drop(lock);
        self.watch_tx.send(()).unwrap_or(());
    }

    pub async fn set_as_missing_if_expired(&self, block: &BlockId) -> Result<(), Error> {
        let mut tx = self.pool.begin_write().await?;

        sqlx::query(
            "UPDATE snapshot_leaf_nodes
             SET block_presence = ?
             WHERE block_id = ? AND block_presence = ?",
        )
        .bind(SingleBlockPresence::Missing)
        .bind(&block)
        .bind(SingleBlockPresence::Expired)
        .execute(&mut tx)
        .await?;

        tx.commit().await?;

        Ok(())
    }

    pub fn handle_block_removed(&self, block: &BlockId) {
        self.shared.lock().unwrap().handle_block_removed(block);
    }

    pub fn set_expiration_time(&self, expiration_time: Duration) {
        self.expiration_time_tx.send(expiration_time).unwrap_or(());
    }

    pub fn block_expiration(&self) -> Duration {
        *self.expiration_time_tx.borrow()
    }
}

// For semantics
type TimeUpdated = SystemTime;

struct Shared {
    // Invariant #1: There exists (`block`, `ts`) in `blocks_by_id` iff there exists `block` in
    // `blocks_by_expiration[ts]`.
    //
    // Invariant #2: `blocks_by_expiration[x]` is never empty for any `x`.
    //
    blocks_by_id: HashMap<BlockId, TimeUpdated>,
    blocks_by_expiration: BTreeMap<TimeUpdated, HashSet<BlockId>>,
}

impl Shared {
    /// Add the `block` into `Self`. If it's already there, remove it and add it back with the new
    /// time stamp.
    fn handle_block_update(&mut self, block: &BlockId, ts: TimeUpdated) {
        // Asserts and unwraps are OK due to the `Shared` invariants defined above.
        match self.blocks_by_id.entry(*block) {
            hash_map::Entry::Occupied(mut entry) => {
                let old_ts = *entry.get();

                if old_ts == ts {
                    return;
                }

                entry.insert(ts);

                let mut entry = match self.blocks_by_expiration.entry(old_ts) {
                    btree_map::Entry::Occupied(entry) => entry,
                    btree_map::Entry::Vacant(_) => unreachable!(),
                };

                assert!(entry.get_mut().remove(block));

                if entry.get().is_empty() {
                    entry.remove();
                }

                assert!(self
                    .blocks_by_expiration
                    .entry(ts)
                    .or_insert_with(Default::default)
                    .insert(*block));
            }
            hash_map::Entry::Vacant(entry) => {
                assert!(self
                    .blocks_by_expiration
                    .entry(ts)
                    .or_insert_with(Default::default)
                    .insert(*block));

                entry.insert(ts);
            }
        }
    }

    /// Remove `block` from `Self`.
    fn handle_block_removed(&mut self, block: &BlockId) {
        // Asserts and unwraps are OK due to the `Shared` invariants defined above.
        let ts = match self.blocks_by_id.entry(*block) {
            hash_map::Entry::Occupied(entry) => entry.remove(),
            hash_map::Entry::Vacant(_) => return,
        };

        let mut entry = match self.blocks_by_expiration.entry(ts) {
            btree_map::Entry::Occupied(entry) => entry,
            btree_map::Entry::Vacant(_) => unreachable!(),
        };

        assert!(entry.get_mut().remove(block));

        if entry.get().is_empty() {
            entry.remove();
        }
    }

    #[cfg(test)]
    fn assert_invariants(&self) {
        // #1 =>
        for (block, ts) in self.blocks_by_id.iter() {
            assert!(self.blocks_by_expiration.get(ts).unwrap().contains(block));
        }
        // #1 <=
        for (ts, blocks) in self.blocks_by_expiration.iter() {
            for block in blocks.iter() {
                assert_eq!(self.blocks_by_id.get(block).unwrap(), ts);
            }
        }
        // Degenerate case
        assert_eq!(
            self.blocks_by_id.is_empty(),
            self.blocks_by_expiration.is_empty()
        );
        // #2
        for blocks in self.blocks_by_expiration.values() {
            assert!(!blocks.is_empty());
        }
    }
}

async fn run_task(
    shared: Arc<BlockingMutex<Shared>>,
    pool: db::Pool,
    mut watch_rx: uninitialized_watch::Receiver<()>,
    mut expiration_time_rx: watch::Receiver<Duration>,
) -> Result<(), Error> {
    loop {
        let expiration_time = *expiration_time_rx.borrow();

        let (ts, block) = {
            let oldest_entry = shared
                .lock()
                .unwrap()
                .blocks_by_expiration
                .first_entry()
                // Unwrap OK due to the invariant #2.
                .map(|e| (*e.key(), *e.get().iter().next().unwrap()));

            match oldest_entry {
                Some((ts, block)) => (ts, block),
                None => {
                    if watch_rx.changed().await.is_err() {
                        return Ok(());
                    }
                    continue;
                }
            }
        };

        let expires_at = ts + expiration_time;
        let now = SystemTime::now();

        if expires_at > now {
            match expires_at.duration_since(now) {
                Ok(duration) => {
                    select! {
                        _ = sleep(duration) => (),
                        _ = expiration_time_rx.changed() => {
                            continue;
                        }
                    }
                }
                Err(_) => (),
            }

            // Check it's still the oldest block.

            let mut lock = shared.lock().unwrap();

            let first_entry = match lock.blocks_by_expiration.first_entry() {
                Some(first_entry) => first_entry,
                None => continue,
            };

            if *first_entry.key() > ts || !first_entry.get().contains(&block) {
                continue;
            }
        }

        let mut tx = pool.begin_write().await?;

        let update_result = sqlx::query(
            "UPDATE snapshot_leaf_nodes
             SET block_presence = ?
             WHERE block_id = ? AND block_presence = ?",
        )
        .bind(SingleBlockPresence::Expired)
        .bind(&block)
        .bind(SingleBlockPresence::Present)
        .execute(&mut tx)
        .await?;

        if update_result.rows_affected() > 0 {
            sqlx::query("DELETE FROM blocks WHERE id = ?")
                .bind(&block)
                .execute(&mut tx)
                .await?;
        }

        // We need to remove the block from `shared` here while the database is locked for writing.
        // If we did it after the commit, it could happen that after the commit but between the
        // removal someone re-adds the block into the database. That would result in there being a
        // block in the database, but not in the BlockExpirationTracker. That is, the block will
        // have been be forgotten by the tracker.
        //
        // Such situation can also happen in this current scenario where we first remove the block
        // from `shared` and then commit if the commit fails. But that case will be detected.
        // TODO: Should we then restart the tracker or do we rely on the fact that if committing
        // into the database fails, then we know the app will be closed? The situation would
        // resolve itself upon restart.
        shared.lock().unwrap().handle_block_removed(&block);

        tx.commit().await?;
    }
}

#[cfg(test)]
mod test {
    use super::super::*;
    use super::*;
    use crate::protocol::{BLOCK_NONCE_SIZE, BLOCK_SIZE};
    use rand::Rng;
    use tempfile::TempDir;

    #[test]
    fn shared_state() {
        let mut shared = Shared {
            blocks_by_id: Default::default(),
            blocks_by_expiration: Default::default(),
        };

        // add once

        let ts = SystemTime::now();
        let block: BlockId = rand::random();

        shared.handle_block_update(&block, ts);

        assert_eq!(*shared.blocks_by_id.get(&block).unwrap(), ts);
        shared.assert_invariants();

        shared.handle_block_removed(&block);

        assert!(shared.blocks_by_id.is_empty());
        shared.assert_invariants();

        // add twice

        shared.handle_block_update(&block, ts);
        shared.handle_block_update(&block, ts);

        assert_eq!(*shared.blocks_by_id.get(&block).unwrap(), ts);
        shared.assert_invariants();

        shared.handle_block_removed(&block);

        assert!(shared.blocks_by_id.is_empty());
        shared.assert_invariants();
    }

    async fn setup() -> (TempDir, Store) {
        let (temp_dir, pool) = db::create_temp().await.unwrap();
        (temp_dir, Store::new(pool))
    }

    #[tokio::test]
    async fn remove_blocks() {
        //
        //  *--------->                 // first block
        //         *--------->          // second block
        //
        //  |      |     |       |
        //  0ms    700ms 1300ms  2200ms
        //
        crate::test_utils::init_log();

        let (_base_dir, store) = setup().await;
        let write_keys = Keypair::random();
        let branch_id = PublicKey::random();

        add_block(&write_keys, &branch_id, &store).await;

        assert_eq!(count_blocks(store.db()).await, 1);

        let tracker =
            BlockExpirationTracker::enable_expiration(store.db().clone(), Duration::from_secs(1))
                .await
                .unwrap();

        sleep(Duration::from_millis(700)).await;

        let block_id = add_block(&write_keys, &branch_id, &store).await;
        tracker.handle_block_update(&block_id);

        assert_eq!(count_blocks(store.db()).await, 2);

        sleep(Duration::from_millis(600) /* -> 1300ms from start */).await;

        assert_eq!(count_blocks(store.db()).await, 1);

        sleep(Duration::from_millis(900) /* -> 2200ms from start */).await;

        assert_eq!(count_blocks(store.db()).await, 0);
    }

    async fn add_block(write_keys: &Keypair, branch_id: &PublicKey, store: &Store) -> BlockId {
        let mut writer = store.begin_write().await.unwrap();

        let block_id: BlockId = rand::random();
        let block_data = random_block_content();
        let block_nonce: [u8; BLOCK_NONCE_SIZE] = rand::random();

        writer
            .write_block(&block_id, &block_data, &block_nonce)
            .await
            .unwrap();

        writer
            .link_block(
                &branch_id,
                &rand::random(),
                &block_id,
                SingleBlockPresence::Present,
                &write_keys,
            )
            .await
            .unwrap();

        writer.commit().await.unwrap();

        block_id
    }

    fn random_block_content() -> Vec<u8> {
        let mut content = vec![0; BLOCK_SIZE];
        rand::thread_rng().fill(&mut content[..]);
        content
    }

    async fn count_blocks(pool: &db::Pool) -> u64 {
        block::count(&mut pool.acquire().await.unwrap())
            .await
            .unwrap()
    }
}
