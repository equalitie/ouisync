use crate::{
    blob::Blob,
    blob_id::BlobId,
    crypto::Cryptor,
    db,
    entry_type::EntryType,
    error::{Error, Result},
    file::File,
    global_locator::GlobalLocator,
    index::BranchData,
    locator::Locator,
    replica_id::ReplicaId,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{btree_map, BTreeMap},
    fmt,
};

#[derive(Clone)]
pub struct Directory {
    blob: Blob,
    content: Content,
}

#[allow(clippy::len_without_is_empty)]
impl Directory {
    /// Opens existing directory.
    pub(crate) async fn open(
        pool: db::Pool,
        branch: BranchData,
        cryptor: Cryptor,
        locator: Locator,
    ) -> Result<Self> {
        let mut blob = Blob::open(pool, branch, cryptor, locator).await?;
        let buffer = blob.read_to_end().await?;
        let content = bincode::deserialize(&buffer).map_err(Error::MalformedDirectory)?;

        Ok(Self { blob, content })
    }

    /// Creates new directory.
    pub(crate) fn create(
        pool: db::Pool,
        branch: BranchData,
        cryptor: Cryptor,
        locator: Locator,
    ) -> Self {
        let local_branch_id = *branch.replica_id();
        let blob = Blob::create(pool, branch, cryptor, locator);

        Self {
            blob,
            content: Content {
                entries: Default::default(),
                dirty: true,
                local_branch_id,
            },
        }
    }

    /// Flushes this directory ensuring that any pending changes are written to the store.
    pub async fn flush(&mut self) -> Result<()> {
        if !self.content.dirty {
            return Ok(());
        }

        let buffer =
            bincode::serialize(&self.content).expect("failed to serialize directory content");

        self.blob.truncate(0).await?;
        self.blob.write(&buffer).await?;
        self.blob.flush().await?;

        self.content.dirty = false;

        Ok(())
    }

    /// Returns iterator over the entries of this directory.
    pub fn entries(&self) -> impl Iterator<Item = EntryRef> + DoubleEndedIterator + Clone {
        self.content
            .entries
            .iter()
            .flat_map(move |(name, versions)| {
                versions
                    .iter()
                    .map(move |(branch_id, data)| EntryRef::new(&self.blob, name, data, branch_id))
            })
    }

    /// Lookup an entry of this directory by name.
    pub fn lookup(&self, name: &'_ str) -> Result<impl Iterator<Item = EntryRef>> {
        self.content
            .entries
            .get_key_value(name)
            .map(|(name, versions)| {
                versions
                    .iter()
                    .map(move |(branch_id, data)| EntryRef::new(&self.blob, name, data, branch_id))
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
                    .map(|(branch_id, data)| EntryRef::new(&self.blob, name, data, branch_id))
            })
            .ok_or(Error::EntryNotFound)
    }

    /// Creates a new file inside this directory.
    pub fn create_file(&mut self, name: String) -> Result<File> {
        let tag = self.content.insert(name, EntryType::File)?;

        Ok(File::create(
            self.blob.db_pool().clone(),
            self.blob.branch().clone(),
            self.blob.cryptor().clone(),
            Locator::Head(tag),
        ))
    }

    /// Creates a new subdirectory of this directory.
    pub fn create_directory(&mut self, name: String) -> Result<Self> {
        let tag = self.content.insert(name, EntryType::Directory)?;

        Ok(Self::create(
            self.blob.db_pool().clone(),
            self.blob.branch().clone(),
            self.blob.cryptor().clone(),
            Locator::Head(tag),
        ))
    }

    /// Creates a file with locators pointing to the same blocks that the src_locators point to.
    /// Blocks themselves are not being duplicated. Return the locator to the newly created file.
    pub async fn copy_file<I>(
        &mut self,
        dst_name: &str,
        src_locators: I,
        src_branch: &BranchData,
    ) -> Result<Locator>
    where
        I: Iterator<Item = Locator>,
    {
        let tag = self.content.insert(dst_name.to_string(), EntryType::File)?;

        let mut tx = self.blob.db_pool().begin().await?;
        let cryptor = self.blob.cryptor();

        let dst_head = Locator::Head(tag);

        let mut dst_locator = dst_head;
        let dst_branch = self.blob.branch();

        for src_locator in src_locators {
            let src_locator = src_locator.encode(cryptor);
            let block_id = src_branch.get(&mut tx, &src_locator).await?;

            dst_branch
                .insert(&mut tx, &block_id, &dst_locator.encode(cryptor))
                .await?;

            dst_locator = dst_locator.next();
        }

        // TODO: Maybe commit on each N copied colators to avoid running out of memory on large
        // files?
        tx.commit().await?;

        Ok(dst_head)
    }

