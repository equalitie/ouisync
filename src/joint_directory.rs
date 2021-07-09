use crate::{
    directory::{Directory, EntryInfo},
    entry::EntryType,
    file::File,
    iterator::{sorted_union, Accumulate, MaybeIterator},
    joint_entry::JointEntry,
    replica_id::ReplicaId,
    Error, Result,
};
use std::{
    collections::btree_map::{Entry as MapEntry, Values},
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    path::PathBuf,
    slice,
};

#[derive(Clone)]
pub struct JointDirectory {
    versions: BTreeMap<ReplicaId, Directory>,
}

impl JointDirectory {
    pub fn new() -> Self {
        Self {
            versions: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, directory: Directory) {
        match self.versions.entry(directory.global_locator().branch) {
            MapEntry::Vacant(entry) => {
                entry.insert(directory);
                ()
            }
            MapEntry::Occupied(_) => panic!("Double insert into JointDirectory"),
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
        branch: &ReplicaId,
        name: &OsStr,
    ) -> Result<JointDirectory> {
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
            // TODO: Continue with the rest if any fails?
            dir.flush().await?;
        }
        Ok(())
    }

    pub fn entries(&self) -> impl Iterator<Item = (OsString, EntryType)> + '_ {
        self.joint_entries().flat_map(|vs| Self::unique_names(&vs))
    }

    fn joint_entries(&self) -> impl Iterator<Item = JointEntryView> + '_ {
        // Map<ReplicaId, Directory> -> [[(EntryInfo, ReplicaId)]]
        let entries = self.versions.iter().map(|(replica_id, directory)| {
            directory
                .entries()
                .map(move |entry_info| (entry_info, replica_id))
        });

        // [[(EntryInfo, ReplicaId)]] -> [(EntryInfo, ReplicaId)]
        let entries = sorted_union::new_from_many(entries, |(entry, _)| entry.name());

        // [(EntryInfo, ReplicaId)] -> [(name, [(EntryInfo, ReplicaId)])]
        let entries = Accumulate::new(entries, |(entry, _)| entry.name());

        entries.map(|(name, versions)| JointEntryView { name, versions })
    }

    pub async fn cd_into(&self, directory: &'_ OsStr) -> Result<JointDirectory> {
        let mut retval = JointDirectory::new();
        let mut count = 0;

        for (dir, branch) in self.lookup(directory)?.directories() {
            match dir.open_directory().await {
                Ok(dir) => {
                    retval.insert(dir);
                    count += 1;
                }
                Err(e) => {
                    log::warn!(
                        "Failed to load directory {:?} on branch {:?}: {:?}",
                        directory,
                        branch,
                        e
                    );
                }
            }
        }

        if count == 0 {
            return Err(Error::EntryNotDirectory);
        }

        Ok(retval)
    }

    pub async fn cd_into_path(&self, path: &'_ PathBuf) -> Result<JointDirectory> {
        let mut retval = self.clone();

        for name in path {
            retval = retval.cd_into(name).await?;
        }

        Ok(retval)
    }

    pub fn lookup<'a>(&'a self, target_name: &'a OsStr) -> Result<Lookup<'a>> {
        let versions = self
            .joint_entries()
            .find(|v| v.name == target_name)
            .ok_or(Error::EntryNotFound)?;

        // TODO:
        let first = versions.versions[0];

        match first.0.entry_type() {
            EntryType::File => Ok(Lookup::File(first.0, &first.1)),
            EntryType::Directory => Ok(Lookup::Directory(versions)),
        }
    }

    fn unique_names(entry: &JointEntryView) -> BTreeSet<(OsString, EntryType)> {
        assert!(!entry.versions.is_empty());

        if entry.versions.len() == 1 {
            return entry.versions
                .iter()
                .map(|(entry_info, _)| (entry.name.to_os_string(), entry_info.entry_type()))
                .collect();
        }

        entry.versions
            .iter()
            .map(|(entry_info, replica_id)| {
                (Self::add_label(entry.name, replica_id), entry_info.entry_type())
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

type Version<'a> = (EntryInfo<'a>, &'a ReplicaId);

pub enum Lookup<'a> {
    Directory(JointEntryView<'a>),
    File(EntryInfo<'a>, &'a ReplicaId),
}

impl<'a> Lookup<'a> {
    pub async fn open(&self) -> Result<JointEntry> {
        match self {
            Self::Directory(versions) => {
                let mut joint_dir = JointDirectory::new();

                for (entry_info, branch) in versions.directories() {
                    match entry_info.open_directory().await {
                        Ok(dir) => {
                            joint_dir.insert(dir);
                        }
                        Err(e) => {
                            log::warn!(
                                "Failed to open directory {:?} on branch {:?}: {:?}",
                                versions.name,
                                branch,
                                e
                            );
                        }
                    }
                }

                Ok(JointEntry::Directory(joint_dir))
            }
            Self::File(entry_info, _replica_id) => {
                Ok(JointEntry::File(entry_info.open_file().await?))
            }
        }
    }

    pub fn directories(&'a self) -> MaybeIterator<DirectoryVersions<'a>> {
        match self {
            Self::Directory(versions) => MaybeIterator::SomeIterator(versions.directories()),
            Self::File(_, _) => MaybeIterator::NoIterator,
        }
    }
}

pub struct DirectoryVersions<'a> {
    mix: slice::Iter<'a, Version<'a>>,
}

impl<'a> Iterator for DirectoryVersions<'a> {
    type Item = Version<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(r) = self.mix.next() {
            if r.0.entry_type() == EntryType::Directory {
                return Some(*r);
            }
        }
        None
    }
}

pub struct JointEntryView<'a> {
    name: &'a OsStr,
    versions: Vec<Version<'a>>,
}

impl<'a> JointEntryView<'a> {
    fn directories(&'a self) -> DirectoryVersions<'a> {
        DirectoryVersions {
            mix: self.versions.iter(),
        }
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

        root.insert(r0);
        root.insert(r1);

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

        root.insert(r0);
        root.insert(r1);

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
