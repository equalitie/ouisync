#[cfg(test)]
mod tests;

use crate::{
    branch::Branch,
    conflict,
    crypto::sign::PublicKey,
    directory::{
        self, Directory, DirectoryFallback, DirectoryRef, EntryRef, EntryTombstoneData, EntryType,
        FileRef,
    },
    error::{Error, Result},
    file::File,
    iterator::{Accumulate, SortedUnion},
    store,
    version_vector::VersionVector,
    versioned::{self, PreferBranch},
};
use async_recursion::async_recursion;
use camino::{Utf8Component, Utf8Path};
use either::Either;
use std::{
    borrow::Cow,
    collections::{BTreeMap, VecDeque},
    fmt, iter, mem,
};
use tracing::{instrument, Instrument};

/// Unified view over multiple concurrent versions of a directory.
#[derive(Clone)]
pub struct JointDirectory {
    versions: BTreeMap<PublicKey, Directory>,
    local_branch: Option<Branch>,
}

impl JointDirectory {
    /// Creates a new `JointDirectory` over the specified directory versions.
    ///
    /// Note: if `local_branch` is `None` then the created joint directory is read-only.
    pub fn new<I>(local_branch: Option<Branch>, versions: I) -> Self
    where
        I: IntoIterator<Item = Directory>,
    {
        let versions = versions
            .into_iter()
            .map(|dir| (*dir.branch().id(), dir))
            .collect();

        Self {
            versions,
            local_branch,
        }
    }

    pub(crate) fn local_version(&self) -> Option<&Directory> {
        self.local_branch
            .as_ref()
            .and_then(|branch| self.versions.get(branch.id()))
    }

    pub(crate) fn local_version_mut(&mut self) -> Option<&mut Directory> {
        self.local_branch
            .as_ref()
            .and_then(|branch| self.versions.get_mut(branch.id()))
    }

    pub fn is_empty(&self) -> bool {
        self.entries().next().is_none()
    }

