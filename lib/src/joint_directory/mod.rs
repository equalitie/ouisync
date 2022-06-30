#[cfg(test)]
mod tests;
pub(crate) mod versioned;

use crate::{
    branch::Branch,
    crypto::sign::PublicKey,
    db,
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
use std::{
    borrow::Cow,
    collections::{BTreeMap, VecDeque},
    fmt, iter, mem,
};

/// Unified view over multiple concurrent versions of a directory.
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
            .map(|dir| (*dir.branch_id(), dir))
            .collect();

        Self {
            versions,
            local_branch,
        }
    }

    // TODO: replace with regular `Clone` impl once we get rid of the Directory lock.
    pub async fn clone(&self) -> JointDirectory {
        let mut versions = BTreeMap::new();
        for (id, version) in &self.versions {
            versions.insert(*id, version.clone().await);
        }

        Self {
            versions,
            local_branch: self.local_branch.clone(),
        }
    }

    /// Lock this joint directory for reading.
    pub async fn read(&self) -> Reader<'_> {
        let mut versions = BTreeMap::new();

        for (branch_id, dir) in &self.versions {
            versions.insert(branch_id, dir.read().await);
        }

        Reader {
            versions,
            local_branch: self.local_branch.as_ref(),
        }
    }

    /// Descends into an arbitrarily nested subdirectory of this directory at the specified path.
    /// Note: non-normalized paths (i.e. containing "..") or Windows-style drive prefixes
    /// (e.g. "C:") are not supported.
    pub async fn cd(&self, conn: &mut db::Connection, path: impl AsRef<Utf8Path>) -> Result<Self> {
        let mut curr = self.clone().await;

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
                        .open(conn, MissingVersionStrategy::Skip)
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

    /// Removes the specified entry from this directory. If the entry is a subdirectory, it has to
    /// be empty. Use [Self::remove_entry_recursively] to remove non-empty subdirectories.
    pub async fn remove_entry(&mut self, conn: &mut db::Connection, name: &str) -> Result<()> {
        self.remove_entries(conn, Pattern::Unique(name)).await
    }

    /// Removes the specified entry from this directory, including all its content if it is a
    /// subdirectory.
    pub async fn remove_entry_recursively(
        &mut self,
        conn: &mut db::Connection,
        name: &str,
    ) -> Result<()> {
        self.remove_entries_recursively(conn, Pattern::Unique(name))
            .await
    }

    async fn remove_entries(
        &mut self,
        conn: &mut db::Connection,
        pattern: Pattern<'_>,
    ) -> Result<()> {
        let local_branch = self.local_branch.as_ref().ok_or(Error::PermissionDenied)?;
        let mut local = fork(conn, &mut self.versions, local_branch).await?;

        let entries: Vec<_> = pattern
            .apply(&self.read().await)?
            .map(|entry| {
                let name = entry.name().to_owned();
                let branch_id = match &entry {
                    JointEntryRef::File(entry) => *entry.branch_id(),
                    JointEntryRef::Directory(_) => *local_branch.id(),
                };
                let vv = entry.version_vector().into_owned();

                (name, branch_id, vv)
            })
            .collect();

        for (name, branch_id, vv) in entries {
            local.remove_entry(conn, &name, &branch_id, vv).await?;
        }

        Ok(())
    }

    #[async_recursion]
    async fn remove_entries_recursively<'a>(
        &'a mut self,
        conn: &mut db::Connection,
        pattern: Pattern<'a>,
    ) -> Result<()> {
        let mut dirs = Vec::new();

        for entry in pattern
            .apply(&self.read().await)?
            .filter_map(|e| e.directory().ok())
        {
            dirs.push(entry.open(conn, MissingVersionStrategy::Skip).await?);
        }

        for mut dir in dirs {
            dir.remove_entries_recursively(conn, Pattern::All).await?;
        }

        self.remove_entries(conn, pattern).await
    }

    /// Merge all versions of this `JointDirectory` into a single `Directory`.
    ///
    /// In the presence of conflicts (multiple concurrent versions of the same file) this function
    /// still proceeds as far as it can, but the conflicting files remain unmerged.
    ///
    /// Note: unlikely all the other methods in `JointDirectory`, this one takes `db::Pool`, not
    /// `db::Connection`. The reason is that for large repositories, this function can take long
    /// time to complete and this allows it to periodically release (and re-acquire) the connection
    /// to prevent starving other tasks.
    ///
    /// TODO: consider returning the conflicting paths as well.
    #[async_recursion]
    pub async fn merge(&mut self, db: &db::Pool) -> Result<Directory> {
        let mut conn = db.acquire().await?;

        let local_branch = self.local_branch.as_ref().ok_or(Error::PermissionDenied)?;
        let local_version = fork(&mut conn, &mut self.versions, local_branch).await?;

        let new_version_vector = self.merge_version_vectors(&mut conn).await?;
        let old_version_vector = local_version.version_vector(&mut conn).await?;

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
                JointEntryRef::File(entry) => files.push(entry.open(&mut conn).await?),
                JointEntryRef::Directory(entry) => {
                    subdirs.push(entry.open(&mut conn, MissingVersionStrategy::Fail).await?)
                }
            }
        }

        // Drop the connection now and re-acquire it for each file/subdirectory separately, to give
        // other tasks chance to acquire it too.
        drop(conn);

        let mut bump = true;

        // Fork files.
        for mut file in files {
            let mut conn = db.acquire().await?;

            match file.fork(&mut conn, local_branch.clone()).await {
                Ok(()) => (),
                Err(Error::EntryExists) => {
                    // This error indicates the local and the remote files are in conflict and
                    // so can't be automatically merged. We still proceed with merging the
                    // remaining entries but we won't mark this directory as merged (by bumping its
                    // vv) to prevent the conflicting remote file from being collected.
                    bump = false;
                }
                Err(error) => return Err(error),
            }
        }

        // Merge subdirectories
        for mut dir in subdirs {
            dir.merge(db).await?;
        }

        if bump {
            let mut conn = db.acquire().await?;
            local_version.bump(&mut conn, new_version_vector).await?;
        }

        Ok(local_version)
    }

    // Merge the version vectors of all the versions in this joint directory.
    async fn merge_version_vectors(&self, conn: &mut db::Connection) -> Result<VersionVector> {
        let mut outcome = VersionVector::new();

        for version in self.versions.values() {
            outcome.merge(&version.version_vector(conn).await?);
        }

        Ok(outcome)
    }
}

