use super::Error;
use crate::{
    collections::{hash_map::Entry, HashMap},
    crypto::Hash,
    db,
    protocol::{BlockId, SingleBlockPresence},
};
use futures_util::TryStreamExt;
use sqlx::Row;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

// TODO: invalidation!

#[derive(Eq, PartialEq, Debug)]
pub(super) enum LookupError {
    NotFound,
    CacheMiss,
}

/// Cache for fast block id lookups.
#[derive(Clone, Default)]
pub(super) struct BlockIdCache {
    snapshots: Arc<Mutex<HashMap<Hash, Snapshot>>>,
    notify: Arc<Notify>,
}

enum Snapshot {
    Loading,
    Loaded(HashMap<Hash, (BlockId, SingleBlockPresence)>),
}

impl BlockIdCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Looks up a block id (and its block presence) in the given snapshot by the given locator.
    ///
    /// If this returns `LookupError::CacheMiss` then the cache for the given snapshot hasn't been
    /// populated yet. To populate it, call `load` with the same root hash, then try again.
    pub fn lookup(
        &self,
        root_hash: &Hash,
        encoded_locator: &Hash,
    ) -> Result<(BlockId, SingleBlockPresence), LookupError> {
        match self.snapshots.lock().unwrap().get(root_hash) {
            Some(Snapshot::Loaded(snapshot)) => match snapshot.get(encoded_locator) {
                Some((block_id, block_presence)) => Ok((*block_id, *block_presence)),
                None => Err(LookupError::NotFound),
            },
            Some(Snapshot::Loading) | None => Err(LookupError::CacheMiss),
        }
    }

    /// Populate the cache with the data from the given snapshot.
    ///
    /// Note: This method is idempotent, even when called concurrently.
    pub async fn load(&self, conn: &mut db::Connection, root_hash: &Hash) -> Result<(), Error> {
        loop {
            let notified = self.notify.notified();

            match self.snapshots.lock().unwrap().entry(*root_hash) {
                Entry::Occupied(entry) => match entry.get() {
                    Snapshot::Loading => (),
                    Snapshot::Loaded(_) => return Ok(()),
                },
                Entry::Vacant(entry) => {
                    entry.insert(Snapshot::Loading);
                    break;
                }
            };

            notified.await;
        }

        let guard = LoadGuard::new(self, root_hash);

        let block_ids = sqlx::query(
            "WITH RECURSIVE
                 inner_nodes(hash) AS (
                     SELECT first.hash FROM snapshot_inner_nodes AS first WHERE first.parent = ?
                     UNION ALL
                     SELECT next.hash
                         FROM snapshot_inner_nodes AS next
                         INNER JOIN inner_nodes AS prev ON prev.hash = next.parent
                 )
             SELECT locator, block_id, block_presence
                 FROM snapshot_leaf_nodes
                 WHERE parent IN inner_nodes
             ",
        )
        .bind(root_hash)
        .fetch(conn)
        .map_ok(|row| (row.get(0), (row.get(1), row.get(2))))
        .try_collect()
        .await?;

        guard.complete(block_ids);

        Ok(())
    }

    /// Marks previously missing blocks as present.
    ///
    /// Note: each entry is a pair of encoded locator and block id. The locator is there to make
    /// this operation constant time (per entry and snapshot). Without it it would have to perform
    /// linear search for each entry.
    pub fn set_present(&self, entries: &[(Hash, BlockId)]) {
        let mut snapshots = self.snapshots.lock().unwrap();

        for snapshot in snapshots.values_mut() {
            let Snapshot::Loaded(snapshot) = snapshot else {
                continue;
            };

            for (encoded_locator, block_id) in entries {
                if let Some(block_presence) = snapshot
                    .get_mut(encoded_locator)
                    .filter(|(cached_block_id, _)| cached_block_id == block_id)
                    .map(|(_, block_presence)| block_presence)
                {
                    *block_presence = SingleBlockPresence::Present;
                }
            }
        }
    }
}

