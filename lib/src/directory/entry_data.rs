use crate::{
    blob::{self, Shared},
    blob_id::BlobId,
    version_vector::VersionVector,
};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    sync::{Arc, Weak},
};
use tokio::sync::Mutex;

//--------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub(crate) enum EntryData {
    File(EntryFileData),
    Directory(EntryDirectoryData),
    Tombstone(EntryTombstoneData),
}

impl EntryData {
    pub fn new(entry_type: NewEntryType, blob_id: BlobId, version_vector: VersionVector) -> Self {
        match entry_type {
            NewEntryType::File(shared) => Self::file(blob_id, version_vector, shared),
            NewEntryType::Directory => Self::directory(blob_id, version_vector),
        }
    }

    pub fn file(
        blob_id: BlobId,
        version_vector: VersionVector,
        blob_shared: Weak<Mutex<Shared>>,
    ) -> Self {
        Self::File(EntryFileData {
            blob_id,
            version_vector,
            blob_shared: Arc::new(Mutex::new(blob_shared)),
        })
    }

    pub fn directory(blob_id: BlobId, version_vector: VersionVector) -> Self {
        Self::Directory(EntryDirectoryData {
            blob_id,
            version_vector,
        })
    }

    pub fn version_vector(&self) -> &VersionVector {
        match self {
            Self::File(f) => &f.version_vector,
            Self::Directory(d) => &d.version_vector,
            Self::Tombstone(t) => &t.version_vector,
        }
    }

    pub fn version_vector_mut(&mut self) -> &mut VersionVector {
        match self {
            Self::File(f) => &mut f.version_vector,
            Self::Directory(d) => &mut d.version_vector,
            Self::Tombstone(t) => &mut t.version_vector,
        }
    }

    pub fn blob_id(&self) -> Option<&BlobId> {
        match self {
            Self::File(f) => Some(&f.blob_id),
            Self::Directory(d) => Some(&d.blob_id),
            Self::Tombstone(_) => None,
        }
    }
}

pub(crate) enum NewEntryType {
    File(Weak<Mutex<Shared>>),
    Directory,
}

//--------------------------------------------------------------------

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct EntryFileData {
    pub blob_id: BlobId,
    pub version_vector: VersionVector,
    #[serde(skip)]
    // The Arc here is so that Self is Clone.
    pub blob_shared: Arc<Mutex<Weak<Mutex<blob::Shared>>>>,
}

impl fmt::Debug for EntryFileData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("EntryFileData")
            .field("blob_id", &self.blob_id)
            .field("vv", &self.version_vector)
            .finish()
    }
}

impl PartialEq for EntryFileData {
    fn eq(&self, other: &Self) -> bool {
        self.blob_id == other.blob_id && self.version_vector == other.version_vector
    }
}

impl Eq for EntryFileData {}

//--------------------------------------------------------------------

#[derive(Clone, Deserialize, Serialize, Eq, PartialEq)]
pub(crate) struct EntryDirectoryData {
    pub blob_id: BlobId,
    pub version_vector: VersionVector,
}

impl fmt::Debug for EntryDirectoryData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("EntryDirectoryData")
            .field("blob_id", &self.blob_id)
            .field("vv", &self.version_vector)
            .finish()
    }
}

//--------------------------------------------------------------------

#[derive(Clone, Deserialize, Serialize, Eq, PartialEq)]
pub struct EntryTombstoneData {
    pub version_vector: VersionVector,
}

impl fmt::Debug for EntryTombstoneData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("EntryTombstoneData")
            .field("vv", &self.version_vector)
            .finish()
    }
}

//--------------------------------------------------------------------
