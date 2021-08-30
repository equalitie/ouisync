use crate::{
    blob::{self, Blob},
    blob_id::BlobId,
    branch::Branch,
    debug_printer::DebugPrinter,
    entry_type::EntryType,
    error::{Error, Result},
    file::File,
    locator::Locator,
    parent_context::ParentContext,
    replica_id::ReplicaId,
    version_vector::VersionVector,
};
use async_recursion::async_recursion;
use serde::{Deserialize, Serialize};
use std::{
    collections::{btree_map, hash_map, BTreeMap, HashMap},
    fmt, iter,
    ops::DerefMut,
    sync::{Arc, Weak},
};
use tokio::sync::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};

#[derive(Clone)]
pub struct Directory {
    inner: Arc<RwLock<Inner>>,
    local_branch: Branch,
    parent: Option<Box<ParentContext>>, // box needed to avoid infinite type recursion
}

#[allow(clippy::len_without_is_empty)]
impl Directory {
    /// Opens the root directory.
    /// For internal use only. Use [`Branch::open_root`] instead.
    pub(crate) async fn open_root(owner_branch: Branch, local_branch: Branch) -> Result<Self> {
        Self::open(owner_branch, local_branch, Locator::Root, None).await
    }

    /// Opens the root directory or creates it if it doesn't exist.
    /// For internal use only. Use [`Branch::open_or_create_root`] instead.
    pub(crate) async fn open_or_create_root(branch: Branch) -> Result<Self> {
        // TODO: make sure this is atomic

        let locator = Locator::Root;

        match Self::open(branch.clone(), branch.clone(), locator, None).await {
            Ok(dir) => Ok(dir),
            Err(Error::EntryNotFound) => Ok(Self::create(branch, locator, None)),
            Err(error) => Err(error),
        }
    }

