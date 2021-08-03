//! Overhaul of `JoinDirectory` with improved API and functionality. Will eventually replace it.

use crate::{
    directory::{Directory, DirectoryRef, EntryRef, FileRef},
    entry::EntryType,
    error::{Error, Result},
    iterator::{Accumulate, SortedUnion},
    replica_id::ReplicaId,
    versioned_file_name,
};
use camino::{Utf8Component, Utf8Path};
use futures_util::future;
use std::{collections::BTreeMap, fmt, mem};

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
                .map(|dir| (dir.global_locator().branch_id, dir))
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

    /// Looks up entry with the specified name and returns all concurrent versions of the entry.
    pub fn lookup<'a>(&'a self, name: &'a str) -> impl Iterator<Item = JointEntryRef<'a>> {
        let exact = Merge::new(
            self.versions
                .values()
                .flat_map(move |dir| dir.lookup(name).ok().into_iter().flatten()),
        );

        let (name, branch_id_prefix) = versioned_file_name::parse(name);
        let versioned = branch_id_prefix
            .map(|branch_id_prefix| {
                self.versions
                    .values()
                    .flat_map(move |dir| dir.lookup(name).ok().into_iter().flatten())
                    .filter_map(|entry| entry.file().ok())
                    .filter(move |file| file.branch_id().starts_with(&branch_id_prefix))
                    .map(JointEntryRef::File)
            })
            .into_iter()
            .flatten();

        exact.chain(versioned)
    }

    /// Looks up entry with the specified name.
    /// - If there is only one version of the entry and it is a file, returns it as
    ///   `JointEntryRef::File`.
    /// - If there are multiple versions, but all are directories, returns them in a single
    ///   `JointEntryRef::Directory`.
    /// - If there are mutliple file versions or at least one directory and one file version,
    ///   returns `AmbiguousEntry` error.
    /// - If there are no versions, returns `EntryNotFound`.
    pub fn lookup_unique<'a>(&'a self, name: &'a str) -> Result<JointEntryRef<'a>> {
        let mut entries = self.lookup(name);
        let first = entries.next().ok_or(Error::EntryNotFound)?;

        if entries.next().is_none() {
            Ok(first)
        } else {
            Err(Error::AmbiguousEntry)
        }
    }

    /// Looks up a subdirectory with the specified name. Useful in case of a conflict between
    /// a file and a directory with the same names.
    pub fn lookup_directory(&self, name: &'_ str) -> Result<JointDirectoryRef> {
        JointDirectoryRef::new(
            self.versions
                .values()
                .flat_map(|dir| dir.lookup(name).ok().into_iter().flatten())
                .filter_map(|entry| entry.directory().ok())
                .collect(),
        )
        .ok_or(Error::EntryNotFound)
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
                    curr = curr.lookup_directory(name)?.open().await?;
                }
                Utf8Component::ParentDir | Utf8Component::Prefix(_) => {
                    return Err(Error::OperationNotSupported)
                }
            }
        }

        Ok(curr)
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
// `EntryRef::Directory` items into a single `JointDirectoryRef` item.
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
        crypto::Cryptor,
        db,
        directory::Directory,
        index::{BranchData, Index},
        locator::Locator,
    };
    use futures_util::future;

    #[tokio::test(flavor = "multi_thread")]
    async fn no_conflict() {
        let index = setup(2).await;
        let branches = index.branches().await;

        let mut root0 = Directory::create(
            index.pool.clone(),
            branches[0].clone(),
            Cryptor::Null,
            Locator::Root,
        );
        root0
            .create_file("file0.txt".to_owned())
            .unwrap()
            .flush()
            .await
            .unwrap();
        root0.flush().await.unwrap();

        let mut root1 = Directory::create(
            index.pool.clone(),
            branches[1].clone(),
            Cryptor::Null,
            Locator::Root,
        );
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
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn conflict_independent_files() {
        let index = setup(2).await;
        let branches = index.branches().await;

        let mut root0 = Directory::create(
            index.pool.clone(),
            branches[0].clone(),
            Cryptor::Null,
            Locator::Root,
        );
        root0
            .create_file("file.txt".to_owned())
            .unwrap()
            .flush()
            .await
            .unwrap();
        root0.flush().await.unwrap();

        let mut root1 = Directory::create(
            index.pool.clone(),
            branches[1].clone(),
            Cryptor::Null,
            Locator::Root,
        );
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
                .find(|file| file.branch_id() == branch.replica_id())
                .unwrap();
            assert_eq!(file.name(), "file.txt");

            assert_eq!(
                root.lookup(&versioned_file_name::create(
                    "file.txt",
                    branch.replica_id()
                ))
                .collect::<Vec<_>>(),
                vec![JointEntryRef::File(*file)]
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
                .find(|file| file.branch_id() == branch.replica_id())
                .unwrap();
            assert_eq!(file.name(), "file.txt");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn conflict_forked_files() {
        let index = setup(2).await;
        let branches = index.branches().await;

        let mut root0 = Directory::create(
            index.pool.clone(),
            branches[0].clone(),
            Cryptor::Null,
            Locator::Root,
        );
        let mut file0 = root0.create_file("file.txt".to_owned()).unwrap();
        file0.flush().await.unwrap();
        root0.flush().await.unwrap();

        let mut root1 = Directory::create(
            index.pool.clone(),
            branches[1].clone(),
            Cryptor::Null,
            Locator::Root,
        );
        root1
            .copy_file("file.txt", file0.locators(), &branches[0])
            .await
            .unwrap();
        root1.flush().await.unwrap();

        let root = JointDirectory::new(vec![root0, root1]);

        let files: Vec<_> = root.entries().map(|entry| entry.file().unwrap()).collect();

        assert_eq!(files.len(), 2);

        for branch in &branches {
            let file = files
                .iter()
                .find(|file| file.branch_id() == branch.replica_id())
                .unwrap();
            assert_eq!(file.name(), "file.txt");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn conflict_directories() {
        let index = setup(2).await;
        let branches = index.branches().await;

        let mut root0 = Directory::create(
            index.pool.clone(),
            branches[0].clone(),
            Cryptor::Null,
            Locator::Root,
        );

        let mut dir0 = root0.create_directory("dir".to_owned()).unwrap();
        dir0.flush().await.unwrap();
        root0.flush().await.unwrap();

        let mut root1 = Directory::create(
            index.pool.clone(),
            branches[1].clone(),
            Cryptor::Null,
            Locator::Root,
        );

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
        let index = setup(2).await;
        let branches = index.branches().await;

        let mut root0 = Directory::create(
            index.pool.clone(),
            branches[0].clone(),
            Cryptor::Null,
            Locator::Root,
        );

        let mut file0 = root0.create_file("config".to_owned()).unwrap();
        file0.flush().await.unwrap();
        root0.flush().await.unwrap();

        let mut root1 = Directory::create(
            index.pool.clone(),
            branches[1].clone(),
            Cryptor::Null,
            Locator::Root,
        );

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
            JointEntryRef::File(file) => file.branch_id() == branches[0].replica_id(),
            JointEntryRef::Directory(_) => false,
        }));
        assert!(entries
            .iter()
            .any(|entry| entry.entry_type() == EntryType::Directory));

        let entries: Vec<_> = root.lookup("config").collect();
        assert_eq!(entries.len(), 2);

        let name = versioned_file_name::create("config", branches[0].replica_id());
        let mut entries: Vec<_> = root.lookup(&name).collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry_type(), EntryType::File);
        assert_eq!(
            entries.remove(0).file().unwrap().branch_id(),
            branches[0].replica_id()
        );
    }

    // TODO: test conflict_forked_directories
    // TODO: test conflict_multiple_files_and_directories
    // TODO: test conflict_file_with_name_containing_branch_prefix

    #[tokio::test(flavor = "multi_thread")]
    async fn cd_into_concurrent_directory() {
        let index = setup(2).await;
        let branches = index.branches().await;

        let mut root0 = Directory::create(
            index.pool.clone(),
            branches[0].clone(),
            Cryptor::Null,
            Locator::Root,
        );

        let mut dir0 = root0.create_directory("pics".to_owned()).unwrap();
        let mut file0 = dir0.create_file("dog.jpg".to_owned()).unwrap();

        file0.flush().await.unwrap();
        dir0.flush().await.unwrap();
        root0.flush().await.unwrap();

        let mut root1 = Directory::create(
            index.pool.clone(),
            branches[1].clone(),
            Cryptor::Null,
            Locator::Root,
        );

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

    async fn setup(branch_count: usize) -> Index {
        let pool = db::init(db::Store::Memory).await.unwrap();
        let branches =
            future::try_join_all((0..branch_count).map(|_| BranchData::new(&pool, rand::random())))
                .await
                .unwrap();

        Index::load(pool, *branches[0].replica_id()).await.unwrap()
    }
}
