use crate::{
    blob::{Blob, BlobId},
    crypto::SecretKey,
    db,
    error::{Error, Result},
    file::File,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{btree_map, BTreeMap},
    ffi::{OsStr, OsString},
};

pub struct Directory {
    blob: Blob,
    content: Content,
    content_dirty: bool,
}

impl Directory {
    /// Opens existing directory.
    pub(crate) async fn open(pool: db::Pool, secret_key: SecretKey, id: BlobId) -> Result<Self> {
        let mut blob = Blob::open(pool, secret_key, id).await?;
        let buffer = blob.read_to_end().await?;
        let content = bincode::deserialize(&buffer).map_err(Error::MalformedDirectory)?;

        Ok(Self {
            blob,
            content,
            content_dirty: false,
        })
    }

    /// Creates new directory.
    pub(crate) fn create(pool: db::Pool, secret_key: SecretKey, id: BlobId) -> Self {
        let blob = Blob::create(pool, secret_key, id);

        Self {
            blob,
            content: Content::new(),
            content_dirty: true,
        }
    }

    /// Flushed this directory ensuring that any pending changes are written to the store.
    pub async fn flush(&mut self) -> Result<()> {
        if !self.content_dirty {
            return Ok(());
        }

        let buffer =
            bincode::serialize(&self.content).expect("failed to serialize directory content");

        self.blob.truncate().await?;
        self.blob.write(&buffer).await?;
        self.blob.flush().await?;

        self.content_dirty = false;

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
            self.blob.secret_key().clone(),
            BlobId::new(*self.blob.head_name(), seq),
        ))
    }

    /// Creates a new subdirectory of this directory.
    pub fn create_subdirectory(&mut self, name: OsString) -> Result<Self> {
        let seq = self.content.insert(name, EntryType::Directory)?;

        Ok(Self::create(
            self.blob.db_pool().clone(),
            self.blob.secret_key().clone(),
            BlobId::new(*self.blob.head_name(), seq),
        ))
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug, Deserialize, Serialize)]
pub enum EntryType {
    File,
    Directory,
}

/// Info about a directory entry.
pub struct EntryInfo<'a> {
    parent_blob: &'a Blob,
    name: &'a OsStr,
    data: &'a EntryData,
}

impl<'a> EntryInfo<'a> {
    pub fn name(&self) -> &OsStr {
        self.name
    }

    /// Open the entry.
    pub async fn open(&self) -> Result<Entry> {
        let id = BlobId::new(*self.parent_blob.head_name(), self.data.seq);

        match self.data.entry_type {
            EntryType::File => Ok(Entry::File(
                File::open(
                    self.parent_blob.db_pool().clone(),
                    self.parent_blob.secret_key().clone(),
                    id,
                )
                .await?,
            )),
            EntryType::Directory => Ok(Entry::Directory(
                Directory::open(
                    self.parent_blob.db_pool().clone(),
                    self.parent_blob.secret_key().clone(),
                    id,
                )
                .await?,
            )),
        }
    }
}

/// Filesystem entry.
pub enum Entry {
    File(File),
    Directory(Directory),
}

impl Entry {
    pub fn entry_type(&self) -> EntryType {
        match self {
            Self::File(_) => EntryType::File,
            Self::Directory(_) => EntryType::Directory,
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Content {
    entries: BTreeMap<OsString, EntryData>,
}

impl Content {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    fn insert(&mut self, name: OsString, entry_type: EntryType) -> Result<u32> {
        let seq = self.next_seq();

        match self.entries.entry(name) {
            btree_map::Entry::Vacant(entry) => {
                entry.insert(EntryData { entry_type, seq });

                Ok(seq)
            }
            btree_map::Entry::Occupied(_) => Err(Error::EntryExists),
        }
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

#[derive(Deserialize, Serialize)]
struct EntryData {
    entry_type: EntryType,
    seq: u32,
    // TODO: metadata
}
