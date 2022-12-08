use std::{collections::HashSet, future};

use super::{
    node::{self, InnerNode, LeafNode, RootNode, SingleBlockPresence, INNER_LAYER_COUNT},
    path::Path,
    proof::Proof,
    VersionVectorOp,
};
use crate::{
    block::BlockId,
    crypto::{
        sign::{Keypair, PublicKey},
        Hash,
    },
    db,
    error::{Error, Result},
    event::{Event, Payload},
    joint_directory::versioned::Versioned,
    version_vector::VersionVector,
};
use futures_util::{Stream, TryStreamExt};
use tokio::{pin, sync::broadcast};

type LocatorHash = Hash;

#[derive(Clone)]
pub(crate) struct BranchData {
    writer_id: PublicKey,
    notify_tx: broadcast::Sender<Event>,
}

impl BranchData {
    /// Construct a branch data using the provided root node.
    pub fn new(writer_id: PublicKey, notify_tx: broadcast::Sender<Event>) -> Self {
        Self {
            writer_id,
            notify_tx,
        }
    }

    /// Load all branches
    pub fn load_all(
        conn: &mut db::Connection,
        notify_tx: broadcast::Sender<Event>,
    ) -> impl Stream<Item = Result<Self>> + '_ {
        SnapshotData::load_all(conn, notify_tx).map_ok(move |snapshot| snapshot.to_branch_data())
    }

    pub async fn load_snapshot(&self, conn: &mut db::Connection) -> Result<SnapshotData> {
        let root_node = RootNode::load_latest_complete_by_writer(conn, self.writer_id).await?;

        Ok(SnapshotData {
            root_node,
            notify_tx: self.notify_tx.clone(),
        })
    }

    pub async fn load_or_create_snapshot(
        &self,
        conn: &mut db::Connection,
        write_keys: &Keypair,
    ) -> Result<SnapshotData> {
        let root_node = match RootNode::load_latest_complete_by_writer(conn, self.writer_id).await {
            Ok(root_node) => Some(root_node),
            Err(Error::EntryNotFound) => None,
            Err(error) => return Err(error),
        };

        let root_node = if let Some(root_node) = root_node {
            root_node
        } else {
            RootNode::empty(self.writer_id, write_keys)
        };

        Ok(SnapshotData {
            root_node,
            notify_tx: self.notify_tx.clone(),
        })
    }

    /// Returns the id of the replica that owns this branch.
    pub fn id(&self) -> &PublicKey {
        &self.writer_id
    }

    /// Loads the version vector of this branch.
    pub async fn load_version_vector(&self, conn: &mut db::Connection) -> Result<VersionVector> {
        match self.load_snapshot(conn).await {
            Ok(snapshot) => Ok(snapshot.root_node.proof.into_version_vector()),
            Err(Error::EntryNotFound) => Ok(VersionVector::new()),
            Err(error) => Err(error),
        }
    }

    /// Inserts a new block into the index.
    ///
    /// # Cancel safety
    ///
    /// This operation is executed inside a db transaction which makes it atomic even in the
    /// presence of cancellation.
    #[cfg(test)] // currently used only in tests
    pub async fn insert(
        &self,
        tx: &mut db::Transaction<'_>,
        encoded_locator: &LocatorHash,
        block_id: &BlockId,
        block_presence: SingleBlockPresence,
        write_keys: &Keypair,
    ) -> Result<bool> {
        self.load_or_create_snapshot(tx, write_keys)
            .await?
            .insert_block(tx, encoded_locator, block_id, block_presence, write_keys)
            .await
    }

    /// Removes the block identified by encoded_locator from the index.
    ///
    /// # Cancel safety
    ///
    /// This operation is executed inside a db transaction which makes it atomic even in the
    /// presence of cancellation.
    #[cfg(test)] // currently used only in tests
    pub async fn remove(
        &self,
        tx: &mut db::Transaction<'_>,
        encoded_locator: &Hash,
        write_keys: &Keypair,
    ) -> Result<()> {
        self.load_snapshot(tx)
            .await?
            .remove_block(tx, encoded_locator, write_keys)
            .await
    }

    /// Retrieve `BlockId` of a block with the given encoded `Locator`.
    pub async fn get(
        &self,
        conn: &mut db::Connection,
        encoded_locator: &Hash,
    ) -> Result<(BlockId, SingleBlockPresence)> {
        self.load_snapshot(conn)
            .await?
            .get_block(conn, encoded_locator)
            .await
    }

    #[cfg(test)]
    pub async fn count_leaf_nodes(&self, conn: &mut db::Connection) -> Result<usize> {
        let root_hash = self.load_snapshot(conn).await?.root_node.proof.hash;
        count_leaf_nodes(conn, 0, &root_hash).await
    }

    /// Trigger a notification event from this branch.
    pub fn notify(&self) {
        self.notify_tx
            .send(Event::new(Payload::BranchChanged(self.writer_id)))
            .unwrap_or(0);
    }

    /// Update the root version vector of this branch.
    ///
    /// # Cancel safety
    ///
    /// This operation is atomic even in the presence of cancellation - it either executes fully or
    /// it doesn't execute at all.
    pub async fn bump(
        &self,
        tx: &mut db::Transaction<'_>,
        op: &VersionVectorOp,
        write_keys: &Keypair,
    ) -> Result<()> {
        self.load_or_create_snapshot(tx, write_keys)
            .await?
            .bump(tx, op, write_keys)
            .await
    }
}

