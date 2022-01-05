use super::{
    broadcast,
    node::{InnerNode, LeafNode, RootNode, INNER_LAYER_COUNT},
    path::Path,
};
use crate::{
    block::BlockId,
    crypto::{
        sign::{Keypair, PublicKey},
        Hash,
    },
    db,
    error::{Error, Result},
    version_vector::VersionVector,
};
use sqlx::Acquire;
use std::mem;
use tokio::sync::{RwLock, RwLockReadGuard};

type LocatorHash = Hash;

pub(crate) struct BranchData {
    writer_id: PublicKey,
    root_node: RwLock<RootNode>,
    notify_tx: async_broadcast::Sender<PublicKey>,
}

impl BranchData {
    pub async fn new(
        conn: &mut db::Connection,
        writer_id: PublicKey,
        notify_tx: async_broadcast::Sender<PublicKey>,
    ) -> Result<Self> {
        let root_node = RootNode::load_latest_or_create(conn, &writer_id).await?;
        Ok(Self::with_root_node(writer_id, root_node, notify_tx))
    }

    pub fn with_root_node(
        writer_id: PublicKey,
        root_node: RootNode,
        notify_tx: async_broadcast::Sender<PublicKey>,
    ) -> Self {
        Self {
            writer_id,
            root_node: RwLock::new(root_node),
            notify_tx,
        }
    }

    /// Returns the id of the replica that owns this branch.
    pub fn id(&self) -> &PublicKey {
        &self.writer_id
    }

    /// Returns the root node of the latest snapshot of this branch.
    pub async fn root(&self) -> RwLockReadGuard<'_, RootNode> {
        self.root_node.read().await
    }

    /// Update the root version vector of this branch.
    pub async fn update_root_version_vector(
        &self,
        tx: db::Transaction<'_>,
        version_vector_override: Option<&VersionVector>,
    ) -> Result<()> {
        // TODO: should we emit a notification event here?

        self.root_node
            .write()
            .await
            .update_version_vector(tx, version_vector_override)
            .await
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
        let mut path = self.get_path(conn, &lock.hash, encoded_locator).await?;

        // We shouldn't be inserting a block to a branch twice. If we do, the assumption is that we
        // hit one in 2^sizeof(BlockVersion) chance that we randomly generated the same
        // BlockVersion twice.
        assert!(!path.has_leaf(block_id));

        path.set_leaf(block_id);
        self.write_path(conn, &mut lock, &path, write_keys).await
    }

    /// Retrieve `BlockId` of a block with the given encoded `Locator`.
    pub async fn get(&self, conn: &mut db::Connection, encoded_locator: &Hash) -> Result<BlockId> {
        let root_node = self.root_node.read().await;
        let path = self
            .get_path(conn, &root_node.hash, encoded_locator)
            .await?;

        match path.get_leaf() {
            Some(block_id) => Ok(block_id),
            None => Err(Error::EntryNotFound),
        }
    }

    /// Remove the block identified by encoded_locator from the index. Returns the id of the
    /// removed block.
    pub async fn remove(
        &self,
        conn: &mut db::Connection,
        encoded_locator: &Hash,
        write_keys: &Keypair,
    ) -> Result<()> {
        let mut lock = self.root_node.write().await;
        let mut path = self.get_path(conn, &lock.hash, encoded_locator).await?;
        path.remove_leaf(encoded_locator)
            .ok_or(Error::EntryNotFound)?;
        self.write_path(conn, &mut lock, &path, write_keys).await
    }

    /// Trigger a notification event from this branch.
    pub async fn notify(&self) {
        broadcast(&self.notify_tx, self.writer_id).await
    }

    /// Update the root node of this branch. Does nothing if the version of `new_root` is not
    /// greater than the version of the current root.
    pub async fn update_root(&self, conn: &mut db::Connection, new_root: RootNode) -> Result<()> {
        let mut old_root = self.root_node.write().await;

        if new_root.versions.get(&self.writer_id) <= old_root.versions.get(&self.writer_id) {
            return Ok(());
        }

        self.replace_root(conn, &mut old_root, new_root).await
    }

    pub async fn reload_root(&self, db: &mut db::Connection) -> Result<()> {
        self.root_node.write().await.reload(db).await
    }

    async fn get_path(
        &self,
        conn: &mut db::Connection,
        root_hash: &Hash,
        encoded_locator: &LocatorHash,
    ) -> Result<Path> {
        let mut path = Path::new(*root_hash, *encoded_locator);

        path.layers_found += 1;

        let mut parent = path.root_hash;

        for level in 0..INNER_LAYER_COUNT {
            path.inner[level] = InnerNode::load_children(&mut *conn, &parent).await?;

            if let Some(node) = path.inner[level].get(path.get_bucket(level)) {
                parent = node.hash
            } else {
                return Ok(path);
            };

            path.layers_found += 1;
        }

        path.leaves = LeafNode::load_children(&mut *conn, &parent).await?;

        if path.leaves.get(encoded_locator).is_some() {
            path.layers_found += 1;
        }

        Ok(path)
    }

    // TODO: make sure nodes are saved as complete.
    async fn write_path(
        &self,
        conn: &mut db::Connection,
        old_root: &mut RootNode,
        path: &Path,
        _write_keys: &Keypair,
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

        // TODO: sign the new root
        let new_root = old_root.next_version(&mut tx, path.root_hash).await?;
        self.replace_root(&mut tx, old_root, new_root).await?;

        tx.commit().await?;

        Ok(())
    }

    async fn replace_root(
        &self,
        conn: &mut db::Connection,
        old_root: &mut RootNode,
        new_root: RootNode,
    ) -> Result<()> {
        let old_root = mem::replace(old_root, new_root);

        // TODO: remove only if new_root is complete
        old_root.remove_recursive(conn).await?;

        self.notify().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crypto::{cipher::SecretKey, sign::Keypair},
        index,
        locator::Locator,
    };
    use sqlx::{Connection, Row};

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

    async fn init_db() -> db::Connection {
        let mut conn = db::Connection::connect(":memory:").await.unwrap();
        index::init(&mut conn).await.unwrap();
        conn
    }

    async fn setup() -> (db::Connection, BranchData) {
        let mut conn = init_db().await;
        let (notify_tx, _) = async_broadcast::broadcast(1);
        let branch = BranchData::new(&mut conn, PublicKey::random(), notify_tx)
            .await
            .unwrap();

        (conn, branch)
    }

    fn random_head_locator() -> Locator {
        Locator::head(rand::random())
    }
}
