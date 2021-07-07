use crate::{
    blob::Blob,
    crypto::{Cryptor, Hashable},
    db,
    entry::{Entry, EntryType},
    error::{Error, Result},
    file::File,
    global_locator::GlobalLocator,
    index::Branch,
    locator::Locator,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{btree_map, BTreeMap},
    ffi::{OsStr, OsString},
};

pub struct Directory {
    blob: Blob,
    content: Content,
}

#[allow(clippy::len_without_is_empty)]
impl Directory {
    /// Opens existing directory.
    pub(crate) async fn open(
        pool: db::Pool,
        branch: Branch,
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
        branch: Branch,
        cryptor: Cryptor,
        locator: Locator,
    ) -> Self {
        let blob = Blob::create(pool, branch, cryptor, locator);

        Self {
            blob,
            content: Content {
                dirty: true,
                ..Default::default()
            },
        }
    }

    /// Flushed this directory ensuring that any pending changes are written to the store.
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
    pub fn entries(
        &self,
    ) -> impl Iterator<Item = EntryInfo> + DoubleEndedIterator + ExactSizeIterator + Clone {
        self.content
            .entries
            .iter()
            .map(move |(name, data)| EntryInfo {
                parent_blob: &self.blob,
                name,
                data,
            })
    }

    /// Lookup an entry of this directory by name.
    pub fn lookup(&self, name: &'_ OsStr) -> Result<EntryInfo> {
        self.content
            .entries
            .get_key_value(name)
            .map(|(name, data)| EntryInfo {
                parent_blob: &self.blob,
                name,
                data,
            })
            .ok_or(Error::EntryNotFound)
    }

    /// Creates a new file inside this directory.
    pub fn create_file(&mut self, name: OsString) -> Result<File> {
        let seq = self.content.insert(name, EntryType::File)?;

        Ok(File::create(
            self.blob.db_pool().clone(),
            self.blob.branch().clone(),
            self.blob.cryptor().clone(),
            Locator::Head(self.locator().hash(), seq),
        ))
    }

    /// Creates a new subdirectory of this directory.
    pub fn create_directory(&mut self, name: OsString) -> Result<Self> {
        let seq = self.content.insert(name, EntryType::Directory)?;

        Ok(Self::create(
            self.blob.db_pool().clone(),
            self.blob.branch().clone(),
            self.blob.cryptor().clone(),
            Locator::Head(self.locator().hash(), seq),
        ))
    }

    pub async fn remove_file(&mut self, name: &OsStr) -> Result<()> {
        self.lookup(name)?.open_file().await?.remove().await?;
        self.content.remove(name)
    }

    pub async fn remove_directory(&mut self, name: &OsStr) -> Result<()> {
        self.lookup(name)?.open_directory().await?.remove().await?;
        self.content.remove(name)
    }

