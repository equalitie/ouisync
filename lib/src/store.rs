//! Operation that affect both the index and the block store.

use crate::{
    block::{self, BlockData, BlockId, BlockNonce, BlockTracker},
    db,
    error::{Error, Result},
    event::{Event, Payload},
    index::{self, Index},
    progress::Progress,
    repository::LocalId,
};
use futures_util::TryStreamExt;
use sqlx::Row;
use std::collections::BTreeSet;
use tracing::Span;

#[derive(Clone)]
pub struct Store {
    pub(crate) index: Index,
    pub(crate) block_tracker: BlockTracker,
    pub(crate) block_request_mode: BlockRequestMode,
    pub(crate) local_id: LocalId,
    pub(crate) label: String,
}

impl Store {
    pub(crate) fn db(&self) -> &db::Pool {
        &self.index.pool
    }

    pub(crate) fn span(&self) -> Span {
        tracing::info_span!("repository", label = self.label)
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

    pub(crate) async fn require_missing_block(&self, block_id: BlockId) -> Result<()> {
        let require = self.block_tracker.begin_require(block_id);
        let mut conn = self.db().acquire().await?;

        if !block::exists(&mut conn, &block_id).await? {
            require.commit();
        }

        Ok(())
    }

    /// Write a block received from a remote replica to the block store. The block must already be
    /// referenced by the index, otherwise an `BlockNotReferenced` error is returned.
    pub(crate) async fn write_received_block(
        &self,
        data: &BlockData,
        nonce: &BlockNonce,
    ) -> Result<()> {
        let mut tx = self.db().begin_write().await?;

        let writer_ids = match index::receive_block(&mut tx, &data.id).await {
            Ok(writer_ids) => writer_ids,
            Err(error) => {
                if matches!(error, Error::BlockNotReferenced) {
                    // We no longer need this block but we still need to un-track it.
                    self.block_tracker.complete(&data.id);
                }

                return Err(error);
            }
        };

        block::write(&mut tx, &data.id, &data.content, nonce).await?;

        tx.commit().await?;

        self.block_tracker.complete(&data.id);

        // Notify affected branches.
        for writer_id in writer_ids {
            self.index.notify(Event::new(Payload::BlockReceived {
                block_id: data.id,
                branch_id: writer_id,
            }));
        }

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
                        WHERE r.is_complete = 1
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        block::{self, BlockId, BlockNonce, BLOCK_SIZE},
        crypto::{
            cipher::SecretKey,
            sign::{Keypair, PublicKey},
            Hash,
        },
        db,
        error::Error,
        index::{
            node_test_utils::{receive_blocks, receive_nodes, Block, Snapshot},
            BranchData, Proof, ReceiveFilter, SingleBlockPresence, Summary,
        },
        locator::Locator,
        repository::RepositoryId,
        test_utils,
        version_vector::VersionVector,
    };
    use assert_matches::assert_matches;
    use rand::{distributions::Standard, rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
    use std::collections::HashSet;
    use tempfile::TempDir;
    use test_strategy::proptest;
    use tokio::sync::broadcast;

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_block() {
        let (_base_dir, pool) = setup().await;

        let read_key = SecretKey::random();
        let write_keys = Keypair::random();
        let (notify_tx, _) = broadcast::channel(1);

        let branch0 = BranchData::new(PublicKey::random(), notify_tx.clone());
        let branch1 = BranchData::new(PublicKey::random(), notify_tx);

        let block_id = rand::random();
        let buffer = vec![0; BLOCK_SIZE];

        let mut tx = pool.begin_write().await.unwrap();

        block::write(&mut tx, &block_id, &buffer, &BlockNonce::default())
            .await
            .unwrap();

        let locator0 = Locator::head(rand::random());
        let locator0 = locator0.encode(&read_key);
        branch0
            .insert(
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
            .insert(
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
        let (notify_tx, _) = broadcast::channel(1);

        let branch = BranchData::new(PublicKey::random(), notify_tx.clone());

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
            .insert(
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
        branch
            .insert(
                &mut tx,
                &locator,
                &id1,
                SingleBlockPresence::Present,
                &write_keys,
            )
            .await
            .unwrap();

        branch
            .load_snapshot(&mut tx)
            .await
            .unwrap()
            .remove_all_older(&mut tx)
            .await
            .unwrap();

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

        let snapshot = Snapshot::generate(&mut rand::thread_rng(), 5);

        receive_nodes(
            &store.index,
            &write_keys,
            branch_id,
            VersionVector::first(branch_id),
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

        for block in snapshot.blocks().values() {
            assert_matches!(
                store.write_received_block(&block.data, &block.nonce).await,
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
            .insert(
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

        let branch_id = PublicKey::random();
        let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);

        receive_nodes(
            &store.index,
            &write_keys,
            branch_id,
            VersionVector::first(branch_id),
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

        let mut receive_filter = ReceiveFilter::new(store.db().clone());
        let version_vector = VersionVector::first(branch_id);
        let proof = Proof::new(
            branch_id,
            version_vector,
            *snapshot.root_hash(),
            &write_keys,
        );

        store
            .index
            .receive_root_node(proof.into(), Summary::INCOMPLETE)
            .await
            .unwrap();

        for layer in snapshot.inner_layers() {
            for (_, nodes) in layer.inner_maps() {
                store
                    .index
                    .receive_inner_nodes(nodes.clone().into(), &mut receive_filter)
                    .await
                    .unwrap();
            }
        }

        for (_, nodes) in snapshot.leaf_sets().take(1) {
            store
                .index
                .receive_leaf_nodes(nodes.clone().into())
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
            &snapshot_0,
        )
        .await;

        receive_nodes(
            &store.index,
            &write_keys,
            branch_id_1,
            VersionVector::first(branch_id_1),
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

        let branch_id = PublicKey::random();
        let snapshot = Snapshot::generate(&mut rand::thread_rng(), 3);

        receive_nodes(
            &store.index,
            &write_keys,
            branch_id,
            VersionVector::first(branch_id),
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
        let (event_tx, _) = broadcast::channel(1);
        let index = Index::new(pool, repo_id, event_tx);
        Store {
            index,
            block_tracker: BlockTracker::new(),
            block_request_mode: BlockRequestMode::Lazy,
            local_id: LocalId::new(),
            label: "test".to_owned(),
        }
    }
}