    /// Lock this directory for reading.
    pub async fn read(&self) -> Reader<'_> {
        Reader {
            outer: self,
            inner: self.inner.read().await,
        }
    }

    /// Flushes this directory ensuring that any pending changes are written to the store and the
    /// version vectors of this and the ancestor directories are properly incremented.
    /// Also flushes all ancestor directories.
    pub async fn flush(&self) -> Result<()> {
        if !self.inner.write().await.flush().await? {
            return Ok(());
        }

        for ctx in self.ancestors() {
            ctx.increment_version().await?;
            ctx.directory.inner.write().await.flush().await?;
        }

        Ok(())
    }

    /// Creates a new file inside this directory.
    ///
    /// # Panics
    ///
    /// Panics if this directory is not in the local branch.
    pub async fn create_file(&self, name: String) -> Result<File> {
        let mut inner = self.write().await;

        let author = *self.local_branch.id();
        let vv = VersionVector::first(author);
        let blob_id = inner
            .insert_entry(author, name.clone(), EntryType::File, vv)
            .await?;
        let locator = Locator::Head(blob_id);
        let parent = ParentContext {
            directory: self.clone(),
            entry_name: name,
            entry_author: author,
        };

        Ok(File::create(self.local_branch.clone(), locator, parent).await)
    }

    /// Creates a new subdirectory of this directory.
    ///
    /// # Panics
    ///
    /// Panics if this directory is not in the local branch.
    pub async fn create_directory(&self, name: String) -> Result<Self> {
        let mut inner = self.write().await;

        let author = *self.local_branch.id();
        let vv = VersionVector::first(author);
        let blob_id = inner
            .insert_entry(author, name.clone(), EntryType::Directory, vv)
            .await?;
        let locator = Locator::Head(blob_id);
        let parent = ParentContext {
            directory: self.clone(),
            entry_name: name,
            entry_author: author,
        };

        inner
            .open_directories
            .create(self.local_branch.clone(), locator, parent)
            .await
    }

    /// Removes a file from this directory.
    ///
    /// # Panics
    ///
    /// Panics if this directory is not in the local branch.
    pub async fn remove_file(&self, name: &str) -> Result<()> {
        let mut inner = self.write().await;

        for entry in lookup(self, &*inner, name)? {
            entry.file()?.open().await?.remove().await?;
        }

        inner.content.remove(name)
        // TODO: Add tombstone
    }

    /// Removes a subdirectory from this directory.
    ///
    /// # Panics
    ///
    /// Panics if this directory is not in the local branch.
    pub async fn remove_directory(&self, name: &str) -> Result<()> {
        let mut inner = self.write().await;

        for entry in lookup(self, &*inner, name)? {
            entry.directory()?.open().await?.remove().await?;
        }

        inner.content.remove(name)
        // TODO: Add tombstone
    }

    /// Removes this directory if its empty, otherwise fails.
    pub async fn remove(&self) -> Result<()> {
        let mut inner = self.write().await;

        if !inner.content.entries.is_empty() {
            return Err(Error::DirectoryNotEmpty);
        }

        inner.blob.remove().await
    }

    // Forks this directory into the local branch.
    #[async_recursion] // TODO: consider rewriting this to avoid recursion
    pub async fn fork(&self) -> Result<Directory> {
        if let Some(parent) = &self.parent {
            let parent_dir = parent.directory.fork().await?;

            if let Ok(entry) = parent_dir
                .read()
                .await
                .lookup_version(&parent.entry_name, self.local_branch.id())
            {
                return entry.directory()?.open().await;
            }

            parent_dir
                .create_directory(parent.entry_name.to_string())
                .await
        } else {
            self.local_branch.open_or_create_root().await
        }
    }

    /// Returns the parent context, if any.
    pub(crate) fn parent(&self) -> Option<&ParentContext> {
        self.parent.as_deref()
    }

    /// Returns iterator of ancestor parent contexts from the current directory to the root.
    pub(crate) fn ancestors(&self) -> impl Iterator<Item = &ParentContext> {
        iter::successors(self.parent(), |prev| prev.directory.parent())
    }

    /// Inserts a dangling entry into this directory. It's the responsibility of the caller to make
    /// sure the returned locator eventually points to an actual file or directory.
    /// For internal use only!
    ///
    /// # Panics
    ///
    /// Panics if this directory is not in the local branch.
    pub(crate) async fn insert_entry(
        &self,
        name: String,
        entry_type: EntryType,
        version_vector: VersionVector,
    ) -> Result<BlobId> {
        let mut inner = self.write().await;
        inner
            .insert_entry(*self.local_branch.id(), name, entry_type, version_vector)
            .await
    }

    /// Increment version of the specified entry.
    pub(crate) async fn increment_entry_version(&self, name: &str) -> Result<()> {
        let mut inner_guard = self.write().await;
        let inner = inner_guard.deref_mut();

        let author = self.local_branch.id();

        // Asserting we are in the local branch
        assert_eq!(author, inner.blob.branch().id());

        inner
            .content
            .entries
            .get_mut(name)
            .and_then(|versions| versions.get_mut(author))
            .map(|entry| {
                entry.version_vector.increment(*author);
            })
            .ok_or(Error::EntryNotFound)
    }

    async fn open(
        owner_branch: Branch,
        local_branch: Branch,
        locator: Locator,
        parent: Option<ParentContext>,
    ) -> Result<Self> {
        let mut blob = Blob::open(owner_branch, locator).await?;
        let buffer = blob.read_to_end().await?;
        let content = bincode::deserialize(&buffer).map_err(Error::MalformedDirectory)?;

        Ok(Self {
            inner: Arc::new(RwLock::new(Inner {
                blob,
                content,
                open_directories: SubdirectoryCache::new(),
            })),
            local_branch,
            parent: parent.map(Box::new),
        })
    }

    fn create(owner_branch: Branch, locator: Locator, parent: Option<ParentContext>) -> Self {
        let blob = Blob::create(owner_branch.clone(), locator);

        Directory {
            inner: Arc::new(RwLock::new(Inner {
                blob,
                content: Content::new(),
                open_directories: SubdirectoryCache::new(),
            })),
            local_branch: owner_branch,
            parent: parent.map(Box::new),
        }
    }

    // Lock this directory for writing.
    //
    // # Panics
    //
    // Panics if not in the local branch.
    async fn write(&self) -> RwLockWriteGuard<'_, Inner> {
        let inner = self.inner.write().await;
        assert_eq!(
            inner.blob.branch().id(),
            self.local_branch.id(),
            "mutable operations not allowed - directory is not in the local branch"
        );
        inner
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        let inner = self.inner.read().await;

        for (name, versions) in &inner.content.entries {
            print.display(name);
            let print = print.indent();

            for (author, entry_data) in versions {
                print.display(&format!(
                    "{:?}: {:?}, blob_id:{:?}, {:?}",
                    author, entry_data.entry_type, entry_data.blob_id, entry_data.version_vector
                ));

                if entry_data.entry_type == EntryType::File {
                    let print = print.indent();

                    let parent_context = ParentContext {
                        directory: self.clone(),
                        entry_name: name.into(),
                        entry_author: *author,
                    };

                    let file = File::open(
                        inner.blob.branch().clone(),
                        self.local_branch.clone(),
                        Locator::Head(entry_data.blob_id),
                        parent_context,
                    )
                    .await;

                    match file {
                        Ok(mut file) => {
                            let content = file.read_to_end().await;
                            match content {
                                Ok(content) => {
                                    print.display(&format!(
                                        "Content: {:?}",
                                        std::str::from_utf8(&content)
                                    ));
                                }
                                Err(e) => {
                                    print.display(&format!("Failed to read {:?}", e));
                                }
                            }
                        }
                        Err(e) => {
                            print.display(&format!("Failed to open {:?}", e));
                        }
                    }
                }
            }
        }
    }
}

