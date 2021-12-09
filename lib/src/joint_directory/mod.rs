#[cfg(test)]
mod tests;

use crate::{
    crypto::sign::PublicKey,
    directory::{self, Directory, DirectoryRef, EntryRef, EntryType, FileRef},
    error::{Error, Result},
    file::File,
    iterator::{Accumulate, SortedUnion},
    version_vector::VersionVector,
    versioned_file_name,
};
use async_recursion::async_recursion;
use camino::{Utf8Component, Utf8Path};
use either::Either;
use futures_util::future;
use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::{BTreeMap, VecDeque},
    fmt, iter, mem,
};

/// Unified view over multiple concurrent versions of a directory.
#[derive(Clone)]
pub struct JointDirectory {
    versions: BTreeMap<PublicKey, Directory>,
}

impl JointDirectory {
    /// Creates a new `JointDirectory` over the specified directory versions.
    pub fn new<I>(versions: I) -> Self
    where
        I: IntoIterator<Item = Directory>,
    {
        let versions: BTreeMap<_, _> = versions
            .into_iter()
            .map(|dir| (*dir.branch_id(), dir))
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
    pub async fn cd(&self, path: impl AsRef<Utf8Path>) -> Result<Self> {
        let mut curr = Cow::Borrowed(self);

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
                        .open(MissingVersionStrategy::Skip)
                        .await?;
                    curr = Cow::Owned(next);
                }
                Utf8Component::ParentDir | Utf8Component::Prefix(_) => {
                    return Err(Error::OperationNotSupported)
                }
            }
        }

        Ok(curr.into_owned())
    }

    /// Removes the specified entry from this directory. If the entry is a subdirectory, it has to
    /// be empty. Use [Self::remove_entry_recursively] to remove non-empty subdirectories.
    pub async fn remove_entry(&mut self, name: &str) -> Result<()> {
        self.remove_entries(Pattern::Unique(name)).await
    }

    /// Removes the specified entry from this directory, including all its content if it is a
    /// subdirectory.
    pub async fn remove_entry_recursively(&mut self, name: &str) -> Result<()> {
        self.remove_entries_recursively(Pattern::Unique(name)).await
    }

    async fn remove_entries(&mut self, pattern: Pattern<'_>) -> Result<()> {
        let local = self.fork().await?;

        let entries: Vec<_> = pattern
            .apply(&self.read().await)?
            .map(|entry| {
                let name = entry.name().to_owned();
                let author = *entry.author().unwrap_or_else(|| local.this_writer_id());
                let vv = entry.version_vector().into_owned();

                (name, author, vv)
            })
            .collect();

        let mut local_writer = local.write().await;

        for (name, author, vv) in entries {
            local_writer.remove_entry(&name, &author, vv, None).await?;
        }

        local_writer.flush(None).await
    }

    #[async_recursion]
    async fn remove_entries_recursively<'a>(&'a mut self, pattern: Pattern<'a>) -> Result<()> {
        let dirs = future::try_join_all(
            pattern
                .apply(&self.read().await)?
                .filter_map(|e| e.directory().ok())
                .map(|e| async move { e.open(MissingVersionStrategy::Skip).await }),
        )
        .await?;

        for mut dir in dirs {
            dir.remove_entries_recursively(Pattern::All).await?;
        }

        self.remove_entries(pattern).await
    }

    pub async fn flush(&mut self) -> Result<()> {
        if let Some(version) = self.local_version().await {
            version.flush(None).await?
        }

        Ok(())
    }

    #[async_recursion]
    pub async fn merge(&mut self) -> Result<Directory> {
        let local_version = self.fork().await?;

        let new_version_vector = self.merge_version_vectors().await;
        let old_version_vector = local_version.read().await.version_vector().await;

        if old_version_vector >= new_version_vector {
            // Local version already up to date, nothing to do.
            return Ok(local_version);
        }

        // To avoid deadlock, collect the files and directories and only fork/merge them after
        // releasing the read lock.
        let mut files = vec![];
        let mut subdirs = vec![];

        for entry in self.read().await.entries() {
            match entry {
                JointEntryRef::File(entry) => files.push(entry.open().await?),
                JointEntryRef::Directory(entry) => {
                    subdirs.push(entry.open(MissingVersionStrategy::Fail).await?)
                }
            }
        }

        // NOTE: we might consider doing the following concurrently using `try_join` or similar,
        // but as they all need to access the same database there would probably be no benefit in
        // it. It might actually end up being slower due to overhead.

        // Fork files
        for mut file in files {
            match file.fork().await {
                // `EntryExists` error means the file already exists locally at the same or greater
                // version than the remote file which is OK and expected, so we ignore it.
                Ok(()) | Err(Error::EntryExists) => (),
                Err(error) => return Err(error),
            }
        }

        // Merge subdirectories
        for mut dir in subdirs {
            dir.merge().await?;
        }

        local_version.flush(Some(&new_version_vector)).await?;

        Ok(local_version)
    }

    // Ensures this joint directory contains a local version and returns it.
    async fn fork(&mut self) -> Result<Directory> {
        if let Some(local) = self.local_version().await {
            return Ok(local.clone());
        }

        // Grab any version and fork it to create the local one.
        let remote = self.versions.values().next().ok_or(Error::EntryNotFound)?;
        let local = remote.clone().fork().await?;

        let id = *local.read().await.branch().id();
        self.versions.insert(id, local.clone());

        Ok(local)
    }

    // Merge the version vectors of all the versions in this joint directory.
    async fn merge_version_vectors(&self) -> VersionVector {
        let mut outcome = VersionVector::new();

        for version in self.versions.values() {
            outcome.merge(&version.read().await.version_vector().await);
        }

        outcome
    }

    async fn local_version(&self) -> Option<&Directory> {
        // TODO: Consider storing the local version separately, so accessing it is quicker (O(1)).

        for version in self.versions.values() {
            if version.read().await.is_local() {
                return Some(version);
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
    pub fn lookup<'a>(&'a self, name: &'a str) -> impl Iterator<Item = JointEntryRef<'a>> + 'a {
        Merge::new(
            self.0
                .iter()
                .filter_map(move |dir| dir.lookup(name).ok())
                .flatten(),
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

        for entry in Merge::new(self.entry_versions(name)) {
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

        let entries = self
            .entry_versions(name)
            .filter_map(|entry| entry.file().ok())
            .filter(|entry| entry.author().starts_with(&branch_id_prefix));

        // At this point, `entries` contains files from only a single author. It may still be the
        // case however that there are multiple versions of the entry because each branch may
        // contain one.
        // NOTE: Using keep_maximal may be an overkill in this case because of the invariant that
        // no single author/replica can create concurrent versions of an entry.
        let mut entries = keep_maximal(entries).into_iter();

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
    pub fn lookup_version(&self, name: &'_ str, branch_id: &'_ PublicKey) -> Result<FileRef> {
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

    pub(crate) fn merge_version_vectors(&self, name: &str) -> VersionVector {
        self.entry_versions(name)
            .fold(VersionVector::new(), |mut vv, entry| {
                vv.merge(entry.version_vector());
                vv
            })
    }

    fn entry_versions<'a>(&'a self, name: &'a str) -> impl Iterator<Item = EntryRef<'a>> {
        self.0
            .iter()
            .filter_map(move |r| r.lookup(name).ok())
            .flatten()
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
            Self::File(_) => EntryType::File,
            Self::Directory(_) => EntryType::Directory,
        }
    }

    pub fn version_vector(&'a self) -> Cow<'a, VersionVector> {
        match self {
            Self::File(r) => Cow::Borrowed(r.version_vector()),
            Self::Directory(r) => Cow::Owned(r.version_vector()),
        }
    }

    // If this is `Directory`, returns `None` because a joint directory can have multiple authors.
    pub fn author(&self) -> Option<&'a PublicKey> {
        match self {
            Self::File(r) => Some(r.author()),
            Self::Directory(_) => None,
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
            Self::File(_) => Err(Error::EntryIsFile),
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
            Cow::from(versioned_file_name::create(self.name(), self.author()))
        } else {
            Cow::from(self.name())
        }
    }

    pub async fn open(&self) -> Result<File> {
        self.file.open().await
    }

    pub fn author(&self) -> &'a PublicKey {
        self.file.author()
    }

    pub fn version_vector(&self) -> &'a VersionVector {
        self.file.version_vector()
    }

    pub fn parent(&self) -> &Directory {
        self.file.parent()
    }

    pub fn inner(&self) -> FileRef<'a> {
        self.file
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

    pub fn version_vector(&self) -> VersionVector {
        self.0.iter().fold(VersionVector::new(), |mut vv, dir| {
            vv.merge(dir.version_vector());
            vv
        })
    }

    pub async fn open(
        &self,
        missing_version_strategy: MissingVersionStrategy,
    ) -> Result<JointDirectory> {
        let directories = future::try_join_all(self.0.iter().map(|dir| async move {
            match dir.open().await {
                Ok(open_dir) => Ok(Some(open_dir)),
                Err(e) if dir.is_local() => {
                    log::error!(
                        "failed to open directory '{}' on the local branch: {:?}",
                        self.name(),
                        e
                    );
                    Err(e)
                }
                Err(Error::EntryNotFound | Error::BlockNotFound(_))
                    if matches!(missing_version_strategy, MissingVersionStrategy::Skip) =>
                {
                    // Some of the directories on remote branches may fail due to them not yet
                    // being fully downloaded from remote peers. This is OK and we'll treat such
                    // cases as if this replica doesn't know about those directories.
                    Ok(None)
                }
                Err(e) => {
                    log::error!(
                        "failed to open directory '{}' on a remote branch: {:?}",
                        self.name(),
                        e
                    );
                    Err(e)
                }
            }
        }))
        .await?
        .into_iter()
        .flatten();

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

/// How to handle opening a joint directory that has some versions that are not fully loaded yet.
#[derive(Copy, Clone)]
pub enum MissingVersionStrategy {
    /// Ignore the missing versions
    Skip,
    /// Fail the whole open operation
    Fail,
}

// Iterator adaptor that maps iterator of `EntryRef` to iterator of `JointEntryRef` by filtering
// out the outdated (according the their version vectors) versions and then merging all
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

        let entries = keep_maximal(entries);

        for entry in entries {
            match entry {
                EntryRef::File(file) => files.push_back(file),
                EntryRef::Directory(dir) => directories.push(dir),
                EntryRef::Tombstone(_) => {}
            }
        }

        let needs_disambiguation = files.len() > 1;

        Self {
            files,
            directories,
            needs_disambiguation,
        }
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

trait Versioned {
    fn version_vector(&self) -> &VersionVector;
    fn is_local(&self) -> bool;
}

impl Versioned for EntryRef<'_> {
    fn version_vector(&self) -> &VersionVector {
        EntryRef::version_vector(self)
    }

    fn is_local(&self) -> bool {
        EntryRef::is_local(self)
    }
}

impl Versioned for FileRef<'_> {
    fn version_vector(&self) -> &VersionVector {
        FileRef::version_vector(self)
    }

    fn is_local(&self) -> bool {
        FileRef::is_local(self)
    }
}

// Returns the entries with the maximal version vectors.
fn keep_maximal<E: Versioned>(entries: impl Iterator<Item = E>) -> Vec<E> {
    let mut max: Vec<E> = Vec::new();

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

enum Pattern<'a> {
    // Fetch all entries
    All,
    // Fetch single entry that matches the given unique name
    Unique(&'a str),
}

impl<'a> Pattern<'a> {
    fn apply(&self, reader: &'a Reader) -> Result<impl Iterator<Item = JointEntryRef<'a>>> {
        match self {
            Self::All => Ok(Either::Left(reader.entries())),
            Self::Unique(name) => reader
                .lookup_unique(name)
                .map(|entry| Either::Right(iter::once(entry))),
        }
    }
}
