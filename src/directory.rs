use crate::{
    blob::Blob,
    blob_id::BlobId,
    branch::Branch,
    entry_type::EntryType,
    error::{Error, Result},
    file::File,
    locator::Locator,
    replica_id::ReplicaId,
    version_vector::VersionVector,
    write_context::WriteContext,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{btree_map, hash_map, BTreeMap, HashMap},
    fmt,
    sync::{Arc, Weak},
};
use tokio::sync::{Mutex, RwLock, RwLockReadGuard};

#[derive(Clone)]
pub struct Directory {
    inner: Arc<RwLock<Inner>>,
    local_branch: Branch,
}

#[allow(clippy::len_without_is_empty)]
impl Directory {
    /// Opens the root directory.
    /// For internal use only. Use [`Branch::open_root`] instead.
    pub(crate) async fn open_root(owner_branch: Branch, local_branch: Branch) -> Result<Self> {
        Self::open(
            owner_branch,
            Locator::Root,
            WriteContext::new_for_root(local_branch),
        )
        .await
    }

    /// Opens the root directory or creates it if it doesn't exist.
    /// For internal use only. Use [`Branch::open_or_create_root`] instead.
    pub(crate) async fn open_or_create_root(branch: Branch) -> Result<Self> {
        // TODO: make sure this is atomic

        let locator = Locator::Root;
        let write_context = WriteContext::new_for_root(branch.clone());

        match Self::open(branch.clone(), locator, write_context.clone()).await {
            Ok(dir) => Ok(dir),
            Err(Error::EntryNotFound) => Ok(Self::create(branch, locator, write_context).await),
            Err(error) => Err(error),
        }
    }

    /// Lock this directory for reading.
    pub async fn read(&self) -> Reader<'_> {
        self.inner.read().await
    }

    /// Flushes this directory ensuring that any pending changes are written to the store and the
    /// version vectors of this and the ancestor directories are properly incremented.
    pub async fn flush(&self) -> Result<()> {
        self.inner.write().await.flush().await
    }

    /// Writes the pending changes to the store without incrementing the version vectors.
    /// For internal use only!
    ///
    /// # Panics
    ///
    /// Panics if not dirty or not in the local branch.
    pub(crate) async fn apply(&self) -> Result<()> {
        self.inner.write().await.apply().await
    }

    /// Creates a new file inside this directory.
    pub async fn create_file(&self, name: String) -> Result<File> {
        self.inner.write().await.create_file(name).await
    }

    /// Creates a new subdirectory of this directory.
    pub async fn create_directory(&self, name: String) -> Result<Self> {
        self.inner.write().await.create_directory(name).await
    }

    /// Inserts a dangling entry into this directory. It's the responsibility of the caller to make
    /// sure the returned locator eventually points to an actual file or directory.
    pub(crate) async fn insert_entry(
        &self,
        name: String,
        entry_type: EntryType,
        version_vector: VersionVector,
    ) -> Result<Arc<EntryData>> {
        self.inner
            .write()
            .await
            .insert_entry(name, entry_type, version_vector)
    }

    pub async fn remove_file(&self, name: &str) -> Result<()> {
        for entry in self.read().await.lookup(name)? {
            entry.file()?.open().await?.remove().await?;
        }

        self.inner.write().await.content.remove(name)
        // TODO: Add tombstone
    }

    pub async fn remove_directory(&self, name: &str) -> Result<()> {
        for entry in self.read().await.lookup(name)? {
            entry.directory()?.open().await?.remove().await?;
        }

        self.inner.write().await.content.remove(name)
        // // TODO: Add tombstone
    }

    /// Increment version of the specified entry.
    pub async fn increment_entry_version(&self, _name: &str) -> Result<()> {
        // TODO
        Ok(())
    }

    /// Removes this directory if its empty, otherwise fails.
    pub async fn remove(&self) -> Result<()> {
        let mut inner = self.inner.write().await;

        if !inner.content.entries.is_empty() {
            return Err(Error::DirectoryNotEmpty);
        }

        inner.blob.remove().await
    }

    async fn open(
        branch: Branch,
        locator: Locator,
        write_context: Arc<WriteContext>,
    ) -> Result<Self> {
        let mut blob = Blob::open(branch, locator).await?;
        let buffer = blob.read_to_end().await?;
        let content = bincode::deserialize(&buffer).map_err(Error::MalformedDirectory)?;

        let local_branch = write_context.local_branch().await;

        Ok(Self {
            inner: Arc::new(RwLock::new(Inner {
                blob,
                content,
                write_context,
                open_directories: SubdirectoryCache::new(),
            })),
            local_branch,
        })
    }

    async fn create(branch: Branch, locator: Locator, write_context: Arc<WriteContext>) -> Self {
        let blob = Blob::create(branch, locator);
        let local_branch = write_context.local_branch().await;

        Directory {
            inner: Arc::new(RwLock::new(Inner {
                blob,
                content: Content::new(),
                write_context,
                open_directories: SubdirectoryCache::new(),
            })),
            local_branch,
        }
    }
}

