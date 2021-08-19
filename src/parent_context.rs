use crate::{
    directory::{Directory, EntryData},
    error::Result,
    replica_id::ReplicaId
};
use std::sync::Arc;

/// Info about an entry in the context of its parent directory.
#[derive(Clone)]
pub struct ParentContext {
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

    pub async fn entry_data(&self) -> Arc<EntryData> {
        self.directory.get_entry(&self.entry_name, &self.entry_author).await.unwrap()
    }
}
