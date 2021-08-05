use crate::{
    directory::{Directory, DirectoryRef, EntryRef, FileRef},
    entry_type::EntryType,
    error::{Error, Result},
    iterator::{Accumulate, SortedUnion},
    replica_id::ReplicaId,
    versioned_file_name,
};
use camino::{Utf8Component, Utf8Path};
use futures_util::future;
use std::{
    collections::{btree_map::Entry, BTreeMap},
    fmt, iter, mem,
};

/// Unified view over multiple concurrent versions of a directory.
pub struct JointDirectory {
    versions: BTreeMap<ReplicaId, Directory>,
}

impl JointDirectory {
    pub fn new<I>(versions: I) -> Self
    where
        I: IntoIterator<Item = Directory>,
    {
        Self {
            versions: versions
                .into_iter()
                .map(|dir| (*dir.branch().id(), dir))
                .collect(),
        }
    }

    /// Returns iterator over the entries of this directory. Multiple concurrent versions of the
    /// same file are returned as separate `JointEntryRef::File` entries. Multiple concurrent
    /// versions of the same directory are returned as a single `JointEntryRef::Directory` entry.
    pub fn entries(&self) -> impl Iterator<Item = JointEntryRef> {
        let entries = self.versions.values().map(|directory| directory.entries());
        let entries = SortedUnion::new(entries, |entry| entry.name());
        let entries = Accumulate::new(entries, |entry| entry.name());

        entries.flat_map(|(_, entries)| Merge::new(entries.into_iter()))
    }

    /// Returns all versions of an entry with the given name. Concurrent file versions are returned
    /// separately but concurrent directory versions are merged into a single `JointDirectory`.
    // TODO: try to find a way to avoid needing `name` to outlive the return value as this prevents
    //       us to pass in a temporary which hurts ergonomy.
    pub fn lookup<'a>(&'a self, name: &'a str) -> impl Iterator<Item = JointEntryRef<'a>> {
        Merge::new(
            self.versions
                .values()
                .flat_map(move |dir| dir.lookup(name).ok().into_iter().flatten()),
        )
    }

    /// Looks up single entry with the specified name if it is unique.
    ///
    /// - If there is only one version of a entry with the specified name, it is returned.
    /// - If there are multiple versions and all of them are files, an `AmbiguousEntry` error is
    ///   returned. To lookup a single version, include a disambiguator in the `name`.
    /// - If there are multiple versiond and all of them are directories, they are merged into a
    ///   single `JointEntryRef::Directory` and returned.
    /// - Finally, if there are both files and directories, only the directories are retured (merged
    ///   into a `JointEntryRef::Directory`) and the files are discarded. This is so it's possible
    ///   to unambiguously lookup a directory even in the presence of conflicting files.
    pub fn lookup_unique<'a>(&'a self, name: &'a str) -> Result<JointEntryRef<'a>> {
        // First try exact match as it is more common.
        let mut last_file = None;

        for entry in self.lookup(name) {
            match entry {
                JointEntryRef::Directory(_) => return Ok(entry),
                JointEntryRef::File(_) if last_file.is_none() => {
                    last_file = Some(entry);
                }
                JointEntryRef::File(_) => return Err(Error::AmbiguousEntry),
            }
        }

        if let Some(entry) = last_file {
            return Ok(entry);
        }

        // If not found, extract the disambiguator and try to lookup an entry whose branch id
        // matches it.
        let (name, branch_id_prefix) = versioned_file_name::parse(name);
        let branch_id_prefix = branch_id_prefix.ok_or(Error::EntryNotFound)?;

        let mut entries = self
            .versions
            .values()
            .flat_map(|dir| dir.lookup(name).ok().into_iter().flatten())
            .filter_map(|entry| entry.file().ok())
            .filter(|entry| entry.branch_id().starts_with(&branch_id_prefix));

        let first = entries.next().ok_or(Error::EntryNotFound)?;

        if entries.next().is_none() {
            Ok(JointEntryRef::File(first))
        } else {
            Err(Error::AmbiguousEntry)
        }
    }

