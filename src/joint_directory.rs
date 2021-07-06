use crate::{
    directory::Directory,
    entry::EntryType,
    file::File,
    iterator::sorted_union,
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
        match self.versions.entry(directory.global_locator().branch) {
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

    pub fn entries(&self) -> impl Iterator<Item = (OsString, EntryType)> + '_ {
        // Map<ReplicaId, Directory> -> [[EntryInfo]]
        let entries = self
            .versions
            .values()
            .map(|directory| directory.entries());

        // [[EntryInfo]] -> [EntryInfo]
        let flat_entries = sorted_union::new_from_many(entries, |entry| entry.name());

        flat_entries.map(|entry_info|
                         (entry_info.name().to_os_string(), entry_info.entry_type()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db,
        index::Branch,
        Cryptor, Locator
    };

    #[tokio::test(flavor = "multi_thread")]
    async fn test_no_conflict() {
        let (pool, branches) = setup(2).await;

        let mut r0 = Directory::create(
            pool.clone(),
            branches[0].clone(),
            Cryptor::Null,
            Locator::Root,
        );
        let mut r1 = Directory::create(
            pool.clone(),
            branches[1].clone(),
            Cryptor::Null,
            Locator::Root,
        );

        r0.create_file("file0.txt".into())
            .unwrap()
            .flush()
            .await
            .unwrap();
        r1.create_file("file1.txt".into())
            .unwrap()
            .flush()
            .await
            .unwrap();

        r0.flush().await.unwrap();
        r1.flush().await.unwrap();

        let mut root = JointDirectory::new(*branches[0].replica_id());

        root.insert(r0).unwrap();
        root.insert(r1).unwrap();

        assert_eq!(
            root.entries().collect::<Vec<_>>(),
            vec![("file0.txt".into(), EntryType::File), ("file1.txt".into(), EntryType::File)]
        );
    }

    async fn setup(branch_count: usize) -> (db::Pool, Vec<Branch>) {
        let pool = db::init(db::Store::Memory).await.unwrap();

        let mut branches = Vec::new();

        for _ in 0..branch_count {
            let branch = Branch::new(&pool, ReplicaId::random()).await.unwrap();
            branches.push(branch);
        }

        (pool, branches)
    }
}
