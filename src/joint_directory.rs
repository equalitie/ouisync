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
    this_replica_id: ReplicaId,
    versions: BTreeMap<ReplicaId, Directory>,
}

impl JointDirectory {
    pub fn new(this_replica_id: ReplicaId) -> Self {
        Self {
            this_replica_id,
            versions: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, directory: Directory) -> Result<()> {
        match self.versions.entry(self.this_replica_id) {
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
        name: &OsStr,
    ) -> Result<JointDirectory> {
        self.versions
            .get_mut(&self.this_replica_id)
            .ok_or(Error::OperationNotSupported)
            .and_then(|dir| dir.create_directory(name.to_owned()))?;

        let mut result = JointDirectory::new(self.this_replica_id);

        for (r_id, dir) in self.versions.iter() {
            // TODO: When r_id == this_replica_id, we can avoid one (the most likely) async call to
            // open_directory() by reusing the directory we created above.
            if let Ok(entry_info) = dir.lookup(name) {
                // Ignore if it's a file
                if let Ok(subdir) = entry_info.open_directory().await {
                    result.versions.insert(*r_id, subdir).unwrap();
                }
            }
        }

        Ok(result)
    }

    pub fn create_file(&mut self, name: OsString) -> Result<File> {
        self.versions
            .get_mut(&self.this_replica_id)
            .ok_or(Error::OperationNotSupported)?
            .create_file(name)
    }

    pub async fn remove_file(&mut self, name: &OsStr) -> Result<()> {
        self.versions
            .get_mut(&self.this_replica_id)
            .ok_or(Error::OperationNotSupported)?
            .remove_file(name)
            .await
    }

    pub async fn remove_directory(&mut self, name: &OsStr) -> Result<()> {
        self.versions
            .get_mut(&self.this_replica_id)
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

    pub fn entries(&self) -> impl Iterator<Item = EntryInfo> {
        self.versions
            .get(&self.this_replica_id)
            .into_iter()
            .flat_map(|dir| dir.entries())
    }
}
