//! Operation that affect both the index and the block store.

use crate::{
    block::{self, tracker::BlockPromise, BlockData, BlockId, BlockNonce, BlockTracker},
    crypto::sign::PublicKey,
    db,
    error::{Error, Result},
    event::Payload,
    index::{self, Index, NodeState, SingleBlockPresence},
    progress::Progress,
    repository::{quota, LocalId, Metadata, RepositoryMonitor},
    storage_size::StorageSize,
};
use futures_util::{Stream, TryStreamExt};
use sqlx::Row;
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone)]
pub struct Store {
    pub(crate) index: Index,
    pub(crate) block_tracker: BlockTracker,
    pub(crate) block_request_mode: BlockRequestMode,
    pub(crate) local_id: LocalId,
    pub(crate) monitor: Arc<RepositoryMonitor>,
}

impl Store {
    pub(crate) fn db(&self) -> &db::Pool {
        &self.index.pool
    }

    pub(crate) async fn count_blocks(&self) -> Result<usize> {
        block::count(&mut *self.db().acquire().await?).await
    }

    /// Retrieve the syncing progress of this repository (number of downloaded blocks / number of
    /// all blocks)
    pub(crate) async fn sync_progress(&self) -> Result<Progress> {
        let mut conn = self.db().acquire().await?;

        let total = db::decode_u64(
            sqlx::query("SELECT COUNT(*) FROM snapshot_leaf_nodes")
                .fetch_one(&mut *conn)
                .await?
                .get(0),
        );

        let present = db::decode_u64(
            sqlx::query("SELECT COUNT(*) FROM blocks")
                .fetch_one(&mut *conn)
                .await?
                .get(0),
        );

        Ok(Progress {
            value: present,
            total,
        })
    }

    /// Write a block received from a remote replica to the block store. The block must already be
    /// referenced by the index, otherwise an `BlockNotReferenced` error is returned.
    pub(crate) async fn write_received_block(
        &self,
        data: &BlockData,
        nonce: &BlockNonce,
        promise: Option<BlockPromise>,
    ) -> Result<()> {
        let mut tx = self.db().begin_write().await?;

        let writer_ids = match index::receive_block(&mut tx, &data.id).await {
            Ok(writer_ids) => writer_ids,
            Err(error) => {
                if matches!(error, Error::BlockNotReferenced) {
                    // We no longer need this block but we still need to un-track it.
                    if let Some(promise) = promise {
                        promise.complete();
                    }
                }

                return Err(error);
            }
        };

        block::write(&mut tx, &data.id, &data.content, nonce).await?;

        let data_id = data.id;
        let event_tx = self.index.notify().clone();

        tx.commit_and_then(move || {
            // Notify affected branches.
            for writer_id in writer_ids {
                event_tx.send(Payload::BlockReceived {
                    block_id: data_id,
                    branch_id: writer_id,
                });
            }

            if let Some(promise) = promise {
                promise.complete();
            }
        })
        .await?;

        Ok(())
    }

    /// Returns all block ids referenced from complete snapshots. The result is paginated (with
    /// `page_size` entries per page) to avoid loading too many items into memory.
    pub(crate) fn block_ids(&self, page_size: u32) -> BlockIdsPage {
        BlockIdsPage {
            pool: self.db().clone(),
            lower_bound: None,
            page_size,
        }
    }

    pub(crate) fn metadata(&self) -> Metadata {
        Metadata::new(self.db().clone())
    }

    /// Total size of the stored data
    pub(crate) async fn size(&self) -> Result<StorageSize> {
        let mut conn = self.db().acquire().await?;

        // Note: for simplicity, we are currently counting only blocks (content + id + nonce)
        let count = db::decode_u64(
            sqlx::query("SELECT COUNT(*) FROM blocks")
                .fetch_one(&mut *conn)
                .await?
                .get(0),
        );

        Ok(StorageSize::from_blocks(count))
    }

    pub(crate) async fn set_quota(&self, quota: Option<StorageSize>) -> Result<()> {
        let mut tx = self.db().begin_write().await?;

        if let Some(quota) = quota {
            quota::set(&mut tx, quota.to_bytes()).await?
        } else {
            quota::remove(&mut tx).await?
        }

        tx.commit().await?;

        Ok(())
    }

