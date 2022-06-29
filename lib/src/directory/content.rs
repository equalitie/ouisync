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

#[derive(Clone)]
pub(super) struct Content(ContentV1);

impl Content {
    pub fn empty() -> Self {
        Self(BTreeMap::new())
    }

    pub(super) fn deserialize(mut input: &[u8]) -> Result<Self> {
        let version = vint64::decode(&mut input).map_err(|_| Error::MalformedDirectory)?;
        let content = match version {
            VERSION => bincode::deserialize(input).map_err(|_| Error::MalformedDirectory),
            0 => Ok(upgrade_from_v0(
                bincode::deserialize(input).map_err(|_| Error::MalformedDirectory)?,
            )),
            _ => Err(Error::StorageVersionMismatch),
        };

        Ok(Self(content?))
    }

    pub(super) fn serialize(&self) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(vint64::encode(VERSION).as_ref());
        bincode::serialize_into(&mut output, &self.0)
            .expect("failed to serialize directory content");
        output
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn iter(&self) -> btree_map::Iter<String, EntryData> {
        self.0.iter()
    }

    pub(super) fn get(&self, name: &str) -> Option<&EntryData> {
        self.0.get(name)
    }

    pub(super) fn get_key_value(&self, name: &str) -> Option<(&String, &EntryData)> {
        self.0.get_key_value(name)
    }

    pub(super) fn insert(
        &mut self,
        branch: &Branch,
        name: String,
        mut new_data: EntryData,
    ) -> Result<()> {
        match self.0.entry(name) {
            Entry::Vacant(entry) => {
                entry.insert(new_data);
            }
            Entry::Occupied(mut entry) => {
                match entry.get() {
                    // Overwrite files only if the new version is more up to date than the old version
                    // and the old version is not currently open.
                    EntryData::File(old_data)
                        if new_data.version_vector() > &old_data.version_vector
                            && !branch.is_blob_open(&old_data.blob_id) => {}
                    // Overwrite directories only if the new version is more up to date than the old
                    // version.
                    EntryData::Directory(old_data)
                        if new_data.version_vector() > &old_data.version_vector => {}
                    EntryData::File(_) | EntryData::Directory(_) => return Err(Error::EntryExists),
                    // Always overwrite tombstones but update the new version vector so it's more up to
                    // date than the tombstone.
                    EntryData::Tombstone(old_data) => {
                        let mut vv = old_data.version_vector.clone();
                        vv.bump(new_data.version_vector(), branch.id());
                        *new_data.version_vector_mut() = vv;
                    }
                }

                entry.insert(new_data);
            }
        }

        Ok(())
    }

    /// Updates the version vector of entry at `name`.
    pub(super) fn bump(&mut self, branch: &Branch, name: &str, bump: &VersionVector) -> Result<()> {
        self.0
            .get_mut(name)
            .ok_or(Error::EntryNotFound)?
            .version_vector_mut()
            .bump(bump, branch.id());

        Ok(())
    }
}

impl<'a> IntoIterator for &'a Content {
    type Item = <Self::IntoIter as Iterator>::Item;
    type IntoIter = btree_map::Iter<'a, String, EntryData>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub(super) fn overwritten<'a>(
    old: &'a Content,
    new: &'a Content,
) -> impl Iterator<Item = &'a BlobId> {
    old.iter().filter_map(|(name, old_data)| {
        let new_data = new.get(name)?;

        if new_data.version_vector() > old_data.version_vector() {
            old_data.blob_id()
        } else {
            None
        }
    })
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