/// View of a `Directory` for performing read-only queries.
pub struct Reader<'a> {
    outer: &'a Directory,
    inner: RwLockReadGuard<'a, Inner>,
}

impl Reader<'_> {
    /// Returns iterator over the entries of this directory.
    pub fn entries(&self) -> impl Iterator<Item = EntryRef> + DoubleEndedIterator + Clone {
        self.inner
            .content
            .entries
            .iter()
            .flat_map(move |(name, versions)| {
                versions.iter().map(move |(author, data)| {
                    EntryRef::new(self.outer, &*self.inner, name, data, author)
                })
            })
    }

    /// Lookup an entry of this directory by name.
    pub fn lookup(
        &self,
        name: &'_ str,
    ) -> Result<impl Iterator<Item = EntryRef> + ExactSizeIterator> {
        lookup(self.outer, &*self.inner, name)
    }

    pub fn lookup_version(&self, name: &'_ str, author: &ReplicaId) -> Result<EntryRef> {
        self.inner
            .content
            .entries
            .get_key_value(name)
            .and_then(|(name, versions)| {
                versions.get_key_value(author).map(|(author, data)| {
                    EntryRef::new(self.outer, &*self.inner, name, data, author)
                })
            })
            .ok_or(Error::EntryNotFound)
    }

    /// Length of this directory in bytes. Does not include the content, only the size of directory
    /// itself.
    pub async fn len(&self) -> u64 {
        self.inner.blob.len().await
    }

    /// Locator of this directory
    pub fn locator(&self) -> &Locator {
        self.inner.blob.locator()
    }

    /// Branch of this directory
    pub fn branch(&self) -> &Branch {
        self.inner.blob.branch()
    }
}

struct Inner {
    blob: Blob,
    content: Content,
    // Cache of open subdirectories. Used to make sure that multiple instances of the same directory
    // all share the same internal state.
    open_directories: SubdirectoryCache,
}

impl Inner {
    async fn flush(&mut self) -> Result<bool> {
        if !self.content.dirty {
            return Ok(false);
        }

        let buffer =
            bincode::serialize(&self.content).expect("failed to serialize directory content");

        self.blob.truncate(0).await?;
        self.blob.write(&buffer).await?;
        self.blob.flush().await?;

        self.content.dirty = false;

        Ok(true)
    }

