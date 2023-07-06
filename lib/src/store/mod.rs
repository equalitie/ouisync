mod error;
mod inner_node;
mod leaf_node;
mod path;
mod root_node;

#[cfg(test)]
mod tests;

use crate::{
    block::{BlockId, BlockNonce, BLOCK_SIZE},
    crypto::{
        sign::{Keypair, PublicKey},
        Hash,
    },
    db,
    index::{Proof, SingleBlockPresence, Summary},
};
use sqlx::Row;
use std::ops::{Deref, DerefMut};

pub use error::Error;

// TODO: these items should mostly be internal to this module
#[cfg(test)]
pub(crate) use self::{
    inner_node::{get_bucket, EMPTY_INNER_HASH},
    leaf_node::EMPTY_LEAF_HASH,
};
pub(crate) use self::{
    inner_node::{InnerNode, InnerNodeMap, INNER_LAYER_COUNT},
    leaf_node::{LeafNode, LeafNodeSet},
    path::Path,
    root_node::RootNode,
};

/// Data store
#[derive(Clone)]
pub(crate) struct Store {
    db: db::Pool,
}

impl Store {
    pub fn new(db: db::Pool) -> Self {
        Self { db }
    }

    /// Acquires a `Reader`
    pub async fn acquire_read(&self) -> Result<Reader, Error> {
        Ok(Reader {
            inner: Handle::Connection(self.db.acquire().await?),
        })
    }

    /// Begins a `ReadTransaction`
    pub async fn begin_read(&self) -> Result<ReadTransaction, Error> {
        Ok(ReadTransaction {
            inner: Reader {
                inner: Handle::ReadTransaction(self.db.begin_read().await?),
            },
        })
    }

    /// Begins a `WriteTransaction`
    pub async fn begin_write(&self) -> Result<WriteTransaction, Error> {
        Ok(WriteTransaction {
            inner: ReadTransaction {
                inner: Reader {
                    inner: Handle::WriteTransaction(self.db.begin_write().await?),
                },
            },
        })
    }

    /// Closes the store. Waits until all `Reader`s and `{Read|Write}Transactions` obtained from
    /// this store are dropped.
    pub async fn close(&self) -> Result<(), Error> {
        Ok(self.db.close().await?)
    }

    // TODO: remove this method once the refactoring is complete
    pub fn raw(&self) -> &db::Pool {
        &self.db
    }
}

/// Read-only operations. This is an up-to-date view of the data.
pub(crate) struct Reader {
    inner: Handle,
}

impl Reader {
    /// Reads a block from the store into a buffer.
    ///
    /// # Panics
    ///
    /// Panics if `buffer` length is less than [`BLOCK_SIZE`].
    pub async fn read_block(
        &mut self,
        id: &BlockId,
        buffer: &mut [u8],
    ) -> Result<BlockNonce, Error> {
        assert!(
            buffer.len() >= BLOCK_SIZE,
            "insufficient buffer length for block read"
        );

        let row = sqlx::query("SELECT nonce, content FROM blocks WHERE id = ?")
            .bind(id)
            .fetch_optional(self.raw_mut())
            .await?
            .ok_or(Error::BlockNotFound)?;

        let nonce: &[u8] = row.get(0);
        let nonce = BlockNonce::try_from(nonce).map_err(|_| Error::MalformedData)?;

        let content: &[u8] = row.get(1);
        if content.len() != BLOCK_SIZE {
            tracing::error!(
                expected = BLOCK_SIZE,
                actual = content.len(),
                "wrong block length"
            );
            return Err(Error::MalformedData);
        }

        buffer.copy_from_slice(content);

        Ok(nonce)
    }

    /// Checks whether the block exists in the store.
    #[cfg(test)]
    pub(crate) async fn block_exists(&mut self, id: &BlockId) -> Result<bool, Error> {
        Ok(sqlx::query("SELECT 0 FROM blocks WHERE id = ?")
            .bind(id)
            .fetch_optional(self.raw_mut())
            .await?
            .is_some())
    }

