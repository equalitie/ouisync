use crate::{
    blob::Blob,
    blob_id::BlobId,
    branch::Branch,
    entry_type::EntryType,
    error::{Error, Result},
    file::File,
    locator::Locator,
    replica_id::ReplicaId,
    write_context::WriteContext,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{btree_map, BTreeMap},
    fmt,
};

pub struct Directory {
    blob: Blob,
    content: Content,
    write_context: WriteContext,
}

#[allow(clippy::len_without_is_empty)]
impl Directory {
    /// Opens existing directory.
    pub(crate) async fn open(
        branch: Branch,
        locator: Locator,
        write_context: WriteContext,
    ) -> Result<Self> {
        let mut blob = Blob::open(branch, locator).await?;
        let buffer = blob.read_to_end().await?;
        let content = bincode::deserialize(&buffer).map_err(Error::MalformedDirectory)?;

        Ok(Self {
            blob,
            content,
            write_context,
        })
    }

    /// Creates the root directory.
    pub(crate) fn create_root(branch: Branch) -> Self {
        let blob = Blob::create(branch.clone(), Locator::Root);

        Self {
            blob,
            content: Content::new(),
            write_context: WriteContext::new("/".into(), branch),
        }
    }

    /// Flushes this directory ensuring that any pending changes are written to the store and the
    /// version vectors of this and the ancestor directories are properly incremented.
    pub async fn flush(&mut self) -> Result<()> {
        if !self.content.dirty {
            return Ok(());
        }

        self.write_context
            .begin(EntryType::Directory, &mut self.blob)
            .await?;
        self.write().await?;
        self.write_context.commit().await?;

        Ok(())
    }