    async fn insert_entry(
        &mut self,
        author: ReplicaId,
        name: String,
        entry_type: EntryType,
        version_vector: VersionVector,
    ) -> Result<BlobId> {
        let (new_blob_id, old_blob_id) =
            self.content
                .insert(author, name, entry_type, version_vector)?;

        if let Some(old_blob_id) = old_blob_id {
            // TODO: This should succeed/fail atomically with the above.
            Blob::open(self.blob.branch().clone(), Locator::Head(old_blob_id))
                .await?
                .remove()
                .await?;
        }

        Ok(new_blob_id)
    }
}

fn lookup<'a>(
    outer: &'a Directory,
    inner: &'a Inner,
    name: &'_ str,
) -> Result<impl Iterator<Item = EntryRef<'a>> + ExactSizeIterator> {
    inner
        .content
        .entries
        .get_key_value(name)
        .map(|(name, versions)| {
            versions
                .iter()
                .map(move |(author, data)| EntryRef::new(outer, inner, name, data, author))
        })
        .ok_or(Error::EntryNotFound)
}

/// Info about a directory entry.
#[derive(Copy, Clone, Debug)]
pub enum EntryRef<'a> {
    File(FileRef<'a>),
    Directory(DirectoryRef<'a>),
}

impl<'a> EntryRef<'a> {
    fn new(
        parent_outer: &'a Directory,
        parent_inner: &'a Inner,
        name: &'a str,
        entry_data: &'a EntryData,
        author: &'a ReplicaId,
    ) -> Self {
        let inner = RefInner {
            parent_outer,
            parent_inner,
            entry_data,
            name,
            author,
        };

        match entry_data.entry_type {
            EntryType::File => Self::File(FileRef { inner }),
            EntryType::Directory => Self::Directory(DirectoryRef { inner }),
        }
    }

    pub fn name(&self) -> &'a str {
        match self {
            Self::File(r) => r.name(),
            Self::Directory(r) => r.name(),
        }
    }

    pub fn entry_type(&self) -> EntryType {
        match self {
            Self::File(_) => EntryType::File,
            Self::Directory(_) => EntryType::Directory,
        }
    }

    pub fn blob_id(&self) -> &BlobId {
        &self.inner().entry_data.blob_id
    }

    pub fn version_vector(&self) -> &VersionVector {
        &self.inner().entry_data.version_vector
    }

    pub fn locator(&self) -> Locator {
        Locator::Head(*self.blob_id())
    }

    pub fn file(self) -> Result<FileRef<'a>> {
        match self {
            Self::File(r) => Ok(r),
            Self::Directory(_) => Err(Error::EntryIsDirectory),
        }
    }

    pub fn directory(self) -> Result<DirectoryRef<'a>> {
        match self {
            Self::Directory(r) => Ok(r),
            Self::File(_) => Err(Error::EntryNotDirectory),
        }
    }

    pub fn is_file(&self) -> bool {
        matches!(self, Self::File(_))
    }

    pub fn is_directory(&self) -> bool {
        matches!(self, Self::Directory(_))
    }

    fn inner(&self) -> &RefInner {
        match self {
            Self::File(r) => &r.inner,
            Self::Directory(r) => &r.inner,
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub struct FileRef<'a> {
    inner: RefInner<'a>,
}

impl<'a> FileRef<'a> {
    pub fn name(&self) -> &'a str {
        self.inner.name
    }

    pub fn locator(&self) -> Locator {
        Locator::Head(self.inner.entry_data.blob_id)
    }

    pub fn author(&self) -> &'a ReplicaId {
        self.inner.author
    }

    pub async fn open(&self) -> Result<File> {
        let mut guard = self.inner.entry_data.blob_core.lock().await;
        let blob_core = &mut *guard;

        if let Some(blob_core) = blob_core.upgrade() {
            File::reopen(
                blob_core,
                self.inner.parent_outer.local_branch.clone(),
                self.inner.parent_context(),
            )
            .await
        } else {
            let file = File::open(
                self.inner.parent_inner.blob.branch().clone(),
                self.inner.parent_outer.local_branch.clone(),
                self.locator(),
                self.inner.parent_context(),
            )
            .await?;

            *blob_core = Arc::downgrade(file.blob_core());

            Ok(file)
        }
    }
}

