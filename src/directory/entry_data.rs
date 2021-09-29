use crate::{blob, blob_id::BlobId, version_vector::VersionVector};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    sync::{Arc, Weak},
};
use tokio::sync::Mutex;

//--------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub enum EntryData {
    File(EntryFileData),
    Directory(EntryDirectoryData),
    Tombstone(EntryTombstoneData),
}

impl EntryData {
    pub(super) fn file(blob_id: BlobId, version_vector: VersionVector) -> Self {
        Self::File(EntryFileData {
            blob_id,
            version_vector,
            blob_core: Arc::new(Mutex::new(Weak::new())),
        })
    }

    pub(super) fn directory(blob_id: BlobId, version_vector: VersionVector) -> Self {
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
}

//--------------------------------------------------------------------

#[derive(Clone, Deserialize, Serialize)]
pub struct EntryFileData {
    pub blob_id: BlobId,
    pub version_vector: VersionVector,
    #[serde(skip)]
    // The Arc here is so that Self is Clone.
    pub blob_core: Arc<Mutex<Weak<Mutex<blob::Core>>>>,
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
pub struct EntryDirectoryData {
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