pub(crate) struct SnapshotData {
    pub(super) root_node: RootNode,
    notify_tx: broadcast::Sender<Event>,
}

impl SnapshotData {
    /// Load all latest snapshots
    pub fn load_all(
        conn: &mut db::Connection,
        notify_tx: broadcast::Sender<Event>,
    ) -> impl Stream<Item = Result<Self>> + '_ {
        RootNode::load_all_latest_complete(conn).map_ok(move |root_node| Self {
            root_node,
            notify_tx: notify_tx.clone(),
        })
    }

    /// Load previous snapshot
    pub async fn load_prev(&self, conn: &mut db::Connection) -> Result<Option<Self>> {
        Ok(self.root_node.load_prev(conn).await?.map(|root_node| Self {
            root_node,
            notify_tx: self.notify_tx.clone(),
        }))
    }

    /// Returns the id of the replica that owns this branch.
    pub fn branch_id(&self) -> &PublicKey {
        &self.root_node.proof.writer_id
    }

    pub fn to_branch_data(&self) -> BranchData {
        BranchData {
            writer_id: self.root_node.proof.writer_id,
            notify_tx: self.notify_tx.clone(),
        }
    }

    /// Gets the version vector of this snapshot.
    pub fn version_vector(&self) -> &VersionVector {
        &self.root_node.proof.version_vector
    }

    /// Inserts a new block into the index.
    ///
    /// # Cancel safety
    ///
    /// This operation is executed inside a db transaction which makes it atomic even in the
    /// presence of cancellation.
    pub async fn insert_block(
        &mut self,
        tx: &mut db::Transaction<'_>,
        encoded_locator: &LocatorHash,
        block_id: &BlockId,
        block_presence: SingleBlockPresence,
        write_keys: &Keypair,
    ) -> Result<bool> {
        let mut path = self.load_path(tx, encoded_locator).await?;

        if path.has_leaf(block_id) {
            return Ok(false);
        }

        path.set_leaf(block_id, block_presence);

        self.save_path(tx, &path, write_keys).await?;

        Ok(true)
    }

    /// Removes the block identified by encoded_locator from the index.
    ///
    /// # Cancel safety
    ///
    /// This operation is executed inside a db transaction which makes it atomic even in the
    /// presence of cancellation.
    pub async fn remove_block(
        &mut self,
        tx: &mut db::Transaction<'_>,
        encoded_locator: &Hash,
        write_keys: &Keypair,
    ) -> Result<()> {
        let mut path = self.load_path(tx, encoded_locator).await?;

        path.remove_leaf(encoded_locator)
            .ok_or(Error::EntryNotFound)?;
        self.save_path(tx, &path, write_keys).await?;

        Ok(())
    }

    /// Retrieve `BlockId` of a block with the given encoded `Locator`.
    pub async fn get_block(
        &self,
        conn: &mut db::Connection,
        encoded_locator: &Hash,
    ) -> Result<(BlockId, SingleBlockPresence)> {
        self.load_path(conn, encoded_locator)
            .await?
            .get_leaf()
            .ok_or(Error::EntryNotFound)
    }

    /// Update the root version vector of this branch.
    ///
    /// # Cancel safety
    ///
    /// This operation is atomic even in the presence of cancellation - it either executes fully or
    /// it doesn't execute at all.
    pub async fn bump(
        self,
        tx: &mut db::Transaction<'_>,
        op: &VersionVectorOp,
        write_keys: &Keypair,
    ) -> Result<()> {
        let mut new_vv = self.root_node.proof.version_vector.clone();
        op.apply(self.branch_id(), &mut new_vv);

        let new_proof = Proof::new(
            self.root_node.proof.writer_id,
            new_vv,
            self.root_node.proof.hash,
            write_keys,
        );

        self.root_node.update_proof(tx, new_proof).await?;

        Ok(())
    }

    /// Remove this snapshot
    pub async fn remove(&self, conn: &mut db::Connection) -> Result<()> {
        self.root_node.remove_recursively(conn).await
    }

    /// Remove all snapshots of this branch older than this one.
    pub async fn remove_all_older(&self, conn: &mut db::Connection) -> Result<()> {
        self.root_node.remove_recursively_all_older(conn).await
    }

    /// Prune outdated older snapshots. Note this is not the same as `remove_all_older` because this
    /// preserves older snapshots that can be used as fallback for the latest snapshot and only
    // removed those that can't.
    pub async fn prune(&self, conn: &mut db::Connection) -> Result<()> {
        // First remove all incomplete snapshots
        self.root_node
            .remove_recursively_all_older_incomplete(conn)
            .await?;

        // Then remove those snapshots that can't serve as fallback for the current one.
        let fallback_checker = FallbackChecker::new(conn, &self.root_node).await?;
        let mut maybe_old = self.root_node.load_prev(conn).await?;

        while let Some(old) = maybe_old {
            if fallback_checker.check(conn, &old).await? {
                // `old` can serve as fallback for `self` and so we can't prune it yet. Try the
                // previous snapshot.
                tracing::trace!(
                    branch.id = ?old.proof.writer_id,
                    vv = ?old.proof.version_vector,
                    "not removing outdated snapshot - possible fallback"
                );

                maybe_old = old.load_prev(conn).await?;
            } else {
                // `old` can't serve as fallback for `self` and so we can safely remove it
                // including all its predecessors.
                tracing::trace!(
                    branch.id = ?old.proof.writer_id,
                    vv = ?old.proof.version_vector,
                    "removing outdated snapshot"
                );

                old.remove_recursively(conn).await?;
                old.remove_recursively_all_older(conn).await?;

                break;
            }
        }

        Ok(())
    }

    async fn load_path(
        &self,
        conn: &mut db::Connection,
        encoded_locator: &LocatorHash,
    ) -> Result<Path> {
        let mut path = Path::new(
            self.root_node.proof.hash,
            self.root_node.summary,
            *encoded_locator,
        );
        let mut parent = path.root_hash;

        for level in 0..INNER_LAYER_COUNT {
            path.inner[level] = InnerNode::load_children(conn, &parent).await?;

            if let Some(node) = path.inner[level].get(path.get_bucket(level)) {
                parent = node.hash
            } else {
                return Ok(path);
            };
        }

        path.leaves = LeafNode::load_children(conn, &parent).await?;

        Ok(path)
    }

    async fn save_path(
        &mut self,
        tx: &mut db::Transaction<'_>,
        path: &Path,
        write_keys: &Keypair,
    ) -> Result<()> {
        for (i, inner_layer) in path.inner.iter().enumerate() {
            if let Some(parent_hash) = path.hash_at_layer(i) {
                inner_layer.save(tx, &parent_hash).await?;
            }
        }

        let layer = Path::total_layer_count() - 1;
        if let Some(parent_hash) = path.hash_at_layer(layer - 1) {
            path.leaves.save(tx, &parent_hash).await?;
        }

        let writer_id = self.root_node.proof.writer_id;
        let new_version_vector = self
            .root_node
            .proof
            .version_vector
            .clone()
            .incremented(writer_id);
        let new_proof = Proof::new(writer_id, new_version_vector, path.root_hash, write_keys);
        let new_root_node = RootNode::create(tx, new_proof, path.root_summary).await?;

        // NOTE: It is not enough to remove only the old_root because there may be a non zero
        // number of incomplete roots that have been downloaded prior to new_root becoming
        // complete.
        new_root_node.remove_recursively_all_older(tx).await?;

        self.root_node = new_root_node;

        Ok(())
    }
}