    pub(crate) async fn quota(&self) -> Result<Option<StorageSize>> {
        let mut conn = self.db().acquire().await?;
        match quota::get(&mut conn).await {
            Ok(quota) => Ok(Some(StorageSize::from_bytes(quota))),
            Err(Error::EntryNotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn approve_offers(&self, branch_id: &PublicKey) -> Result<()> {
        let mut tx = self.db().begin_read().await?;
        let mut block_ids = branch_missing_block_ids(&mut tx, branch_id);

        while let Some(block_id) = block_ids.try_next().await? {
            self.block_tracker.approve(&block_id);
        }

        Ok(())
    }
}

pub(crate) struct BlockIdsPage {
    pool: db::Pool,
    lower_bound: Option<BlockId>,
    page_size: u32,
}

impl BlockIdsPage {
    /// Returns the next page of the results. If the returned collection is empty it means the end
    /// of the results was reached. Calling `next` afterwards resets the page back to zero.
    pub async fn next(&mut self) -> Result<BTreeSet<BlockId>> {
        let mut conn = self.pool.acquire().await?;

        let ids: Result<BTreeSet<_>> = sqlx::query(
            "WITH RECURSIVE
                 inner_nodes(hash) AS (
                     SELECT i.hash
                        FROM snapshot_inner_nodes AS i
                        INNER JOIN snapshot_root_nodes AS r ON r.hash = i.parent
                        WHERE r.state = ?
                     UNION ALL
                     SELECT c.hash
                        FROM snapshot_inner_nodes AS c
                        INNER JOIN inner_nodes AS p ON p.hash = c.parent
                 )
             SELECT DISTINCT block_id
                 FROM snapshot_leaf_nodes
                 WHERE parent IN inner_nodes AND block_id > COALESCE(?, x'')
                 ORDER BY block_id
                 LIMIT ?",
        )
        .bind(NodeState::Approved)
        .bind(self.lower_bound.as_ref())
        .bind(self.page_size)
        .fetch(&mut *conn)
        .map_ok(|row| row.get::<BlockId, _>(0))
        .err_into()
        .try_collect()
        .await;

        let ids = ids?;

        // TODO: use `last` when we bump to rust 1.66
        self.lower_bound = ids.iter().next_back().copied();

        Ok(ids)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum BlockRequestMode {
    // Request only required blocks
    Lazy,
    // Request all blocks
    Greedy,
}

/// Yields all missing block ids referenced from the latest complete snapshot of the given branch.
fn branch_missing_block_ids<'a>(
    conn: &'a mut db::Connection,
    branch_id: &'a PublicKey,
) -> impl Stream<Item = Result<BlockId>> + 'a {
    sqlx::query(
        "WITH RECURSIVE
             inner_nodes(hash) AS (
                 SELECT i.hash
                     FROM snapshot_inner_nodes AS i
                     INNER JOIN snapshot_root_nodes AS r ON r.hash = i.parent
                     WHERE r.snapshot_id = (
                         SELECT MAX(snapshot_id)
                         FROM snapshot_root_nodes
                         WHERE writer_id = ? AND state = ?
                     )
                 UNION ALL
                 SELECT c.hash
                     FROM snapshot_inner_nodes AS c
                     INNER JOIN inner_nodes AS p ON p.hash = c.parent
             )
         SELECT DISTINCT block_id
             FROM snapshot_leaf_nodes
             WHERE parent IN inner_nodes AND block_presence = ?
         ",
    )
    .bind(branch_id)
    .bind(NodeState::Approved)
    .bind(SingleBlockPresence::Missing)
    .fetch(conn)
    .map_ok(|row| row.get(0))
    .err_into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        block::{self, tracker::OfferState, BlockId, BlockNonce, BLOCK_SIZE},
        collections::HashSet,
        crypto::{
            cipher::SecretKey,
            sign::{Keypair, PublicKey},
            Hash,
        },
        db,
        error::Error,
        event::EventSender,
        index::{
            node_test_utils::{receive_blocks, receive_nodes, Block, Snapshot},
            BranchData, MultiBlockPresence, Proof, ReceiveFilter, SingleBlockPresence,
        },
        locator::Locator,
        metrics::Metrics,
        repository::RepositoryId,
        state_monitor::StateMonitor,
        test_utils,
        version_vector::VersionVector,
    };
    use assert_matches::assert_matches;
    use rand::{distributions::Standard, rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
    use tempfile::TempDir;
    use test_strategy::proptest;

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_block() {
        let (_base_dir, pool) = setup().await;

        let read_key = SecretKey::random();
        let write_keys = Keypair::random();

        let branch0 = BranchData::new(PublicKey::random());
        let branch1 = BranchData::new(PublicKey::random());

        let block_id = rand::random();
        let buffer = vec![0; BLOCK_SIZE];

        let mut tx = pool.begin_write().await.unwrap();

        block::write(&mut tx, &block_id, &buffer, &BlockNonce::default())
            .await
            .unwrap();

        let locator0 = Locator::head(rand::random());
        let locator0 = locator0.encode(&read_key);
        branch0
            .load_or_create_snapshot(&mut tx, &write_keys)
            .await
            .unwrap()
            .insert_block(
                &mut tx,
                &locator0,
                &block_id,
                SingleBlockPresence::Present,
                &write_keys,
            )
            .await
            .unwrap();

        let locator1 = Locator::head(rand::random());
        let locator1 = locator1.encode(&read_key);
        branch1
            .load_or_create_snapshot(&mut tx, &write_keys)
            .await
            .unwrap()
            .insert_block(
                &mut tx,
                &locator1,
                &block_id,
                SingleBlockPresence::Present,
                &write_keys,
            )
            .await
            .unwrap();

        assert!(block::exists(&mut tx, &block_id).await.unwrap());

        let mut snapshot0 = branch0.load_snapshot(&mut tx).await.unwrap();
        snapshot0
            .remove_block(&mut tx, &locator0, None, &write_keys)
            .await
            .unwrap();
        snapshot0.remove_all_older(&mut tx).await.unwrap();
        assert!(block::exists(&mut tx, &block_id).await.unwrap());

        let mut snapshot1 = branch1.load_snapshot(&mut tx).await.unwrap();
        snapshot1
            .remove_block(&mut tx, &locator1, None, &write_keys)
            .await
            .unwrap();
        snapshot1.remove_all_older(&mut tx).await.unwrap();
        assert!(!block::exists(&mut tx, &block_id).await.unwrap(),);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn overwrite_block() {
        let (_base_dir, pool) = setup().await;
        let mut rng = rand::thread_rng();

        let read_key = SecretKey::random();
        let write_keys = Keypair::random();

        let branch = BranchData::new(PublicKey::random());

        let locator = Locator::head(rng.gen());
        let locator = locator.encode(&read_key);

        let mut buffer = vec![0; BLOCK_SIZE];

        rng.fill(&mut buffer[..]);
        let id0 = BlockId::from_content(&buffer);

        let mut tx = pool.begin_write().await.unwrap();

        block::write(&mut tx, &id0, &buffer, &rng.gen())
            .await
            .unwrap();
        branch
            .load_or_create_snapshot(&mut tx, &write_keys)
            .await
            .unwrap()
            .insert_block(
                &mut tx,
                &locator,
                &id0,
                SingleBlockPresence::Present,
                &write_keys,
            )
            .await
            .unwrap();

        assert!(block::exists(&mut tx, &id0).await.unwrap());
        assert_eq!(block::count(&mut tx).await.unwrap(), 1);

        rng.fill(&mut buffer[..]);
        let id1 = BlockId::from_content(&buffer);

        block::write(&mut tx, &id1, &buffer, &rng.gen())
            .await
            .unwrap();

        let mut snapshot = branch
            .load_or_create_snapshot(&mut tx, &write_keys)
            .await
            .unwrap();
        snapshot
            .insert_block(
                &mut tx,
                &locator,
                &id1,
                SingleBlockPresence::Present,
                &write_keys,
            )
            .await
            .unwrap();
        snapshot.remove_all_older(&mut tx).await.unwrap();

        assert!(!block::exists(&mut tx, &id0).await.unwrap());
        assert!(block::exists(&mut tx, &id1).await.unwrap());
        assert_eq!(block::count(&mut tx).await.unwrap(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn receive_valid_blocks() {
        let (_base_dir, pool) = setup().await;

        let branch_id = PublicKey::random();
        let write_keys = Keypair::random();
        let repository_id = RepositoryId::from(write_keys.public);
        let store = create_store(pool, repository_id);
        let receive_filter = ReceiveFilter::new(store.db().clone());

        let snapshot = Snapshot::generate(&mut rand::thread_rng(), 5);

        receive_nodes(
            &store.index,
            &write_keys,
            branch_id,
            VersionVector::first(branch_id),
            &receive_filter,
            &snapshot,
        )
        .await;
        receive_blocks(&store, &snapshot).await;

        let mut conn = store.db().acquire().await.unwrap();

        for (id, block) in snapshot.blocks() {
            let mut content = vec![0; BLOCK_SIZE];
            let nonce = block::read(&mut conn, id, &mut content).await.unwrap();

            assert_eq!(&content[..], &block.data.content[..]);
            assert_eq!(nonce, block.nonce);
            assert_eq!(BlockId::from_content(&content), *id);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn receive_orphaned_block() {
        let (_base_dir, pool) = setup().await;
        let store = create_store(pool, RepositoryId::random());

        let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);
        let block_tracker = store.block_tracker.client();

        for block in snapshot.blocks().values() {
            store.block_tracker.require(*block.id());
            block_tracker.offer(*block.id(), OfferState::Approved);
            let promise = block_tracker.acceptor().try_accept().unwrap();

            assert_matches!(
                store
                    .write_received_block(&block.data, &block.nonce, Some(promise))
                    .await,
                Err(Error::BlockNotReferenced)
            );
        }

        let mut conn = store.db().acquire().await.unwrap();
        for id in snapshot.blocks().keys() {
            assert!(!block::exists(&mut conn, id).await.unwrap());
        }
    }

    #[proptest]
    fn sync_progress(
        #[strategy(1usize..16)] block_count: usize,
        #[strategy(1usize..5)] branch_count: usize,
        #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
    ) {
        test_utils::run(sync_progress_case(block_count, branch_count, rng_seed))
    }

    async fn sync_progress_case(block_count: usize, branch_count: usize, rng_seed: u64) {
        let mut rng = StdRng::seed_from_u64(rng_seed);

        let (_base_dir, pool) = setup().await;
        let write_keys = Keypair::generate(&mut rng);
        let repository_id = RepositoryId::from(write_keys.public);
        let store = create_store(pool, repository_id);
        let receive_filter = ReceiveFilter::new(store.db().clone());

        let all_blocks: Vec<(Hash, Block)> =
            (&mut rng).sample_iter(Standard).take(block_count).collect();
        let branches: Vec<(PublicKey, Snapshot)> = (0..branch_count)
            .map(|_| {
                let block_count = rng.gen_range(0..block_count);
                let blocks = all_blocks.choose_multiple(&mut rng, block_count).cloned();
                let snapshot = Snapshot::new(blocks);
                let branch_id = PublicKey::generate(&mut rng);

                (branch_id, snapshot)
            })
            .collect();

        let mut expected_total_blocks = HashSet::new();
        let mut expected_received_blocks = HashSet::new();

        assert_eq!(
            store.sync_progress().await.unwrap(),
            Progress {
                value: expected_received_blocks.len() as u64,
                total: expected_total_blocks.len() as u64
            }
        );

        for (branch_id, snapshot) in branches {
            receive_nodes(
                &store.index,
                &write_keys,
                branch_id,
                VersionVector::first(branch_id),
                &receive_filter,
                &snapshot,
            )
            .await;
            expected_total_blocks.extend(snapshot.blocks().keys().copied());

            assert_eq!(
                store.sync_progress().await.unwrap(),
                Progress {
                    value: expected_received_blocks.len() as u64,
                    total: expected_total_blocks.len() as u64,
                }
            );

            receive_blocks(&store, &snapshot).await;
            expected_received_blocks.extend(snapshot.blocks().keys().copied());

            assert_eq!(
                store.sync_progress().await.unwrap(),
                Progress {
                    value: expected_received_blocks.len() as u64,
                    total: expected_total_blocks.len() as u64,
                }
            );
        }

        // HACK: prevent "too many open files" error.
        store.db().close().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn block_ids_local() {
        let (_base_dir, pool) = setup().await;
        let read_key = SecretKey::random();
        let write_keys = Keypair::random();
        let store = create_store(pool, RepositoryId::from(write_keys.public));

        let branch = store.index.get_branch(PublicKey::random());

        let locator = Locator::head(rand::random());
        let locator = locator.encode(&read_key);
        let block_id = rand::random();

        let mut tx = store.db().begin_write().await.unwrap();
        branch
            .load_or_create_snapshot(&mut tx, &write_keys)
            .await
            .unwrap()
            .insert_block(
                &mut tx,
                &locator,
                &block_id,
                SingleBlockPresence::Present,
                &write_keys,
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let actual = store.block_ids(u32::MAX).next().await.unwrap();
        let expected = [block_id].into_iter().collect();

        assert_eq!(actual, expected);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn block_ids_remote() {
        let (_base_dir, pool) = setup().await;
        let write_keys = Keypair::random();
        let store = create_store(pool, RepositoryId::from(write_keys.public));
        let receive_filter = ReceiveFilter::new(store.db().clone());

        let branch_id = PublicKey::random();
        let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);

        receive_nodes(
            &store.index,
            &write_keys,
            branch_id,
            VersionVector::first(branch_id),
            &receive_filter,
            &snapshot,
        )
        .await;

        let actual = store.block_ids(u32::MAX).next().await.unwrap();
        let expected = snapshot.blocks().keys().copied().collect();

        assert_eq!(actual, expected);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn block_ids_excludes_blocks_from_incomplete_snapshots() {
        let (_base_dir, pool) = setup().await;
        let write_keys = Keypair::random();
        let store = create_store(pool, RepositoryId::from(write_keys.public));

        let branch_id = PublicKey::random();

        // Create snapshot with two leaf nodes but receive only one of them.
        let snapshot = loop {
            let snapshot = Snapshot::generate(&mut rand::thread_rng(), 2);
            if snapshot.leaf_sets().count() > 1 {
                break snapshot;
            }
        };

        let receive_filter = ReceiveFilter::new(store.db().clone());
        let version_vector = VersionVector::first(branch_id);
        let proof = Proof::new(
            branch_id,
            version_vector,
            *snapshot.root_hash(),
            &write_keys,
        );

        store
            .index
            .receive_root_node(proof.into(), MultiBlockPresence::None)
            .await
            .unwrap();

        for layer in snapshot.inner_layers() {
            for (_, nodes) in layer.inner_maps() {
                store
                    .index
                    .receive_inner_nodes(nodes.clone().into(), &receive_filter, None)
                    .await
                    .unwrap();
            }
        }

        for (_, nodes) in snapshot.leaf_sets().take(1) {
            store
                .index
                .receive_leaf_nodes(nodes.clone().into(), None)
                .await
                .unwrap();
        }

        let actual = store.block_ids(u32::MAX).next().await.unwrap();
        assert!(actual.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn block_ids_multiple_branches() {
        let (_base_dir, pool) = setup().await;
        let write_keys = Keypair::random();
        let store = create_store(pool, RepositoryId::from(write_keys.public));
        let receive_filter = ReceiveFilter::new(store.db().clone());

        let branch_id_0 = PublicKey::random();
        let branch_id_1 = PublicKey::random();

        // One block is common between both branches and one for each branch is unique.
        let all_blocks: [(Hash, Block); 3] = rand::random();
        let blocks_0 = &all_blocks[..2];
        let blocks_1 = &all_blocks[1..];

        let snapshot_0 = Snapshot::new(blocks_0.iter().cloned());
        let snapshot_1 = Snapshot::new(blocks_1.iter().cloned());

        receive_nodes(
            &store.index,
            &write_keys,
            branch_id_0,
            VersionVector::first(branch_id_0),
            &receive_filter,
            &snapshot_0,
        )
        .await;

        receive_nodes(
            &store.index,
            &write_keys,
            branch_id_1,
            VersionVector::first(branch_id_1),
            &receive_filter,
            &snapshot_1,
        )
        .await;

        let actual = store.block_ids(u32::MAX).next().await.unwrap();
        let expected = all_blocks
            .iter()
            .map(|(_, block)| block.id())
            .copied()
            .collect();

        assert_eq!(actual, expected);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn block_ids_pagination() {
        let (_base_dir, pool) = setup().await;
        let write_keys = Keypair::random();
        let store = create_store(pool, RepositoryId::from(write_keys.public));
        let receive_filter = ReceiveFilter::new(store.db().clone());

        let branch_id = PublicKey::random();
        let snapshot = Snapshot::generate(&mut rand::thread_rng(), 3);

        receive_nodes(
            &store.index,
            &write_keys,
            branch_id,
            VersionVector::first(branch_id),
            &receive_filter,
            &snapshot,
        )
        .await;

        let mut sorted_blocks: Vec<_> = snapshot.blocks().keys().copied().collect();
        sorted_blocks.sort();

        let mut page = store.block_ids(2);

        let actual = page.next().await.unwrap();
        let expected = sorted_blocks[..2].iter().copied().collect();
        assert_eq!(actual, expected);

        let actual = page.next().await.unwrap();
        let expected = sorted_blocks[2..].iter().copied().collect();
        assert_eq!(actual, expected);

        let actual = page.next().await.unwrap();
        assert!(actual.is_empty());
    }

    async fn setup() -> (TempDir, db::Pool) {
        db::create_temp().await.unwrap()
    }

    fn create_store(pool: db::Pool, repo_id: RepositoryId) -> Store {
        let event_tx = EventSender::new(1);
        let index = Index::new(pool, repo_id, event_tx);
        Store {
            index,
            block_tracker: BlockTracker::new(),
            block_request_mode: BlockRequestMode::Lazy,
            local_id: LocalId::new(),
            monitor: Arc::new(RepositoryMonitor::new(
                StateMonitor::make_root(),
                Metrics::new(),
                "test",
            )),
        }
    }
}
