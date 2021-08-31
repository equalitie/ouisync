use crate::{
    directory::{Directory, ModifyEntry},
    error::Result,
    replica_id::ReplicaId,
    version_vector::VersionVector,
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
    /// Start modifying the entry.
    pub async fn modify(&self) -> Result<ModifyEntry<'_>> {
        self.directory
            .modify_entry(&self.entry_name, &self.entry_author)
            .await
    }

    // TODO: Can this be done without cloning the VersionVector? E.g. by returning some kind of
    // read lock.
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