    /// Returns the total number of blocks in the store.
    pub(crate) async fn count_blocks(&mut self) -> Result<usize, Error> {
        Ok(db::decode_u64(
            sqlx::query("SELECT COUNT(*) FROM blocks")
                .fetch_one(self.raw_mut())
                .await?
                .get(0),
        ) as usize)
    }

    pub async fn load_latest_root_node(&mut self, branch_id: PublicKey) -> Result<RootNode, Error> {
        root_node::load_latest(self.raw_mut(), branch_id).await
    }

    pub async fn load_prev_root_node(
        &mut self,
        node: &RootNode,
    ) -> Result<Option<RootNode>, Error> {
        root_node::load_prev(self.raw_mut(), node).await
    }

    pub async fn root_node_exists(&mut self, node: &RootNode) -> Result<bool, Error> {
        root_node::exists(self.raw_mut(), node).await
    }

    fn raw_mut(&mut self) -> &mut db::Connection {
        &mut self.inner
    }
}

/// Read-only transaction. This is a snapshot of the data at the time the transaction was
/// acquired.
pub(crate) struct ReadTransaction {
    inner: Reader,
}

impl ReadTransaction {
    /// Finds the block id corresponding to the given locator in the given branch.
    pub async fn find_block(
        &mut self,
        branch_id: PublicKey,
        encoded_locator: Hash,
    ) -> Result<(BlockId, SingleBlockPresence), Error> {
        let root_node = self.load_latest_root_node(branch_id).await?;
        self.find_block_in(&root_node, encoded_locator).await
    }

    pub async fn find_block_in(
        &mut self,
        root_node: &RootNode,
        encoded_locator: Hash,
    ) -> Result<(BlockId, SingleBlockPresence), Error> {
        let path = self.load_path(root_node, encoded_locator).await?;
        path.get_leaf().ok_or(Error::LocatorNotFound)
    }

    async fn load_path(
        &mut self,
        root_node: &RootNode,
        encoded_locator: Hash,
    ) -> Result<Path, Error> {
        let mut path = Path::new(root_node.proof.hash, root_node.summary, encoded_locator);
        let mut parent = path.root_hash;

        for level in 0..INNER_LAYER_COUNT {
            path.inner[level] = inner_node::load_children(self.raw_mut(), &parent).await?;

            if let Some(node) = path.inner[level].get(path.get_bucket(level)) {
                parent = node.hash
            } else {
                return Ok(path);
            };
        }

        path.leaves = leaf_node::load_children(self.raw_mut(), &parent).await?;

        Ok(path)
    }

    // TODO: remove `pub` from this method once the refactoring is complete
    pub fn raw_mut(&mut self) -> &mut db::ReadTransaction {
        match &mut self.inner.inner {
            Handle::ReadTransaction(tx) => tx,
            Handle::WriteTransaction(tx) => tx,
            Handle::Connection(_) => unreachable!(),
        }
    }
}

impl Deref for ReadTransaction {
    type Target = Reader;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for ReadTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

pub(crate) struct WriteTransaction {
    inner: ReadTransaction,
}

impl WriteTransaction {
    /// Links the given block id into the given branch under the given locator.
    pub async fn link_block(
        &mut self,
        branch_id: PublicKey,
        encoded_locator: Hash,
        block_id: &BlockId,
        block_presence: SingleBlockPresence,
        write_keys: &Keypair,
    ) -> Result<bool, Error> {
        let root_node = root_node::load_or_create(self.raw_mut(), branch_id, write_keys).await?;
        let mut path = self.load_path(&root_node, encoded_locator).await?;

        if path.has_leaf(block_id) {
            return Ok(false);
        }

        path.set_leaf(block_id, block_presence);

        self.save_path(&path, &root_node, write_keys).await?;

        Ok(true)
    }

