//! Operation that affect both the index and the block store.

use crate::{
    block::{self, BlockData, BlockNonce, BlockTracker},
    db,
    error::{Error, Result},
    index::{self, Index},
    progress::Progress,
    repository::LocalId,
};
use sqlx::Row;
use tracing::Span;

#[derive(Clone)]
pub struct Store {
    pub(crate) index: Index,
    pub(crate) block_tracker: BlockTracker,
    pub(crate) local_id: LocalId,
    pub(crate) span: Span,
}

impl Store {
    pub fn db(&self) -> &db::Pool {
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
    ) -> Result<()> {
        let mut tx = self.db().begin().await?;

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
            self.index.get_branch(writer_id).notify()
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

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
            BranchData,
        },
        locator::Locator,
        repository::RepositoryId,
        test_utils,
        version_vector::VersionVector,
    };
    use assert_matches::assert_matches;
    use rand::{distributions::Standard, rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
    use tempfile::TempDir;
    use test_strategy::proptest;
    use tokio::sync::broadcast;

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_block() {
        let (_base_dir, pool) = setup().await;
        let mut conn = pool.acquire().await.unwrap();

        let read_key = SecretKey::random();
        let write_keys = Keypair::random();
        let (notify_tx, _) = broadcast::channel(1);

        let branch0 = BranchData::new(PublicKey::random(), notify_tx.clone());
        let branch1 = BranchData::new(PublicKey::random(), notify_tx);

        let block_id = rand::random();
        let buffer = vec![0; BLOCK_SIZE];

        let mut tx = conn.begin().await.unwrap();

        block::write(&mut tx, &block_id, &buffer, &BlockNonce::default())
            .await
            .unwrap();

        let locator0 = Locator::head(rand::random());
        let locator0 = locator0.encode(&read_key);
        branch0
            .insert(&mut tx, &block_id, &locator0, &write_keys)
            .await
            .unwrap();

        let locator1 = Locator::head(rand::random());
        let locator1 = locator1.encode(&read_key);
        branch1
            .insert(&mut tx, &block_id, &locator1, &write_keys)
            .await
            .unwrap();

        assert!(block::exists(&mut tx, &block_id).await.unwrap());

        branch0
            .remove(&mut tx, &locator0, &write_keys)
            .await
            .unwrap();
        assert!(block::exists(&mut tx, &block_id).await.unwrap());

        branch1
            .remove(&mut tx, &locator1, &write_keys)
            .await
            .unwrap();
        assert!(!block::exists(&mut tx, &block_id).await.unwrap(),);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn overwrite_block() {
        let (_base_dir, pool) = setup().await;
        let mut conn = pool.acquire().await.unwrap();
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

        let mut tx = conn.begin().await.unwrap();
        block::write(&mut tx, &id0, &buffer, &rng.gen())
            .await
            .unwrap();
        branch
            .insert(&mut tx, &id0, &locator, &write_keys)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        assert!(block::exists(&mut conn, &id0).await.unwrap());
        assert_eq!(block::count(&mut conn).await.unwrap(), 1);

        rng.fill(&mut buffer[..]);
        let id1 = BlockId::from_content(&buffer);

        let mut tx = conn.begin().await.unwrap();
        block::write(&mut tx, &id1, &buffer, &rng.gen())
            .await
            .unwrap();
        branch
            .insert(&mut tx, &id1, &locator, &write_keys)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        assert!(!block::exists(&mut conn, &id0).await.unwrap());
        assert!(block::exists(&mut conn, &id1).await.unwrap());
        assert_eq!(block::count(&mut conn).await.unwrap(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn receive_valid_blocks() {
        let (_base_dir, pool) = setup().await;

        let branch_id = PublicKey::random();
        let write_keys = Keypair::random();
        let repository_id = RepositoryId::from(write_keys.public);
        let (event_tx, _) = broadcast::channel(1);
        let index = Index::new(pool, repository_id, event_tx);
        let store = Store {
            index,
            block_tracker: BlockTracker::lazy(),
            local_id: LocalId::new(),
            span: Span::none(),
        };

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

        let repository_id = RepositoryId::random();
        let (event_tx, _) = broadcast::channel(1);
        let index = Index::new(pool, repository_id, event_tx);
        let store = Store {
            index,
            block_tracker: BlockTracker::lazy(),
            local_id: LocalId::new(),
            span: Span::none(),
        };

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
        let (event_tx, _) = broadcast::channel(1);
        let index = Index::new(pool.clone(), repository_id, event_tx);
        let store = Store {
            index,
            block_tracker: BlockTracker::lazy(),
            local_id: LocalId::new(),
            span: Span::none(),
        };

        let all_blocks: Vec<(Block, Hash)> =
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
        pool.close().await;
    }

    async fn setup() -> (TempDir, db::Pool) {
        db::create_temp().await.unwrap()
    }
}