// Ensures this joint directory contains a local version and returns it.
// Note this is not a method to work around borrow checker.
async fn fork(
    conn: &mut db::Connection,
    versions: &mut BTreeMap<PublicKey, Directory>,
    local_branch: &Branch,
) -> Result<Directory> {
    if let Some(local) = versions.get(local_branch.id()) {
        return Ok(local.clone().await);
    }

    // Grab any version and fork it to create the local one.
    let remote = versions.values().next().ok_or(Error::EntryNotFound)?;
    let local = remote.fork(conn, local_branch).await?;

    versions.insert(*local_branch.id(), local.clone().await);

    Ok(local)
}

impl fmt::Debug for JointDirectory {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("JointDirectory").finish()
    }
}

/// View of a `JointDirectory` for performing read-only queries.
pub struct Reader<'a> {
    versions: BTreeMap<&'a PublicKey, directory::Reader<'a>>,
    local_branch: Option<&'a Branch>,
}

impl Reader<'_> {
    /// Returns iterator over the entries of this directory. Multiple concurrent versions of the
    /// same file are returned as separate `JointEntryRef::File` entries. Multiple concurrent
    /// versions of the same directory are returned as a single `JointEntryRef::Directory` entry.
    pub fn entries(&self) -> impl Iterator<Item = JointEntryRef> {
        let entries = self.versions.values().map(|directory| directory.entries());
        let entries = SortedUnion::new(entries, |entry| entry.name());
        let entries = Accumulate::new(entries, |entry| entry.name());

        entries.flat_map(|(_, entries)| Merge::new(entries.into_iter(), self.local_branch))
    }

    /// Returns all versions of an entry with the given name. Concurrent file versions are returned
    /// separately but concurrent directory versions are merged into a single `JointDirectory`.
    pub fn lookup<'a>(&'a self, name: &'a str) -> impl Iterator<Item = JointEntryRef<'a>> + 'a {
        Merge::new(
            self.versions
                .values()
                .filter_map(move |dir| dir.lookup(name).ok()),
            self.local_branch,
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

        for entry in Merge::new(self.entry_versions(name), self.local_branch) {
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
            .filter(|entry| entry.branch().id().starts_with(&branch_id_prefix));

        // At this point, `entries` contains files from only a single author. It may still be the
        // case however that there are multiple versions of the entry because each branch may
        // contain one.
        // NOTE: Using keep_maximal may be an overkill in this case because of the invariant that
        // no single author/replica can create concurrent versions of an entry.
        let mut entries =
            versioned::keep_maximal(entries, self.local_branch.map(Branch::id)).into_iter();

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
    pub fn lookup_version(&self, name: &'_ str, branch_id: &'_ PublicKey) -> Result<FileRef> {
        self.versions
            .get(branch_id)
            .ok_or(Error::EntryNotFound)
            .and_then(|dir| dir.lookup(name))
            .and_then(|entry| entry.file())
    }

    /// Length of the directory in bytes. If there are multiple versions, returns the sum of their
    /// lengths.
    #[allow(clippy::len_without_is_empty)]
    pub async fn len(&self) -> u64 {
        let mut sum = 0;
        for dir in self.versions.values() {
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
        self.versions
            .values()
            .filter_map(move |r| r.lookup(name).ok())
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
            Cow::from(versioned_file_name::create(
                self.name(),
                self.file.branch().id(),
            ))
        } else {
            Cow::from(self.name())
        }
    }

    pub async fn open(&self, conn: &mut db::Connection) -> Result<File> {
        self.file.open(conn).await
    }

    pub fn version_vector(&self) -> &'a VersionVector {
        self.file.version_vector()
    }

    pub fn branch_id(&self) -> &PublicKey {
        self.file.branch().id()
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
}

impl<'a> JointDirectoryRef<'a> {
    fn new(versions: Vec<DirectoryRef<'a>>, local_branch: Option<&'a Branch>) -> Option<Self> {
        if versions.is_empty() {
            None
        } else {
            Some(Self {
                versions,
                local_branch,
            })
        }
    }

    pub fn name(&self) -> &'a str {
        self.versions
            .first()
            .expect("joint directory must contain at least one directory")
            .name()
    }

    pub fn version_vector(&self) -> VersionVector {
        self.versions
            .iter()
            .fold(VersionVector::new(), |mut vv, dir| {
                vv.merge(dir.version_vector());
                vv
            })
    }

    pub async fn open(
        &self,
        conn: &mut db::Connection,
        missing_version_strategy: MissingVersionStrategy,
    ) -> Result<JointDirectory> {
        let mut versions = Vec::new();
        for version in &self.versions {
            match version.open(conn).await {
                Ok(open_dir) => versions.push(open_dir),
                Err(e)
                    if self
                        .local_branch
                        .map(|local_branch| version.branch().id() == local_branch.id())
                        .unwrap_or(false) =>
                {
                    log::error!(
                        "failed to open directory '{}' on the local branch: {:?}",
                        self.name(),
                        e
                    );
                    return Err(e);
                }
                Err(Error::EntryNotFound | Error::BlockNotFound(_))
                    if matches!(missing_version_strategy, MissingVersionStrategy::Skip) =>
                {
                    // Some of the directories on remote branches may fail due to them not yet
                    // being fully downloaded from remote peers. This is OK and we'll treat such
                    // cases as if this replica doesn't know about those directories.
                    continue;
                }
                Err(e) => {
                    log::error!(
                        "failed to open directory '{}' on a remote branch: {:?}",
                        self.name(),
                        e
                    );
                    return Err(e);
                }
            }
        }

        Ok(JointDirectory::new(self.local_branch.cloned(), versions))
    }

    pub(crate) fn versions(&self) -> &[DirectoryRef] {
        &self.versions
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
    local_branch: Option<&'a Branch>,
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

        let entries = versioned::keep_maximal(entries, local_branch.map(Branch::id));

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
            local_branch,
        }
    }
}

impl<'a> Iterator for Merge<'a> {
    type Item = JointEntryRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.directories.is_empty() {
            return JointDirectoryRef::new(mem::take(&mut self.directories), self.local_branch)
                .map(JointEntryRef::Directory);
        }

        let file = self.files.pop_front()?;

        Some(JointEntryRef::File(JointFileRef {
            file,
            needs_disambiguation: self.needs_disambiguation,
        }))
    }
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
