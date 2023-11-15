use super::{error::Error, Store, WriteTransaction};
use crate::{
    crypto::sign::{Keypair, PublicKey},
    repository::data_version,
};

pub const DATA_VERSION: u64 = 1;

pub(super) async fn run_data(
    store: &Store,
    this_writer_id: PublicKey,
    write_keys: &Keypair,
) -> Result<(), Error> {
    v1::run(store, this_writer_id, write_keys).await?;

    // Ensure we are at the latest version.
    assert_eq!(
        data_version::get(store.acquire_read().await?.db()).await?,
        DATA_VERSION
    );

    Ok(())
}

async fn begin(store: &Store, dst_version: u64) -> Result<Option<WriteTransaction>, Error> {
    let mut tx = store.begin_write().await?;

    let src_version = data_version::get(tx.db()).await?;
    if src_version >= dst_version {
        return Ok(None);
    }

    assert_eq!(
        dst_version,
        src_version + 1,
        "migrations must be applied in order"
    );

    // Bumping the data version before running the migration. This is OK because if the migration
    // fails, it gets rolled back.
    data_version::set(tx.db(), dst_version).await?;

    Ok(Some(tx))
}

/// Recompute block ids so the new ids are computed from both the ciphertext and the nonce (as
/// opposed from only the ciphertext), to protect against nonce tampering.
mod v1 {
    use super::{
        super::{root_node, Changeset, Reader},
        *,
    };
    use crate::{
        crypto::{sign::Keypair, Hash},
        protocol::{BlockContent, BlockId, BlockNonce, LeafNode, Proof, RootNode, RootNodeFilter},
    };
    use futures_util::TryStreamExt;
    use sqlx::Row;

    pub(super) async fn run(
        store: &Store,
        this_writer_id: PublicKey,
        write_keys: &Keypair,
    ) -> Result<(), Error> {
        let Some(mut tx) = begin(store, 1).await? else {
            return Ok(());
        };

        // Temporary table to map old block ids to new block ids.
        sqlx::query(
            "CREATE TEMPORARY TABLE block_id_translations (
                 old_block_id BLOB NOT NULL UNIQUE,
                 new_block_id BLOB NOT NULL UNIQUE
             )",
        )
        .execute(tx.db())
        .await?;

        recompute_block_ids(&mut tx).await?;
        recompute_index_hashes(&mut tx, this_writer_id, write_keys).await?;

        // Remove the temp table
        sqlx::query("DROP TABLE block_id_translations")
            .execute(tx.db())
            .await?;

        tx.commit().await?;

