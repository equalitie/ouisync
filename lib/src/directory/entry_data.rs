use crate::{
    blob::{self, Shared},
    blob_id::BlobId,
    sync::Mutex as AsyncMutex,
    version_vector::VersionVector,
};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    sync::{Mutex as BlockingMutex, Weak},
};

//--------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub(crate) enum EntryData {
    File(EntryFileData),
    Directory(EntryDirectoryData),
    Tombstone(EntryTombstoneData),
}

impl EntryData {
    pub fn file(
        blob_id: BlobId,
        version_vector: VersionVector,
        blob_shared: Weak<AsyncMutex<Shared>>,
    ) -> Self {
        Self::File(EntryFileData {
            blob_id,
            version_vector,
            blob_shared: BlockingMutex::new(blob_shared),
        })
    }

    pub fn directory(blob_id: BlobId, version_vector: VersionVector) -> Self {
        Self::Directory(EntryDirectoryData {
            blob_id,
            version_vector,
        })
    }

    pub fn tombstone(version_vector: VersionVector) -> Self {
        Self::Tombstone(EntryTombstoneData { version_vector })
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

    pub fn into_version_vector(self) -> VersionVector {
        match self {
            Self::File(f) => f.version_vector,
            Self::Directory(d) => d.version_vector,
            Self::Tombstone(t) => t.version_vector,
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

//--------------------------------------------------------------------

#[derive(Deserialize, Serialize)]
pub(crate) struct EntryFileData {
    pub blob_id: BlobId,
    pub version_vector: VersionVector,
    #[serde(skip)]
    // Using blocking mutex here to avoid `async` to lock it. We never need to lock it across await
    // points so it should be fine.
    // TODO: consider using the arc_swap crate for this.
    pub blob_shared: BlockingMutex<Weak<AsyncMutex<blob::Shared>>>,
}

impl Clone for EntryFileData {
    fn clone(&self) -> Self {
        Self {
            blob_id: self.blob_id,
            version_vector: self.version_vector.clone(),
            blob_shared: BlockingMutex::new(self.blob_shared.lock().unwrap().clone()),
        }
    }
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
