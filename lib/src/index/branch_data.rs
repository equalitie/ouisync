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
    version_vector::VersionVector,
};

type LocatorHash = Hash;

pub(crate) struct BranchData {
    writer_id: PublicKey,
    notify_tx: broadcast::OverflowSender<PublicKey>,
}

impl BranchData {
    /// Construct a branch data using the provided root node.
    pub fn new(writer_id: PublicKey, notify_tx: broadcast::Sender<PublicKey>) -> Self {
        Self {
            writer_id,
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

        RootNode::create(conn, Proof::first(writer_id, write_keys), Summary::FULL).await?;

        Ok(Self::new(writer_id, notify_tx))
    }

    /// Destroy this branch
    pub async fn destroy(&self, conn: &mut db::Connection) -> Result<()> {
        let root = self.load_root(conn).await?;

        root.remove_recursively_all_older(conn).await?;
        root.remove_recursively(conn).await?;

        self.notify();

        Ok(())
    }

    /// Returns the id of the replica that owns this branch.
    pub fn id(&self) -> &PublicKey {
        &self.writer_id
    }

    /// Loads the current root node of this branch
    pub async fn load_root(&self, conn: &mut db::Connection) -> Result<RootNode> {
        RootNode::load_latest_complete_by_writer(conn, self.writer_id).await
    }

    /// Loads the version vector of this branch.
    pub async fn load_version_vector(&self, conn: &mut db::Connection) -> Result<VersionVector> {
        // TODO: use a separate `RootNode::load_version_vector` function to avoid loading
        // unnecessary data.
        Ok(self.load_root(conn).await?.proof.into_version_vector())
    }

    /// Inserts a new block into the index.
    ///
    /// # Cancel safety
    ///
    /// This operation is atomic even in the presence of cancellation - it either executes fully or
    /// it doesn't execute at all.
    pub async fn insert(
        &self,
        tx: &mut db::Transaction<'_>,
        block_id: &BlockId,
        encoded_locator: &LocatorHash,
        write_keys: &Keypair,
    ) -> Result<()> {
        let root = self.load_root(tx).await?;
        let mut path = load_path(tx, &root.proof.hash, encoded_locator).await?;

        // We shouldn't be inserting a block to a branch twice. If we do, the assumption is that we
        // hit one in 2^sizeof(BlockId) chance that we randomly generated the same BlockId twice.
        assert!(!path.has_leaf(block_id));

        path.set_leaf(block_id);
        save_path(tx, &root, &path, write_keys).await?;

        Ok(())
    }

    /// Removes the block identified by encoded_locator from the index.
    ///
    /// # Cancel safety
    ///
    /// This operation is atomic even in the presence of cancellation - it either executes fully or
    /// it doesn't execute at all.
    pub async fn remove(
        &self,
        tx: &mut db::Transaction<'_>,
        encoded_locator: &Hash,
        write_keys: &Keypair,
    ) -> Result<()> {
        let root = self.load_root(tx).await?;
        let mut path = load_path(tx, &root.proof.hash, encoded_locator).await?;

        path.remove_leaf(encoded_locator)
            .ok_or(Error::EntryNotFound)?;
        save_path(tx, &root, &path, write_keys).await?;

        Ok(())
    }

    /// Retrieve `BlockId` of a block with the given encoded `Locator`.
    pub async fn get(&self, conn: &mut db::Connection, encoded_locator: &Hash) -> Result<BlockId> {
        let root_hash = self.load_root(conn).await?.proof.hash;
        let path = load_path(conn, &root_hash, encoded_locator).await?;

        match path.get_leaf() {
            Some(block_id) => Ok(block_id),
            None => Err(Error::EntryNotFound),
        }
    }

    #[cfg(test)]
    pub async fn count_leaf_nodes(&self, conn: &mut db::Connection) -> Result<usize> {
        let root_hash = self.load_root(conn).await?.proof.hash;
        count_leaf_nodes(conn, 0, &root_hash).await
    }

    /// Trigger a notification event from this branch.
    pub fn notify(&self) {
        self.notify_tx.broadcast(self.writer_id).unwrap_or(())
    }

    /// Update the root version vector of this branch.
    ///
    /// # Cancel safety
    ///
    /// This operation is atomic even in the presence of cancellation - it either executes fully or
    /// it doesn't execute at all.
    pub async fn bump(
        &self,
        mut tx: db::Transaction<'_>,
        vv: &VersionVector,
        write_keys: &Keypair,
    ) -> Result<()> {
        let root_node = self.load_root(&mut tx).await?;

        let mut new_vv = root_node.proof.version_vector.clone();
        new_vv.bump(vv, self.id());

        let new_proof = Proof::new(
            root_node.proof.writer_id,
            new_vv,
            root_node.proof.hash,
            write_keys,
        );

        root_node.update_proof(&mut tx, new_proof).await?;

        tx.commit().await?;

        self.notify();

        Ok(())
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

async fn save_path(
    tx: &mut db::Transaction<'_>,
    old_root: &RootNode,
    path: &Path,
    write_keys: &Keypair,
) -> Result<()> {
    for (i, inner_layer) in path.inner.iter().enumerate() {
        if let Some(parent_hash) = path.hash_at_layer(i) {
            for (bucket, node) in inner_layer {
                node.save(tx, &parent_hash, bucket).await?;
            }
        }
    }

    let layer = Path::total_layer_count() - 1;
    if let Some(parent_hash) = path.hash_at_layer(layer - 1) {
        for leaf in &path.leaves {
            leaf.save(tx, &parent_hash).await?;
        }
    }

    let new_proof = Proof::new(
        old_root.proof.writer_id,
        old_root.proof.version_vector.clone(),
        path.root_hash,
        write_keys,
    );

    let new_root = RootNode::create(tx, new_proof, old_root.summary).await?;

    // NOTE: It is not enough to remove only the old_root because there may be a non zero
    // number of incomplete roots that have been downloaded prior to new_root becoming
    // complete.
    new_root.remove_recursively_all_older(tx).await?;

    Ok(())
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
    use sqlx::{Connection, Row};
    use test_strategy::proptest;

    #[tokio::test(flavor = "multi_thread")]
    async fn insert_and_read() {
        let (mut conn, branch) = setup().await;
        let read_key = SecretKey::random();
        let write_keys = Keypair::random();

        let block_id = rand::random();
        let locator = random_head_locator();
        let encoded_locator = locator.encode(&read_key);

        let mut tx = conn.begin().await.unwrap();
        branch
            .insert(&mut tx, &block_id, &encoded_locator, &write_keys)
            .await
            .unwrap();

        let r = branch.get(&mut tx, &encoded_locator).await.unwrap();

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

            let mut tx = conn.begin().await.unwrap();

            branch
                .insert(&mut tx, &b1, &encoded_locator, &write_keys)
                .await
                .unwrap();

            branch
                .insert(&mut tx, &b2, &encoded_locator, &write_keys)
                .await
                .unwrap();

            let r = branch.get(&mut tx, &encoded_locator).await.unwrap();

            assert_eq!(r, b2);

            assert_eq!(
                INNER_LAYER_COUNT + 1,
                count_branch_forest_entries(&mut tx).await
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

        let mut tx = conn.begin().await.unwrap();

        assert_eq!(0, count_branch_forest_entries(&mut tx).await);

        branch
            .insert(&mut tx, &b, &encoded_locator, &write_keys)
            .await
            .unwrap();
        let r = branch.get(&mut tx, &encoded_locator).await.unwrap();
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
        let (mut conn, branch) = setup().await;
        let write_keys = Keypair::generate(&mut rng);

        let mut locators = Vec::new();
        let mut tx = conn.begin().await.unwrap();

        // Add blocks
        for _ in 0..leaf_count {
            let locator = rng.gen();
            let block_id = rng.gen();

            branch
                .insert(&mut tx, &block_id, &locator, &write_keys)
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