/// View of a `Directory` for performing read-only queries.
pub type Reader<'a> = RwLockReadGuard<'a, Inner>;

pub struct Inner {
    blob: Blob,
    content: Content,
    write_context: Arc<WriteContext>,
    // Cache of open subdirectories. Used to make sure that multiple instances of the same directory
    // all share the same internal state.
    open_directories: SubdirectoryCache,
}

impl Inner {
    /// Returns iterator over the entries of this directory.
    pub fn entries(&self) -> impl Iterator<Item = EntryRef> + DoubleEndedIterator + Clone {
        self.content
            .entries
            .iter()
            .flat_map(move |(name, versions)| {
                versions
                    .iter()
                    .map(move |(branch_id, data)| EntryRef::new(self, name, data, branch_id))
            })
    }

    /// Lookup an entry of this directory by name.
    pub fn lookup(
        &self,
        name: &'_ str,
    ) -> Result<impl Iterator<Item = EntryRef> + ExactSizeIterator> {
        self.content
            .entries
            .get_key_value(name)
            .map(|(name, versions)| {
                versions
                    .iter()
                    .map(move |(branch_id, data)| EntryRef::new(self, name, data, branch_id))
            })
            .ok_or(Error::EntryNotFound)
    }

    pub fn lookup_version(&self, name: &'_ str, author: &ReplicaId) -> Result<EntryRef> {
        self.content
            .entries
            .get_key_value(name)
            .and_then(|(name, versions)| {
                versions
                    .get_key_value(author)
                    .map(|(branch_id, data)| EntryRef::new(self, name, data, branch_id))
            })
            .ok_or(Error::EntryNotFound)
    }

    /// Length of this directory in bytes. Does not include the content, only the size of directory
    /// itself.
    pub fn len(&self) -> u64 {
        self.blob.len()
    }

    /// Locator of this directory
    pub fn locator(&self) -> &Locator {
        self.blob.locator()
    }

    /// Branch of this directory
    pub fn branch(&self) -> &Branch {
        self.blob.branch()
    }

    async fn flush(&mut self) -> Result<()> {
        if !self.content.dirty {
            return Ok(());
        }

        self.write_context
            .begin(EntryType::Directory, &mut self.blob)
            .await?;
        self.apply().await?;
        self.write_context.commit().await?;

        Ok(())
    }

    async fn apply(&mut self) -> Result<()> {
        assert!(self.content.dirty);
        assert_eq!(self.branch().id(), self.local_branch_id());

        let buffer =
            bincode::serialize(&self.content).expect("failed to serialize directory content");

        self.blob.truncate(0).await?;
        self.blob.write(&buffer).await?;
        self.blob.flush().await?;

        self.content.dirty = false;

        Ok(())
    }

    async fn create_file(&mut self, name: String) -> Result<File> {
        let vv = VersionVector::first(*self.local_branch_id());
        let entry = self.insert_entry(name.clone(), EntryType::File, vv)?;
        let locator = entry.locator();
        let write_context = self.write_context.child(name, entry).await;

        Ok(File::create(self.blob.branch().clone(), locator, write_context).await)
    }

    async fn create_directory(&mut self, name: String) -> Result<Directory> {
        let vv = VersionVector::first(*self.local_branch_id());
        let entry = self.insert_entry(name.clone(), EntryType::Directory, vv)?;
        let locator = entry.locator();
        let write_context = self.write_context.child(name, entry).await;

        self.open_directories
            .create(self.blob.branch().clone(), locator, write_context)
            .await
    }

    fn insert_entry(
        &mut self,
        name: String,
        entry_type: EntryType,
        version_vector: VersionVector,
    ) -> Result<Arc<EntryData>> {
        self.content
            .insert(*self.local_branch_id(), name, entry_type, version_vector)
    }

    fn local_branch_id(&self) -> &ReplicaId {
        self.write_context.local_branch_id()
    }
}

/// Info about a directory entry.
#[derive(Copy, Clone)]
pub enum EntryRef<'a> {
    File(FileRef<'a>),
    Directory(DirectoryRef<'a>),
}

impl<'a> EntryRef<'a> {
    fn new(
        parent: &'a Inner,
        name: &'a str,
        parent_entry: &'a Arc<EntryData>,
        branch_id: &'a ReplicaId,
    ) -> Self {
        let inner = RefInner {
            parent,
            parent_entry,
            name,
        };

        match parent_entry.entry_type {
            EntryType::File => Self::File(FileRef { inner, branch_id }),
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

    pub fn blob_id(&self) -> &'a BlobId {
        match self {
            Self::File(r) => &r.inner.parent_entry.blob_id,
            Self::Directory(r) => &r.inner.parent_entry.blob_id,
        }
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
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub struct FileRef<'a> {
    inner: RefInner<'a>,
    branch_id: &'a ReplicaId,
}

impl<'a> FileRef<'a> {
    pub fn name(&self) -> &'a str {
        self.inner.name
    }

    pub fn locator(&self) -> Locator {
        Locator::Head(self.inner.parent_entry.blob_id)
    }

    pub fn branch_id(&self) -> &'a ReplicaId {
        self.branch_id
    }

    pub async fn open(&self) -> Result<File> {
        File::open(
            self.inner.parent.blob.branch().clone(),
            self.locator(),
            self.inner.write_context().await,
        )
        .await
    }
}

impl fmt::Debug for FileRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("FileRef")
            .field("name", &self.inner.name)
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
        Locator::Head(self.inner.parent_entry.blob_id)
    }

    pub async fn open(&self) -> Result<Directory> {
        self.inner
            .parent
            .open_directories
            .open(
                self.inner.parent.blob.branch().clone(),
                self.locator(),
                self.inner.write_context().await,
            )
            .await
    }
}