impl fmt::Debug for FileRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("FileRef")
            .field("name", &self.inner.name)
            .field("author", &self.inner.author)
            .field("vv", &self.inner.entry_data.version_vector)
            .field("locator", &self.locator())
            .finish()
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub struct DirectoryRef<'a> {
    inner: RefInner<'a>,
}

impl<'a> DirectoryRef<'a> {
    pub fn name(&self) -> &'a str {
        self.inner.name
    }

    pub fn locator(&self) -> Locator {
        Locator::Head(self.inner.entry_data.blob_id)
    }

    pub async fn open(&self) -> Result<Directory> {
        self.inner
            .parent_inner
            .open_directories
            .open(
                self.inner.parent_inner.blob.branch().clone(),
                self.inner.parent_outer.local_branch.clone(),
                self.locator(),
                self.inner.parent_context(),
            )
            .await
    }
}

impl fmt::Debug for DirectoryRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("DirectoryRef")
            .field("name", &self.inner.name)
            .finish()
    }
}

#[derive(Copy, Clone)]
struct RefInner<'a> {
    parent_outer: &'a Directory,
    parent_inner: &'a Inner,
    entry_data: &'a EntryData,
    name: &'a str,
    author: &'a ReplicaId,
}

impl RefInner<'_> {
    fn parent_context(&self) -> ParentContext {
        ParentContext {
            directory: self.parent_outer.clone(),
            entry_name: self.name.into(),
            entry_author: *self.author,
        }
    }
}

impl PartialEq for RefInner<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.parent_inner.blob.branch().id() == other.parent_inner.blob.branch().id()
            && self.parent_inner.blob.locator() == other.parent_inner.blob.locator()
            && self.entry_data == other.entry_data
            && self.name == other.name
    }
}

impl Eq for RefInner<'_> {}

/// Destination directory of a move operation.
#[allow(clippy::large_enum_variant)]
pub enum MoveDstDirectory {
    /// Same as src
    Src,
    /// Other than src
    Other(Directory),
}

impl MoveDstDirectory {
    pub fn get<'a>(&'a mut self, src: &'a mut Directory) -> &'a mut Directory {
        match self {
            Self::Src => src,
            Self::Other(other) => other,
        }
    }