    /// Looks up a specific version of a file.
    pub fn lookup_version(&self, name: &'_ str, branch_id: &'_ ReplicaId) -> Result<FileRef> {
        let mut entries = self
            .versions
            .values()
            .filter_map(|dir| dir.lookup_version(name, branch_id).ok())
            .filter_map(|entry| entry.file().ok());

        let first = entries.next().ok_or(Error::EntryNotFound)?;

        if entries.next().is_none() {
            Ok(first)
        } else {
            // TODO: fetch the most recent one instead
            Err(Error::AmbiguousEntry)
        }
    }

    /// Descends into an arbitrarily nested subdirectory of this directory at the specified path.
    /// Note: non-normalized paths (i.e. containing "..") or Windows-style drive prefixes
    /// (e.g. "C:") are not supported.
    // TODO: as this consumes `self`, we should return `self` back in case of an error.
    pub async fn cd(self, path: impl AsRef<Utf8Path>) -> Result<Self> {
        let mut curr = self;

        for component in path.as_ref().components() {
            match component {
                Utf8Component::RootDir | Utf8Component::CurDir => (),
                Utf8Component::Normal(name) => {
                    let next = curr
                        .lookup(name)
                        .find_map(|entry| entry.directory().ok())
                        .ok_or(Error::EntryNotFound)?
                        .open()
                        .await?;
                    curr = next;
                }
                Utf8Component::ParentDir | Utf8Component::Prefix(_) => {
                    return Err(Error::OperationNotSupported)
                }
            }
        }

        Ok(curr)
    }

    /// Length of the directory in bytes. If there are multiple versions, returns the sum of their
    /// lengths.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> u64 {
        self.versions.values().map(|dir| dir.len()).sum()
    }

    /// Creates a subdirectory of this directory owned by `branch` and returns it as
    /// `JointDirectory` which would already include all previousy existing versions.
    pub async fn create_directory(&mut self, branch: &ReplicaId, name: &str) -> Result<Self> {
        let mut old_dir =
            if let Some(entry) = self.lookup(name).find_map(|entry| entry.directory().ok()) {
                entry.open().await?
            } else {
                Self::new(iter::empty())
            };

        match old_dir.versions.entry(*branch) {
            Entry::Vacant(entry) => {
                let new_version = self
                    .versions
                    .get_mut(branch)
                    .ok_or(Error::EntryNotFound)?
                    .create_directory(name.to_owned())?;
                entry.insert(new_version);
            }
            Entry::Occupied(_) => return Err(Error::EntryExists),
        }

        Ok(old_dir)
    }

    pub async fn remove_file(&mut self, branch: &ReplicaId, name: &str) -> Result<()> {
        self.versions
            .get_mut(branch)
            .ok_or(Error::EntryNotFound)?
            .remove_file(name)
            .await
    }

    pub async fn remove_directory(&mut self, branch: &ReplicaId, name: &str) -> Result<()> {
        self.versions
            .get_mut(branch)
            .ok_or(Error::EntryNotFound)?
            .remove_directory(name)
            .await
    }

    pub async fn flush(&mut self) -> Result<()> {
        future::try_join_all(self.versions.values_mut().map(|dir| dir.flush())).await?;
        Ok(())
    }
}

impl fmt::Debug for JointDirectory {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("JointDirectory").finish()
    }
}

#[derive(Eq, PartialEq, Debug)]
pub enum JointEntryRef<'a> {
    File(FileRef<'a>),
    Directory(JointDirectoryRef<'a>),
}

impl<'a> JointEntryRef<'a> {
    pub fn name(&self) -> &'a str {
        match self {
            Self::File(r) => r.name(),
            Self::Directory(r) => r.name(),
        }
    }

    pub fn entry_type(&self) -> EntryType {
        match self {
            Self::File { .. } => EntryType::File,
            Self::Directory(_) => EntryType::Directory,
        }
    }

    pub fn file(self) -> Result<FileRef<'a>> {
        match self {
            Self::File(r) => Ok(r),
            Self::Directory(_) => Err(Error::EntryIsDirectory),
        }
    }

    pub fn directory(self) -> Result<JointDirectoryRef<'a>> {
        match self {
            Self::Directory(r) => Ok(r),
            Self::File(_) => Err(Error::EntryNotDirectory),
        }
    }
}

