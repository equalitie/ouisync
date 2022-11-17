use crate::{blob_id::BlobId, version_vector::VersionVector};
use serde::{Deserialize, Serialize};

//--------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub(crate) enum EntryData {
    File(EntryFileData),
    Directory(EntryDirectoryData),
    Tombstone(EntryTombstoneData),
}

impl EntryData {
    pub fn file(blob_id: BlobId, version_vector: VersionVector) -> Self {
        Self::File(EntryFileData {
            blob_id,
            version_vector,
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

//--------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct EntryFileData {
    pub blob_id: BlobId,
    pub version_vector: VersionVector,
}

impl Clone for EntryFileData {
    fn clone(&self) -> Self {
        Self {
            blob_id: self.blob_id,
            version_vector: self.version_vector.clone(),
        }
    }
}

impl PartialEq for EntryFileData {
    fn eq(&self, other: &Self) -> bool {
        self.blob_id == other.blob_id && self.version_vector == other.version_vector
    }
}

impl Eq for EntryFileData {}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub(crate) struct EntryDirectoryData {
    pub blob_id: BlobId,
    pub version_vector: VersionVector,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub(crate) struct EntryTombstoneData {
    pub cause: TombstoneCause,
    pub version_vector: VersionVector,
}

impl EntryTombstoneData {
    /// Create tombstone date with `Removed` cause
    pub fn removed(version_vector: VersionVector) -> Self {
        Self {
            cause: TombstoneCause::Removed,
            version_vector,
        }
    }

    /// Create tombstone date with `Moved` cause
    pub fn moved(version_vector: VersionVector) -> Self {
        Self {
            cause: TombstoneCause::Moved,
            version_vector,
        }
    }

    pub fn merge(&mut self, other: &Self) {
        use std::cmp::Ordering::*;
        use TombstoneCause::*;

        self.cause = match self.version_vector.partial_cmp(&other.version_vector) {
            Some(Greater) => self.cause,
            Some(Less) => other.cause,
            Some(Equal) | None => {
                // In case of conflict, prefer `Moved` to avoid deleting the moved blob
                match (self.cause, other.cause) {
                    (Removed, Removed) => Removed,
                    (Removed, Moved) | (Moved, Removed) | (Moved, Moved) => Moved,
                }
            }
        };

        self.version_vector.merge(&other.version_vector);
    }
}

/// What caused a tombstone to be created: either the original entry was removed or moved to some
/// other location.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub(crate) enum TombstoneCause {
    Removed,
    Moved,
}