    /// Writes a block into the store.
    ///
    /// If a block with the same id already exists, this is a no-op.
    ///
    /// # Panics
    ///
    /// Panics if buffer length is not equal to [`BLOCK_SIZE`].
    ///
    pub async fn write_block(
        &mut self,
        id: &BlockId,
        buffer: &[u8],
        nonce: &BlockNonce,
    ) -> Result<(), Error> {
        assert_eq!(
            buffer.len(),
            BLOCK_SIZE,
            "incorrect buffer length for block write"
        );

        sqlx::query(
            "INSERT INTO blocks (id, nonce, content)
             VALUES (?, ?, ?)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(nonce.as_slice())
        .bind(buffer)
        .execute(self.raw_mut())
        .await?;

        Ok(())
    }

    /// Removes the specified block from the store.
    // TODO: also mark the block as missing in the index
    pub(crate) async fn remove_block(&mut self, id: &BlockId) -> Result<(), Error> {
        sqlx::query("DELETE FROM blocks WHERE id = ?")
            .bind(id)
            .execute(self.raw_mut())
            .await?;

        Ok(())
    }

    pub async fn commit(self) -> Result<(), Error> {
        match self.inner.inner.inner {
            Handle::WriteTransaction(tx) => Ok(tx.commit().await?),
            Handle::Connection(_) | Handle::ReadTransaction(_) => unreachable!(),
        }
    }

    /// Commits the transaction and if (and only if) the commit completes successfully, runs the
    /// given closure.
    ///
    /// See `db::WriteTransaction::commit_and_then` for explanation why this is necessary.
    pub async fn commit_and_then<F, R>(self, f: F) -> Result<R, sqlx::Error>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        match self.inner.inner.inner {
            Handle::WriteTransaction(tx) => Ok(tx.commit_and_then(f).await?),
            Handle::Connection(_) | Handle::ReadTransaction(_) => unreachable!(),
        }
    }

    async fn save_path(
        &mut self,
        path: &Path,
        old_root_node: &RootNode,
        write_keys: &Keypair,
    ) -> Result<(), Error> {
        for (i, inner_layer) in path.inner.iter().enumerate() {
            if let Some(parent_hash) = path.hash_at_layer(i) {
                inner_layer.save(self.raw_mut(), &parent_hash).await?;
            }
        }

        let layer = Path::total_layer_count() - 1;
        if let Some(parent_hash) = path.hash_at_layer(layer - 1) {
            path.leaves.save(self.raw_mut(), &parent_hash).await?;
        }

        let writer_id = old_root_node.proof.writer_id;
        let new_version_vector = old_root_node
            .proof
            .version_vector
            .clone()
            .incremented(writer_id);
        let new_proof = Proof::new(writer_id, new_version_vector, path.root_hash, write_keys);

        self.create_root_node(new_proof, path.root_summary).await
    }

    async fn create_root_node(
        &mut self,
        new_proof: Proof,
        new_summary: Summary,
    ) -> Result<(), Error> {
        let root_node = root_node::create(self.raw_mut(), new_proof, new_summary).await?;
        root_node::remove_older_snapshots(self.raw_mut(), root_node.snapshot_id).await?;

        tracing::trace!(
            vv = ?root_node.proof.version_vector,
            hash = ?root_node.proof.hash,
            branch_id = ?root_node.proof.writer_id,
            "create local snapshot"
        );

        Ok(())
    }

    // TODO: un-expose this method once the refactoring is complete
    pub fn raw_mut(&mut self) -> &mut db::WriteTransaction {
        match &mut self.inner.inner.inner {
            Handle::WriteTransaction(tx) => tx,
            Handle::Connection(_) | Handle::ReadTransaction(_) => unreachable!(),
        }
    }
}

impl Deref for WriteTransaction {
    type Target = ReadTransaction;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for WriteTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

enum Handle {
    Connection(db::PoolConnection),
    ReadTransaction(db::ReadTransaction),
    WriteTransaction(db::WriteTransaction),
}

impl Deref for Handle {
    type Target = db::Connection;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Connection(conn) => conn,
            Self::ReadTransaction(tx) => tx,
            Self::WriteTransaction(tx) => tx,
        }
    }
}

impl DerefMut for Handle {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Connection(conn) => conn,
            Self::ReadTransaction(tx) => &mut *tx,
            Self::WriteTransaction(tx) => &mut *tx,
        }
    }
}
