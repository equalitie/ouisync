use crate::{
    directory::{Directory, EntryInfo},
    entry::EntryType,
    file::File,
    iterator::{accumulate::Accumulate, sorted_union},
    replica_id::ReplicaId,
    Error, Result,
};
use std::{
    collections::btree_map::{Entry, Values},
    collections::{BTreeMap, BTreeSet},
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

    pub async fn create_directory(&mut self, branch: &ReplicaId, name: &OsStr) -> Result<JointDirectory> {
        self.versions
            .get_mut(branch)
            .ok_or(Error::OperationNotSupported)
            .and_then(|dir| dir.create_directory(name.to_owned()))?;

        let mut result = JointDirectory::new();

        for (r_id, dir) in self.versions.iter() {
            // TODO: When r_id == branch, we can avoid one (the most likely) async call to
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

    pub fn create_file(&mut self, branch: &ReplicaId, name: OsString) -> Result<File> {
        self.versions
            .get_mut(branch)
            .ok_or(Error::OperationNotSupported)?
            .create_file(name)
    }

    pub async fn remove_file(&mut self, branch: &ReplicaId, name: &OsStr) -> Result<()> {
        self.versions
            .get_mut(branch)
            .ok_or(Error::OperationNotSupported)?
            .remove_file(name)
            .await
    }

    pub async fn remove_directory(&mut self, branch: &ReplicaId, name: &OsStr) -> Result<()> {
        self.versions
            .get_mut(branch)
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
        // Map<ReplicaId, Directory> -> [[(EntryInfo, ReplicaId)]]
        let entries = self.versions.iter().map(|(replica_id, directory)| {
            directory
                .entries()
                .map(move |entry_info| (entry_info, *replica_id))
        });

        // [[(EntryInfo, ReplicaId)]] -> [(EntryInfo, ReplicaId)]
        let entries = sorted_union::new_from_many(entries, |(entry, _)| entry.name());

        // [(EntryInfo, ReplicaId)] -> [(name, [(EntryInfo, ReplicaId)])]
        let entries = Accumulate::new(entries, |(entry, _)| entry.name());

        entries.flat_map(|(base_name, entries)| Self::unique_names(base_name, &entries))
    }

    fn unique_names(
        base_name: &OsStr,
        entries: &[(EntryInfo, ReplicaId)],
    ) -> BTreeSet<(OsString, EntryType)> {
        assert!(!entries.is_empty());

        if entries.len() == 1 {
            return entries
                .iter()
                .map(|(entry_info, _)| (base_name.to_os_string(), entry_info.entry_type()))
                .collect();
        }

        entries
            .iter()
            .map(|(entry_info, replica_id)| {
                (
                    Self::add_label(base_name, replica_id),
                    entry_info.entry_type(),
                )
            })
            .collect()
    }

    fn add_label(name: &OsStr, replica_id: &ReplicaId) -> OsString {
        let mut s = name.to_os_string();
        s.push("-");
        s.push(Self::replica_id_to_label(replica_id));
        s
    }

    fn replica_id_to_label(replica_id: &ReplicaId) -> OsString {
        let r = replica_id.as_ref();
        OsString::from(format!("{:02x}{:02x}{:02x}{:02x}", r[0], r[1], r[2], r[3]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, index::Branch, Cryptor, Locator};

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

        let mut root = JointDirectory::new();

        root.insert(r0).unwrap();
        root.insert(r1).unwrap();

        assert_eq!(
            root.entries().collect::<Vec<_>>(),
            vec![
                ("file0.txt".into(), EntryType::File),
                ("file1.txt".into(), EntryType::File)
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_conflict() {
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

        r0.create_file("file.txt".into())
            .unwrap()
            .flush()
            .await
            .unwrap();
        r1.create_file("file.txt".into())
            .unwrap()
            .flush()
            .await
            .unwrap();

        r0.flush().await.unwrap();
        r1.flush().await.unwrap();

        let mut root = JointDirectory::new();

        root.insert(r0).unwrap();
        root.insert(r1).unwrap();

        let entries = root.entries().collect::<Vec<_>>();

        assert_eq!(entries.len(), 2);

        assert!(entries[0].0 < entries[1].0); // Unique and ordered alphabetically

        assert_eq!(entries[0].1, EntryType::File);
        assert_eq!(entries[1].1, EntryType::File);
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
