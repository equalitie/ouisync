use super::{
    error::Error,
    index::{self, UpdateSummaryReason},
    leaf_node, root_node,
};
use crate::{
    collections::HashSet,
    crypto::sign::PublicKey,
    db,
    future::try_collect_into,
    protocol::{BlockData, BlockId, BlockNonce, BLOCK_SIZE},
};
use futures_util::TryStreamExt;
use sqlx::Row;

#[derive(Default)]
pub(crate) struct ReceiveStatus {
    /// List of branches that reference the received block.
    pub branches: HashSet<PublicKey>,
}

/// Write a block received from a remote replica.
pub(super) async fn receive(
    tx: &mut db::WriteTransaction,
    block: &BlockData,
    nonce: &BlockNonce,
) -> Result<ReceiveStatus, Error> {
    if !leaf_node::set_present(tx, &block.id).await? {
        return Ok(ReceiveStatus::default());
    }

    let nodes = leaf_node::load_parent_hashes(tx, &block.id)
        .try_collect()
        .await?;
    let mut branches = HashSet::default();

    for (hash, state) in index::update_summaries(tx, nodes, UpdateSummaryReason::Other).await? {
        if !state.is_approved() {
            continue;
        }

        try_collect_into(root_node::load_writer_ids(tx, &hash), &mut branches).await?;
    }

    write(tx, &block.id, &block.content, nonce).await?;

    Ok(ReceiveStatus { branches })
}

/// Reads a block from the store into a buffer.
///
/// # Panics
///
/// Panics if `buffer` length is less than [`BLOCK_SIZE`].
pub(super) async fn read(
    conn: &mut db::Connection,
    id: &BlockId,
    buffer: &mut [u8],
) -> Result<BlockNonce, Error> {
    assert!(
        buffer.len() >= BLOCK_SIZE,
        "insufficient buffer length for block read"
    );

    let row = sqlx::query("SELECT nonce, content FROM blocks WHERE id = ?")
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(Error::BlockNotFound)
        .map_err(|error| {
            tracing::trace!(?id, ?error);
            error
        })?;

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

/// Writes a block into the store.
///
/// If a block with the same id already exists, this is a no-op.
///
/// # Panics
///
/// Panics if buffer length is not equal to [`BLOCK_SIZE`].
///
pub(super) async fn write(
    tx: &mut db::WriteTransaction,
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
    .execute(tx)
    .await?;

    Ok(())
}

pub(super) async fn remove(tx: &mut db::WriteTransaction, id: &BlockId) -> Result<(), Error> {
    sqlx::query("DELETE FROM blocks WHERE id = ?")
        .bind(id)
        .execute(tx)
        .await?;

    Ok(())
}

/// Returns the total number of blocks in the store.
pub(super) async fn count(conn: &mut db::Connection) -> Result<u64, Error> {
    Ok(db::decode_u64(
        sqlx::query("SELECT COUNT(*) FROM blocks")
            .fetch_one(conn)
            .await?
            .get(0),
    ))
}

/// Checks whether the block exists in the store.
#[cfg(test)]
pub(super) async fn exists(conn: &mut db::Connection, id: &BlockId) -> Result<bool, Error> {
    Ok(sqlx::query("SELECT 0 FROM blocks WHERE id = ?")
        .bind(id)
        .fetch_optional(conn)
        .await?
        .is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;
    use tempfile::TempDir;

    #[tokio::test(flavor = "multi_thread")]
    async fn write_and_read_block() {
        let (_base_dir, pool) = setup().await;

        let content = random_block_content();
        let id = BlockId::from_content(&content);
        let nonce = BlockNonce::default();

        let mut tx = pool.begin_write().await.unwrap();

        write(&mut tx, &id, &content, &nonce).await.unwrap();

        let mut buffer = vec![0; BLOCK_SIZE];
        read(&mut tx, &id, &mut buffer).await.unwrap();

        assert_eq!(buffer, content);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn try_read_missing_block() {
        let (_base_dir, pool) = setup().await;

        let mut buffer = vec![0; BLOCK_SIZE];
        let id = BlockId::from_content(&buffer);

        let mut conn = pool.acquire().await.unwrap();

        match read(&mut conn, &id, &mut buffer).await {
            Err(Error::BlockNotFound) => (),
            Err(error) => panic!("unexpected error: {:?}", error),
            Ok(_) => panic!("unexpected success"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn try_write_existing_block() {
        let (_base_dir, pool) = setup().await;

        let content0 = random_block_content();
        let id = BlockId::from_content(&content0);
        let nonce = BlockNonce::default();

        let mut tx = pool.begin_write().await.unwrap();

        write(&mut tx, &id, &content0, &nonce).await.unwrap();
        write(&mut tx, &id, &content0, &nonce).await.unwrap();
    }

    async fn setup() -> (TempDir, db::Pool) {
        db::create_temp().await.unwrap()
    }

    fn random_block_content() -> Vec<u8> {
        let mut content = vec![0; BLOCK_SIZE];
        rand::thread_rng().fill(&mut content[..]);
        content
    }
}
