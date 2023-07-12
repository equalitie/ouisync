use super::{
    error::Error,
    leaf_node::LeafNode,
    receive::{self, UpdateSummaryReason},
    root_node::RootNode,
};
use crate::{
    block::{BlockData, BlockId, BlockNonce, BLOCK_SIZE},
    collections::HashSet,
    crypto::sign::PublicKey,
    db,
    future::try_collect_into,
};
use futures_util::TryStreamExt;

#[derive(Default)]
pub(crate) struct ReceiveStatus {
    /// List of branches that reference the received block.
    pub branches: HashSet<PublicKey>,
}

pub(super) async fn receive(
    tx: &mut db::WriteTransaction,
    block: &BlockData,
    nonce: &BlockNonce,
) -> Result<ReceiveStatus, Error> {
    if !LeafNode::set_present(tx, &block.id).await? {
        return Ok(ReceiveStatus::default());
    }

    let nodes = LeafNode::load_parent_hashes(tx, &block.id)
        .try_collect()
        .await?;
    let mut branches = HashSet::default();

    for (hash, state) in receive::update_summaries(tx, nodes, UpdateSummaryReason::Other).await? {
        if !state.is_approved() {
            continue;
        }

        try_collect_into(RootNode::load_writer_ids(tx, &hash), &mut branches).await?;
    }

    write(tx, &block.id, &block.content, nonce).await?;

    Ok(ReceiveStatus { branches })
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