impl Versioned for SnapshotData {
    fn compare_versions(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.version_vector().partial_cmp(other.version_vector())
    }

    fn branch_id(&self) -> &PublicKey {
        self.branch_id()
    }
}

// Helper to check whether an old snapshot can be used as fallback for the new snapshot.
struct FallbackChecker {
    new_missing_locators: HashSet<Hash>,
}

impl FallbackChecker {
    async fn new(conn: &mut db::Connection, new: &RootNode) -> Result<Self> {
        let new_missing_locators = node::leaf_nodes(conn, new, SingleBlockPresence::Missing)
            .map_ok(|node| node.locator)
            .try_collect()
            .await?;

        Ok(Self {
            new_missing_locators,
        })
    }

    async fn check(&self, conn: &mut db::Connection, old: &RootNode) -> Result<bool> {
        let old_present_locators =
            node::leaf_nodes(conn, old, SingleBlockPresence::Present).map_ok(|node| node.locator);

        let stream = old_present_locators
            .try_filter(|item| future::ready(self.new_missing_locators.contains(item)));
        pin!(stream);

        Ok(stream.try_next().await?.is_some())
    }
}

#[cfg(test)]
use async_recursion::async_recursion;

#[async_recursion]
#[cfg(test)]
async fn count_leaf_nodes(
    conn: &mut db::Connection,
    current_layer: usize,
    node: &Hash,
) -> Result<usize> {
    if current_layer < INNER_LAYER_COUNT {
        let children = InnerNode::load_children(conn, node).await?;

        let mut sum = 0;

        for (_bucket, child) in children {
            sum += count_leaf_nodes(conn, current_layer + 1, &child.hash).await?;
        }

        Ok(sum)
    } else {
        Ok(LeafNode::load_children(conn, node).await?.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crypto::{cipher::SecretKey, sign::Keypair},
        index::EMPTY_INNER_HASH,
        locator::Locator,
        test_utils,
    };
    use rand::{rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
    use sqlx::Row;
    use tempfile::TempDir;
    use test_strategy::proptest;

    #[tokio::test(flavor = "multi_thread")]
    async fn insert_and_read() {
        let (_base_dir, pool, branch) = setup().await;
        let read_key = SecretKey::random();
        let write_keys = Keypair::random();

        let block_id = rand::random();
        let locator = random_head_locator();
        let encoded_locator = locator.encode(&read_key);

        let mut tx = pool.begin().await.unwrap();

        branch
            .insert(
                &mut tx,
                &encoded_locator,
                &block_id,
                SingleBlockPresence::Present,
                &write_keys,
            )
            .await
            .unwrap();

        let (r, _) = branch.get(&mut tx, &encoded_locator).await.unwrap();

        assert_eq!(r, block_id);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rewrite_locator() {
        for _ in 0..32 {
            let (_base_dir, pool, branch) = setup().await;
            let read_key = SecretKey::random();
            let write_keys = Keypair::random();

            let b1 = rand::random();
            let b2 = rand::random();

            let locator = random_head_locator();
            let encoded_locator = locator.encode(&read_key);

            let mut tx = pool.begin().await.unwrap();

            branch
                .insert(
                    &mut tx,
                    &encoded_locator,
                    &b1,
                    SingleBlockPresence::Present,
                    &write_keys,
                )
                .await
                .unwrap();

            branch
                .insert(
                    &mut tx,
                    &encoded_locator,
                    &b2,
                    SingleBlockPresence::Present,
                    &write_keys,
                )
                .await
                .unwrap();

            let (r, _) = branch.get(&mut tx, &encoded_locator).await.unwrap();

            assert_eq!(r, b2);

            assert_eq!(
                INNER_LAYER_COUNT + 1,
                count_branch_forest_entries(&mut tx).await
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_locator() {
        let (_base_dir, pool, branch) = setup().await;
        let read_key = SecretKey::random();
        let write_keys = Keypair::random();

        let b = rand::random();
        let locator = random_head_locator();
        let encoded_locator = locator.encode(&read_key);

        let mut tx = pool.begin().await.unwrap();

        assert_eq!(0, count_branch_forest_entries(&mut tx).await);

        branch
            .insert(
                &mut tx,
                &encoded_locator,
                &b,
                SingleBlockPresence::Present,
                &write_keys,
            )
            .await
            .unwrap();
        let (r, _) = branch.get(&mut tx, &encoded_locator).await.unwrap();
        assert_eq!(r, b);

        assert_eq!(
            INNER_LAYER_COUNT + 1,
            count_branch_forest_entries(&mut tx).await
        );

        branch
            .remove(&mut tx, &encoded_locator, &write_keys)
            .await
            .unwrap();

        match branch.get(&mut tx, &encoded_locator).await {
            Err(Error::EntryNotFound) => { /* OK */ }
            Err(_) => panic!("Error should have been EntryNotFound"),
            Ok(_) => panic!("BranchData shouldn't have contained the block ID"),
        }

        assert_eq!(0, count_branch_forest_entries(&mut tx).await);
    }

    #[proptest]
    fn empty_nodes_are_not_stored(
        #[strategy(1usize..32)] leaf_count: usize,
        #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
    ) {
        test_utils::run(empty_nodes_are_not_stored_case(leaf_count, rng_seed))
    }

    async fn empty_nodes_are_not_stored_case(leaf_count: usize, rng_seed: u64) {
        let mut rng = StdRng::seed_from_u64(rng_seed);
        let (_base_dir, pool, branch) = setup().await;
        let write_keys = Keypair::generate(&mut rng);

        let mut locators = Vec::new();
        let mut tx = pool.begin().await.unwrap();

        // Add blocks
        for _ in 0..leaf_count {
            let locator = rng.gen();
            let block_id = rng.gen();

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

            locators.push(locator);

            assert!(!has_empty_inner_node(&mut tx).await);
        }

        // Remove blocks
        locators.shuffle(&mut rng);

        for locator in locators {
            branch.remove(&mut tx, &locator, &write_keys).await.unwrap();

            assert!(!has_empty_inner_node(&mut tx).await);
        }
    }

    async fn count_branch_forest_entries(conn: &mut db::Connection) -> usize {
        sqlx::query(
            "SELECT
                 (SELECT COUNT(*) FROM snapshot_inner_nodes) +
                 (SELECT COUNT(*) FROM snapshot_leaf_nodes)",
        )
        .fetch_one(conn)
        .await
        .unwrap()
        .get::<u32, _>(0) as usize
    }

    async fn has_empty_inner_node(conn: &mut db::Connection) -> bool {
        sqlx::query("SELECT 0 FROM snapshot_inner_nodes WHERE hash = ? LIMIT 1")
            .bind(&*EMPTY_INNER_HASH)
            .fetch_optional(conn)
            .await
            .unwrap()
            .is_some()
    }

    async fn init_db() -> (TempDir, db::Pool) {
        db::create_temp().await.unwrap()
    }

    async fn setup() -> (TempDir, db::Pool, BranchData) {
        let (base_dir, pool) = init_db().await;

        let (notify_tx, _) = broadcast::channel(1);
        let branch = BranchData::new(PublicKey::random(), notify_tx);

        (base_dir, pool, branch)
    }

    fn random_head_locator() -> Locator {
        Locator::head(rand::random())
    }
}