    /// Renames or moves an entry.
    /// If the destination entry already exists and is a file, it is overwritten. If it is a
    /// directory, no change is performed and an error is returned instead.
    pub async fn move_entry(
        &mut self,
        src_name: &OsStr,
        dst_dir: &mut MoveDstDirectory,
        dst_name: &OsStr,
    ) -> Result<()> {
        // Check we are moving entry to itself and if so, do nothing to prevent data loss.
        if let MoveDstDirectory::Src = dst_dir {
            if src_name == dst_name {
                return Ok(());
            }
        }

        let mut tx = self.blob.db_pool().begin().await?;

        let (src_locator, src_entry_type) = {
            let info = self.lookup(src_name)?;
            (info.locator(), info.entry_type())
        };
        let src_head_block_id = self
            .blob
            .branch()
            .get(&mut tx, &src_locator.encode(self.blob.cryptor()))
            .await?;

        let dst_dir = dst_dir.get(self);

        // Can't move over an existing directory.
        // NOTE: we could move *into* the directory instead, but there would still be edge cases
        // e.g. the dst entry being an existing directory there which we would still have to deal
        // with. Keeping things simple for now.
        if dst_dir
            .lookup(dst_name)
            .map(|info| info.entry_type() == EntryType::Directory)
            .unwrap_or(false)
        {
            return Err(Error::EntryIsDirectory);
        }

        let dst_entry = dst_dir.content.vacant_entry();
        let new_dst_locator = Locator::Head(dst_dir.blob.locator().hash(), dst_entry.seq());

        dst_dir
            .blob
            .branch()
            .insert(
                &mut tx,
                &src_head_block_id,
                &new_dst_locator.encode(dst_dir.blob.cryptor()),
            )
            .await?;

        // TODO: remove the previous dst entry from the db, if it existed.

        dst_entry.insert_or_replace(dst_name.to_owned(), src_entry_type);

        match self.content.remove(src_name) {
            Ok(_) | Err(Error::EntryNotFound) => (),
            Err(_) => unreachable!(),
        }

        tx.commit().await?;

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

    /// Locator of this directory
    pub fn global_locator(&self) -> &GlobalLocator {
        self.blob.global_locator()
    }
}

/// Info about a directory entry.
#[derive(Copy, Clone)]
pub struct EntryInfo<'a> {
    parent_blob: &'a Blob,
    name: &'a OsStr,
    data: &'a EntryData,
}

impl<'a> EntryInfo<'a> {
    pub fn name(&self) -> &'a OsStr {
        self.name
    }

    pub fn entry_type(&self) -> EntryType {
        self.data.entry_type
    }

    pub fn locator(&self) -> Locator {
        Locator::Head(self.parent_blob.locator().hash(), self.data.seq)
    }

    /// Opens this entry.
    pub async fn open(&self) -> Result<Entry> {
        match self.entry_type() {
            EntryType::File => Ok(Entry::File(self.open_file_unchecked().await?)),
            EntryType::Directory => Ok(Entry::Directory(self.open_directory_unchecked().await?)),
        }
    }

    /// Opens this entry if it's a file.
    pub async fn open_file(&self) -> Result<File> {
        self.entry_type().check_is_file()?;
        self.open_file_unchecked().await
    }

    /// Opens this entry if it is a directory.
    pub async fn open_directory(&self) -> Result<Directory> {
        self.entry_type().check_is_directory()?;
        self.open_directory_unchecked().await
    }

    async fn open_file_unchecked(&self) -> Result<File> {
        File::open(
            self.parent_blob.db_pool().clone(),
            self.parent_blob.branch().clone(),
            self.parent_blob.cryptor().clone(),
            self.locator(),
        )
        .await
    }

    async fn open_directory_unchecked(&self) -> Result<Directory> {
        Directory::open(
            self.parent_blob.db_pool().clone(),
            self.parent_blob.branch().clone(),
            self.parent_blob.cryptor().clone(),
            self.locator(),
        )
        .await
    }
}

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

#[derive(Default, Deserialize, Serialize)]
struct Content {
    entries: BTreeMap<OsString, EntryData>,
    #[serde(skip)]
    dirty: bool,
}

impl Content {
    fn insert(&mut self, name: OsString, entry_type: EntryType) -> Result<u32> {
        self.vacant_entry().insert(name, entry_type)
    }

    // Reserve an entry to be inserted into later. Useful when the `seq` number is needed before
    // creating the entry.
    fn vacant_entry(&mut self) -> VacantEntry {
        let seq = self.next_seq();
        VacantEntry { seq, content: self }
    }

    fn remove(&mut self, name: &OsStr) -> Result<()> {
        self.entries
            .remove(name)
            .map(|data| data.seq)
            .ok_or(Error::EntryNotFound)?;
        self.dirty = true;

        Ok(())
    }

    // Returns next available seq number.
    fn next_seq(&self) -> u32 {
        // TODO: reuse previously deleted entries

        match self.entries.values().map(|data| data.seq).max() {
            Some(seq) => seq.checked_add(1).expect("directory entry limit exceeded"), // TODO: return error instead
            None => 0,
        }
    }
}