    /// Writes the pending changes to the store without incrementing the version vectors.
    /// For internal use only!
    ///
    /// # Panics
    ///
    /// Panics if not dirty or not in the local branch.
    pub async fn write(&mut self) -> Result<()> {
        assert!(self.content.dirty);
        assert!(self.blob.branch().id() == self.write_context.local_branch().id());

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
                    .map(move |(branch_id, data)| EntryRef::new(self, name, data, branch_id))
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

    /// Creates a new file inside this directory.
    pub fn create_file(&mut self, name: String) -> Result<File> {
        let path = self.write_context.path().join(&name);
        let locator = self.insert_entry(name, EntryType::File)?;
        Ok(File::create(self.blob.branch().clone(), locator, path))
    }

    /// Creates a new subdirectory of this directory.
    pub fn create_directory(&mut self, name: String) -> Result<Self> {
        let write_context = self.write_context.child(&name);
        let locator = self.insert_entry(name, EntryType::Directory)?;
        let blob = Blob::create(self.blob.branch().clone(), locator);

        Ok(Self {
            blob,
            content: Content::new(),
            write_context,
        })
    }

    /// Inserts a dangling entry into this directory. It's the responsibility of the caller to make
    /// sure the returned locator eventually points to an actual file or directory.
    pub(crate) fn insert_entry(&mut self, name: String, entry_type: EntryType) -> Result<Locator> {
        let blob_id =
            self.content
                .insert(*self.write_context.local_branch().id(), name, entry_type)?;
        Ok(Locator::Head(blob_id))
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

    /// Increment version of the specified entry.
    pub fn increment_entry_version(&mut self, _name: &str) -> Result<()> {
        // TODO
        Ok(())
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

    /// Branch of this directory
    pub fn branch(&self) -> &Branch {
        self.blob.branch()
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
        parent: &'a Directory,
        name: &'a str,
        data: &'a EntryData,
        branch_id: &'a ReplicaId,
    ) -> Self {
        let inner = RefInner {
            parent,
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
            self.inner.parent.blob.branch().clone(),
            self.locator(),
            self.inner.write_context(),
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
            self.inner.parent.blob.branch().clone(),
            self.locator(),
            self.inner.write_context(),
        )
        .await
    }
}

#[derive(Copy, Clone)]
struct RefInner<'a> {
    parent: &'a Directory,
    name: &'a str,
    blob_id: &'a BlobId,
}

impl RefInner<'_> {
    fn write_context(&self) -> WriteContext {
        self.parent.write_context.child(self.name)
    }
}

impl PartialEq for RefInner<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.parent.branch().id() == other.parent.branch().id()
            && self.parent.locator() == other.parent.locator()
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
    ) -> Result<BlobId> {
        let blob_id = rand::random();
        let versions = self.entries.entry(name).or_insert_with(Default::default);

        match versions.entry(branch_id) {
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
    use crate::{crypto::Cryptor, db, index::BranchData};
    use camino::Utf8Path;
    use std::collections::BTreeSet;

    #[tokio::test(flavor = "multi_thread")]
    async fn create_and_list_entries() {
        let branch = setup().await;

        // Create the root directory and put some file in it.
        let mut dir = Directory::create_root(branch.clone());

        let mut file_dog = dir.create_file("dog.txt".into()).unwrap();
        file_dog.write(b"woof").await.unwrap();
        file_dog.flush().await.unwrap();

        let mut file_cat = dir.create_file("cat.txt".into()).unwrap();
        file_cat.write(b"meow").await.unwrap();
        file_cat.flush().await.unwrap();

        dir.flush().await.unwrap();

        let write_context = dir.write_context.clone();

        // Reopen the dir and try to read the files.
        let dir = Directory::open(branch.clone(), Locator::Root, write_context)
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
        let branch = setup().await;

        // Create empty directory
        let mut dir = Directory::create_root(branch.clone());
        dir.flush().await.unwrap();

        let write_context = dir.write_context.clone();

        // Reopen it and add a file to it.
        let mut dir = Directory::open(branch.clone(), Locator::Root, write_context.clone())
            .await
            .unwrap();
        dir.create_file("none.txt".into()).unwrap();
        dir.flush().await.unwrap();

        // Reopen it again and check the file is still there.
        let dir = Directory::open(branch, Locator::Root, write_context)
            .await
            .unwrap();
        assert!(dir.lookup("none.txt").is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_file() {
        let branch = setup().await;

        let name = "monkey.txt";

        // Create a directory with a single file.
        let mut parent_dir = Directory::create_root(branch.clone());
        let mut file = parent_dir.create_file(name.into()).unwrap();
        file.flush().await.unwrap();
        parent_dir.flush().await.unwrap();

        let file_locator = *file.locator();
        let write_context = parent_dir.write_context.clone();

        // Reopen and remove the file
        let mut parent_dir = Directory::open(branch.clone(), Locator::Root, write_context.clone())
            .await
            .unwrap();
        parent_dir.remove_file(name).await.unwrap();
        parent_dir.flush().await.unwrap();

        // Reopen again and check the file entry was removed.
        let parent_dir = Directory::open(branch.clone(), Locator::Root, write_context)
            .await
            .unwrap();
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
            WriteContext::new(Utf8Path::new("/").join(name), branch),
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
        let mut parent_dir = Directory::create_root(branch.clone());
        let mut dir = parent_dir.create_directory(name.into()).unwrap();
        dir.flush().await.unwrap();
        parent_dir.flush().await.unwrap();

        let parent_dir_write_context = parent_dir.write_context.clone();
        let dir_write_context = dir.write_context.clone();
        let dir_locator = *dir.locator();

        // Reopen and remove the subdirectory
        let mut parent_dir = Directory::open(
            branch.clone(),
            Locator::Root,
            parent_dir_write_context.clone(),
        )
        .await
        .unwrap();
        parent_dir.remove_directory(name).await.unwrap();
        parent_dir.flush().await.unwrap();

        // Reopen again and check the subdiretory entry was removed.
        let parent_dir = Directory::open(branch.clone(), Locator::Root, parent_dir_write_context)
            .await
            .unwrap();
        match parent_dir.lookup(name) {
            Err(Error::EntryNotFound) => (),
            Err(error) => panic!("unexpected error {:?}", error),
            Ok(_) => panic!("entry should not exists but it does"),
        }

        // Check the directory itself was removed as well.
        match Directory::open(branch, dir_locator, dir_write_context).await {
            Err(Error::EntryNotFound) => (),
            Err(error) => panic!("unexpected error {:?}", error),
            Ok(_) => panic!("directory should not exists but it does"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fork() {
        let branch0 = setup().await;
        let branch1 = create_branch(branch0.db_pool().clone()).await;

        // Create a nested directory by branch 0
        let mut root0 = Directory::create_root(branch0.clone());
        root0.flush().await.unwrap();

        let mut dir0 = root0.create_directory("dir".into()).unwrap();
        dir0.flush().await.unwrap();
        root0.flush().await.unwrap();

        // Open it by branch 1 and modify it
        let mut dir1 = Directory::open(
            branch0.clone(),
            *dir0.locator(),
            WriteContext::new("/dir".into(), branch1.clone()),
        )
        .await
        .unwrap();

        dir1.create_file("dog.jpg".into()).unwrap();
        dir1.flush().await.unwrap();

        assert_eq!(dir1.branch().id(), branch1.id());

        // Reopen orig dir and verify it's unchanged
        let dir = Directory::open(
            branch0.clone(),
            *dir0.locator(),
            WriteContext::new("/dir".into(), branch0),
        )
        .await
        .unwrap();
        assert_eq!(dir.entries().count(), 0);

        // Reopen forked dir and verify it contains the new file
        let dir = Directory::open(
            branch1.clone(),
            *dir1.locator(),
            WriteContext::new("/dir".into(), branch1.clone()),
        )
        .await
        .unwrap();

        assert_eq!(
            dir.entries().map(|entry| entry.name()).next(),
            Some("dog.jpg")
        );

        // Verify the root dir got forked as well
        Directory::open(
            branch1.clone(),
            Locator::Root,
            WriteContext::new("/".into(), branch1),
        )
        .await
        .unwrap();
    }

    async fn setup() -> Branch {
        let pool = db::init(db::Store::Memory).await.unwrap();
        create_branch(pool).await
    }

    async fn create_branch(pool: db::Pool) -> Branch {
        let branch_data = BranchData::new(&pool, rand::random()).await.unwrap();
        Branch::new(pool, branch_data, Cryptor::Null)
    }
}
