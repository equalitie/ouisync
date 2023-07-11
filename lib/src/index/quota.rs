use super::{try_collect_into, RootNode};
use crate::{
    crypto::Hash,
    db,
    error::{Error, Result},
    iterator,
    storage_size::StorageSize,
    versioned,
};
use sqlx::Row;
use thiserror::Error;

/// Check whether the repository would be within the given block count quota if the snapshot with
/// the given root hash was approved.
pub(super) async fn check(
    conn: &mut db::Connection,
    candidate_root_hash: &Hash,
    quota: StorageSize,
) -> Result<(), QuotaError> {
    let root_hashes = load_candidate_latest_root_hashes(conn, candidate_root_hash).await?;

    // The candidate snapshot is already outdated, reject it straight away.
    if root_hashes.iter().all(|hash| hash != candidate_root_hash) {
        return Err(QuotaError::Outdated);
    }

    let block_count = count_referenced_blocks(conn, &root_hashes).await?;
    let size = StorageSize::from_blocks(block_count);

    if size <= quota {
        Ok(())
    } else {
        Err(QuotaError::Exceeded(size))
    }
}

#[derive(Debug, Error)]
pub(super) enum QuotaError {
    #[error("quota exceeded")]
    Exceeded(StorageSize),
    #[error("snapshot outdated")]
    Outdated,
    #[error("fatal error")]
    Fatal(#[from] Error),
}

/// Load the most up-to-date root node hashes considering also the unapproved candidate.
async fn load_candidate_latest_root_hashes(
    conn: &mut db::Connection,
    candidate_root_hash: &Hash,
) -> Result<Vec<Hash>> {
    let mut nodes = Vec::new();

    try_collect_into(
        RootNode::load_all_by_hash(conn, candidate_root_hash),
        &mut nodes,
    )
    .await?;
    try_collect_into(RootNode::load_all_latest_approved(conn), &mut nodes).await?;

    let nodes = versioned::keep_maximal(nodes, ());

    let mut hashes: Vec<_> = nodes.into_iter().map(|node| node.proof.hash).collect();
    hashes.sort();
    hashes.dedup();

    Ok(hashes)
}

/// Count blocks referenced from the given root nodes. Blocks referenced from more than one
/// node are counted only once.
async fn count_referenced_blocks(conn: &mut db::Connection, root_hashes: &[Hash]) -> Result<u64> {
    // NOTE: sqlx currently doesn't support bindings collections to queries (but they are working
    // on it: https://github.com/launchbadge/sqlx/issues/875) so we need to build the sql
    // programatically.
    const SQL_TEMPLATE: &str = "
        WITH RECURSIVE
            inner_nodes(hash) AS (
                SELECT i.hash
                    FROM snapshot_inner_nodes AS i
                    INNER JOIN snapshot_root_nodes AS r ON r.hash = i.parent
                    WHERE r.hash IN ({root_hashes})
                UNION ALL
                SELECT c.hash
                    FROM snapshot_inner_nodes AS c
                    INNER JOIN inner_nodes AS p ON p.hash = c.parent
            )
        SELECT COUNT(DISTINCT block_id)
            FROM snapshot_leaf_nodes
            WHERE parent IN inner_nodes
    ";

    let sql = SQL_TEMPLATE.replace(
        "{root_hashes}",
        &iterator::join(root_hashes.iter().map(|_| '?'), ", "),
    );

    let mut query = sqlx::query(&sql);

    for hash in root_hashes {
        query = query.bind(hash);
    }

    let row = query.fetch_one(conn).await?;
    let num = db::decode_u64(row.get(0));

    Ok(num)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crypto::sign::{Keypair, PublicKey},
        index::SingleBlockPresence,
        store::Store,
    };
    use tempfile::TempDir;

    #[tokio::test]
    async fn count_referenced_blocks_empty() {
        let (_base_dir, store) = setup().await;
        let mut reader = store.acquire_read().await.unwrap();
        assert_eq!(
            count_referenced_blocks(reader.raw_mut(), &[])
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn count_referenced_blocks_one_branch() {
        let (_base_dir, store) = setup().await;
        let write_keys = Keypair::random();
        let branch_id = PublicKey::random();

        let mut tx = store.begin_write().await.unwrap();

        assert_eq!(count_referenced_blocks(tx.raw_mut(), &[]).await.unwrap(), 0);

        tx.link_block(
            &branch_id,
            &rand::random(),
            &rand::random(),
            SingleBlockPresence::Present,
            &write_keys,
        )
        .await
        .unwrap();

        let root_hash = tx
            .load_latest_root_node(&branch_id)
            .await
            .unwrap()
            .proof
            .hash;

        assert_eq!(
            count_referenced_blocks(tx.raw_mut(), &[root_hash])
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn count_referenced_blocks_two_branches() {
        let (_base_dir, pool) = setup().await;
        let write_keys = Keypair::random();

        let branch_a_id = PublicKey::random();
        let branch_b_id = PublicKey::random();

        let mut tx = pool.begin_write().await.unwrap();

        // unique blocks
        for branch_id in [&branch_a_id, &branch_b_id] {
            tx.link_block(
                branch_id,
                &rand::random(),
                &rand::random(),
                SingleBlockPresence::Present,
                &write_keys,
            )
            .await
            .unwrap();
        }

        // shared blocks
        let shared_locator = rand::random();
        let shared_block_id = rand::random();

        for branch_id in [&branch_a_id, &branch_b_id] {
            tx.link_block(
                branch_id,
                &shared_locator,
                &shared_block_id,
                SingleBlockPresence::Present,
                &write_keys,
            )
            .await
            .unwrap();
        }

        let root_hash_a = tx
            .load_latest_root_node(&branch_a_id)
            .await
            .unwrap()
            .proof
            .hash;
        let root_hash_b = tx
            .load_latest_root_node(&branch_b_id)
            .await
            .unwrap()
            .proof
            .hash;

        assert_eq!(count_referenced_blocks(tx.raw_mut(), &[]).await.unwrap(), 0);
        assert_eq!(
            count_referenced_blocks(tx.raw_mut(), &[root_hash_a])
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            count_referenced_blocks(tx.raw_mut(), &[root_hash_b])
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            count_referenced_blocks(tx.raw_mut(), &[root_hash_a, root_hash_b])
                .await
                .unwrap(),
            3
        );
    }

    async fn setup() -> (TempDir, Store) {
        let (temp_dir, pool) = db::create_temp().await.unwrap();
        (temp_dir, Store::new(pool))
    }
}