#[derive(Eq, PartialEq)]
pub struct JointDirectoryRef<'a>(Vec<DirectoryRef<'a>>);

impl<'a> JointDirectoryRef<'a> {
    fn new(versions: Vec<DirectoryRef<'a>>) -> Option<Self> {
        if versions.is_empty() {
            None
        } else {
            Some(Self(versions))
        }
    }

    pub fn name(&self) -> &'a str {
        self.0
            .first()
            .expect("joint directory must contain at least one directory")
            .name()
    }

    pub async fn open(&self) -> Result<JointDirectory> {
        let directories = future::try_join_all(self.0.iter().map(|dir| dir.open())).await?;
        Ok(JointDirectory::new(directories))
    }
}

impl fmt::Debug for JointDirectoryRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("JointDirectoryRef")
            .field("name", &self.name())
            .finish()
    }
}

// Iterator adaptor that maps iterator of `EntryRef` to iterator of `JointEntryRef` by mering all
// `EntryRef::Directory` items into a single `JointDirectoryRef` item but keeping `EntryRef::File`
// items separate.
struct Merge<'a, I> {
    entries: I,
    directories: Vec<DirectoryRef<'a>>,
}

impl<'a, I> Merge<'a, I>
where
    I: Iterator<Item = EntryRef<'a>>,
{
    fn new(entries: I) -> Self {
        Self {
            entries,
            directories: vec![],
        }
    }
}