        Ok(())
    }

    async fn recompute_block_ids(tx: &mut WriteTransaction) -> Result<(), Error> {
        loop {
            let map: Vec<_> = sqlx::query(
                "SELECT id, nonce, content
                 FROM blocks
                 WHERE id NOT IN (SELECT new_block_id FROM block_id_translations)
                 LIMIT 1024",
            )
            .fetch(tx.db())
            .try_filter_map(|row| async move {
                let old_id: BlockId = row.get(0);

                let nonce: &[u8] = row.get(1);
                let nonce = BlockNonce::try_from(nonce)
                    .map_err(|error| sqlx::Error::Decode(error.into()))?;

                let mut content = BlockContent::new();
                content.copy_from_slice(row.get(2));

                let new_id = BlockId::new(&content, &nonce);

                if new_id != old_id {
                    Ok(Some((old_id, new_id)))
                } else {
                    Ok(None)
                }
            })
            .try_collect()
            .await?;

            if map.is_empty() {
                break;
            }

            for (old_id, new_id) in map {
                sqlx::query("UPDATE blocks SET id = ? WHERE id = ?")
                    .bind(&new_id)
                    .bind(&old_id)
                    .execute(tx.db())
                    .await?;

                sqlx::query(
                    "INSERT INTO block_id_translations (old_block_id, new_block_id) VALUES (?, ?)",
                )
                .bind(&old_id)
                .bind(&new_id)
                .execute(tx.db())
                .await?;
            }
        }

        Ok(())
    }

    async fn recompute_index_hashes(
        tx: &mut WriteTransaction,
        this_writer_id: PublicKey,
        write_keys: &Keypair,
    ) -> Result<(), Error> {
        let root_nodes: Vec<_> = root_node::load_all(tx.db()).try_collect().await?;

        for root_node in root_nodes {
            recompute_index_hashes_at(tx, root_node, this_writer_id, write_keys).await?;
        }

        Ok(())
    }

    async fn recompute_index_hashes_at(
        tx: &mut WriteTransaction,
        root_node: RootNode,
        this_writer_id: PublicKey,
        write_keys: &Keypair,
    ) -> Result<(), Error> {
        let mut last_locator = Hash::from([0; Hash::SIZE]);

        loop {
            let leaf_nodes = load_leaf_nodes(tx, &root_node, &last_locator, 1024).await?;
            if leaf_nodes.is_empty() {
                break;
            }

            let mut changeset = Changeset::new();

            // Link the locators to the new block ids
            for leaf_node in leaf_nodes {
                changeset.link_block(
                    leaf_node.locator,
                    leaf_node.block_id,
                    leaf_node.block_presence,
                );

                last_locator = leaf_node.locator;
            }

            changeset
                .apply(tx, &root_node.proof.writer_id, write_keys)
                .await?;
        }

        let new_root_node = root_node::load(tx.db(), &root_node.proof.writer_id).await?;
        let new_root_node = if new_root_node.proof.writer_id == this_writer_id {
            // Bump the vv of the local branch
            let hash = new_root_node.proof.hash;
            let version_vector = new_root_node
                .proof
                .into_version_vector()
                .incremented(this_writer_id);
            let proof = Proof::new(this_writer_id, version_vector, hash, write_keys);
            let (new_root_node, _) =
                root_node::create(tx.db(), proof, new_root_node.summary, RootNodeFilter::Any)
                    .await?;

            new_root_node
        } else {
            new_root_node
        };

        // Remove the original snapshot and any intermediate snapshots created during the migration
        // (child nodes are removed by db triggers).
        sqlx::query(
            "DELETE FROM snapshot_root_nodes
             WHERE writer_id = ? AND snapshot_id >= ? AND snapshot_id < ?",
        )
        .bind(&root_node.proof.writer_id)
        .bind(root_node.snapshot_id)
        .bind(new_root_node.snapshot_id)
        .execute(tx.db())
        .await?;

        Ok(())
    }

    // Load batch of leaf nodes belonging to the given root node with their block ids translated
    // from the old ones to the new ones.
    async fn load_leaf_nodes(
        r: &mut Reader,
        root_node: &RootNode,
        last_locator: &Hash,
        batch_size: u32,
    ) -> Result<Vec<LeafNode>, Error> {
        sqlx::query(
            "WITH RECURSIVE
                 inner_nodes(hash) AS (
                     SELECT i.hash
                         FROM snapshot_inner_nodes AS i
                             INNER JOIN snapshot_root_nodes AS r ON r.hash = i.parent
                         WHERE r.snapshot_id = ?
                     UNION ALL
                     SELECT c.hash
                         FROM snapshot_inner_nodes AS c
                             INNER JOIN inner_nodes AS p ON p.hash = c.parent
                 )
             SELECT l.locator, l.block_presence, t.new_block_id
                 FROM snapshot_leaf_nodes AS l
                     INNER JOIN block_id_translations AS t ON t.old_block_id = l.block_id
                 WHERE l.parent IN inner_nodes AND l.locator > ?
                 ORDER BY l.locator
                 LIMIT ?
             ",
        )
        .bind(root_node.snapshot_id)
        .bind(last_locator)
        .bind(batch_size)
        .fetch(r.db())
        .map_ok(|row| LeafNode {
            locator: row.get(0),
            block_id: row.get(2),
            block_presence: row.get(1),
        })
        .err_into()
        .try_collect()
        .await
    }
}
