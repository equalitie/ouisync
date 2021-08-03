//! Overhaul of `JoinDirectory` with improved API and functionality. Will eventually replace it.

use crate::{
    directory::{Directory, DirectoryRef, EntryRef, FileRef},
    entry::EntryType,
    error::{Error, Result},
    iterator::{Accumulate, DrainFilter, SortedUnion},
    locator::Locator,
    replica_id::ReplicaId,
    versioned_file_name,
};
use std::{
    collections::{BTreeMap, HashMap},
    fmt, iter,
};

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
    /// same entry are returned in a single `JoinEntryRef`. To obtain each version as a separate
    /// entry, use [`JointEntryRef::split`]:
    ///
    /// ```ignore
    /// dir.entries().flat_map(|entry| entry.split())
    /// ```
    pub fn entries(&self) -> impl Iterator<Item = JointEntryRef> {
        let entries = self.versions.values().map(|directory| directory.entries());
        let entries = SortedUnion::new(entries, |entry| entry.name());
        let entries = Accumulate::new(entries, |entry| entry.name());

        entries.map(|(_, entries)| JointEntryRef(entries))
    }

    // pub fn lookup(&self, name: &'_ str) -> Result<EntryInfo> {
    //     // Look for exact match first as that is the most likely case.
    //     if let Some((name, versions)) = self.entries.get_key_value(name) {
    //         if !versions.directories.is_empty() {
    //             return Ok(EntryInfo {
    //                 name,
    //                 version: EntryVersion::Directory(&versions.directories),
    //             });
    //         }

    //         if versions.files.len() > 1 {
    //             return Err(Error::AmbiguousEntry(
    //                 versions.files.keys().copied().collect(),
    //             ));
    //         }

    //         if let Some((branch_id, locator)) = versions.files.iter().next() {
    //             return Ok(EntryInfo {
    //                 name,
    //                 version: EntryVersion::File { branch_id, locator },
    //             });
    //         }

    //         unreachable!("joint directory entry must contain at least one file or directory")
    //     }

    //     // Now try to strip the branch suffix and lookup the versioned entry
    //     let (name, branch_id_prefix) = versioned_file_name::parse(name);
    //     let branch_id_prefix = branch_id_prefix.ok_or(Error::EntryNotFound)?;
    //     let (name, versions) = self
    //         .entries
    //         .get_key_value(name)
    //         .ok_or(Error::EntryNotFound)?;

    //     let mut matches = versions
    //         .files
    //         .iter()
    //         .filter(|(branch_id, _)| branch_id.starts_with(&branch_id_prefix))
    //         .peekable();

    //     let (branch_id, locator) = matches.next().ok_or(Error::EntryNotFound)?;

    //     if matches.peek().is_some() {
    //         return Err(Error::AmbiguousEntry(
    //             iter::once(branch_id)
    //                 .chain(matches.map(|(branch_id, _)| branch_id))
    //                 .copied()
    //                 .collect(),
    //         ));
    //     }

    //     Ok(EntryInfo {
    //         name,
    //         version: EntryVersion::File { branch_id, locator },
    //     })
    // }
}

#[derive(Default)]
pub struct JointEntryRef<'a>(Vec<EntryRef<'a>>);

impl<'a> JointEntryRef<'a> {
    pub fn name(&self) -> &'a str {
        self.0
            .first()
            .expect("EntryVersions must contain at least one entry")
            .name()
    }

    pub fn split(mut self) -> impl Iterator<Item = UniqueEntryRef<'a>> {
        let directories: Vec<_> = DrainFilter::new(&mut self.0, |entry| entry.is_directory())
            .map(|entry| entry.directory().unwrap())
            .collect();
        let directories = if directories.is_empty() {
            None
        } else {
            Some(UniqueEntryRef::Directory(JointDirectoryRef(directories)))
        };

        let files = self
            .0
            .into_iter()
            .filter_map(|entry| entry.file().ok())
            .map(UniqueEntryRef::File);

        directories.into_iter().chain(files)
    }
}

pub enum UniqueEntryRef<'a> {
    File(FileRef<'a>),
    Directory(JointDirectoryRef<'a>),
}

impl<'a> UniqueEntryRef<'a> {
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

pub struct JointDirectoryRef<'a>(Vec<DirectoryRef<'a>>);

/*

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crypto::Cryptor,
        db,
        directory::Directory,
        index::{BranchData, Index},
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

        assert_eq!(root.lookup("file0.txt").unwrap(), entries[0]);
        assert_eq!(root.lookup("file1.txt").unwrap(), entries[1]);
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

        let entries: Vec<_> = root.entries().collect();

        assert_eq!(entries.len(), 2);

        for branch in &branches {
            let entry = entries
                .iter()
                .find(|entry| entry.branch_id() == Some(branch.replica_id()))
                .unwrap();
            assert_eq!(entry.name(), "file.txt");
            assert_eq!(entry.entry_type(), EntryType::File);

            assert_eq!(
                root.lookup(&format!("file.txt.v{:8x}", branch.replica_id()))
                    .unwrap(),
                *entry
            );
        }

        assert_matches!(root.lookup("file.txt"), Err(Error::AmbiguousEntry(branch_ids)) => {
            assert_eq!(branch_ids.len(), 2);
            assert!(branch_ids.contains(branches[0].replica_id()));
            assert!(branch_ids.contains(branches[1].replica_id()));
        });
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

        let entries: Vec<_> = root.entries().collect();

        assert_eq!(entries.len(), 2);

        for branch in &branches {
            let entry = entries
                .iter()
                .find(|entry| entry.branch_id() == Some(branch.replica_id()))
                .unwrap();
            assert_eq!(entry.name(), "file.txt");
            assert_eq!(entry.entry_type(), EntryType::File);
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

        let entries: Vec<_> = root.entries().collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name(), "dir");
        assert_eq!(entries[0].entry_type(), EntryType::Directory);

        // TODO: verify locators
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

*/
