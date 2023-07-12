use super::{
    error::Error,
    leaf_node,
    receive::{self, UpdateSummaryReason},
    root_node,
};
use crate::{
    block::{BlockData, BlockId, BlockNonce, BLOCK_SIZE},
    collections::HashSet,
    crypto::sign::PublicKey,
    db,
    future::try_collect_into,
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

    for (hash, state) in receive::update_summaries(tx, nodes, UpdateSummaryReason::Other).await? {
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
pub(super) async fn count(conn: &mut db::Connection) -> Result<usize, Error> {
    Ok(db::decode_u64(
        sqlx::query("SELECT COUNT(*) FROM blocks")
            .fetch_one(conn)
            .await?
            .get(0),
    ) as usize)
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