struct VacantEntry<'a> {
    content: &'a mut Content,
    seq: u32,
}

impl VacantEntry<'_> {
    fn seq(&self) -> u32 {
        self.seq
    }

    fn insert(self, name: OsString, entry_type: EntryType) -> Result<u32> {
        match self.content.entries.entry(name) {
            btree_map::Entry::Vacant(entry) => {
                entry.insert(EntryData {
                    entry_type,
                    seq: self.seq,
                });
                self.content.dirty = true;

                Ok(self.seq)
            }
            btree_map::Entry::Occupied(_) => Err(Error::EntryExists),
        }
    }

    fn insert_or_replace(self, name: OsString, entry_type: EntryType) -> u32 {
        self.content.entries.insert(
            name,
            EntryData {
                entry_type,
                seq: self.seq,
            },
        );
        self.content.dirty = true;
        self.seq
    }
}

#[derive(Deserialize, Serialize)]
struct EntryData {
    entry_type: EntryType,
    seq: u32,
    // TODO: metadata
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{index::Branch, replica_id::ReplicaId};
    use rand::{distributions::Standard, Rng};
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

        let expected_names: BTreeSet<_> = vec![OsStr::new("dog.txt"), OsStr::new("cat.txt")]
            .into_iter()
            .collect();
        let actual_names: BTreeSet<_> = dir.entries().map(|entry| entry.name()).collect();
        assert_eq!(actual_names, expected_names);

