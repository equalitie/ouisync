use crate::{directory::Directory, replica_id::ReplicaId, Error, Result};
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct JointDirectory {
    versions: BTreeMap<ReplicaId, Directory>,
}

impl JointDirectory {
    pub fn new() -> Self {
        todo!()
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
}