    pub fn get_other(&mut self) -> Option<&mut Directory> {
        match self {
            Self::Src => None,
            Self::Other(other) => Some(other),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct Content {
    entries: BTreeMap<String, BTreeMap<ReplicaId, EntryData>>,
    #[serde(skip)]
    dirty: bool,
}

impl Content {
    fn new() -> Self {
        Self {
            entries: Default::default(),
            dirty: true,
        }
    }

    /// Insert entry into the content.
    ///
    /// This operation succeeds if either there is no such entry in already
    /// `self.entries[name][author]` or if the existing entry's version vector "happens before" the
    /// newly inserted entry.
    ///
    /// In the latter case, the existing entry's BlobId is returned and it's the responsibility of
    /// the caller to remove it from the index.
    fn insert(
        &mut self,
        author: ReplicaId,
        name: String,
        entry_type: EntryType,
        version_vector: VersionVector,
    ) -> Result<(BlobId, Option<BlobId>)> {
        let blob_id = rand::random();
        let versions = self.entries.entry(name).or_insert_with(Default::default);

        match versions.entry(author) {
            btree_map::Entry::Vacant(entry) => {
                entry.insert(EntryData {
                    entry_type,
                    blob_id,
                    version_vector,
                    blob_core: Arc::new(Mutex::new(Weak::new())),
                });
                self.dirty = true;
                Ok((blob_id, None))
            }
            btree_map::Entry::Occupied(mut entry) => {
                let data = entry.get_mut();
                if data.entry_type != entry_type {
                    return Err(Error::EntryExists);
                }

                // If the existing entry is
                //     1. "same", or
                //     2. "happens after", or
                //     3. "concurrent"
                // then don't update it. Note that #3 should not happen because of the invariant
                // that one replica (version author) must not create concurrent entries.
                #[allow(clippy::neg_cmp_op_on_partial_ord)]
                if !(data.version_vector < version_vector) {
                    return Err(Error::EntryExists);
                }

                let old_blob_id = data.blob_id;

                data.blob_id = blob_id;
                data.version_vector = version_vector;
                self.dirty = true;

                Ok((blob_id, Some(old_blob_id)))
            }
        }
    }

    fn remove(&mut self, name: &str) -> Result<()> {
        self.entries.remove(name).ok_or(Error::EntryNotFound)?;
        self.dirty = true;

        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EntryData {
    entry_type: EntryType,
    blob_id: BlobId,
    version_vector: VersionVector,
    #[serde(skip)]
    blob_core: Arc<Mutex<Weak<Mutex<blob::Core>>>>,
}

impl PartialEq for EntryData {
    fn eq(&self, other: &Self) -> bool {
        self.entry_type == other.entry_type
            && self.blob_id == other.blob_id
            && self.version_vector == other.version_vector
    }
}

impl Eq for EntryData {}

// Cache for open root directory
// TODO: consider using the `ArcSwap` crate here.
pub struct RootDirectoryCache(Mutex<Option<Weak<RwLock<Inner>>>>);

impl RootDirectoryCache {
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }

    pub async fn open(&self, owner_branch: Branch, local_branch: Branch) -> Result<Directory> {
        let mut slot = self.0.lock().await;

        if let Some(inner) = slot.as_mut().and_then(|inner| inner.upgrade()) {
            Ok(Directory {
                inner,
                local_branch,
                parent: None,
            })
        } else {
            let dir = Directory::open_root(owner_branch, local_branch).await?;
            *slot = Some(Arc::downgrade(&dir.inner));
            Ok(dir)
        }
    }

    pub async fn open_or_create(&self, branch: Branch) -> Result<Directory> {
        let mut slot = self.0.lock().await;

        if let Some(inner) = slot.as_mut().and_then(|inner| inner.upgrade()) {
            Ok(Directory {
                inner,
                local_branch: branch,
                parent: None,
            })
        } else {
            let dir = Directory::open_or_create_root(branch).await?;
            *slot = Some(Arc::downgrade(&dir.inner));
            Ok(dir)
        }
    }
}

// Cache of open subdirectories.
struct SubdirectoryCache(Mutex<HashMap<Locator, Weak<RwLock<Inner>>>>);

impl SubdirectoryCache {
    fn new() -> Self {
        Self(Mutex::new(HashMap::new()))
    }

    async fn open(
        &self,
        owner_branch: Branch,
        local_branch: Branch,
        locator: Locator,
        parent: ParentContext,
    ) -> Result<Directory> {
        let mut map = self.0.lock().await;

        let dir = match map.entry(locator) {
            hash_map::Entry::Occupied(mut entry) => {
                if let Some(inner) = entry.get().upgrade() {
                    Directory {
                        inner,
                        local_branch,
                        parent: Some(Box::new(parent)),
                    }
                } else {
                    let dir =
                        Directory::open(owner_branch, local_branch, locator, Some(parent)).await?;
                    entry.insert(Arc::downgrade(&dir.inner));
                    dir
                }
            }
            hash_map::Entry::Vacant(entry) => {
                let dir =
                    Directory::open(owner_branch, local_branch, locator, Some(parent)).await?;
                entry.insert(Arc::downgrade(&dir.inner));
                dir
            }
        };

        // Cleanup dead entries.
        map.retain(|_, dir| dir.upgrade().is_some());

        Ok(dir)
    }

    async fn create(
        &self,
        branch: Branch,
        locator: Locator,
        parent: ParentContext,
    ) -> Result<Directory> {
        let mut map = self.0.lock().await;

        let dir = match map.entry(locator) {
            hash_map::Entry::Occupied(_) => return Err(Error::EntryExists),
            hash_map::Entry::Vacant(entry) => {
                let dir = Directory::create(branch, locator, Some(parent));
                entry.insert(Arc::downgrade(&dir.inner));
                dir
            }
        };

        map.retain(|_, dir| dir.upgrade().is_some());

        Ok(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{crypto::Cryptor, db, index::BranchData};
    use std::collections::BTreeSet;

    #[tokio::test(flavor = "multi_thread")]
    async fn create_and_list_entries() {
        let branch = setup().await;

        // Create the root directory and put some file in it.
        let dir = branch.open_or_create_root().await.unwrap();

        let mut file_dog = dir.create_file("dog.txt".into()).await.unwrap();
        file_dog.write(b"woof").await.unwrap();
        file_dog.flush().await.unwrap();

        let mut file_cat = dir.create_file("cat.txt".into()).await.unwrap();
        file_cat.write(b"meow").await.unwrap();
        file_cat.flush().await.unwrap();

        dir.flush().await.unwrap();

        // Reopen the dir and try to read the files.
        let dir = branch.open_root(branch.clone()).await.unwrap();
        let dir = dir.read().await;

        let expected_names: BTreeSet<_> = vec!["dog.txt", "cat.txt"].into_iter().collect();
        let actual_names: BTreeSet<_> = dir.entries().map(|entry| entry.name()).collect();
        assert_eq!(actual_names, expected_names);

        for &(file_name, expected_content) in &[("dog.txt", b"woof"), ("cat.txt", b"meow")] {
            let mut versions: Vec<_> = dir.lookup(file_name).unwrap().collect();
            assert_eq!(versions.len(), 1);
            let mut file = versions
                .first_mut()
                .unwrap()
                .file()
                .unwrap()
                .open()
                .await
                .unwrap();
            let actual_content = file.read_to_end().await.unwrap();
            assert_eq!(actual_content, expected_content);
        }
    }

    // TODO: test update existing directory
    #[tokio::test(flavor = "multi_thread")]
    async fn add_entry_to_existing_directory() {
        let branch = setup().await;

        // Create empty directory
        let dir = branch.open_or_create_root().await.unwrap();
        dir.flush().await.unwrap();

        // Reopen it and add a file to it.
        let dir = branch.open_root(branch.clone()).await.unwrap();
        dir.create_file("none.txt".into()).await.unwrap();
        dir.flush().await.unwrap();

        // Reopen it again and check the file is still there.
        let dir = branch.open_root(branch.clone()).await.unwrap();
        assert!(dir.read().await.lookup("none.txt").is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_file() {
        let branch = setup().await;

        let name = "monkey.txt";

        // Create a directory with a single file.
        let parent_dir = branch.open_or_create_root().await.unwrap();
        let mut file = parent_dir.create_file(name.into()).await.unwrap();
        file.flush().await.unwrap();
        parent_dir.flush().await.unwrap();

        let file_locator = *file.locator();

        // Reopen and remove the file
        let parent_dir = branch.open_root(branch.clone()).await.unwrap();
        parent_dir.remove_file(name).await.unwrap();
        parent_dir.flush().await.unwrap();

        // Reopen again and check the file entry was removed.
        let parent_dir = branch.open_root(branch.clone()).await.unwrap();
        let parent_dir = parent_dir.read().await;

        match parent_dir.lookup(name) {
            Err(Error::EntryNotFound) => (),
            Err(error) => panic!("unexpected error {:?}", error),
            Ok(_) => panic!("entry should not exists but it does"),
        }

        assert_eq!(parent_dir.entries().count(), 0);

        // Check the file blob itself was removed as well.
        match Blob::open(branch, file_locator).await {
            Err(Error::EntryNotFound) => (),
            Err(error) => panic!("unexpected error {:?}", error),
            Ok(_) => panic!("file blob should not exists but it does"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_subdirectory() {
        let branch = setup().await;

        let name = "dir";

        // Create a directory with a single subdirectory.
        let parent_dir = branch.open_or_create_root().await.unwrap();
        let dir = parent_dir.create_directory(name.into()).await.unwrap();
        dir.flush().await.unwrap();
        parent_dir.flush().await.unwrap();

        let dir_locator = *dir.read().await.locator();

        // Reopen and remove the subdirectory
        let parent_dir = branch.open_root(branch.clone()).await.unwrap();
        parent_dir.remove_directory(name).await.unwrap();
        parent_dir.flush().await.unwrap();

        // Reopen again and check the subdirectory entry was removed.
        let parent_dir = branch.open_root(branch.clone()).await.unwrap();
        let parent_dir = parent_dir.read().await;
        match parent_dir.lookup(name) {
            Err(Error::EntryNotFound) => (),
            Err(error) => panic!("unexpected error {:?}", error),
            Ok(_) => panic!("entry should not exists but it does"),
        }

        // Check the directory blob itself was removed as well.
        match Blob::open(branch, dir_locator).await {
            Err(Error::EntryNotFound) => (),
            Err(error) => panic!("unexpected error {:?}", error),
            Ok(_) => panic!("directory blob should not exists but it does"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fork() {
        let branch0 = setup().await;
        let branch1 = create_branch(branch0.db_pool().clone()).await;

        // Create a nested directory by branch 0
        let root0 = branch0.open_or_create_root().await.unwrap();
        root0.flush().await.unwrap();

        let dir0 = root0.create_directory("dir".into()).await.unwrap();
        dir0.flush().await.unwrap();
        root0.flush().await.unwrap();

        // Fork it by branch 1 and modify it
        let dir0 = branch0
            .open_root(branch1.clone())
            .await
            .unwrap()
            .read()
            .await
            .lookup_version("dir", branch0.id())
            .unwrap()
            .directory()
            .unwrap()
            .open()
            .await
            .unwrap();
        let dir1 = dir0.fork().await.unwrap();

        dir1.create_file("dog.jpg".into()).await.unwrap();
        dir1.flush().await.unwrap();

        assert_eq!(dir1.read().await.branch().id(), branch1.id());

        // Reopen orig dir and verify it's unchanged
        let dir = branch0
            .open_root(branch0.clone())
            .await
            .unwrap()
            .read()
            .await
            .lookup_version("dir", branch0.id())
            .unwrap()
            .directory()
            .unwrap()
            .open()
            .await
            .unwrap();

        assert_eq!(dir.read().await.entries().count(), 0);

        // Reopen forked dir and verify it contains the new file
        let dir = branch1
            .open_root(branch1.clone())
            .await
            .unwrap()
            .read()
            .await
            .lookup_version("dir", branch1.id())
            .unwrap()
            .directory()
            .unwrap()
            .open()
            .await
            .unwrap();

        assert_eq!(
            dir.read().await.entries().map(|entry| entry.name()).next(),
            Some("dog.jpg")
        );

        // Verify the root dir got forked as well
        branch1.open_root(branch1.clone()).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn modify_directory_concurrently() {
        let branch = setup().await;
        let root = branch.open_or_create_root().await.unwrap();

        // Obtain two instances of the same directory, create a new file in one of them and verify
        // the file immediately exists in the other one as well.

        let dir0 = root.create_directory("dir".to_owned()).await.unwrap();
        let dir1 = root
            .read()
            .await
            .lookup("dir")
            .unwrap()
            .next()
            .unwrap()
            .directory()
            .unwrap()
            .open()
            .await
            .unwrap();

        let mut file0 = dir0.create_file("file.txt".to_owned()).await.unwrap();
        file0.write(b"hello").await.unwrap();
        file0.flush().await.unwrap();

        let mut file1 = dir1
            .read()
            .await
            .lookup("file.txt")
            .unwrap()
            .next()
            .unwrap()
            .file()
            .unwrap()
            .open()
            .await
            .unwrap();
        assert_eq!(file1.read_to_end().await.unwrap(), b"hello");
    }

    async fn setup() -> Branch {
        let pool = db::init(db::Store::Memory).await.unwrap();
        create_branch(pool).await
    }

    async fn create_branch(pool: db::Pool) -> Branch {
        let branch_data = BranchData::new(&pool, rand::random()).await.unwrap();
        Branch::new(pool, Arc::new(branch_data), Cryptor::Null)
    }
}
