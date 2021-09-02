use crate::{
    directory::{self, Directory, DirectoryRef, EntryRef, FileRef},
    entry_type::EntryType,
    error::{Error, Result},
    file::File,
    iterator::{Accumulate, SortedUnion},
    locator::Locator,
    replica_id::ReplicaId,
    versioned_file_name,
};
use camino::{Utf8Component, Utf8Path};
use futures_util::future;
use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::{btree_map::Entry, BTreeMap, VecDeque},
    fmt, iter, mem,
};

/// Unified view over multiple concurrent versions of a directory.
pub struct JointDirectory {
    versions: BTreeMap<ReplicaId, Directory>,
}

impl JointDirectory {
    pub async fn new<I>(versions: I) -> Self
    where
        I: IntoIterator<Item = Directory>,
    {
        let versions = future::join_all(versions.into_iter().map(|dir| async move {
            let branch_id = *dir.read().await.branch().id();
            (branch_id, dir)
        }))
        .await
        .into_iter()
        .collect();

        Self { versions }
    }

    /// Lock this joint directory for reading.
    pub async fn read(&self) -> Reader<'_> {
        Reader(future::join_all(self.versions.values().map(|dir| dir.read())).await)
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
                        .read()
                        .await
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

    /// Creates a subdirectory of this directory owned by `branch` and returns it as
    /// `JointDirectory` which would already include all previousy existing versions.
    pub async fn create_directory(&self, branch: &ReplicaId, name: &str) -> Result<Self> {
        let mut old_dir = if let Some(entry) = self
            .read()
            .await
            .lookup(name)
            .find_map(|entry| entry.directory().ok())
        {
            entry.open().await?
        } else {
            Self::new(iter::empty()).await
        };

        match old_dir.versions.entry(*branch) {
            Entry::Vacant(entry) => {
                let new_version = self
                    .versions
                    .get(branch)
                    .ok_or(Error::EntryNotFound)?
                    .create_directory(name.to_owned())
                    .await?;
                entry.insert(new_version);
            }
            Entry::Occupied(_) => return Err(Error::EntryExists),
        }

        Ok(old_dir)
    }

    pub async fn remove_file(&self, branch: &ReplicaId, name: &str) -> Result<()> {
        self.versions
            .get(branch)
            .ok_or(Error::EntryNotFound)?
            .remove_file(name)
            .await
    }

    pub async fn remove_directory(&self, branch: &ReplicaId, name: &str) -> Result<()> {
        self.versions
            .get(branch)
            .ok_or(Error::EntryNotFound)?
            .remove_directory(name)
            .await
    }

    pub async fn flush(&mut self) -> Result<()> {
        future::try_join_all(self.versions.values_mut().map(|dir| dir.flush())).await?;
        Ok(())
    }

    pub async fn merge(&mut self) -> Result<()> {
        let mut queue = VecDeque::new();

        self.merge_single(&mut queue).await?;

        while let Some(mut dir) = queue.pop_back() {
            dir.merge_single(&mut queue).await?;
        }

        Ok(())
    }

    async fn merge_single(&mut self, queue: &mut VecDeque<Self>) -> Result<()> {
        // TODO: only process this joint directory if at least one remote version is happens-after
        // or concurrent with the local version, of if the local version doesn't exists.
        self.fork().await?;

        // We can't fork the files as we are iterating the entries because that would deadlock - we
        // collect them here and fork them once done iterating instead.
        let mut files_to_fork = Vec::new();

        for entry in self.read().await.entries() {
            match entry {
                JointEntryRef::File(entry) => files_to_fork.push(entry.open().await?),
                JointEntryRef::Directory(entry) => queue.push_front(entry.open().await?),
            }
        }

        // `EntryExists` error means the file already exists locally at the same or greater version
        // than the remote file which is OK and expected, so we ignore it.
        future::try_join_all(files_to_fork.iter_mut().map(|file| async move {
            match file.fork().await {
                Ok(()) | Err(Error::EntryExists) => Ok(()),
                Err(error) => Err(error),
            }
        }))
        .await?;

        self.remove_remote_versions().await;
        self.flush().await?;

        Ok(())
    }