        for &(file_name, expected_content) in &[
            (OsStr::new("dog.txt"), b"woof"),
            (OsStr::new("cat.txt"), b"meow"),
        ] {
            let mut file = dir.lookup(file_name).unwrap().open_file().await.unwrap();
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
        assert!(dir.lookup(OsStr::new("none.txt")).is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_file() {
        let (pool, branch) = setup().await;

        let name = OsStr::new("monkey.txt");

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

        assert_eq!(parent_dir.entries().len(), 0);

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

        let name = OsStr::new("dir");

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

    #[tokio::test(flavor = "multi_thread")]
    async fn move_entry_to_same_directory() {
        let (pool, branch) = setup().await;

        let src_name = OsStr::new("src.txt");
        let dst_name = OsStr::new("dst.txt");

        let mut dir = Directory::create(pool.clone(), branch.clone(), Cryptor::Null, Locator::Root);

        let mut file = dir.create_file(src_name.to_owned()).unwrap();
        let content = random_content(1024);
        file.write(&content).await.unwrap();
        file.flush().await.unwrap();

        dir.flush().await.unwrap();

        dir.move_entry(src_name, &mut MoveDstDirectory::Src, dst_name)
            .await
            .unwrap();
        dir.flush().await.unwrap();

        match dir.lookup(src_name) {
            Err(Error::EntryNotFound) => (),
            Err(error) => panic!("unexpected error {}", error),
            Ok(_) => panic!("src entry should not exists, but it does"),
        }

        let mut file = dir.lookup(dst_name).unwrap().open_file().await.unwrap();
        let read_content = file.read_to_end().await.unwrap();
        assert_eq!(read_content, content);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn move_entry_to_other_directory() {
        let (pool, branch) = setup().await;

        let mut root_dir =
            Directory::create(pool.clone(), branch.clone(), Cryptor::Null, Locator::Root);
        let mut src_dir = root_dir.create_directory("src".into()).unwrap();
        let mut dst_dir = root_dir.create_directory("dst".into()).unwrap();

        let src_name = OsStr::new("src.txt");
        let dst_name = OsStr::new("dst.txt");

        let mut file = src_dir.create_file(src_name.to_owned()).unwrap();
        let content = random_content(1024);
        file.write(&content).await.unwrap();
        file.flush().await.unwrap();

        src_dir.flush().await.unwrap();
        dst_dir.flush().await.unwrap();

        let mut dst_dir = MoveDstDirectory::Other(dst_dir);
        src_dir
            .move_entry(src_name, &mut dst_dir, dst_name)
            .await
            .unwrap();

        let dst_dir = dst_dir.get_other().unwrap();

        src_dir.flush().await.unwrap();
        dst_dir.flush().await.unwrap();

        match src_dir.lookup(src_name) {
            Err(Error::EntryNotFound) => (),
            Err(error) => panic!("unexpected error {}", error),
            Ok(_) => panic!("src entry should not exists, but it does"),
        }

        let mut file = dst_dir.lookup(dst_name).unwrap().open_file().await.unwrap();
        let read_content = file.read_to_end().await.unwrap();
        assert_eq!(read_content, content);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn move_entry_to_itself() {
        let (pool, branch) = setup().await;

        let name = OsStr::new("src.txt");

        let mut dir = Directory::create(pool.clone(), branch.clone(), Cryptor::Null, Locator::Root);

        let mut file = dir.create_file(name.to_owned()).unwrap();
        let content = random_content(1024);
        file.write(&content).await.unwrap();
        file.flush().await.unwrap();

        dir.flush().await.unwrap();

        dir.move_entry(name, &mut MoveDstDirectory::Src, name)
            .await
            .unwrap();
        dir.flush().await.unwrap();

        let mut file = dir.lookup(name).unwrap().open_file().await.unwrap();
        let read_content = file.read_to_end().await.unwrap();
        assert_eq!(read_content, content);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn move_entry_over_existing_file() {
        let (pool, branch) = setup().await;

        let src_name = OsStr::new("src.txt");
        let dst_name = OsStr::new("dst.txt");

        let mut dir = Directory::create(pool.clone(), branch.clone(), Cryptor::Null, Locator::Root);

        let mut file = dir.create_file(src_name.to_owned()).unwrap();
        let src_content = random_content(1024);
        file.write(&src_content).await.unwrap();
        file.flush().await.unwrap();

        let mut file = dir.create_file(dst_name.to_owned()).unwrap();
        let dst_content = random_content(1024);
        file.write(&dst_content).await.unwrap();
        file.flush().await.unwrap();

        dir.flush().await.unwrap();

        dir.move_entry(src_name, &mut MoveDstDirectory::Src, dst_name)
            .await
            .unwrap();
        dir.flush().await.unwrap();

        match dir.lookup(src_name) {
            Err(Error::EntryNotFound) => (),
            Err(error) => panic!("unexpected error {}", error),
            Ok(_) => panic!("src entry should not exists, but it does"),
        }

        let mut file = dir.lookup(dst_name).unwrap().open_file().await.unwrap();
        let read_content = file.read_to_end().await.unwrap();
        assert_eq!(read_content, src_content);
        assert_ne!(read_content, dst_content);

        // TODO: assert the original dst file has been deleted
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn attempt_to_move_entry_over_existing_directory() {
        let (pool, branch) = setup().await;

        let src_name = OsStr::new("src.txt");
        let dst_name = OsStr::new("dst");

        let mut dir = Directory::create(pool.clone(), branch, Cryptor::Null, Locator::Root);

        let mut src_file = dir.create_file(src_name.to_owned()).unwrap();
        let src_content = random_content(1024);
        src_file.write(&src_content).await.unwrap();
        src_file.flush().await.unwrap();

        let mut dst_dir = dir.create_directory(dst_name.to_owned()).unwrap();
        dst_dir.flush().await.unwrap();

        dir.flush().await.unwrap();

        match dir
            .move_entry(src_name, &mut MoveDstDirectory::Src, dst_name)
            .await
        {
            Err(Error::EntryIsDirectory) => (),
            Err(error) => panic!("unexpected error {}", error),
            Ok(_) => panic!("the move should not have succeeded but it did"),
        }
    }

    async fn setup() -> (db::Pool, Branch) {
        let pool = db::init(db::Store::Memory).await.unwrap();
        let branch = Branch::new(&pool, ReplicaId::random()).await.unwrap();

        (pool, branch)
    }

    fn random_content(len: usize) -> Vec<u8> {
        rand::thread_rng().sample_iter(Standard).take(len).collect()
    }
}
