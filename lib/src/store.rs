//! Operation that affect both the index and the block store.

use crate::{
    block::{self, BlockData, BlockNonce, BlockTracker},
    db,
    error::Result,
    index::{self, Index},
    progress::Progress,
};
use sqlx::{Connection, Row};

#[derive(Clone)]
pub struct Store {
    pub(crate) index: Index,
    pub(crate) block_tracker: BlockTracker,
}

impl Store {
    pub(crate) fn db_pool(&self) -> &db::Pool {
        &self.index.pool
    }

    pub(crate) async fn count_blocks(&self) -> Result<usize> {
        block::count(&mut *self.db_pool().acquire().await?).await
    }

    /// Retrieve the syncing progress of this repository (number of downloaded blocks / number of
    /// all blocks)
    pub(crate) async fn sync_progress(&self) -> Result<Progress> {
        let mut conn = self.db_pool().acquire().await?;

        // TODO: should we use `COUNT(DISTINCT ... )` ?
        Ok(sqlx::query(
            "SELECT
             COUNT(blocks.id),
             COUNT(snapshot_leaf_nodes.block_id)
         FROM
             snapshot_leaf_nodes
             LEFT JOIN blocks ON blocks.id = snapshot_leaf_nodes.block_id
         ",
        )
        .fetch_optional(&mut *conn)
        .await?
        .map(|row| Progress {
            value: db::decode_u64(row.get(0)),
            total: db::decode_u64(row.get(1)),
        })
        .unwrap_or(Progress { value: 0, total: 0 }))
    }

    /// Write a block received from a remote replica to the block store. The block must already be
    /// referenced by the index, otherwise an `BlockNotReferenced` error is returned.
    pub(crate) async fn write_received_block(
        &self,
        data: &BlockData,
        nonce: &BlockNonce,
    ) -> Result<()> {
        let mut cx = self.db_pool().acquire().await?;
        let mut tx = cx.begin().await?;

        let writer_ids = index::receive_block(&mut tx, &data.id).await?;
        block::write(&mut tx, &data.id, &data.content, nonce).await?;
        tx.commit().await?;

        let branches = self.index.branches().await;

        // Reload root nodes of the affected branches and notify them.
        for writer_id in &writer_ids {
            if let Some(branch) = branches.get(writer_id) {
                branch.reload_root(&mut cx).await?;
                branch.notify();
            }
        }

        Ok(())
    }
}

/// Initialize database objects to support operations that affect both the index and the block
/// store.
pub(crate) async fn init(conn: &mut db::Connection) -> Result<()> {
    // Create trigger to delete orphaned blocks.
    sqlx::query(
        "CREATE TRIGGER IF NOT EXISTS blocks_delete_on_leaf_node_deleted
         AFTER DELETE ON snapshot_leaf_nodes
         WHEN NOT EXISTS (SELECT 0 FROM snapshot_leaf_nodes WHERE block_id = old.block_id)
         BEGIN
             DELETE FROM blocks WHERE id = old.block_id;
         END;

         CREATE TEMPORARY TRIGGER IF NOT EXISTS missing_blocks_delete_on_leaf_node_deleted
         AFTER DELETE ON main.snapshot_leaf_nodes
         WHEN NOT EXISTS (SELECT 0 FROM main.snapshot_leaf_nodes WHERE block_id = old.block_id)
         BEGIN
             DELETE FROM missing_blocks WHERE block_id = old.block_id;
         END;
         ",
    )
    .execute(conn)
    .await?;

    Ok(())
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
            self,
            node_test_utils::{receive_blocks, receive_nodes, Block, Snapshot},
            BranchData,
        },
        locator::Locator,
        repository::RepositoryId,
        sync::broadcast,
        test_utils,
        version_vector::VersionVector,
    };
    use assert_matches::assert_matches;
    use rand::{distributions::Standard, rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
    use sqlx::Connection;
    use test_strategy::proptest;

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_block() {
        let mut conn = setup().await.acquire().await.unwrap().detach();

        let read_key = SecretKey::random();
        let write_keys = Keypair::random();
        let notify_tx = broadcast::Sender::new(1);

        let branch0 = BranchData::create(
            &mut conn,
            PublicKey::random(),
            &write_keys,
            notify_tx.clone(),
        )
        .await
        .unwrap();
        let branch1 = BranchData::create(&mut conn, PublicKey::random(), &write_keys, notify_tx)
            .await
            .unwrap();

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
    async fn receive_valid_blocks() {
        let pool = setup().await;

        let branch_id = PublicKey::random();
        let write_keys = Keypair::random();
        let repository_id = RepositoryId::from(write_keys.public);
        let index = Index::load(pool, repository_id).await.unwrap();
        let store = Store {
            index,
            block_tracker: BlockTracker::new(),
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

        let mut conn = store.db_pool().acquire().await.unwrap();

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
        let pool = setup().await;

        let repository_id = RepositoryId::random();
        let index = Index::load(pool, repository_id).await.unwrap();
        let store = Store {
            index,
            block_tracker: BlockTracker::new(),
        };

        let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);

        for block in snapshot.blocks().values() {
            assert_matches!(
                store.write_received_block(&block.data, &block.nonce).await,
                Err(Error::BlockNotReferenced)
            );
        }

        let mut conn = store.db_pool().acquire().await.unwrap();
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

        let pool = setup().await;
        let write_keys = Keypair::generate(&mut rng);
        let repository_id = RepositoryId::from(write_keys.public);
        let index = Index::load(pool, repository_id).await.unwrap();
        let store = Store {
            index,
            block_tracker: BlockTracker::new(),
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
    }

    async fn setup() -> db::Pool {
        let pool = db::open_or_create(&db::Store::Temporary).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();

        index::init(&mut conn).await.unwrap();
        block::init(&mut conn).await.unwrap();
        super::init(&mut conn).await.unwrap();

        pool
    }
}