    // Ensure this joint directory contains a local version.
    async fn fork(&mut self) -> Result<()> {
        if self.local_branch_id().await.is_some() {
            // TODO: we should still proceed with the fork, to update the version vector.
            return Ok(());
        }

        // Grab any version and fork it to create the local one.
        // TODO: fork all versions, not just one, to properly update the version vector.
        let local = if let Some(remote) = self.versions.values().next() {
            remote.clone().fork().await?
        } else {
            return Ok(());
        };

        let id = *local.read().await.branch().id();
        self.versions.insert(id, local);

        Ok(())
    }

    // Remove all versions except the local one.
    async fn remove_remote_versions(&mut self) {
        let local_id = self.local_branch_id().await.copied();
        self.versions.retain(|id, _| Some(id) == local_id.as_ref())
    }

    async fn local_branch_id(&self) -> Option<&ReplicaId> {
        for (id, version) in &self.versions {
            if version.read().await.is_local() {
                return Some(id);
            }
        }

        None
    }
}

impl fmt::Debug for JointDirectory {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("JointDirectory").finish()
    }
}

/// View of a `JointDirectory` for performing read-only queries.
pub struct Reader<'a>(Vec<directory::Reader<'a>>);

impl Reader<'_> {
    /// Returns iterator over the entries of this directory. Multiple concurrent versions of the
    /// same file are returned as separate `JointEntryRef::File` entries. Multiple concurrent
    /// versions of the same directory are returned as a single `JointEntryRef::Directory` entry.
    pub fn entries(&self) -> impl Iterator<Item = JointEntryRef> {
        let entries = self.0.iter().map(|directory| directory.entries());
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
            self.0
                .iter()
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
            .0
            .iter()
            .flat_map(|dir| dir.lookup(name).ok().into_iter().flatten())
            .filter_map(|entry| entry.file().ok())
            .filter(|entry| entry.author().starts_with(&branch_id_prefix));

        let first = entries.next().ok_or(Error::EntryNotFound)?;

        if entries.next().is_none() {
            Ok(JointEntryRef::File(JointFileRef {
                file: first,
                needs_disambiguation: true,
            }))
        } else {
            Err(Error::AmbiguousEntry)
        }
    }

    /// Looks up a specific version of a file.
    ///
    /// NOTE: There can be multiple versions of the file with the same author, but due to the
    /// invariant of there always being at most one version of a file per branch that is also
    /// authored by that branch, there are only two possible outcomes for every pair of such
    /// versions: either one is "happens after" the other, or they are identical. It's not possible
    /// for them to be concurrent. Because of this, this function can never return `AmbiguousEntry`
    /// error.
    pub fn lookup_version(&self, name: &'_ str, branch_id: &'_ ReplicaId) -> Result<FileRef> {
        Merge::new(
            self.0
                .iter()
                .filter_map(|dir| dir.lookup_version(name, branch_id).ok()),
        )
        .find_map(|entry| entry.file().ok())
        .ok_or(Error::EntryNotFound)
    }

    /// Length of the directory in bytes. If there are multiple versions, returns the sum of their
    /// lengths.
    #[allow(clippy::len_without_is_empty)]
    pub async fn len(&self) -> u64 {
        let mut sum = 0;
        for dir in self.0.iter() {
            sum += dir.len().await;
        }
        sum
    }
}

