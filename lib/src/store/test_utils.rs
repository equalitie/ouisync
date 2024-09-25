use super::{client::CommitStatus, ClientWriter, Store};
use crate::{
    crypto::sign::{Keypair, PublicKey},
    protocol::{test_utils::Snapshot, MultiBlockPresence, Proof},
    version_vector::VersionVector,
};

/// Test helper for writing `Snapshot`s to the store.
pub(crate) struct SnapshotWriter<'a> {
    writer: ClientWriter,
    snapshot: &'a Snapshot,
}

impl<'a> SnapshotWriter<'a> {
    pub async fn begin(store: &Store, snapshot: &'a Snapshot) -> Self {
        let writer = store.begin_client_write().await.unwrap();
        Self { writer, snapshot }
    }

    pub async fn save_nodes(
        self,
        write_keys: &Keypair,
        branch_id: PublicKey,
        version_vector: VersionVector,
    ) -> Self {
        self.save_root_nodes(write_keys, branch_id, version_vector)
            .await
            .save_inner_nodes()
            .await
            .save_leaf_nodes()
            .await
    }

    pub async fn save_root_nodes(
        mut self,
        write_keys: &Keypair,
        branch_id: PublicKey,
        version_vector: VersionVector,
    ) -> Self {
        let proof = Proof::new(
            branch_id,
            version_vector,
            *self.snapshot.root_hash(),
            write_keys,
        );
        self.writer
            .save_root_node(proof, &MultiBlockPresence::Full)
            .await
            .unwrap();
        self
    }

    pub async fn save_inner_nodes(mut self) -> Self {
        for (_, nodes) in self.snapshot.inner_sets() {
            self.writer
                .save_inner_nodes(nodes.clone().into())
                .await
                .unwrap();
        }

        self
    }

    pub async fn save_leaf_nodes(mut self) -> Self {
        for (_, nodes) in self.snapshot.leaf_sets() {
            self.writer
                .save_leaf_nodes(nodes.clone().into())
                .await
                .unwrap();
        }

        self
    }

    pub async fn save_blocks(mut self) -> Self {
        for block in self.snapshot.blocks().values() {
            self.writer.save_block(block, None).await.unwrap();
        }
        self
    }

    pub async fn commit(self) -> CommitStatus {
        self.writer.commit().await.unwrap()
    }

    pub fn client_writer(&mut self) -> &mut ClientWriter {
        &mut self.writer
    }
}