    /// Returns iterator over the entries of this directory. Multiple concurrent versions of the
    /// same file are returned as separate `JointEntryRef::File` entries. Multiple concurrent
    /// versions of the same directory are returned as a single `JointEntryRef::Directory` entry.
    pub fn entries(&self) -> impl Iterator<Item = JointEntryRef<'_>> {
        self.merge_entries()
            .flat_map(|(_, merge)| merge.ignore_tombstones())
    }

    fn merge_entries(&self) -> impl Iterator<Item = (&str, Merge<'_>)> {
        let entries = self.versions.values().map(|directory| directory.entries());
        let entries = SortedUnion::new(entries, |entry| entry.name());
        let entries = Accumulate::new(entries, |entry| entry.name());
        entries.map(|(name, entries)| {
            (
                name,
                Merge::new(entries.into_iter(), self.local_branch.as_ref()),
            )
        })
    }

    /// Returns all versions of an entry with the given name. Concurrent file versions are returned
    /// separately but concurrent directory versions are merged into a single `JointDirectory`.
    pub fn lookup<'a>(&'a self, name: &'a str) -> impl Iterator<Item = JointEntryRef<'a>> + 'a {
        Merge::new(
            self.versions
                .values()
                .filter_map(move |dir| dir.lookup(name).ok()),
            self.local_branch.as_ref(),
        )
        .ignore_tombstones()
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
        let mut entries =
            Merge::new(self.entry_versions(name), self.local_branch.as_ref()).ignore_tombstones();
        if let Some(entry) = entries.next() {
            if entries.next().is_none() {
                return Ok(entry);
            } else {
                return Err(Error::AmbiguousEntry);
            }
        }

        // If not found, extract the disambiguator and try to lookup an entry whose branch id
        // matches it.
        let (name, branch_id_prefix) = conflict::parse_unique_name(name);
        let branch_id_prefix = branch_id_prefix.ok_or(Error::EntryNotFound)?;

        let mut entries = Merge::new(self.entry_versions(name), self.local_branch.as_ref())
            .ignore_tombstones()
            .filter(|entry| entry.first_branch().id().starts_with(&branch_id_prefix));

        let first = entries.next().ok_or(Error::EntryNotFound)?;

        if entries.next().is_none() {
            Ok(first)
        } else {
            Err(Error::AmbiguousEntry)
        }
    }

    /// Looks up a specific version of a file.
    #[instrument(skip(self), err(Debug))]
    pub fn lookup_version<'a>(
        &'a self,
        name: &'a str,
        branch_id: &'a PublicKey,
    ) -> Result<FileRef<'a>> {
        self.versions
            .get(branch_id)
            .ok_or(Error::EntryNotFound)
            .and_then(|dir| dir.lookup(name))
            .and_then(|entry| entry.file())
    }

    /// Length of the directory in bytes. If there are multiple versions, returns the sum of their
    /// lengths.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> u64 {
        self.versions.values().map(|dir| dir.len()).sum()
    }

    pub fn has_local_version(&self) -> bool {
        self.local_branch
            .as_ref()
            .map(|local_branch| self.versions.contains_key(local_branch.id()))
            .unwrap_or(false)
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
                        .lookup(name)
                        .find_map(|entry| entry.directory().ok())
                        .ok_or(Error::EntryNotFound)?
                        .open()
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
        let local_branch = self.local_branch.as_ref().ok_or(Error::PermissionDenied)?;

        let entries: Vec<_> = pattern
            .apply(self)?
            .map(|entry| {
                let name = entry.name().to_owned();
                let branch_id = match &entry {
                    JointEntryRef::File(entry) => *entry.branch().id(),
                    JointEntryRef::Directory(_) => *local_branch.id(),
                };
                let vv = entry.version_vector().into_owned();

                (name, branch_id, vv)
            })
            .collect();

        let local_version = self.fork().await?;

        for (name, branch_id, vv) in entries {
            local_version.remove_entry(&name, &branch_id, vv).await?;
        }

        Ok(())
    }

    #[async_recursion]
    async fn remove_entries_recursively<'a>(&'a mut self, pattern: Pattern<'a>) -> Result<()> {
        for entry in pattern.apply(self)?.filter_map(|e| e.directory().ok()) {
            let mut dir = entry
                .open_with(MissingVersionStrategy::Skip, DirectoryFallback::Disabled)
                .await?;
            dir.remove_entries_recursively(Pattern::All).await?;
        }

        if let Some(local_version) = self.local_version_mut() {
            local_version.refresh().await?;
        }

        self.remove_entries(pattern).await
    }

    /// Merge all versions of this `JointDirectory` into a single `Directory`.
    ///
    /// In the presence of conflicts (multiple concurrent versions of the same file) this function
    /// still proceeds as far as it can, but the conflicting files remain unmerged. It signals this
    /// by returning `Error::AmbiguousEntry`.
    #[async_recursion]
    pub async fn merge(&mut self) -> Result<Directory> {
        let old_version_vector = if let Some(local_version) = self.local_version() {
            local_version.version_vector().await?
        } else {
            VersionVector::new()
        };

        let new_version_vector = self.merge_version_vectors().await?;

        if !old_version_vector.is_empty() && old_version_vector >= new_version_vector {
            // Local version already up to date, nothing to do.
            tracing::trace!(old = ?old_version_vector, "Merge not started - already up to date");
            // unwrap is ok because if old_version_vector is non-empty it means the local version
            // must exist.
            return Ok(self.local_version().unwrap().clone());
        } else {
            tracing::trace!(old = ?old_version_vector, new = ?new_version_vector, "Merge started");
        }

        let local_version = self.fork().await?;
        let local_branch = local_version.branch().clone();

        let mut conflict = false;
        let mut check_for_removal = Vec::new();

        for (name, merge) in self.merge_entries() {
            match merge {
                Merge::Existing(existing) => {
                    for entry in existing {
                        match entry {
                            JointEntryRef::File(entry) => {
                                match entry.fork(&local_branch).await {
                                    Ok(()) => {}
                                    Err(Error::EntryExists) => {
                                        // This error indicates the local and the remote files are in conflict and
                                        // so can't be automatically merged. We still proceed with merging the
                                        // remaining entries but we won't mark this directory as merged (by bumping its
                                        // vv) to prevent the conflicting remote file from being collected.
                                        conflict = true;
                                    }
                                    Err(error) => return Err(error),
                                }
                            }
                            JointEntryRef::Directory(entry) => {
                                let mut dir = entry
                                    .open_with(
                                        MissingVersionStrategy::Fail,
                                        DirectoryFallback::Disabled,
                                    )
                                    .await?;
                                match dir
                                    .merge()
                                    .instrument(tracing::info_span!("dir", message = name))
                                    .await
                                {
                                    Ok(_) => (),
                                    Err(Error::AmbiguousEntry) => {
                                        conflict = true;
                                    }
                                    Err(error) => return Err(error),
                                }
                            }
                        }
                    }
                }
                Merge::Tombstone(tombstone) => {
                    check_for_removal.push((name.to_owned(), tombstone));
                }
            }
        }

        // unwrap is ok because we ensured the local version exists by calling `fork` at the
        // beginning of this function.
        let local_version = self.local_version_mut().unwrap();
        local_version.refresh().await?;

        for (name, tombstone) in check_for_removal {
            local_version.create_tombstone(&name, tombstone).await?;
        }

        // Need to bump the root version vector to reflect any non-filesystem changes (e.g.,
        // removal of nodes during garbage collection).
        if !conflict && local_version.is_root() {
            directory::bump_root(&local_branch, new_version_vector).await?;
        }

        if tracing::enabled!(tracing::Level::TRACE) {
            let vv = local_version.version_vector().await?;
            tracing::trace!(?vv, ?conflict, "Merge completed");
        }

        if conflict {
            Err(Error::AmbiguousEntry)
        } else {
            Ok(local_version.clone())
        }
    }

    // Merge the version vectors of all the versions in this joint directory.
    async fn merge_version_vectors(&self) -> Result<VersionVector> {
        let mut outcome = VersionVector::new();

        for version in self.versions.values() {
            outcome.merge(&version.version_vector().await?);
        }

        Ok(outcome)
    }

    async fn fork(&mut self) -> Result<&mut Directory> {
        let local_branch = self.local_branch.as_ref().ok_or(Error::PermissionDenied)?;

        let mut local_version = None;

        // Need to `fork` from each branch individually so that the local version vector is
        // properly updated.
        for (branch_id, version) in &self.versions {
            if branch_id == local_branch.id() {
                continue;
            }

            local_version = Some(version.fork(local_branch).await?);
        }

        if let Some(local_version) = local_version {
            self.versions.insert(*local_branch.id(), local_version);
        }

        // TODO: This can return error only if this `JointDirectory` contains no versions which should
        // never happen. Consider making it an invariant and doing `unwrap` / `expect` here instead.
        self.versions
            .get_mut(local_branch.id())
            .ok_or(Error::EntryNotFound)
    }

    fn entry_versions<'a>(&'a self, name: &'a str) -> impl Iterator<Item = EntryRef<'a>> {
        self.versions
            .values()
            .filter_map(move |v| v.lookup(name).ok())
    }
}

