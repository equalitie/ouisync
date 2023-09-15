use super::{
    cache::{Cache, CacheTransaction},
    error::Error,
    index::{self, UpdateSummaryReason},
    leaf_node, root_node,
};
use crate::{
    block_tracker::BlockTracker as BlockDownloadTracker,
    collections::{hash_map, HashMap, HashSet},
    crypto::sign::PublicKey,
    db,
    deadlock::BlockingMutex,
    future::try_collect_into,
    protocol::{BlockId, SingleBlockPresence},
    sync::{broadcast_hash_set, uninitialized_watch},
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
    expiration_time_tx: watch::Sender<Duration>,
    _task: ScopedJoinHandle<()>,
}

impl BlockExpirationTracker {
    pub(super) async fn enable_expiration(
        pool: db::Pool,
        expiration_time: Duration,
        block_download_tracker: BlockDownloadTracker,
        client_reload_index_tx: broadcast_hash_set::Sender<PublicKey>,
        cache: Arc<Cache>,
    ) -> Result<Self, Error> {
        let mut shared = Shared {
            blocks_by_id: Default::default(),
            blocks_by_expiration: Default::default(),
            to_missing_if_expired: Default::default(),
        };

        let mut tx = pool.begin_read().await?;

        let mut ids =
            sqlx::query("SELECT block_id FROM snapshot_leaf_nodes WHERE block_presence = ?")
                .bind(SingleBlockPresence::Present)
                .fetch(&mut tx)
                .map_ok(|row| row.get(0));

        let now = SystemTime::now();

        while let Some(id) = ids.next().await {
            shared.insert_block(&id?, now, None);
        }

        let (watch_tx, watch_rx) = uninitialized_watch::channel();
        let shared = Arc::new(BlockingMutex::new(shared));

        let (expiration_time_tx, expiration_time_rx) = watch::channel(expiration_time);

        let _task = scoped_task::spawn({
            let shared = shared.clone();

            async move {
                if let Err(err) = run_task(
                    shared,
                    pool,
                    watch_rx,
                    expiration_time_rx,
                    block_download_tracker,
                    client_reload_index_tx,
                    cache,
                )
                .await
                {
                    tracing::error!("BlockExpirationTracker task has ended with {err:?}");
                }
            }
        });

        Ok(Self {
            shared,
            watch_tx,
            expiration_time_tx,
            _task,
        })
    }

    pub fn handle_block_update(
        &self,
        block_id: &BlockId,
        is_missing: bool,
        transaction_id: Option<db::TransactionId>,
    ) {
        // Not inlining these lines to call `SystemTime::now()` only once the `lock` is acquired.
        let mut lock = self.shared.lock().unwrap();
        lock.insert_block(block_id, SystemTime::now(), transaction_id);
        if is_missing {
            lock.to_missing_if_expired.insert(*block_id);
        }
        drop(lock);
        self.watch_tx.send(()).unwrap_or(());
    }

    pub fn set_expiration_time(&self, expiration_time: Duration) {
        self.expiration_time_tx.send(expiration_time).unwrap_or(());
    }

    pub fn block_expiration(&self) -> Duration {
        *self.expiration_time_tx.borrow()
    }

    pub fn begin_untrack_blocks(&self) -> UntrackTransaction {
        UntrackTransaction {
            shared: self.shared.clone(),
            block_ids: Default::default(),
        }
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

    // We require CommitId here instead of the TransactionId to ensure this function is called only
    // after the removal of the blocks has been successfully committed into the database.
    pub fn commit(self, commit_id: db::CommitId) {
        if self.block_ids.is_empty() {
            return;
        }

        let mut shared = self.shared.lock().unwrap();

        for block_id in &self.block_ids {
            shared.remove_block(block_id, commit_id);
        }
    }
}

// For semantics
type TimeUpdated = SystemTime;

#[derive(Eq, PartialEq, Debug)]
struct Entry {
    time_updated: TimeUpdated,
    transaction_id: Option<db::TransactionId>,
}

struct Shared {
    // Invariant #1: There exists `(block, Entry { time_updated: ts, ..})` in `blocks_by_id` *iff*
    // there exists `block` in `blocks_by_expiration[ts]`.
    //
    // Invariant #2: `blocks_by_expiration[x]` is never empty for any `x`.
    //
    blocks_by_id: HashMap<BlockId, Entry>,
    blocks_by_expiration: BTreeMap<TimeUpdated, HashSet<BlockId>>,