impl<'a, I> Iterator for Merge<'a, I>
where
    I: Iterator<Item = EntryRef<'a>>,
{
    type Item = JointEntryRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        for entry in &mut self.entries {
            match entry {
                EntryRef::File(file) => return Some(JointEntryRef::File(file)),
                EntryRef::Directory(dir) => {
                    self.directories.push(dir);
                }
            }
        }

        JointDirectoryRef::new(mem::take(&mut self.directories)).map(JointEntryRef::Directory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        branch::Branch, crypto::Cryptor, db, directory::Directory, file::File, index::BranchData,
        write_context::WriteContext,
    };
    use assert_matches::assert_matches;
    use futures_util::future;

    #[tokio::test(flavor = "multi_thread")]
    async fn no_conflict() {
        let branches = setup(2).await;

        let mut root0 = Directory::create_root(branches[0].clone());
        root0
            .create_file("file0.txt".to_owned())
            .unwrap()
            .flush()
            .await
            .unwrap();
        root0.flush().await.unwrap();

        let mut root1 = Directory::create_root(branches[1].clone());
        root1
            .create_file("file1.txt".to_owned())
            .unwrap()
            .flush()
            .await
            .unwrap();
        root1.flush().await.unwrap();

        let root = JointDirectory::new(vec![root0, root1]);

        let entries: Vec<_> = root.entries().collect();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name(), "file0.txt");
        assert_eq!(entries[0].entry_type(), EntryType::File);
        assert_eq!(entries[1].name(), "file1.txt");
        assert_eq!(entries[1].entry_type(), EntryType::File);

        assert_eq!(root.lookup("file0.txt").collect::<Vec<_>>(), entries[0..1]);
        assert_eq!(root.lookup("file1.txt").collect::<Vec<_>>(), entries[1..2]);

        assert_eq!(root.lookup_unique("file0.txt").unwrap(), entries[0]);
        assert_eq!(root.lookup_unique("file1.txt").unwrap(), entries[1]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn conflict_independent_files() {
        let branches = setup(2).await;

        let mut root0 = Directory::create_root(branches[0].clone());
        root0
            .create_file("file.txt".to_owned())
            .unwrap()
            .flush()
            .await
            .unwrap();
        root0.flush().await.unwrap();

        let mut root1 = Directory::create_root(branches[1].clone());
        root1
            .create_file("file.txt".to_owned())
            .unwrap()
            .flush()
            .await
            .unwrap();
        root1.flush().await.unwrap();

        let root = JointDirectory::new(vec![root0, root1]);

        let files: Vec<_> = root.entries().map(|entry| entry.file().unwrap()).collect();
        assert_eq!(files.len(), 2);

        for branch in &branches {
            let file = files
                .iter()
                .find(|file| file.branch_id() == branch.id())
                .unwrap();
            assert_eq!(file.name(), "file.txt");

            assert_eq!(
                root.lookup_unique(&versioned_file_name::create("file.txt", branch.id()))
                    .unwrap(),
                JointEntryRef::File(*file)
            );
        }

        let files: Vec<_> = root
            .lookup("file.txt")
            .map(|entry| entry.file().unwrap())
            .collect();
        assert_eq!(files.len(), 2);

        for branch in &branches {
            let file = files
                .iter()
                .find(|file| file.branch_id() == branch.id())
                .unwrap();
            assert_eq!(file.name(), "file.txt");
        }

        assert_matches!(root.lookup_unique("file.txt"), Err(Error::AmbiguousEntry));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn conflict_forked_files() {
        let branches = setup(2).await;

        let mut root0 = Directory::create_root(branches[0].clone());
        let mut file0 = root0.create_file("file.txt".to_owned()).unwrap();
        file0.flush().await.unwrap();
        root0.flush().await.unwrap();

        let mut root1 = Directory::create_root(branches[1].clone());
        root1.flush().await.unwrap();

        // Open the file with branch 1 as the local brach and then modify it which copies (forks) it
        // into that branch.
        let mut file1 = File::open(
            branches[0].clone(),
            *file0.locator(),
            WriteContext::new("/file.txt".into(), branches[1].clone()),
        )
        .await
        .unwrap();
        file1.write(&[0]).await.unwrap();
        file1.flush().await.unwrap();

        let root = JointDirectory::new(vec![root0, root1]);

        let files: Vec<_> = root.entries().map(|entry| entry.file().unwrap()).collect();

        assert_eq!(files.len(), 2);

        for branch in &branches {
            let file = files
                .iter()
                .find(|file| file.branch_id() == branch.id())
                .unwrap();
            assert_eq!(file.name(), "file.txt");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn conflict_directories() {
        let branches = setup(2).await;

        let mut root0 = Directory::create_root(branches[0].clone());

        let mut dir0 = root0.create_directory("dir".to_owned()).unwrap();
        dir0.flush().await.unwrap();
        root0.flush().await.unwrap();

        let mut root1 = Directory::create_root(branches[1].clone());

        let mut dir1 = root1.create_directory("dir".to_owned()).unwrap();
        dir1.flush().await.unwrap();
        root1.flush().await.unwrap();

        let root = JointDirectory::new(vec![root0, root1]);

        let directories: Vec<_> = root
            .entries()
            .map(|entry| entry.directory().unwrap())
            .collect();
        assert_eq!(directories.len(), 1);
        assert_eq!(directories[0].name(), "dir");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn conflict_file_and_directory() {
        let branches = setup(2).await;

        let mut root0 = Directory::create_root(branches[0].clone());

        let mut file0 = root0.create_file("config".to_owned()).unwrap();
        file0.flush().await.unwrap();
        root0.flush().await.unwrap();

        let mut root1 = Directory::create_root(branches[1].clone());

        let mut dir1 = root1.create_directory("config".to_owned()).unwrap();
        dir1.flush().await.unwrap();
        root1.flush().await.unwrap();

        let root = JointDirectory::new(vec![root0, root1]);

        let entries: Vec<_> = root.entries().collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries.iter().map(|entry| entry.name()).collect::<Vec<_>>(),
            ["config", "config"]
        );
        assert!(entries.iter().any(|entry| match entry {
            JointEntryRef::File(file) => file.branch_id() == branches[0].id(),
            JointEntryRef::Directory(_) => false,
        }));
        assert!(entries
            .iter()
            .any(|entry| entry.entry_type() == EntryType::Directory));

        let entries: Vec<_> = root.lookup("config").collect();
        assert_eq!(entries.len(), 2);

        let entry = root.lookup_unique("config").unwrap();
        assert_eq!(entry.entry_type(), EntryType::Directory);

        let name = versioned_file_name::create("config", branches[0].id());
        let entry = root.lookup_unique(&name).unwrap();
        assert_eq!(entry.entry_type(), EntryType::File);
        assert_eq!(entry.file().unwrap().branch_id(), branches[0].id());
    }

    // TODO: test conflict_forked_directories
    // TODO: test conflict_multiple_files_and_directories
    // TODO: test conflict_file_with_name_containing_branch_prefix

    #[tokio::test(flavor = "multi_thread")]
    async fn cd_into_concurrent_directory() {
        let branches = setup(2).await;

        let mut root0 = Directory::create_root(branches[0].clone());

        let mut dir0 = root0.create_directory("pics".to_owned()).unwrap();
        let mut file0 = dir0.create_file("dog.jpg".to_owned()).unwrap();

        file0.flush().await.unwrap();
        dir0.flush().await.unwrap();
        root0.flush().await.unwrap();

        let mut root1 = Directory::create_root(branches[1].clone());

        let mut dir1 = root1.create_directory("pics".to_owned()).unwrap();
        let mut file1 = dir1.create_file("cat.jpg".to_owned()).unwrap();

        file1.flush().await.unwrap();
        dir1.flush().await.unwrap();
        root1.flush().await.unwrap();

        let root = JointDirectory::new(vec![root0, root1]);
        let dir = root.cd("pics").await.unwrap();

        let entries: Vec<_> = dir.entries().collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name(), "cat.jpg");
        assert_eq!(entries[1].name(), "dog.jpg");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_directory_with_existing_versions() {
        let branches = setup(2).await;

        let mut root0 = Directory::create_root(branches[0].clone());

        let mut dir0 = root0.create_directory("pics".to_owned()).unwrap();
        let mut file0 = dir0.create_file("dog.jpg".to_owned()).unwrap();

        file0.flush().await.unwrap();
        dir0.flush().await.unwrap();
        root0.flush().await.unwrap();

        let root1 = Directory::create_root(branches[1].clone());

        let mut root = JointDirectory::new(vec![root0, root1]);

        let dir = root
            .create_directory(branches[1].id(), "pics")
            .await
            .unwrap();

        let entries: Vec<_> = dir.entries().collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name(), "dog.jpg");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_directory_without_existing_versions() {
        let branches = setup(1).await;

        let root0 = Directory::create_root(branches[0].clone());

        let mut root = JointDirectory::new(vec![root0]);

        let dir = root
            .create_directory(branches[0].id(), "pics")
            .await
            .unwrap();

        assert_eq!(dir.entries().count(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn attempt_to_create_directory_whose_local_version_already_exists() {
        let branches = setup(2).await;

        let mut root0 = Directory::create_root(branches[0].clone());

        let mut dir0 = root0.create_directory("pics".to_owned()).unwrap();

        dir0.flush().await.unwrap();
        root0.flush().await.unwrap();

        let mut root = JointDirectory::new(vec![root0]);

        assert_matches!(
            root.create_directory(branches[0].id(), "pics").await,
            Err(Error::EntryExists)
        );
    }

    async fn setup(branch_count: usize) -> Vec<Branch> {
        let pool = db::init(db::Store::Memory).await.unwrap();

        future::join_all((0..branch_count).map(|_| async {
            let data = BranchData::new(&pool, rand::random()).await.unwrap();
            Branch::new(pool.clone(), data, Cryptor::Null)
        }))
        .await
    }
}
