//! Directory content

use super::entry_data::EntryData;
use crate::{
    blob_id::BlobId,
    branch::Branch,
    crypto::sign::PublicKey,
    error::{Error, Result},
    version_vector::VersionVector,
};
use std::collections::{
    btree_map::{self, Entry},
    BTreeMap,
};

/// Version of the Directory serialization format.
pub(crate) const VERSION: u64 = 1;

#[derive(Clone, Debug)]
pub(super) struct Content {
    entries: ContentV1,
    overwritten_blobs: Vec<BlobId>,
}

impl Content {
    pub fn empty() -> Self {
        Self {
            entries: BTreeMap::new(),
            overwritten_blobs: Vec::new(),
        }
    }

    pub fn deserialize(mut input: &[u8]) -> Result<Self> {
        let version = vint64::decode(&mut input).map_err(|_| Error::MalformedDirectory)?;
        let content = match version {
            VERSION => bincode::deserialize(input).map_err(|_| Error::MalformedDirectory),
            0 => Ok(upgrade_from_v0(
                bincode::deserialize(input).map_err(|_| Error::MalformedDirectory)?,
            )),
            _ => Err(Error::StorageVersionMismatch),
        };

        Ok(Self {
            entries: content?,
            overwritten_blobs: Vec::new(),
        })
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(vint64::encode(VERSION).as_ref());
        bincode::serialize_into(&mut output, &self.entries)
            .expect("failed to serialize directory content");
        output
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> btree_map::Iter<String, EntryData> {
        self.entries.iter()
    }

    pub fn get_key_value(&self, name: &str) -> Option<(&String, &EntryData)> {
        self.entries.get_key_value(name)
    }

    pub fn insert(&mut self, branch: &Branch, name: String, new_data: EntryData) -> Result<()> {
        match self.entries.entry(name) {
            Entry::Vacant(entry) => {
                entry.insert(new_data);
            }
            Entry::Occupied(mut entry) => {
                // Overwrite entries only if the new version is more up to date than the old
                // version. Additionally, if the old entry is `File`, overwrite it only if it's not
                // currently open.
                match entry.get() {
                    EntryData::File(old_data)
                        if new_data.version_vector() > &old_data.version_vector
                            && !branch.is_blob_open(&old_data.blob_id) =>
                    {
                        self.overwritten_blobs.push(old_data.blob_id);
                    }
                    EntryData::Directory(old_data)
                        if new_data.version_vector() > &old_data.version_vector =>
                    {
                        self.overwritten_blobs.push(old_data.blob_id);
                    }
                    EntryData::Tombstone(old_data)
                        if new_data.version_vector() > &old_data.version_vector => {}
                    EntryData::File(_) | EntryData::Directory(_) | EntryData::Tombstone(_) => {
                        return Err(Error::EntryExists)
                    }
                }

                entry.insert(new_data);
            }
        }

        Ok(())
    }

    /// Updates the version vector of entry at `name`.
    pub fn bump(&mut self, branch: &Branch, name: &str, bump: &VersionVector) -> Result<()> {
        self.entries
            .get_mut(name)
            .ok_or(Error::EntryNotFound)?
            .version_vector_mut()
            .bump(bump, branch.id());

        Ok(())
    }

    pub fn overwritten_blobs(&self) -> &[BlobId] {
        &self.overwritten_blobs
    }

    pub fn clear_overwritten_blobs(&mut self) {
        self.overwritten_blobs.clear()
    }

    /// Initial version vector for a new entry to be inserted.
    pub fn initial_version_vector(&self, name: &str) -> VersionVector {
        if let Some(EntryData::Tombstone(entry)) = self.entries.get(name) {
            entry.version_vector.clone()
        } else {
            VersionVector::new()
        }
    }
}

impl<'a> IntoIterator for &'a Content {
    type Item = <Self::IntoIter as Iterator>::Item;
    type IntoIter = btree_map::Iter<'a, String, EntryData>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

type ContentV1 = BTreeMap<String, EntryData>;
type ContentV0 = BTreeMap<String, BTreeMap<PublicKey, EntryData>>;

fn upgrade_from_v0(v0: ContentV0) -> ContentV1 {
    use crate::versioned_file_name;

    let mut v1 = BTreeMap::new();

    for (name, versions) in v0 {
        if versions.len() <= 1 {
            // If there is only one version, insert it directly
            if let Some(data) = versions.into_values().next() {
                v1.insert(name, data);
            }
        } else {
            // If there is more than one version, create unique name for each of them and insert
            // them as separate entries
            for (author_id, data) in versions {
                v1.insert(versioned_file_name::create(&name, &author_id), data);
            }
        }
    }

    v1
}