    to_missing_if_expired: HashSet<BlockId>,
}

impl Shared {
    /// Add the `block` into `Self`. If it's already there, remove it and add it back with the new
    /// time stamp.
    fn insert_block(
        &mut self,
        block: &BlockId,
        ts: TimeUpdated,
        transaction_id: Option<db::TransactionId>,
    ) {
        // Asserts and unwraps are OK due to the `Shared` invariants defined above.
        match self.blocks_by_id.entry(*block) {
            hash_map::Entry::Occupied(mut entry) => {
                let Entry {
                    time_updated: old_ts,
                    transaction_id: old_id,
                } = *entry.get();

                if (old_ts, old_id) == (ts, transaction_id) {
                    return;
                }

                entry.insert(Entry {
                    time_updated: ts,
                    transaction_id,
                });

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

                entry.insert(Entry {
                    time_updated: ts,
                    transaction_id,
                });
            }
        }
    }

    fn remove_block(&mut self, block: &BlockId, commit_id: db::CommitId) {
        // Asserts and unwraps are OK due to the `Shared` invariants defined above.
        let time_updated = match self.blocks_by_id.entry(*block) {
            hash_map::Entry::Occupied(entry) => {
                if entry.get().transaction_id >= Some(commit_id.as_transaction_id()) {
                    // A race condition happened and operations switched order, we need to ignore
                    // this one. See the `expiration_race` test for more information.
                    return;
                }
                entry.remove().time_updated
            }
            hash_map::Entry::Vacant(_) => return,
        };

        let mut entry = match self.blocks_by_expiration.entry(time_updated) {
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
        for (
            block,
            Entry {
                time_updated: ts, ..
            },
        ) in self.blocks_by_id.iter()
        {
            assert!(self.blocks_by_expiration.get(ts).unwrap().contains(block));
        }
        // #1 <=
        for (ts, blocks) in self.blocks_by_expiration.iter() {
            for block in blocks.iter() {
                assert_eq!(&self.blocks_by_id.get(block).unwrap().time_updated, ts);
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
    cache: Arc<Cache>,
) -> Result<(), Error> {
    loop {
        let expiration_time = *expiration_time_rx.borrow();

        let (ts, block) = {
            enum Enum {
                OldestEntry(Option<(TimeUpdated, BlockId)>),
                ToMissing(HashSet<BlockId>),
            }

            match {
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
            } {
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
                        cache.begin(),
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

        if update_result.rows_affected() == 0 {
            return Ok(());
        }

        sqlx::query("DELETE FROM blocks WHERE id = ?")
            .bind(&block)
            .execute(&mut tx)
            .await?;

        tracing::warn!("Block {block:?} has expired");

        let commit_id = tx.commit().await?;

        shared.lock().unwrap().remove_block(&block, commit_id);
    }
}

async fn set_as_missing_if_expired(
    pool: &db::Pool,
    block_ids: HashSet<BlockId>,
    block_download_tracker: &BlockDownloadTracker,
    client_reload_index_tx: &broadcast_hash_set::Sender<PublicKey>,
    mut cache: CacheTransaction,
) -> Result<(), Error> {
    let mut tx = pool.begin_write().await?;

    // Branches where we have newly missing blocks. We need to tell the client to reload indices
    // for these branches from peers.
    let mut branches: HashSet<PublicKey> = HashSet::default();

    for block_id in &block_ids {
        let changed = leaf_node::set_missing_if_expired(&mut tx, block_id).await?;

        if !changed {
            continue;
        }

        block_download_tracker.require(*block_id);

        let nodes: Vec<_> = leaf_node::load_parent_hashes(&mut tx, block_id)
            .try_collect()
            .await?;

        for (hash, _state) in index::update_summaries(
            &mut tx,
            &mut cache,
            nodes,
            UpdateSummaryReason::BlockRemoved,
        )
        .await?
        {
            try_collect_into(root_node::load_writer_ids(&mut tx, &hash), &mut branches).await?;
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
    use tempfile::TempDir;

    #[test]
    fn shared_state() {
        let mut shared = Shared {
            blocks_by_id: Default::default(),
            blocks_by_expiration: Default::default(),
            to_missing_if_expired: Default::default(),
        };

        // add once

        let ts = SystemTime::now();
        let block: BlockId = rand::random();

        shared.insert_block(&block, ts, Some(db::TransactionId::new(0)));

        assert_eq!(
            *shared.blocks_by_id.get(&block).unwrap(),
            Entry {
                time_updated: ts,
                transaction_id: Some(db::TransactionId::new(0))
            }
        );
        shared.assert_invariants();

        shared.remove_block(&block, db::CommitId::new(1));

        assert!(shared.blocks_by_id.is_empty());
        shared.assert_invariants();

        // add twice

        shared.insert_block(&block, ts, Some(db::TransactionId::new(2)));
        shared.insert_block(&block, ts, Some(db::TransactionId::new(3)));

        assert_eq!(
            *shared.blocks_by_id.get(&block).unwrap(),
            Entry {
                time_updated: ts,
                transaction_id: Some(db::TransactionId::new(3))
            }
        );
        shared.assert_invariants();

        shared.remove_block(&block, db::CommitId::new(4));

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
            Arc::new(Cache::new()),
        )
        .await
        .unwrap();

        sleep(Duration::from_millis(700)).await;

        let block_id = add_block(rand::random(), &write_keys, &branch_id, &store).await;
        tracker.handle_block_update(&block_id, false, Some(db::TransactionId::new(1)));

        assert_eq!(count_blocks(store.db()).await, 2);

        sleep(Duration::from_millis(600) /* -> 1300ms from start */).await;

        assert_eq!(count_blocks(store.db()).await, 1);

        sleep(Duration::from_millis(900) /* -> 2200ms from start */).await;

        assert_eq!(count_blocks(store.db()).await, 0);
    }

    /// This test checks the condition that "if there is a block in the main database, then it must
    /// be in the expiration tracker" in the presence of a race condition as described in the
    /// following example:
    ///
    /// When adding a block (`add`), we do "add the block into the main database" (`add.db`), and then
    /// "add the block into the expiration tracker" (`add.ex`). Similarly, when removing a block (`rm`)
    /// we do "remove the block from the main database" (`rm.db`) and then "remove the block from the
    /// expiration tracker" (`rm.ex`).
    ///
    /// As such calling `rm` and `add` concurrently could result in the `{add.db, add.ex, rm.db,
    /// rm.ex}` operations to be executed in the following order:
    ///
    /// rm.db -> add.db -> add.ex -> rm.ex
    ///
    /// Unless we explicitly take care of this situation, we'll end up with the block being present
    /// in the main database, but not in the expiration tracker. Which would be a violation of the
    /// condition from the first paragraph of this comment.
    #[tokio::test]
    async fn expiration_race() {
        let (_base_dir, store) = setup().await;
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
        let branch_id = branch_id.clone();

        let block: Block = rand::random();
        add_block(block.clone(), &write_keys, &branch_id, &store).await;

        let (on_rm, mut on_rm_controll) = crate::sync::break_point::new();

        let task_rm = tokio::task::spawn({
            let block_id = block.id;
            let store = store.clone();

            async move {
                let mut tx = store.begin_write().await.unwrap();
                tx.break_on_commit(on_rm);
                tx.remove_block(&block_id).await.unwrap();
                tx.commit().await.unwrap();
            }
        });

        let task_add = tokio::task::spawn({
            let block = block.clone();
            let store = store.clone();

            async move {
                on_rm_controll.on_hit().await.unwrap();

                // At this point the other task has already committed the removal from database
                // action (`rm.db`), but not yet the removal from the expiration tracker (`rm.ex`).

                // Perform the `add.db` and `add.ex` actions.
                let mut tx = store.begin_write().await.unwrap();
                tx.receive_block(&block).await.unwrap();
                tx.commit().await.unwrap();

                // Resume the `rm.ex` action in `task_rm`.
                on_rm_controll.release();
            }
        });

        task_rm.await.unwrap();
        task_add.await.unwrap();

        let is_in_exp_tracker = store
            .block_expiration_tracker()
            .await
            .unwrap()
            .has_block(&block.id);

        let is_in_db = block::exists(&mut store.db().acquire().await.unwrap(), &block.id)
            .await
            .unwrap();

        assert!(
            (!is_in_db) || (is_in_db && is_in_exp_tracker),
            "is_in_db:{is_in_db:?} is_in_exp_tracker:{is_in_exp_tracker:?}"
        );
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
