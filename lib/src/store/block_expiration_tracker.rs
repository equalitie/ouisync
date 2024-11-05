use super::{block, error::Error, index, leaf_node, root_node};
use crate::{
    block_tracker::BlockTracker as BlockDownloadTracker,
    collections::{hash_map, HashMap, HashSet},
    crypto::sign::PublicKey,
    db,
    future::TryStreamExt as _,
    protocol::{BlockId, SingleBlockPresence},
    sync::{broadcast_hash_set, uninitialized_watch},
};
use deadlock::BlockingMutex;
use futures_util::{StreamExt, TryStreamExt};
use scoped_task::{self, ScopedJoinHandle};
use sqlx::Row;
use std::{
    collections::{btree_map, BTreeMap},
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::{select, sync::watch, time::sleep};
use tracing::{Instrument, Span};

/// This structure keeps track (in memory) of which blocks are currently in the database. To each
/// one block it assigns a time when it should expire to free space. Once a block is expired, it is
/// removed from the DB and its state is changed from "Present" to "Expired" in the index.
///
/// One tricky thing in implementing this structure properly is to ensure the following invariant
/// holds:
///
/// "If a block is present in the database, then BlockExpirationTracker must know about it"
///
/// If this property did not hold, we could have a block that never expires.
///
/// Note that the opposite property would also be desirable, but is not strictly necessary because
/// if a block is in the expiration tracker but not in the database, it will eventually expire
/// which will then be a noop.
///
/// The above invariant can be broken in two ways:
///
/// 1. The "remove" and "add" operation get reordered due to a race.
/// 2. The removal of a block from the expiration tracker is done, but removal from the database
///    fails.
///
/// For more information about the first case, see the `expiration_race` test below. To ensure it
/// doesn't happen we assign to each "add" and "remove" DB operation a db::TransactionId and then
/// require that no "remove" operation swaps order with an "add" operation.
///
/// The second case is enforced by requiring db::CommitId when invoking the "remove" operation to
/// ensure the block has already been successfully removed from the DB.
pub(crate) struct BlockExpirationTracker {
    shared: Arc<BlockingMutex<Shared>>,
    watch_tx: uninitialized_watch::Sender<()>,
    block_expiration_tx: watch::Sender<Duration>,
    _task: ScopedJoinHandle<()>,
}

impl BlockExpirationTracker {
    pub(super) async fn enable_expiration(
        pool: db::Pool,
        block_expiration: Duration,
        block_download_tracker: BlockDownloadTracker,
        client_reload_index_tx: broadcast_hash_set::Sender<PublicKey>,
    ) -> Result<Self, Error> {
        let now = SystemTime::now();

        let mut shared = Shared {
            blocks_by_id: Default::default(),
            blocks_by_expiration: Default::default(),
            last_block_expiration_time: Some(now),
            to_missing_if_expired: Default::default(),
        };

        // TODO: Consider moving this initialization to `run_task` so that this function doesn't
        // have to be async.
        let mut tx = pool.begin_read().await?;

        let mut ids =
            sqlx::query("SELECT block_id FROM snapshot_leaf_nodes WHERE block_presence = ?")
                .bind(SingleBlockPresence::Present)
                .fetch(&mut tx)
                .map_ok(|row| row.get(0));

        while let Some(id) = ids.next().await {
            shared.insert_block(&id?, now);
        }

        let (watch_tx, watch_rx) = uninitialized_watch::channel();
        let shared = Arc::new(BlockingMutex::new(shared));

        let (block_expiration_tx, block_expiration_rx) = watch::channel(block_expiration);

        let _task = scoped_task::spawn({
            let shared = shared.clone();
            let span = Span::current();

            async move {
                if let Err(err) = run_task(
                    shared,
                    pool,
                    watch_rx,
                    block_expiration_rx,
                    block_download_tracker,
                    client_reload_index_tx,
                )
                .await
                {
                    tracing::error!("BlockExpirationTracker task has ended with {err:?}");
                }
            }
            .instrument(span)
        });

        Ok(Self {
            shared,
            watch_tx,
            block_expiration_tx,
            _task,
        })
    }

    pub fn handle_block_update(&self, block_id: &BlockId, is_missing: bool) {
        // Not inlining these lines to call `SystemTime::now()` only once the `lock` is acquired.
        let mut lock = self.shared.lock().unwrap();
        lock.insert_block(block_id, SystemTime::now());
        if is_missing {
            lock.to_missing_if_expired.insert(*block_id);
        }
        drop(lock);
        self.watch_tx.send(()).unwrap_or(());
    }

    pub fn set_block_expiration(&self, expiration_time: Duration) {
        self.block_expiration_tx.send(expiration_time).unwrap_or(());
    }

    pub fn block_expiration(&self) -> Duration {
        *self.block_expiration_tx.borrow()
    }

    pub fn begin_untrack_blocks(&self) -> UntrackTransaction {
        UntrackTransaction {
            shared: self.shared.clone(),
            block_ids: Default::default(),
        }
    }

    /// Returns the time when the last tracked block expired or was removed. Returns `None` if at
    /// least one block is still unexpired.
    pub fn last_block_expiration_time(&self) -> Option<SystemTime> {
        self.shared.lock().unwrap().last_block_expiration_time
    }

    #[cfg(test)]
    pub fn has_block(&self, block: &BlockId) -> bool {
        self.shared.lock().unwrap().blocks_by_id.contains_key(block)
    }
}

/// This struct is used to stop tracking blocks inside the BlockExpirationTracker. The reason for
/// "untracking" blocks in a transaction - as opposed to just removing blocks through a simple
/// BlockExpirationTracker method - is that we only want to actually untrack a block once it's been
/// removed from the main DB and the removing DB transaction has been committed successfully.
///
/// Not doing so could result in untracking blocks while those blocks still remain in the main DB,
/// therefore they would never expire.
pub(crate) struct UntrackTransaction {
    shared: Arc<BlockingMutex<Shared>>,
    block_ids: HashSet<BlockId>,
}

impl UntrackTransaction {
    pub fn untrack(&mut self, block_id: BlockId) {
        self.block_ids.insert(block_id);
    }

    pub fn commit(self) {
        if self.block_ids.is_empty() {
            return;
        }

        let mut shared = self.shared.lock().unwrap();

        for block_id in &self.block_ids {
            shared.remove_block(block_id);
        }
    }
}

// For semantics
type TimeUpdated = SystemTime;

struct Shared {
    // Invariant #1: There exists `(block, time_updated)` in `blocks_by_id` *iff*
    // there exists `block` in `blocks_by_expiration[time_updated]`.
    //
    // Invariant #2: `blocks_by_expiration[x]` is never empty for any `x`.
    //
    blocks_by_id: HashMap<BlockId, TimeUpdated>,
    blocks_by_expiration: BTreeMap<TimeUpdated, HashSet<BlockId>>,

    // Time since the last block has been removed from the tracker or `None` if there are still some
    // blocks.
    last_block_expiration_time: Option<SystemTime>,

    to_missing_if_expired: HashSet<BlockId>,
}

impl Shared {
    /// Add the `block` into `Self`. If it's already there, remove it and add it back with the new
    /// time stamp.
    fn insert_block(&mut self, block: &BlockId, ts: TimeUpdated) {
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
                    .or_default()
                    .insert(*block));
            }
            hash_map::Entry::Vacant(entry) => {
                assert!(self
                    .blocks_by_expiration
                    .entry(ts)
                    .or_default()
                    .insert(*block));

                entry.insert(ts);

                self.last_block_expiration_time = None;
            }
        }
    }

    fn remove_block(&mut self, block: &BlockId) {
        // Asserts and unwraps are OK due to the `Shared` invariants defined above.
        let Some(time_updated) = self.blocks_by_id.remove(block) else {
            return;
        };

        let mut entry = match self.blocks_by_expiration.entry(time_updated) {
            btree_map::Entry::Occupied(entry) => entry,
            btree_map::Entry::Vacant(_) => unreachable!(),
        };

        assert!(entry.get_mut().remove(block));

        if entry.get().is_empty() {
            entry.remove();
        }

        if self.blocks_by_id.is_empty() {
            self.last_block_expiration_time = Some(SystemTime::now());
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
    block_download_tracker: BlockDownloadTracker,
    client_reload_index_tx: broadcast_hash_set::Sender<PublicKey>,
) -> Result<(), Error> {
    loop {
        let expiration_time = *expiration_time_rx.borrow();

        let (ts, block_id) = {
            enum Enum {
                OldestEntry(Option<(TimeUpdated, BlockId)>),
                ToMissing(HashSet<BlockId>),
            }

            let action = {
                let mut lock = shared.lock().unwrap();

                if !lock.to_missing_if_expired.is_empty() {
                    Enum::ToMissing(std::mem::take(&mut lock.to_missing_if_expired))
                } else {
                    Enum::OldestEntry(
                        lock.blocks_by_expiration
                            .first_entry()
                            // Unwrap OK due to the invariant #2.
                            .map(|e| (*e.key(), *e.get().iter().next().unwrap())),
                    )
                }
            };

            match action {
                Enum::OldestEntry(Some((time_updated, block_id))) => (time_updated, block_id),
                Enum::OldestEntry(None) => {
                    if watch_rx.changed().await.is_err() {
                        return Ok(());
                    }
                    continue;
                }
                Enum::ToMissing(to_missing_if_expired) => {
                    set_as_missing_if_expired(
                        &pool,
                        to_missing_if_expired,
                        &block_download_tracker,
                        &client_reload_index_tx,
                    )
                    .await?;
                    continue;
                }
            }
        };

        let expires_at = ts + expiration_time;
        let now = SystemTime::now();

        if expires_at > now {
            if let Ok(duration) = expires_at.duration_since(now) {
                select! {
                    _ = sleep(duration) => (),
                    _ = expiration_time_rx.changed() => {
                        continue;
                    }
                    _ = watch_rx.changed() => {
                        continue;
                    }
                }
            }

            // Check it's still the oldest block.

            let mut lock = shared.lock().unwrap();

            let first_entry = match lock.blocks_by_expiration.first_entry() {
                Some(first_entry) => first_entry,
                None => continue,
            };

            if *first_entry.key() > ts || !first_entry.get().contains(&block_id) {
                continue;
            }
        }

        let mut tx = pool.begin_write().await?;

        if !leaf_node::set_expired_if_present(&mut tx, &block_id).await? {
            return Ok(());
        }

        block::remove(&mut tx, &block_id).await?;

        tx.commit().await?;

        shared.lock().unwrap().remove_block(&block_id);
    }
}

async fn set_as_missing_if_expired(
    pool: &db::Pool,
    block_ids: HashSet<BlockId>,
    block_download_tracker: &BlockDownloadTracker,
    client_reload_index_tx: &broadcast_hash_set::Sender<PublicKey>,
) -> Result<(), Error> {
    let mut tx = pool.begin_write().await?;

    // Branches where we have newly missing blocks. We need to tell the client to reload indices
    // for these branches from peers.
    let mut branches: HashSet<PublicKey> = HashSet::default();

    for block_id in &block_ids {
        let parent_hashes: Vec<_> = leaf_node::set_missing_if_expired(&mut tx, block_id)
            .try_collect()
            .await?;

        if parent_hashes.is_empty() {
            continue;
        }

        block_download_tracker.require(*block_id);

        for (hash, _state) in index::update_summaries(&mut tx, parent_hashes).await? {
            root_node::load_writer_ids_by_hash(&mut tx, &hash)
                .try_collect_into(&mut branches)
                .await?;
        }
    }

    tx.commit().await?;

    for branch_id in branches {
        // TODO: Throttle these messages.
        client_reload_index_tx.insert(&branch_id);
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::super::*;
    use super::*;
    use crate::crypto::sign::Keypair;
    use crate::protocol::Block;
    use futures_util::future;
    use rand::distributions::Standard;
    use rand::seq::SliceRandom;
    use rand::Rng;
    use tempfile::TempDir;
    use tokio::task;

    #[test]
    fn shared_state() {
        let mut shared = Shared {
            blocks_by_id: Default::default(),
            blocks_by_expiration: Default::default(),
            last_block_expiration_time: None,
            to_missing_if_expired: Default::default(),
        };

        // add once

        let ts = SystemTime::now();
        let block: BlockId = rand::random();

        shared.insert_block(&block, ts);

        assert_eq!(*shared.blocks_by_id.get(&block).unwrap(), ts,);
        shared.assert_invariants();

        shared.remove_block(&block);

        assert!(shared.blocks_by_id.is_empty());
        shared.assert_invariants();

        // add twice

        shared.insert_block(&block, ts);
        shared.insert_block(&block, ts);

        assert_eq!(*shared.blocks_by_id.get(&block).unwrap(), ts);
        shared.assert_invariants();

        shared.remove_block(&block);

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

        add_block(rand::random(), &write_keys, &branch_id, &store).await;

        assert_eq!(count_blocks(store.db()).await, 1);

        let tracker = BlockExpirationTracker::enable_expiration(
            store.db().clone(),
            Duration::from_secs(1),
            BlockDownloadTracker::new(),
            broadcast_hash_set::channel().0,
        )
        .await
        .unwrap();

        sleep(Duration::from_millis(700)).await;

        let block_id = add_block(rand::random(), &write_keys, &branch_id, &store).await;
        tracker.handle_block_update(&block_id, false);

        assert_eq!(count_blocks(store.db()).await, 2);

        sleep(Duration::from_millis(600) /* -> 1300ms from start */).await;

        assert_eq!(count_blocks(store.db()).await, 1);

        sleep(Duration::from_millis(900) /* -> 2200ms from start */).await;

        assert_eq!(count_blocks(store.db()).await, 0);
    }

    /// This test checks the condition that "if there is a block in the main database, then it must
    /// be in the expiration tracker" in the presence of concurrent block insertions and removals.
    #[tokio::test]
    async fn atomicity() {
        let count = 100;

        let (_base_dir, store) = setup().await;
        let mut rng = rand::thread_rng();

        store
            .set_block_expiration(
                // Setting expiration time to something big, we don't care about blocks actually
                // expiring in this test.
                Some(Duration::from_secs(60 * 60 /* one hour */)),
                BlockDownloadTracker::new(),
            )
            .await
            .unwrap();

        let write_keys = Arc::new(Keypair::random());
        let branch_id = PublicKey::random();

        let store = store.clone();
        let write_keys = write_keys.clone();

        // Create blocks
        let blocks: Vec<Block> = (&mut rng).sample_iter(Standard).take(count).collect();

        let mut tx = store.begin_write().await.unwrap();
        let mut changeset = Changeset::new();

        for block in &blocks {
            changeset.link_block(rand::random(), block.id, SingleBlockPresence::Present);
        }

        changeset
            .apply(&mut tx, &branch_id, &write_keys)
            .await
            .unwrap();

        tx.commit().await.unwrap();

        // Run ops concurrently
        enum Op {
            Receive(Block),
            Remove(BlockId),
        }

        let mut ops: Vec<_> = blocks
            .iter()
            .flat_map(|block| [Op::Receive(block.clone()), Op::Remove(block.id)])
            .collect();
        ops.shuffle(&mut rng);

        let handles: Vec<_> = ops
            .into_iter()
            .map(|op| {
                let store = store.clone();

                task::spawn(async move {
                    match op {
                        Op::Receive(block) => {
                            let mut writer = store.begin_client_write().await.unwrap();
                            writer.save_block(&block, None).await.unwrap();
                            writer.commit().await.unwrap();
                        }
                        Op::Remove(id) => {
                            let mut tx = store.begin_write().await.unwrap();
                            tx.remove_block(&id).await.unwrap();
                            tx.commit().await.unwrap();
                        }
                    }
                })
            })
            .collect();

        // Wait for all the ops to complete.
        future::try_join_all(handles).await.unwrap();

        // Verify
        for block in &blocks {
            let is_in_expiration_tracker = store
                .block_expiration_tracker()
                .await
                .unwrap()
                .has_block(&block.id);

            let is_in_db = block::exists(&mut store.db().acquire().await.unwrap(), &block.id)
                .await
                .unwrap();

            assert!(
                !is_in_db || is_in_expiration_tracker,
                "is_in_db:{is_in_db:?} is_in_expiration_tracker:{is_in_expiration_tracker:?}"
            );
        }
    }

    async fn add_block(
        block: Block,
        write_keys: &Keypair,
        branch_id: &PublicKey,
        store: &Store,
    ) -> BlockId {
        let mut writer = store.begin_write().await.unwrap();
        let mut changeset = Changeset::new();

        let block_id = block.id;

        changeset.write_block(block);
        changeset.link_block(rand::random(), block_id, SingleBlockPresence::Present);
        changeset
            .apply(&mut writer, branch_id, write_keys)
            .await
            .unwrap();

        writer.commit().await.unwrap();

        block_id
    }

    async fn count_blocks(pool: &db::Pool) -> u64 {
        block::count(&mut pool.acquire().await.unwrap())
            .await
            .unwrap()
    }
}