/// Cancel safety for `BlockIdCache::load`.
struct LoadGuard<'a> {
    cache: &'a BlockIdCache,
    root_hash: &'a Hash,
    snapshot: Option<HashMap<Hash, (BlockId, SingleBlockPresence)>>,
}

impl<'a> LoadGuard<'a> {
    fn new(cache: &'a BlockIdCache, root_hash: &'a Hash) -> Self {
        Self {
            cache,
            root_hash,
            snapshot: None,
        }
    }

    fn complete(mut self, snapshot: HashMap<Hash, (BlockId, SingleBlockPresence)>) {
        self.snapshot = Some(snapshot);
    }
}

impl Drop for LoadGuard<'_> {
    fn drop(&mut self) {
        // NOTE: Not using `lock().unwrap()` to avoid potential double panic. We don't care about
        // poisoning here anyway (the data in the mutex can't be corrupted by panics in this case).
        let mut snapshots = self
            .cache
            .snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner());

        if let Some(snapshot) = self.snapshot.take() {
            snapshots.insert(*self.root_hash, Snapshot::Loaded(snapshot));
        } else {
            snapshots.remove(self.root_hash);
        }

        drop(snapshots);

        self.cache.notify.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use std::iter;

    use super::{
        super::{test_utils::SnapshotWriter, Store},
        *,
    };
    use crate::{
        crypto::sign::{Keypair, PublicKey},
        protocol::test_utils::Snapshot,
        version_vector::VersionVector,
    };
    use rand::Rng;

    #[tokio::test]
    async fn sanity_check() {
        let num_blocks = 10;
        let num_present = 8;

        let mut rng = rand::thread_rng();
        let (_temp_dir, pool) = db::create_temp().await.unwrap();
        let store = Store::new(pool);

        let write_keys = Keypair::generate(&mut rng);
        let writer_id = PublicKey::generate(&mut rng);
        let vv = VersionVector::first(writer_id);

        // Create snapshot with some blocks...
        let snapshot = Snapshot::generate(&mut rng, num_blocks);

        let mut writer = SnapshotWriter::begin(&store, &snapshot)
            .await
            .save_nodes(&write_keys, writer_id, vv)
            .await;

        // ... make some of them as present.
        for node in snapshot.leaf_nodes().take(num_present) {
            let block = snapshot.blocks().get(&node.block_id).unwrap();
            writer
                .client_writer()
                .save_block(block, None)
                .await
                .unwrap();
        }

        writer.commit().await;

        let cache = BlockIdCache::new();

        // Initially all lookups are cache misses.
        for leaf_node in snapshot.leaf_nodes() {
            assert_eq!(
                cache.lookup(snapshot.root_hash(), &leaf_node.locator),
                Err(LookupError::CacheMiss)
            );
        }

        // Load the snapshot data into the cache.
        cache
            .load(
                &mut store.db().begin_read().await.unwrap(),
                snapshot.root_hash(),
            )
            .await
            .unwrap();

        // Now we get cache hits.
        for (leaf_node, expected_block_presence) in snapshot.leaf_nodes().zip(
            iter::repeat(SingleBlockPresence::Present)
                .take(num_present)
                .chain(iter::repeat(SingleBlockPresence::Missing)),
        ) {
            assert_eq!(
                cache.lookup(snapshot.root_hash(), &leaf_node.locator),
                Ok((leaf_node.block_id, expected_block_presence))
            );
        }

        // Looking up a non-existing locator fails.
        let invalid_locator: Hash = rng.gen();
        assert_eq!(
            cache.lookup(snapshot.root_hash(), &invalid_locator),
            Err(LookupError::NotFound)
        );

        // Update the missing blocks to present.
        let updates: Vec<_> = snapshot
            .leaf_nodes()
            .skip(num_present)
            .map(|node| (node.locator, node.block_id))
            .collect();
        cache.set_present(&updates);

        // All the previously missing blocks are now marked as present.
        for leaf_node in snapshot.leaf_nodes().skip(num_present) {
            assert_eq!(
                cache.lookup(snapshot.root_hash(), &leaf_node.locator),
                Ok((leaf_node.block_id, SingleBlockPresence::Present))
            );
        }
    }
}