    pub async fn remove_file(&mut self, name: &str) -> Result<()> {
        for entry in self.lookup(name)? {
            entry.file()?.open().await?.remove().await?;
        }
        self.content.remove(name)
        // TODO: Add tombstone
    }

    pub async fn remove_directory(&mut self, name: &str) -> Result<()> {
        for entry in self.lookup(name)? {
            entry.directory()?.open().await?.remove().await?;
        }
        self.content.remove(name)
        // TODO: Add tombstone
    }

    /// Removes this directory if its empty, otherwise fails.
    pub async fn remove(self) -> Result<()> {
        if !self.content.entries.is_empty() {
            return Err(Error::DirectoryNotEmpty);
        }

        self.blob.remove().await
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

    /// Locator of this directory
    pub fn global_locator(&self) -> &GlobalLocator {
        self.blob.global_locator()
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
        parent_blob: &'a Blob,
        name: &'a str,
        data: &'a EntryData,
        branch_id: &'a ReplicaId,
    ) -> Self {
        let inner = RefInner {
            parent_blob,
            name,
            blob_id: &data.blob_id,
        };

        match data.entry_type {
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
            Self::File(r) => r.inner.blob_id,
            Self::Directory(r) => r.inner.blob_id,
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
        Locator::Head(*self.inner.blob_id)
    }

    pub fn branch_id(&self) -> &'a ReplicaId {
        self.branch_id
    }

    pub async fn open(&self) -> Result<File> {
        File::open(
            self.inner.parent_blob.db_pool().clone(),
            self.inner.parent_blob.branch().clone(),
            self.inner.parent_blob.cryptor().clone(),
            self.locator(),
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
        Locator::Head(*self.inner.blob_id)
    }

    pub async fn open(&self) -> Result<Directory> {
        Directory::open(
            self.inner.parent_blob.db_pool().clone(),
            self.inner.parent_blob.branch().clone(),
            self.inner.parent_blob.cryptor().clone(),
            self.locator(),
        )
        .await
    }
}

#[derive(Copy, Clone)]
struct RefInner<'a> {
    parent_blob: &'a Blob,
    name: &'a str,
    blob_id: &'a BlobId,
}

impl PartialEq for RefInner<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.parent_blob.global_locator() == other.parent_blob.global_locator()
            && self.name == other.name
            && self.blob_id == other.blob_id
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
    local_branch_id: ReplicaId,
}

