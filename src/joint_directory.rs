use crate::{
    directory::{Directory, EntryInfo},
    file::File,
    replica_id::ReplicaId,
    Error, Result,
};
use std::{
    collections::btree_map::{Entry, Values},
    collections::BTreeMap,
    ffi::{OsStr, OsString},
};

pub struct JointDirectory {
    versions: BTreeMap<ReplicaId, Directory>,
}

impl JointDirectory {
    pub fn new() -> Self {
        Self {
            versions: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, replica_id: ReplicaId, directory: Directory) -> Result<()> {
        match self.versions.entry(replica_id) {
            Entry::Vacant(entry) => {
                entry.insert(directory);
                Ok(())
            }
            Entry::Occupied(_) => Err(Error::EntryExists),
        }
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
        replica_id: &ReplicaId,
        name: &OsStr,
    ) -> Result<JointDirectory> {
        self.versions
            .get_mut(replica_id)
            .ok_or(Error::OperationNotSupported)
            .and_then(|dir| dir.create_directory(name.to_owned()))?;

        let mut result = JointDirectory::new();

        for (r_id, dir) in self.versions.iter() {
            // TODO: When r_id == replica_id, we can avoid one (the most likely) async call to
            // open_directory() by reusing the directory we created above.
            if let Ok(entry_info) = dir.lookup(name) {
                // Ignore if it's a file
                if let Ok(subdir) = entry_info.open_directory().await {
                    result.insert(*r_id, subdir).unwrap();
                }
            }
        }

        Ok(result)
    }

    pub fn create_file(&mut self, replica_id: &ReplicaId, name: OsString) -> Result<File> {
        self.versions
            .get_mut(replica_id)
            .ok_or(Error::OperationNotSupported)?
            .create_file(name)
    }

    pub async fn remove_file(&mut self, replica_id: &ReplicaId, name: &OsStr) -> Result<()> {
        self.versions
            .get_mut(replica_id)
            .ok_or(Error::OperationNotSupported)?
            .remove_file(name)
            .await
    }

    pub async fn remove_directory(&mut self, replica_id: &ReplicaId, name: &OsStr) -> Result<()> {
        self.versions
            .get_mut(replica_id)
            .ok_or(Error::OperationNotSupported)?
            .remove_directory(name)
            .await
    }

    pub async fn flush(&mut self) -> Result<()> {
        for dir in self.versions.values_mut() {
            dir.flush().await?;
        }
        Ok(())
    }

    pub fn entries(&self, replica_id: &ReplicaId) -> impl Iterator<Item = EntryInfo> {
        self.versions
            .get(replica_id)
            .into_iter()
            .flat_map(|dir| dir.entries())
    }
}