impl fmt::Debug for JointDirectory {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("JointDirectory").finish()
    }
}

#[derive(Debug)]
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
            Self::Directory(r) => r.unique_name(),
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

    fn first_branch(&self) -> &Branch {
        match self {
            Self::File(r) => r.branch(),
            Self::Directory(r) => r.first_version().branch(),
        }
    }
}

#[derive(Debug)]
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
            Cow::from(conflict::create_unique_name(
                self.name(),
                self.file.branch().id(),
            ))
        } else {
            Cow::from(self.name())
        }
    }

    pub async fn open(&self) -> Result<File> {
        self.file.open().await
    }

    pub(crate) async fn fork(&self, dst_branch: &Branch) -> Result<()> {
        self.file.fork(dst_branch).await
    }

    pub fn version_vector(&self) -> &'a VersionVector {
        self.file.version_vector()
    }

    pub fn branch(&self) -> &Branch {
        self.file.branch()
    }

    pub fn parent(&self) -> &Directory {
        self.file.parent()
    }

    pub fn inner(&self) -> FileRef<'a> {
        self.file
    }
}

pub struct JointDirectoryRef<'a> {
    versions: Vec<DirectoryRef<'a>>,
    local_branch: Option<&'a Branch>,
    needs_disambiguation: bool,
}

impl<'a> JointDirectoryRef<'a> {
    fn new(
        versions: Vec<DirectoryRef<'a>>,
        local_branch: Option<&'a Branch>,
        needs_disambiguation: bool,
    ) -> Option<Self> {
        if versions.is_empty() {
            None
        } else {
            Some(Self {
                versions,
                local_branch,
                needs_disambiguation,
            })
        }
    }