#[derive(Copy, Clone)]
struct RefInner<'a> {
    parent: &'a Inner,
    parent_entry: &'a Arc<EntryData>,
    name: &'a str,
}

impl RefInner<'_> {
    async fn write_context(&self) -> Arc<WriteContext> {
        self.parent
            .write_context
            .child(self.name.into(), self.parent_entry.clone())
            .await
    }
}

impl PartialEq for RefInner<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.parent.branch().id() == other.parent.branch().id()
            && self.parent.locator() == other.parent.locator()
            && self.parent_entry == other.parent_entry
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
    entries: BTreeMap<String, BTreeMap<ReplicaId, Arc<EntryData>>>,
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

    fn insert(
        &mut self,
        branch_id: ReplicaId,
        name: String,
        entry_type: EntryType,
        version_vector: VersionVector,
    ) -> Result<Arc<EntryData>> {
        let blob_id = rand::random();
        let versions = self.entries.entry(name).or_insert_with(Default::default);

        match versions.entry(branch_id) {
            btree_map::Entry::Vacant(entry) => {
                let data = Arc::new(EntryData {
                    entry_type,
                    blob_id,
                    version_vector,
                });
                entry.insert(data.clone());
                self.dirty = true;
                Ok(data)
            }
            btree_map::Entry::Occupied(_) => Err(Error::EntryExists),
        }
    }

    fn remove(&mut self, name: &str) -> Result<()> {
        self.entries.remove(name).ok_or(Error::EntryNotFound)?;
        self.dirty = true;

        Ok(())
    }
}

#[derive(Clone, Deserialize, Serialize, Eq, PartialEq)]
pub struct EntryData {
    entry_type: EntryType,
    blob_id: BlobId,
    version_vector: VersionVector,
}

impl EntryData {
    pub fn locator(&self) -> Locator {
        Locator::Head(self.blob_id)
    }

    pub fn version_vector(&self) -> &VersionVector {
        &self.version_vector
    }
}

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
            })
        } else {
            let dir = Directory::open_root(owner_branch, local_branch).await?;
            *slot = Some(Arc::downgrade(&dir.inner));
            Ok(dir)
        }
    }

    pub async fn open_or_create(&self, owner_branch: Branch) -> Result<Directory> {
        let mut slot = self.0.lock().await;

        if let Some(inner) = slot.as_mut().and_then(|inner| inner.upgrade()) {
            Ok(Directory {
                inner,
                local_branch: owner_branch,
            })
        } else {
            let dir = Directory::open_or_create_root(owner_branch).await?;
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
        branch: Branch,
        locator: Locator,
        write_context: Arc<WriteContext>,
    ) -> Result<Directory> {
        let mut map = self.0.lock().await;

        let dir = match map.entry(locator) {
            hash_map::Entry::Occupied(mut entry) => {
                if let Some(inner) = entry.get().upgrade() {
                    Directory {
                        inner,
                        local_branch: write_context.local_branch().await,
                    }
                } else {
                    let dir = Directory::open(branch, locator, write_context).await?;
                    entry.insert(Arc::downgrade(&dir.inner));
                    dir
                }
            }
            hash_map::Entry::Vacant(entry) => {
                let dir = Directory::open(branch, locator, write_context).await?;
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
        write_context: Arc<WriteContext>,
    ) -> Result<Directory> {
        let mut map = self.0.lock().await;

        let dir = match map.entry(locator) {
            hash_map::Entry::Occupied(_) => return Err(Error::EntryExists),
            hash_map::Entry::Vacant(entry) => {
                let dir = Directory::create(branch, locator, write_context).await;
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

        // Check the file itself was removed as well.
        match File::open(
            branch.clone(),
            file_locator,
            WriteContext::new_for_root(branch),
        )
        .await
        {
            Err(Error::EntryNotFound) => (),
            Err(error) => panic!("unexpected error {:?}", error),
            Ok(_) => panic!("file should not exists but it does"),
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

        // Open it by branch 1 and modify it
        let dir1 = branch0
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
