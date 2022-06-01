use super::{
    node::{InnerNode, LeafNode, RootNode, INNER_LAYER_COUNT},
    path::Path,
    proof::Proof,
};
use crate::{
    block::BlockId,
    crypto::{
        sign::{Keypair, PublicKey},
        Hash,
    },
    db,
    error::{Error, Result},
    sync::broadcast,
    sync::{RwLock, RwLockReadGuard},
    version_vector::VersionVector,
};

use sqlx::Acquire;

type LocatorHash = Hash;

pub(crate) struct BranchData {
    writer_id: PublicKey, // copied from `root_node` to allow accessing it without locking.
    root_node: RwLock<RootNode>,
    notify_tx: broadcast::OverflowSender<PublicKey>,
}

impl BranchData {
    /// Construct a branch data using the provided root node.
    pub fn new(root_node: RootNode, notify_tx: broadcast::Sender<PublicKey>) -> Self {
        assert!(root_node.summary.is_complete());

        Self {
            writer_id: root_node.proof.writer_id,
            root_node: RwLock::new(root_node),
            notify_tx: broadcast::OverflowSender::new(notify_tx),
        }
    }

    /// Create branch data with the initial root node. Convenience function for tests only.
    #[cfg(test)]
    pub async fn create(
        conn: &mut db::Connection,
        writer_id: PublicKey,
        write_keys: &Keypair,
        notify_tx: broadcast::Sender<PublicKey>,
    ) -> Result<Self> {
        use super::node::Summary;

        Ok(Self::new(
            RootNode::create(conn, Proof::first(writer_id, write_keys), Summary::FULL).await?,
            notify_tx,
        ))
    }

    /// Returns the id of the replica that owns this branch.
    pub fn id(&self) -> &PublicKey {
        &self.writer_id
    }

    /// Returns the root node of the latest snapshot of this branch.
    pub async fn root(&self) -> RwLockReadGuard<'_, RootNode> {
        self.root_node.read().await
    }

    /// Inserts a new block into the index.
    pub async fn insert(
        &self,
        conn: &mut db::Connection,
        block_id: &BlockId,
        encoded_locator: &LocatorHash,
        write_keys: &Keypair,
    ) -> Result<()> {
        let mut lock = self.root_node.write().await;
        let mut path = load_path(conn, &lock.proof.hash, encoded_locator).await?;

        // We shouldn't be inserting a block to a branch twice. If we do, the assumption is that we
        // hit one in 2^sizeof(BlockId) chance that we randomly generated the same BlockId twice.
        assert!(!path.has_leaf(block_id));

        path.set_leaf(block_id);
        save_path(conn, &mut lock, &path, write_keys).await?;

        Ok(())
    }

    /// Removes the block identified by encoded_locator from the index.
    pub async fn remove(
        &self,
        conn: &mut db::Connection,
        encoded_locator: &Hash,
        write_keys: &Keypair,
    ) -> Result<()> {
        let mut lock = self.root_node.write().await;
        let mut path = load_path(conn, &lock.proof.hash, encoded_locator).await?;

        path.remove_leaf(encoded_locator)
            .ok_or(Error::EntryNotFound)?;
        save_path(conn, &mut lock, &path, write_keys).await?;

        Ok(())
    }

    /// Retrieve `BlockId` of a block with the given encoded `Locator`.
    pub async fn get(&self, conn: &mut db::Connection, encoded_locator: &Hash) -> Result<BlockId> {
        let root_hash = self.root_node.read().await.proof.hash;
        let path = load_path(conn, &root_hash, encoded_locator).await?;

        match path.get_leaf() {
            Some(block_id) => Ok(block_id),
            None => Err(Error::EntryNotFound),
        }
    }

    #[cfg(test)]
    pub async fn count_leaf_nodes(&self, conn: &mut db::Connection) -> Result<usize> {
        let root_hash = self.root_node.read().await.proof.hash;
        count_leaf_nodes(conn, 0, &root_hash).await
    }

    /// Trigger a notification event from this branch.
    pub fn notify(&self) {
        self.notify_tx.broadcast(self.writer_id).unwrap_or(())
    }

    /// Update the root node of this branch. Does nothing if the version of `new_root` is not
    /// greater than the version of the current root.
    ///
    /// # Panics
    ///
    /// Panics if `new_root` is not complete.
    pub async fn update_root(&self, conn: &mut db::Connection, new_root: RootNode) -> Result<()> {
        let mut old_root = self.root_node.write().await;

        if new_root.proof.version_vector.get(&self.writer_id)
            <= old_root.proof.version_vector.get(&self.writer_id)
        {
            return Ok(());
        }

        replace_root(conn, &mut old_root, new_root).await?;

        self.notify();

        Ok(())
    }

    /// Update the root version vector of this branch.
    pub async fn update_root_version_vector(
        &self,
        tx: db::Transaction<'_>,
        increment: &VersionVector,
        write_keys: &Keypair,
    ) -> Result<()> {
        let mut root_node = self.root_node.write().await;

        let mut new_version_vector = root_node.proof.version_vector.clone();
        new_version_vector += increment;

        let new_proof = Proof::new(
            root_node.proof.writer_id,
            new_version_vector,
            root_node.proof.hash,
            write_keys,
        );

        root_node.update_proof(tx, new_proof).await?;

        self.notify();

        Ok(())
    }

    pub async fn reload_root(&self, db: &mut db::Connection) -> Result<()> {
        self.root_node.write().await.reload(db).await
    }
}