#[derive(Eq, PartialEq, Debug)]
pub enum JointEntryRef<'a> {
    File(JointFileRef<'a>),
    Directory(JointDirectoryRef<'a>),
}

impl<'a> JointEntryRef<'a> {
    pub fn name(&self) -> &'a str {
        match self {
            Self::File(r) => r.name(),
            Self::Directory(r) => r.name(),
        }
    }

    pub fn unique_name(&self) -> Cow<'a, str> {
        match self {
            Self::File(r) => r.unique_name(),
            Self::Directory(r) => Cow::from(r.name()),
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
            Self::File(r) => Ok(r.file),
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

#[derive(Eq, PartialEq, Debug)]
pub struct JointFileRef<'a> {
    file: FileRef<'a>,
    needs_disambiguation: bool,
}

impl<'a> JointFileRef<'a> {
    pub fn name(&self) -> &'a str {
        self.file.name()
    }

    pub fn unique_name(&self) -> Cow<'a, str> {
        if self.needs_disambiguation {
            Cow::from(versioned_file_name::create(self.name(), self.branch_id()))
        } else {
            Cow::from(self.name())
        }
    }

    pub async fn open(&self) -> Result<File> {
        self.file.open().await
    }

    pub fn locator(&self) -> Locator {
        self.file.locator()
    }

    pub fn branch_id(&self) -> &'a ReplicaId {
        self.file.author()
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
        Ok(JointDirectory::new(directories).await)
    }
}

impl fmt::Debug for JointDirectoryRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("JointDirectoryRef")
            .field("name", &self.name())
            .finish()
    }
}

// Iterator adaptor that maps iterator of `EntryRef` to iterator of `JointEntryRef` by filtering
// out the outdated (according the their version vectors) versions and then mering all
// `EntryRef::Directory` items into a single `JointDirectoryRef` item but keeping `EntryRef::File`
// items separate.
#[derive(Clone)]
struct Merge<'a> {
    // TODO: The most common case for files shall be that there will be only one version of it.
    // Thus it might make sense to have one place holder for the first file to avoid Vec allocation
    // when not needed.
    files: VecDeque<FileRef<'a>>,
    directories: Vec<DirectoryRef<'a>>,
    needs_disambiguation: bool,
}

impl<'a> Merge<'a> {
    // All these entries are expected to have the same name. They can be either files, directories
    // or a mix of the two.
    fn new<I>(entries: I) -> Self
    where
        I: Iterator<Item = EntryRef<'a>>,
    {
        let mut files = VecDeque::new();
        let mut directories = vec![];

        let entries = Self::keep_concurrent(entries);

        for entry in entries {
            match entry {
                EntryRef::File(file) => files.push_back(file),
                EntryRef::Directory(dir) => directories.push(dir),
            }
        }

        let needs_disambiguation = files.len() > 1;

        Self {
            files,
            directories,
            needs_disambiguation,
        }
    }

    // Returns the entries with the maximal version vectors.
    fn keep_concurrent(entries: impl Iterator<Item = EntryRef<'a>>) -> Vec<EntryRef<'a>> {
        let mut max: Vec<EntryRef> = Vec::new();

        for new in entries {
            let mut insert = true;
            let mut remove = None;

            for (index, old) in max.iter().enumerate() {
                match (
                    old.version_vector().partial_cmp(new.version_vector()),
                    new.is_local(),
                ) {
                    // If both have identical versions, prefer the local one
                    (Some(Ordering::Less), _) | (Some(Ordering::Equal), true) => {
                        insert = true;
                        remove = Some(index);
                        break;
                    }
                    (Some(Ordering::Greater), _) | (Some(Ordering::Equal), false) => {
                        insert = false;
                        break;
                    }
                    (None, _) => {
                        insert = true;
                    }
                }
            }

            // Note: using `Vec::remove` to maintain the original order. Is there a more efficient
            // way?
            if let Some(index) = remove {
                max.remove(index);
            }

            if insert {
                max.push(new)
            }
        }

        max
    }
}

impl<'a> Iterator for Merge<'a> {
    type Item = JointEntryRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.directories.is_empty() {
            return JointDirectoryRef::new(mem::take(&mut self.directories))
                .map(JointEntryRef::Directory);
        }

        let file = self.files.pop_front()?;

