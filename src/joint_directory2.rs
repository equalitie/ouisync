//! Overhaul of `JoinDirectory` with improved API and functionality. Will eventually replace it.

use futures_util::future;

use crate::{
    directory::{Directory, DirectoryRef, EntryRef, FileRef},
    entry::EntryType,
    error::{Error, Result},
    iterator::{Accumulate, SortedUnion},
    replica_id::ReplicaId,
    versioned_file_name,
};
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

        if self.directories.is_empty() {
            None
        } else {
            Some(JointEntryRef::Directory(JointDirectoryRef(mem::take(
                &mut self.directories,
            ))))
        }
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
    use assert_matches::assert_matches;
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

            // assert_eq!(
            //     root.lookup(&format!("file.txt.v{:8x}", branch.replica_id()))
            //         .unwrap(),
            //     *entry
            // );
        }

        // assert_matches!(root.lookup("file.txt"), Err(Error::AmbiguousEntry(branch_ids)) => {
        //     assert_eq!(branch_ids.len(), 2);
        //     assert!(branch_ids.contains(branches[0].replica_id()));
        //     assert!(branch_ids.contains(branches[1].replica_id()));
        // });
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

    // TODO: test conflict_forked_directories
    // TODO: test conflict_file_and_directory

    async fn setup(branch_count: usize) -> Index {
        let pool = db::init(db::Store::Memory).await.unwrap();
        let branches =
            future::try_join_all((0..branch_count).map(|_| BranchData::new(&pool, rand::random())))
                .await
                .unwrap();

        Index::load(pool, *branches[0].replica_id()).await.unwrap()
    }
}