async fn load_path(
    conn: &mut db::Connection,
    root_hash: &Hash,
    encoded_locator: &LocatorHash,
) -> Result<Path> {
    let mut path = Path::new(*root_hash, *encoded_locator);

    path.layers_found += 1;

    let mut parent = path.root_hash;

    for level in 0..INNER_LAYER_COUNT {
        path.inner[level] = InnerNode::load_children(conn, &parent).await?;

        if let Some(node) = path.inner[level].get(path.get_bucket(level)) {
            parent = node.hash
        } else {
            return Ok(path);
        };

        path.layers_found += 1;
    }

    path.leaves = LeafNode::load_children(conn, &parent).await?;

    if path.leaves.get(encoded_locator).is_some() {
        path.layers_found += 1;
    }

    Ok(path)
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

async fn save_path(
    conn: &mut db::Connection,
    old_root: &mut RootNode,
    path: &Path,
    write_keys: &Keypair,
) -> Result<()> {
    let mut tx = conn.begin().await?;

    for (i, inner_layer) in path.inner.iter().enumerate() {
        if let Some(parent_hash) = path.hash_at_layer(i) {
            for (bucket, node) in inner_layer {
                node.save(&mut tx, &parent_hash, bucket).await?;
            }
        }
    }

    let layer = Path::total_layer_count() - 1;
    if let Some(parent_hash) = path.hash_at_layer(layer - 1) {
        for leaf in &path.leaves {
            leaf.save(&mut tx, &parent_hash).await?;
        }
    }

    let new_proof = Proof::new(
        old_root.proof.writer_id,
        old_root.proof.version_vector.clone(),
        path.root_hash,
        write_keys,
    );
    let new_root = RootNode::create(&mut tx, new_proof, old_root.summary).await?;

    replace_root(&mut tx, old_root, new_root).await?;

    tx.commit().await?;

    Ok(())
}

// Panics if `new_root` is not complete.
async fn replace_root(
    conn: &mut db::Connection,
    old_root: &mut RootNode,
    new_root: RootNode,
) -> Result<()> {
    assert!(new_root.summary.is_complete());

    // NOTE: It is not enough to remove only the old_root because there may be a non zero
    // number of incomplete roots that have been downloaded prior to new_root becoming
    // complete.
    new_root.remove_recursively_all_older(conn).await?;

    *old_root = new_root;

    Ok(())
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
    use test_strategy::proptest;

    #[tokio::test(flavor = "multi_thread")]
    async fn insert_and_read() {
        let (mut conn, branch) = setup().await;
        let read_key = SecretKey::random();
        let write_keys = Keypair::random();

        let block_id = rand::random();
        let locator = random_head_locator();
        let encoded_locator = locator.encode(&read_key);

        branch
            .insert(&mut conn, &block_id, &encoded_locator, &write_keys)
            .await
            .unwrap();

        let r = branch.get(&mut conn, &encoded_locator).await.unwrap();

        assert_eq!(r, block_id);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rewrite_locator() {
        for _ in 0..32 {
            let (mut conn, branch) = setup().await;
            let read_key = SecretKey::random();
            let write_keys = Keypair::random();

            let b1 = rand::random();
            let b2 = rand::random();

            let locator = random_head_locator();
            let encoded_locator = locator.encode(&read_key);

            branch
                .insert(&mut conn, &b1, &encoded_locator, &write_keys)
                .await
                .unwrap();

            branch
                .insert(&mut conn, &b2, &encoded_locator, &write_keys)
                .await
                .unwrap();

            let r = branch.get(&mut conn, &encoded_locator).await.unwrap();

            assert_eq!(r, b2);

            assert_eq!(
                INNER_LAYER_COUNT + 1,
                count_branch_forest_entries(&mut conn).await
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_locator() {
        let (mut conn, branch) = setup().await;
        let read_key = SecretKey::random();
        let write_keys = Keypair::random();

        let b = rand::random();
        let locator = random_head_locator();
        let encoded_locator = locator.encode(&read_key);

        assert_eq!(0, count_branch_forest_entries(&mut conn).await);

        {
            branch
                .insert(&mut conn, &b, &encoded_locator, &write_keys)
                .await
                .unwrap();
            let r = branch.get(&mut conn, &encoded_locator).await.unwrap();
            assert_eq!(r, b);
        }

        assert_eq!(
            INNER_LAYER_COUNT + 1,
            count_branch_forest_entries(&mut conn).await
        );

        {
            branch
                .remove(&mut conn, &encoded_locator, &write_keys)
                .await
                .unwrap();

            match branch.get(&mut conn, &encoded_locator).await {
                Err(Error::EntryNotFound) => { /* OK */ }
                Err(_) => panic!("Error should have been EntryNotFound"),
                Ok(_) => panic!("BranchData shouldn't have contained the block ID"),
            }
        }

        assert_eq!(0, count_branch_forest_entries(&mut conn).await);
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
        let (mut conn, branch) = setup().await;
        let write_keys = Keypair::generate(&mut rng);

        let mut locators = Vec::new();

        // Add blocks
        for _ in 0..leaf_count {
            let locator = rng.gen();
            let block_id = rng.gen();

            branch
                .insert(&mut conn, &block_id, &locator, &write_keys)
                .await
                .unwrap();

            locators.push(locator);

            assert!(!has_empty_inner_node(&mut conn).await);
        }

        // Remove blocks
        locators.shuffle(&mut rng);

        for locator in locators {
            branch
                .remove(&mut conn, &locator, &write_keys)
                .await
                .unwrap();

            assert!(!has_empty_inner_node(&mut conn).await);
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

    async fn init_db() -> db::PoolConnection {
        db::create(&db::Store::Temporary)
            .await
            .unwrap()
            .acquire()
            .await
            .unwrap()
    }

    async fn setup() -> (db::PoolConnection, BranchData) {
        let mut conn = init_db().await;

        let notify_tx = broadcast::Sender::new(1);
        let branch = BranchData::create(
            &mut conn,
            PublicKey::random(),
            &Keypair::random(),
            notify_tx,
        )
        .await
        .unwrap();

        (conn, branch)
    }

    fn random_head_locator() -> Locator {
        Locator::head(rand::random())
    }
}
