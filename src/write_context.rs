use crate::{
    directory::{Directory, EntryData},
    error::Result,
};
use std::sync::Arc;

/// Info about an entry in the context of its parent directory.
#[derive(Clone)]
pub struct ParentContext {
    /// The parent directory of the entry.
    pub directory: Directory,
    /// The name of the entry in its parent directory.
    pub entry_name: String,
    /// The entry data in its parent directory.
    pub entry_data: Arc<EntryData>,
    // TODO: Should this be std::sync::Weak?
}

impl ParentContext {
    /// Increment the version of the entry.
    pub async fn increment_version(&self) -> Result<()> {
        self.directory
            .increment_entry_version(&self.entry_name)
            .await
    }
}