    pub fn name(&self) -> &'a str {
        self.first_version().name()
    }

    pub fn unique_name(&self) -> Cow<'a, str> {
        if self.needs_disambiguation {
            Cow::from(conflict::create_unique_name(
                self.name(),
                self.first_version().branch().id(),
            ))
        } else {
            Cow::from(self.name())
        }
    }

    pub fn version_vector(&self) -> VersionVector {
        self.versions
            .iter()
            .fold(VersionVector::new(), |mut vv, dir| {
                vv.merge(dir.version_vector());
                vv
            })
    }

    pub async fn open(&self) -> Result<JointDirectory> {
        self.open_with(MissingVersionStrategy::Skip, DirectoryFallback::Enabled)
            .await
    }

    pub(crate) async fn open_with(
        &self,
        missing_version_strategy: MissingVersionStrategy,
        fallback: DirectoryFallback,
    ) -> Result<JointDirectory> {
        let mut versions = Vec::new();
        for version in &self.versions {
            match version.open(fallback).await {
                Ok(open_dir) => versions.push(open_dir),
                Err(e)
                    if self
                        .local_branch
                        .map(|local_branch| version.branch().id() == local_branch.id())
                        .unwrap_or(false) =>
                {
                    return Err(e)
                }
                Err(Error::Store(store::Error::BlockNotFound))
                    if matches!(missing_version_strategy, MissingVersionStrategy::Skip) =>
                {
                    // Some of the directories on remote branches may fail due to them not yet
                    // being fully downloaded from remote peers. This is OK and we'll treat such
                    // cases as if this replica doesn't know about those directories.
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        Ok(JointDirectory::new(self.local_branch.cloned(), versions))
    }

    pub(crate) fn versions(&self) -> &[DirectoryRef<'_>] {
        &self.versions
    }

    fn first_version(&self) -> &DirectoryRef<'a> {
        self.versions
            .first()
            .expect("joint directory must contain at least one directory")
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
enum Merge<'a> {
    // zero or more versions of an entry...
    Existing(Existing<'a>),
    // ...or a single tombstone
    Tombstone(EntryTombstoneData),
}

#[derive(Default, Clone)]
struct Existing<'a> {
    // TODO: The most common case for files shall be that there will be only one version of it.
    // Thus it might make sense to have one place holder for the first file to avoid Vec allocation
    // when not needed.
    files: VecDeque<FileRef<'a>>,
    directories: Vec<DirectoryRef<'a>>,
    needs_disambiguation: bool,
    local_branch: Option<&'a Branch>,
}

impl<'a> Iterator for Existing<'a> {
    type Item = JointEntryRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(dir) = JointDirectoryRef::new(
            mem::take(&mut self.directories),
            self.local_branch,
            self.needs_disambiguation,
        ) {
            return Some(JointEntryRef::Directory(dir));
        }

        Some(JointEntryRef::File(JointFileRef {
            file: self.files.pop_front()?,
            needs_disambiguation: self.needs_disambiguation,
        }))
    }
}

impl<'a> Merge<'a> {
    // All these entries are expected to have the same name. They can be either files, directories
    // or a mix of the two.
    fn new<I>(entries: I, local_branch: Option<&'a Branch>) -> Self
    where
        I: Iterator<Item = EntryRef<'a>>,
    {
        let mut files = VecDeque::new();
        let mut directories = vec![];
        let mut tombstone: Option<EntryTombstoneData> = None;

        // Note that doing this will remove files that have been removed by tombstones as well.
        let entries = versioned::keep_maximal(entries, PreferBranch(local_branch.map(Branch::id)));

        for entry in entries {
            match entry {
                EntryRef::File(file) => files.push_back(file),
                EntryRef::Directory(dir) => directories.push(dir),
                EntryRef::Tombstone(_) if !files.is_empty() || !directories.is_empty() => continue,
                EntryRef::Tombstone(new_tombstone) => {
                    let new_tombstone = if let Some(mut old_tombstone) = tombstone.take() {
                        old_tombstone.merge(new_tombstone.data());
                        old_tombstone
                    } else {
                        new_tombstone.data().clone()
                    };

                    tombstone = Some(new_tombstone);
                }
            }
        }

        let needs_disambiguation = files.len() + if directories.is_empty() { 0 } else { 1 } > 1;

        match tombstone {
            Some(tombstone) if files.is_empty() && directories.is_empty() => {
                Self::Tombstone(tombstone)
            }
            Some(_) | None => Self::Existing(Existing {
                files,
                directories,
                needs_disambiguation,
                local_branch,
            }),
        }
    }

    fn ignore_tombstones(self) -> Existing<'a> {
        match self {
            Self::Existing(existing) => existing,
            Self::Tombstone(_) => Existing::default(),
        }
    }
}

enum Pattern<'a> {
    // Fetch all entries
    All,
    // Fetch single entry that matches the given unique name
    Unique(&'a str),
}

impl<'a> Pattern<'a> {
    fn apply(&self, dir: &'a JointDirectory) -> Result<impl Iterator<Item = JointEntryRef<'a>>> {
        match self {
            Self::All => Ok(Either::Left(dir.entries())),
            Self::Unique(name) => dir
                .lookup_unique(name)
                .map(|entry| Either::Right(iter::once(entry))),
        }
    }
}