impl Content {
    fn insert(&mut self, name: String, entry_type: EntryType) -> Result<BlobId> {
        let blob_id = rand::random();

        let versions = self.entries.entry(name).or_insert_with(Default::default);

        match versions.entry(self.local_branch_id) {
            btree_map::Entry::Vacant(entry) => {
                entry.insert(EntryData {
                    entry_type,
                    blob_id,
                });
                self.dirty = true;
                Ok(blob_id)
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

#[derive(Clone, Deserialize, Serialize)]
struct EntryData {
    entry_type: EntryType,
    blob_id: BlobId,
    // TODO: metadata
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::BranchData;
    use std::collections::BTreeSet;

    #[tokio::test(flavor = "multi_thread")]
    async fn create_and_list_entries() {
        let (pool, branch) = setup().await;

        // Create the root directory and put some file in it.
        let mut dir = Directory::create(pool.clone(), branch.clone(), Cryptor::Null, Locator::Root);

        let mut file_dog = dir.create_file("dog.txt".into()).unwrap();
        file_dog.write(b"woof").await.unwrap();
        file_dog.flush().await.unwrap();

        let mut file_cat = dir.create_file("cat.txt".into()).unwrap();
        file_cat.write(b"meow").await.unwrap();
        file_cat.flush().await.unwrap();

        dir.flush().await.unwrap();

        // Reopen the dir and try to read the files.
        let dir = Directory::open(pool, branch.clone(), Cryptor::Null, Locator::Root)
            .await
            .unwrap();

        let expected_names: BTreeSet<_> = vec!["dog.txt", "cat.txt"].into_iter().collect();
        let actual_names: BTreeSet<_> = dir.entries().map(|entry| entry.name()).collect();
        assert_eq!(actual_names, expected_names);

        for &(file_name, expected_content) in &[("dog.txt", b"woof"), ("cat.txt", b"meow")] {
            let mut versions = dir.lookup(file_name).unwrap().collect::<Vec<_>>();
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
        let (pool, branch) = setup().await;

        // Create empty directory
        let mut dir = Directory::create(pool.clone(), branch.clone(), Cryptor::Null, Locator::Root);
        dir.flush().await.unwrap();

        // Reopen it and add a file to it.
        let mut dir = Directory::open(pool.clone(), branch.clone(), Cryptor::Null, Locator::Root)
            .await
            .unwrap();
        dir.create_file("none.txt".into()).unwrap();
        dir.flush().await.unwrap();

        // Reopen it again and check the file is still there.
        let dir = Directory::open(pool, branch.clone(), Cryptor::Null, Locator::Root)
            .await
            .unwrap();
        assert!(dir.lookup("none.txt").is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_file() {
        let (pool, branch) = setup().await;

        let name = "monkey.txt";

        // Create a directory with a single file.
        let mut parent_dir =
            Directory::create(pool.clone(), branch.clone(), Cryptor::Null, Locator::Root);
        let mut file = parent_dir.create_file(name.into()).unwrap();
        file.flush().await.unwrap();
        parent_dir.flush().await.unwrap();

        let file_locator = *file.locator();

        // Reopen and remove the file
        let mut parent_dir =
            Directory::open(pool.clone(), branch.clone(), Cryptor::Null, Locator::Root)
                .await
                .unwrap();
        parent_dir.remove_file(name).await.unwrap();
        parent_dir.flush().await.unwrap();

        // Reopen again and check the file entry was removed.
        let parent_dir =
            Directory::open(pool.clone(), branch.clone(), Cryptor::Null, Locator::Root)
                .await
                .unwrap();
        match parent_dir.lookup(name) {
            Err(Error::EntryNotFound) => (),
            Err(error) => panic!("unexpected error {:?}", error),
            Ok(_) => panic!("entry should not exists but it does"),
        }

        assert_eq!(parent_dir.entries().count(), 0);

        // Check the file itself was removed as well.
        match File::open(pool, branch.clone(), Cryptor::Null, file_locator).await {
            Err(Error::EntryNotFound) => (),
            Err(error) => panic!("unexpected error {:?}", error),
            Ok(_) => panic!("file should not exists but it does"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_subdirectory() {
        let (pool, branch) = setup().await;

        let name = "dir";

        // Create a directory with a single subdirectory.
        let mut parent_dir =
            Directory::create(pool.clone(), branch.clone(), Cryptor::Null, Locator::Root);
        let mut dir = parent_dir.create_directory(name.into()).unwrap();
        dir.flush().await.unwrap();
        parent_dir.flush().await.unwrap();

        let dir_locator = *dir.locator();

        // Reopen and remove the subdirectory
        let mut parent_dir =
            Directory::open(pool.clone(), branch.clone(), Cryptor::Null, Locator::Root)
                .await
                .unwrap();
        parent_dir.remove_directory(name).await.unwrap();
        parent_dir.flush().await.unwrap();

        // Reopen again and check the subdiretory entry was removed.
        let parent_dir =
            Directory::open(pool.clone(), branch.clone(), Cryptor::Null, Locator::Root)
                .await
                .unwrap();
        match parent_dir.lookup(name) {
            Err(Error::EntryNotFound) => (),
            Err(error) => panic!("unexpected error {:?}", error),
            Ok(_) => panic!("entry should not exists but it does"),
        }

        // Check the directory itself was removed as well.
        match Directory::open(pool, branch.clone(), Cryptor::Null, dir_locator).await {
            Err(Error::EntryNotFound) => (),
            Err(error) => panic!("unexpected error {:?}", error),
            Ok(_) => panic!("directory should not exists but it does"),
        }
    }

    async fn setup() -> (db::Pool, BranchData) {
        let pool = db::init(db::Store::Memory).await.unwrap();
        let branch = BranchData::new(&pool, rand::random()).await.unwrap();

        (pool, branch)
    }
}
