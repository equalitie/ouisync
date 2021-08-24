use crate::{
    directory::Directory, error::Result, replica_id::ReplicaId, version_vector::VersionVector,
};

/// Info about an entry in the context of its parent directory.
#[derive(Clone)]
pub(crate) struct ParentContext {
    /// The parent directory of the entry.
    pub directory: Directory,
    /// The name of the entry in its parent directory.
    pub entry_name: String,
    /// Author of the particular version of entry, i.e. the ID of the replica last to have
    /// incremented the version vector.
    pub entry_author: ReplicaId,
}

impl ParentContext {
    /// Increment the version of the entry.
    pub async fn increment_version(&self) -> Result<()> {
        self.directory
            .increment_entry_version(&self.entry_name)
            .await
    }

    pub async fn entry_version_vector(&self) -> VersionVector {
        self.directory
            .read()
            .await
            .lookup_version(&self.entry_name, &self.entry_author)
            .unwrap()
            .version_vector()
            .clone()
    }
}