        Some(JointEntryRef::File(JointFileRef {
            file,
            needs_disambiguation: self.needs_disambiguation,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{branch::Branch, crypto::Cryptor, db, index::BranchData};
    use assert_matches::assert_matches;
    use futures_util::future;
    use rand::{distributions::Standard, rngs::StdRng, Rng, SeedableRng};
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread")]
    async fn no_conflict() {
        let branches = setup(2).await;

        let root0 = branches[0].open_or_create_root().await.unwrap();
        create_file(&root0, "file0.txt", &[]).await;

        let root1 = branches[1].open_or_create_root().await.unwrap();
        create_file(&root1, "file1.txt", &[]).await;

        let root = JointDirectory::new(vec![root0, root1]).await;
        let root = root.read().await;

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

        let root0 = branches[0].open_or_create_root().await.unwrap();
        create_file(&root0, "file.txt", &[]).await;

        let root1 = branches[1].open_or_create_root().await.unwrap();
        create_file(&root1, "file.txt", &[]).await;

        let root = JointDirectory::new(vec![root0, root1]).await;
        let root = root.read().await;

        let files: Vec<_> = root.entries().map(|entry| entry.file().unwrap()).collect();
        assert_eq!(files.len(), 2);

        for branch in &branches {
            let file = files
                .iter()
                .find(|file| file.author() == branch.id())
                .unwrap();
            assert_eq!(file.name(), "file.txt");

            assert_eq!(
                root.lookup_unique(&versioned_file_name::create("file.txt", branch.id()))
                    .unwrap(),
                JointEntryRef::File(JointFileRef {
                    file: *file,
                    needs_disambiguation: true
                })
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
                .find(|file| file.author() == branch.id())
                .unwrap();
            assert_eq!(file.name(), "file.txt");
        }

        assert_matches!(root.lookup_unique("file.txt"), Err(Error::AmbiguousEntry));

        assert_unique_and_ordered(2, root.entries());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn conflict_forked_files() {
        let branches = setup(2).await;

        let root0 = branches[0].open_or_create_root().await.unwrap();
        create_file(&root0, "file.txt", b"one").await;

        // Open the file with branch 1 as the local branch and then modify it which copies (forks)
        // it into branch 1.
        let root0_by_1 = branches[0].open_root(branches[1].clone()).await.unwrap();
        let mut file1 = open_file_version(&root0_by_1, "file.txt", branches[0].id()).await;
        file1.write(b"two").await.unwrap();
        file1.flush().await.unwrap();

        // Modify the file by branch 0 as well, to create concurrent versions
        let mut file0 = open_file_version(&root0, "file.txt", branches[0].id()).await;
        file0.write(b"three").await.unwrap();
        file0.flush().await.unwrap();

        // Open branch 1's root dir which should have been created in the process.
        let root1 = branches[1].open_root(branches[1].clone()).await.unwrap();

        let root = JointDirectory::new(vec![root0_by_1, root1]).await;
        let root = root.read().await;

        let files: Vec<_> = root.entries().map(|entry| entry.file().unwrap()).collect();

        assert_eq!(files.len(), 2);

        for branch in &branches {
            let file = files
                .iter()
                .find(|file| file.author() == branch.id())
                .unwrap();
            assert_eq!(file.name(), "file.txt");
        }

        assert_unique_and_ordered(2, root.entries());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn conflict_directories() {
        let branches = setup(2).await;

        let mut root0 = branches[0].open_or_create_root().await.unwrap();

        let mut dir0 = root0.create_directory("dir".to_owned()).await.unwrap();
        dir0.flush().await.unwrap();
        root0.flush().await.unwrap();

        let mut root1 = branches[1].open_or_create_root().await.unwrap();

        let mut dir1 = root1.create_directory("dir".to_owned()).await.unwrap();
        dir1.flush().await.unwrap();
        root1.flush().await.unwrap();

        let root = JointDirectory::new(vec![root0, root1]).await;
        let root = root.read().await;

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

        let root0 = branches[0].open_or_create_root().await.unwrap();

        create_file(&root0, "config", &[]).await;

        let root1 = branches[1].open_or_create_root().await.unwrap();

        let mut dir1 = root1.create_directory("config".to_owned()).await.unwrap();
        dir1.flush().await.unwrap();

        let root = JointDirectory::new(vec![root0, root1]).await;
        let root = root.read().await;

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
        assert_eq!(entry.file().unwrap().author(), branches[0].id());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn conflict_identical_versions() {
        let branches = setup(2).await;

        // Create a file by one branch.
        let root0 = branches[0].open_or_create_root().await.unwrap();
        create_file(&root0, "file.txt", b"one").await;

        // Fork it into the other branch, creating an identical version of it.
        let root0_by_1 = branches[0].open_root(branches[1].clone()).await.unwrap();
        let mut file1 = open_file_version(&root0_by_1, "file.txt", branches[0].id()).await;
        file1.fork().await.unwrap();

        let root1 = branches[1].open_root(branches[1].clone()).await.unwrap();

        // Create joint directory using branch 1 as the local branch.
        let root = JointDirectory::new(vec![root0_by_1, root1]).await;
        let root = root.read().await;

        // The file appears among the entries only once...
        assert_eq!(root.entries().count(), 1);

        // ...and it is the local version.
        let file = root
            .entries()
            .next()
            .unwrap()
            .file()
            .unwrap()
            .open()
            .await
            .unwrap();
        assert_eq!(file.branch().id(), branches[1].id());

        // The file can also be retreived using `lookup`...
        let mut versions = root.lookup("file.txt");
        assert!(versions.next().is_some());
        assert!(versions.next().is_none());

        // ...and `lookup_version` using the author branch:
        root.lookup_version("file.txt", branches[0].id()).unwrap();
    }

    //// TODO: test conflict_forked_directories
    //// TODO: test conflict_multiple_files_and_directories
    //// TODO: test conflict_file_with_name_containing_branch_prefix

    #[tokio::test(flavor = "multi_thread")]
    async fn cd_into_concurrent_directory() {
        let branches = setup(2).await;

        let root0 = branches[0].open_or_create_root().await.unwrap();

        let dir0 = root0.create_directory("pics".to_owned()).await.unwrap();
        create_file(&dir0, "dog.jpg", &[]).await;

        let root1 = branches[1].open_or_create_root().await.unwrap();
        let dir1 = root1.create_directory("pics".to_owned()).await.unwrap();
        create_file(&dir1, "cat.jpg", &[]).await;

        let root = JointDirectory::new(vec![root0, root1]).await;
        let dir = root.cd("pics").await.unwrap();
        let dir = dir.read().await;

        let entries: Vec<_> = dir.entries().collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name(), "cat.jpg");
        assert_eq!(entries[1].name(), "dog.jpg");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_directory_with_existing_versions() {
        let branches = setup(2).await;

        let root0 = branches[0].open_or_create_root().await.unwrap();

        let dir0 = root0.create_directory("pics".to_owned()).await.unwrap();
        create_file(&dir0, "dog.jpg", &[]).await;

        let root1 = branches[1].open_or_create_root().await.unwrap();

        let root = JointDirectory::new(vec![root0, root1]).await;

        let dir = root
            .create_directory(branches[1].id(), "pics")
            .await
            .unwrap();
        let dir = dir.read().await;

        let entries: Vec<_> = dir.entries().collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name(), "dog.jpg");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_directory_without_existing_versions() {
        let branches = setup(1).await;

        let root0 = branches[0].open_or_create_root().await.unwrap();

        let root = JointDirectory::new(vec![root0]).await;

        let dir = root
            .create_directory(branches[0].id(), "pics")
            .await
            .unwrap();

        assert_eq!(dir.read().await.entries().count(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn attempt_to_create_directory_whose_local_version_already_exists() {
        let branches = setup(2).await;

        let root0 = branches[0].open_or_create_root().await.unwrap();

        let mut dir0 = root0.create_directory("pics".to_owned()).await.unwrap();
        dir0.flush().await.unwrap();

        let root = JointDirectory::new(vec![root0]).await;

        assert_matches!(
            root.create_directory(branches[0].id(), "pics").await,
            Err(Error::EntryExists)
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn merge_locally_non_existing_file() {
        // 0 - local, 1 - remote
        let branches = setup(2).await;

        let content = b"cat";

        // Create local root dir
        let mut local_root = branches[0].open_or_create_root().await.unwrap();
        local_root.flush().await.unwrap();

        // Create remote root dir
        let remote_root = branches[1].open_or_create_root().await.unwrap();

        // Create a file in the remote root
        create_file(&remote_root, "cat.jpg", content).await;

        // Reopen the remote root locally.
        let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();

        // Construct a joint directory over both root dirs and merge it.
        JointDirectory::new(vec![local_root.clone(), remote_root_on_local])
            .await
            .merge()
            .await
            .unwrap();

        // Verify the file now exists in the local branch.
        let local_content = open_file(&local_root, "cat.jpg")
            .await
            .read_to_end()
            .await
            .unwrap();
        assert_eq!(local_content, content);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn merge_locally_older_file() {
        let branches = setup(2).await;

        let content_v0 = b"version 0";
        let content_v1 = b"version 1";

        let mut local_root = branches[0].open_or_create_root().await.unwrap();
        local_root.flush().await.unwrap();

        let remote_root = branches[1].open_or_create_root().await.unwrap();

        // Create a file in the remote root
        create_file(&remote_root, "cat.jpg", content_v0).await;

        // Merge to transfer the file to the local branch
        let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();
        let mut root =
            JointDirectory::new(vec![local_root.clone(), remote_root_on_local.clone()]).await;
        root.merge().await.unwrap();

        // Modify the file by the remote branch
        update_file(&remote_root, "cat.jpg", content_v1).await;

        JointDirectory::new(vec![local_root.clone(), remote_root_on_local])
            .await
            .merge()
            .await
            .unwrap();

        let reader = local_root.read().await;

        // The remote version is newer, so it overwrites the local version and we end up with only
        // one version in the local branch.
        assert_eq!(reader.lookup("cat.jpg").unwrap().count(), 1);

        let entry = reader
            .lookup("cat.jpg")
            .unwrap()
            .next()
            .unwrap()
            .file()
            .unwrap();

        assert_eq!(entry.author(), branches[1].id());

        let mut file = entry.open().await.unwrap();
        let local_content = file.read_to_end().await.unwrap();
        assert_eq!(local_content, content_v1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn merge_locally_newer_file() {
        let branches = setup(2).await;

        let content_v0 = b"version 0";
        let content_v1 = b"version 1";

        let mut local_root = branches[0].open_or_create_root().await.unwrap();
        local_root.flush().await.unwrap();

        let remote_root = branches[1].open_or_create_root().await.unwrap();

        create_file(&remote_root, "cat.jpg", content_v0).await;

        let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();
        let mut root =
            JointDirectory::new(vec![local_root.clone(), remote_root_on_local.clone()]).await;
        root.merge().await.unwrap();

        // Modify the file by the local branch
        update_file(&local_root, "cat.jpg", content_v1).await;

        JointDirectory::new(vec![local_root.clone(), remote_root_on_local])
            .await
            .merge()
            .await
            .unwrap();

        let reader = local_root.read().await;

        // The local version is newer, so there is only one version in the local branch.
        assert_eq!(reader.lookup("cat.jpg").unwrap().count(), 1);

        let entry = reader
            .lookup("cat.jpg")
            .unwrap()
            .next()
            .unwrap()
            .file()
            .unwrap();

        assert_eq!(entry.author(), branches[0].id());

        let mut file = entry.open().await.unwrap();
        let local_content = file.read_to_end().await.unwrap();
        assert_eq!(local_content, content_v1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn merge_concurrent_file() {
        let branches = setup(2).await;

        let mut local_root = branches[0].open_or_create_root().await.unwrap();
        local_root.flush().await.unwrap();

        let remote_root = branches[1].open_or_create_root().await.unwrap();

        create_file(&remote_root, "cat.jpg", b"v0").await;

        let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();
        let mut root =
            JointDirectory::new(vec![local_root.clone(), remote_root_on_local.clone()]).await;
        root.merge().await.unwrap();

        // Modify the file by both branches concurrently
        update_file(&local_root, "cat.jpg", b"v1").await;
        update_file(&remote_root, "cat.jpg", b"v2").await;

        JointDirectory::new(vec![local_root.clone(), remote_root_on_local])
            .await
            .merge()
            .await
            .unwrap();

        // The versions are concurrent, so both are present in the local branch.
        assert_eq!(
            local_root.read().await.lookup("cat.jpg").unwrap().count(),
            2
        );

        assert_eq!(
            open_file_version(&local_root, "cat.jpg", branches[0].id())
                .await
                .read_to_end()
                .await
                .unwrap(),
            b"v1"
        );
        assert_eq!(
            open_file_version(&local_root, "cat.jpg", branches[1].id())
                .await
                .read_to_end()
                .await
                .unwrap(),
            b"v2"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn local_merge_is_idempotent() {
        let branches = setup(2).await;

        let mut local_root = branches[0].open_or_create_root().await.unwrap();
        local_root.flush().await.unwrap();

        let vv0 = branches[0].data().versions().await.clone();

        let remote_root = branches[1].open_or_create_root().await.unwrap();
        let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();

        // Merge after a remote modification - this causes local modification.
        create_file(&remote_root, "cat.jpg", b"v0").await;
        JointDirectory::new(vec![local_root.clone(), remote_root_on_local.clone()])
            .await
            .merge()
            .await
            .unwrap();

        let vv1 = branches[0].data().versions().await.clone();
        assert!(vv1 > vv0);

        // Merge again. This time there is no local modification because there was no remote
        // modification either.
        JointDirectory::new(vec![local_root.clone(), remote_root_on_local.clone()])
            .await
            .merge()
            .await
            .unwrap();

        let vv2 = branches[0].data().versions().await.clone();
        assert_eq!(vv2, vv1);

        // Perform another remote modification and merge again - this causes local modification
        // again.
        update_file(&remote_root, "cat.jpg", b"v1").await;
        JointDirectory::new(vec![local_root.clone(), remote_root_on_local.clone()])
            .await
            .merge()
            .await
            .unwrap();

        let vv3 = branches[0].data().versions().await.clone();
        assert!(vv3 > vv2);

        // Another idempotent merge which causes no local modification.
        JointDirectory::new(vec![local_root, remote_root_on_local])
            .await
            .merge()
            .await
            .unwrap();

        let vv4 = branches[0].data().versions().await.clone();
        assert_eq!(vv4, vv3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remote_merge_is_idempotent() {
        let branches = setup(2).await;

        let mut local_root = branches[0].open_or_create_root().await.unwrap();
        local_root.flush().await.unwrap();
        let local_root_on_remote = branches[0].open_root(branches[1].clone()).await.unwrap();

        let mut remote_root = branches[1].open_or_create_root().await.unwrap();
        remote_root.flush().await.unwrap();
        let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();

        create_file(&remote_root, "cat.jpg", b"v0").await;

        // First merge remote into local
        JointDirectory::new(vec![local_root, remote_root_on_local])
            .await
            .merge()
            .await
            .unwrap();

        let vv0 = branches[0].data().versions().await.clone();

        // Then merge local back into remote. This has no effect.
        JointDirectory::new(vec![remote_root, local_root_on_remote])
            .await
            .merge()
            .await
            .unwrap();

        let vv1 = branches[0].data().versions().await.clone();
        assert_eq!(vv1, vv0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn merge_remote_only() {
        let branches = setup(2).await;

        let remote_root = branches[1].open_or_create_root().await.unwrap();
        let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();

        create_file(&remote_root, "cat.jpg", b"v0").await;

        // When passing only the remote dir to the joint directory the merge still works.
        JointDirectory::new(iter::once(remote_root_on_local))
            .await
            .merge()
            .await
            .unwrap();

        let local_root = branches[0].open_root(branches[0].clone()).await.unwrap();
        local_root
            .read()
            .await
            .lookup_version("cat.jpg", branches[1].id())
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn merge_sequential_modifications() {
        let branches = setup_with_rng(StdRng::seed_from_u64(0), 2).await;

        let mut local_root = branches[0].open_or_create_root().await.unwrap();
        local_root.flush().await.unwrap();
        let local_root_on_remote = branches[0].open_root(branches[1].clone()).await.unwrap();

        let mut remote_root = branches[1].open_or_create_root().await.unwrap();
        remote_root.flush().await.unwrap();
        let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();

        // Create a file by local, then modify it by remote, then read it back by local verifying
        // the modification by remote got through.

        create_file(&local_root, "dog.jpg", b"v0").await;

        JointDirectory::new(vec![remote_root.clone(), local_root_on_remote])
            .await
            .merge()
            .await
            .unwrap();

        update_file(&remote_root, "dog.jpg", b"v1").await;

        JointDirectory::new(vec![local_root.clone(), remote_root_on_local])
            .await
            .merge()
            .await
            .unwrap();

        let content = open_file(&local_root, "dog.jpg")
            .await
            .read_to_end()
            .await
            .unwrap();
        assert_eq!(content, b"v1");
    }

    async fn setup(branch_count: usize) -> Vec<Branch> {
        setup_with_rng(StdRng::from_entropy(), branch_count).await
    }

    // Useful for debugging non-deterministic failures.
    async fn setup_with_rng(rng: StdRng, branch_count: usize) -> Vec<Branch> {
        let pool = db::init(db::Store::Memory).await.unwrap();
        let pool = &pool;

        let ids = rng.sample_iter(Standard).take(branch_count);

        future::join_all(ids.map(|id| async move {
            let data = BranchData::new(pool, id).await.unwrap();
            Branch::new(pool.clone(), Arc::new(data), Cryptor::Null)
        }))
        .await
    }

    fn assert_unique_and_ordered<'a, I>(count: usize, mut entries: I)
    where
        I: Iterator<Item = JointEntryRef<'a>>,
    {
        let prev = entries.next();

        if prev.is_none() {
            assert!(count == 0);
            return;
        }

        let mut prev = prev.unwrap();
        let mut prev_i = 1;

        for entry in entries {
            assert!(prev.unique_name() < entry.unique_name());
            prev_i += 1;
            prev = entry;
        }

        assert_eq!(prev_i, count);
    }

    async fn create_file(parent: &Directory, name: &str, content: &[u8]) {
        let mut file = parent.create_file(name.to_owned()).await.unwrap();

        if !content.is_empty() {
            file.write(content).await.unwrap();
        }

        file.flush().await.unwrap();
    }

    async fn update_file(parent: &Directory, name: &str, content: &[u8]) {
        let mut file = open_file(parent, name).await;

        file.truncate(0).await.unwrap();
        file.write(content).await.unwrap();
        file.flush().await.unwrap();
    }

    async fn open_file(parent: &Directory, name: &str) -> File {
        parent
            .read()
            .await
            .lookup(name)
            .unwrap()
            .next()
            .unwrap()
            .file()
            .unwrap()
            .open()
            .await
            .unwrap()
    }

    async fn open_file_version(parent: &Directory, name: &str, branch_id: &ReplicaId) -> File {
        parent
            .read()
            .await
            .lookup_version(name, branch_id)
            .unwrap()
            .file()
            .unwrap()
            .open()
            .await
            .unwrap()
    }
}
