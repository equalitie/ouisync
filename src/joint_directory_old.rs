use crate::{directory::Directory, file::File, replica_id::ReplicaId, Error, Result};
use std::{collections::btree_map::Values, collections::BTreeMap};

#[derive(Clone)]
pub struct JointDirectory {
    versions: BTreeMap<ReplicaId, Directory>,
}

impl JointDirectory {
    pub fn new() -> Self {
        todo!()
    }

    pub fn get(&self, replica_id: &ReplicaId) -> Option<&Directory> {
        self.versions.get(replica_id)
    }

    pub fn get_mut(&mut self, replica_id: &ReplicaId) -> Option<&mut Directory> {
        self.versions.get_mut(replica_id)
    }

    pub fn values(&self) -> Values<'_, ReplicaId, Directory> {
        self.versions.values()
    }

    pub async fn create_directory(
        &mut self,
        branch: &ReplicaId,
        name: &str,
    ) -> Result<JointDirectory> {
        let new_dir = self
            .versions
            .get_mut(branch)
            .ok_or(Error::OperationNotSupported)
            .and_then(|dir| dir.create_directory(name.to_owned()))?;

        let mut result = JointDirectory::new();

        result.versions.insert(*branch, new_dir);

        for (r_id, dir) in self.versions.iter() {
            if r_id == branch {
                // This is the one we already inserted above.
                continue;
            }
            if let Ok(versions) = dir.lookup(name) {
                for entry_info in versions {
                    // Ignore if it's a file
                    if let Ok(subdir) = entry_info.directory()?.open().await {
                        // TODO: Once we have version vectors in place, ensure here that we only
                        // replace existing versions if the new one "happened after".  NOTE: that
                        // they won't be concurrent as one replica can't create concurrent versions
                        // of the same directory (where that replica is the author).
                        result.versions.insert(*r_id, subdir).unwrap();
                    }
                }
            }
        }

        Ok(result)
    }

    pub fn create_file(&mut self, branch: &ReplicaId, name: String) -> Result<File> {
        self.versions
            .get_mut(branch)
            .ok_or(Error::OperationNotSupported)?
            .create_file(name)
    }

    pub async fn remove_file(&mut self, branch: &ReplicaId, name: &str) -> Result<()> {
        self.versions
            .get_mut(branch)
            .ok_or(Error::OperationNotSupported)?
            .remove_file(name)
            .await
    }

    pub async fn remove_directory(&mut self, branch: &ReplicaId, name: &str) -> Result<()> {
        self.versions
            .get_mut(branch)
            .ok_or(Error::OperationNotSupported)?
            .remove_directory(name)
            .await
    }

    pub async fn flush(&mut self) -> Result<()> {
        for dir in self.versions.values_mut() {
            // TODO: Continue with the rest if any fails?
            dir.flush().await?;
        }
        Ok(())
    }
}

impl Default for JointDirectory {
    fn default() -> Self {
        Self::new()
    }
}
